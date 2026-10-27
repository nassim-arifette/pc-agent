//! Local human presentation only; never used to construct model, verification or JSON payloads.
use crate::{Metric, Phrase, Quantity};
use serde_json::Value;
use std::{collections::BTreeMap, str::FromStr, sync::OnceLock};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Language {
    #[default]
    En,
    Fr,
}

impl FromStr for Language {
    type Err = &'static str;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "en" => Ok(Self::En),
            "fr" => Ok(Self::Fr),
            _ => Err("language must be en or fr"),
        }
    }
}

fn catalogue() -> &'static Value {
    static CATALOGUE: OnceLock<Value> = OnceLock::new();
    CATALOGUE.get_or_init(|| {
        serde_json::from_str(include_str!("phrases.json")).expect("phrase catalogue")
    })
}

impl Language {
    /// Unknown keys or incomplete values never become invented text.
    pub fn render(self, phrase: &Phrase) -> Option<String> {
        let values = phrase
            .values
            .iter()
            .map(|(name, value)| Some((*name, self.quantity(value)?)))
            .collect::<Option<BTreeMap<_, _>>>()?;
        self.fill(phrase.key, &values)
    }

    pub fn label(self, key: &str) -> Option<&'static str> {
        catalogue()[match self {
            Self::En => "en",
            Self::Fr => "fr",
        }][key]
            .as_str()
    }

    /// Substitute only placeholders in the template, never braces inside supplied text.
    pub fn fill(self, key: &str, values: &BTreeMap<&str, String>) -> Option<String> {
        let mut rest = self.label(key)?;
        let mut result = String::new();
        while let Some(start) = rest.find('{') {
            result.push_str(&rest[..start]);
            let end = rest[start..].find('}')? + start;
            let value = values.get(&rest[start + 1..end])?;
            result.push_str(value);
            rest = &rest[end + 1..];
        }
        result.push_str(rest);
        Some(result)
    }

    pub fn metric(self, metric: Metric) -> Option<&'static str> {
        let code = serde_json::to_value(metric).ok()?;
        self.label(&format!("metric.{}", code.as_str()?))
    }

    pub fn metric_value(self, metric: Metric, value: u64) -> Option<String> {
        let quantity = match metric.unit() {
            "bytes" => Quantity::Bytes(value),
            "bytes_per_second" => Quantity::BytesPerSecond(value),
            unit if unit.starts_with("basis_points_") => Quantity::BasisPoints(value),
            "microseconds" => Quantity::Microseconds(value),
            "pages_per_second_x100" => Quantity::PagesPerSecondX100(value),
            "degrees_celsius" => Quantity::Celsius(value),
            "megahertz" => Quantity::Megahertz(value),
            "milliwatts" => Quantity::Milliwatts(value),
            "nvml_slowdown_reason_bits" => Quantity::GpuSlowdown(crate::gpu_slowdown_codes(value)),
            "outstanding_requests" => Quantity::Count(value),
            _ => return None,
        };
        self.quantity(&quantity)
    }

    fn decimal(self, value: u64, divisor: u64, digits: u32) -> String {
        let scale = 10_u128.pow(digits);
        let rounded = (u128::from(value) * scale + u128::from(divisor) / 2) / u128::from(divisor);
        let whole = rounded / scale;
        let mut fraction = format!("{:0width$}", rounded % scale, width = digits as usize);
        while fraction.ends_with('0') {
            fraction.pop();
        }
        if fraction.is_empty() {
            whole.to_string()
        } else {
            format!(
                "{whole}{}{fraction}",
                if self == Self::Fr { ',' } else { '.' }
            )
        }
    }

    pub fn bytes(self, value: u64) -> String {
        let units = if self == Self::Fr {
            ["o", "Kio", "Mio", "Gio", "Tio", "Pio", "Eio"]
        } else {
            ["B", "KiB", "MiB", "GiB", "TiB", "PiB", "EiB"]
        };
        let mut unit = 0;
        let mut divisor = 1_u64;
        while unit + 1 < units.len() && value / divisor >= 1024 {
            divisor *= 1024;
            unit += 1;
        }
        let number = self.decimal(value, divisor, if unit >= 2 { 2 } else { 1 });
        format!(
            "{number}{}{}",
            if self == Self::Fr { '\u{a0}' } else { ' ' },
            units[unit]
        )
    }

    pub fn quantity(self, value: &Quantity) -> Option<String> {
        Some(match value {
            Quantity::Bytes(value) => self.bytes(*value),
            Quantity::BytesPerSecond(value) => format!("{}/s", self.bytes(*value)),
            Quantity::BasisPoints(value) => format!(
                "{}{}%",
                self.decimal(*value, 100, 1),
                if self == Self::Fr { "\u{a0}" } else { "" }
            ),
            Quantity::Microseconds(value) => format!("{} ms", self.decimal(*value, 1000, 1)),
            Quantity::Milliseconds(value) => format!("{value} ms"),
            Quantity::PagesPerSecondX100(value) => self.fill(
                "unit.pages",
                &BTreeMap::from([("value", self.decimal(*value, 100, 1))]),
            )?,
            Quantity::Count(value) => value.to_string(),
            Quantity::Celsius(value) => format!("{value} °C"),
            Quantity::Megahertz(value) => format!("{value} MHz"),
            Quantity::Milliwatts(value) => format!("{} W", self.decimal(*value, 1000, 1)),
            Quantity::GpuSlowdown(codes) => {
                if codes.is_empty() {
                    self.label("slowdown.none")?.into()
                } else {
                    codes
                        .iter()
                        .map(|code| self.label(&format!("slowdown.{code}")))
                        .collect::<Option<Vec<_>>>()?
                        .join(", ")
                }
            }
            Quantity::Metric(metric) => self.metric(*metric)?.into(),
            Quantity::Metrics(metrics) => {
                if metrics.is_empty() {
                    return None;
                }
                metrics
                    .iter()
                    .map(|metric| self.metric(*metric))
                    .collect::<Option<Vec<_>>>()?
                    .join(", ")
            }
            Quantity::Entity(reference) => {
                if reference == "system" {
                    self.label("entity.system")?.into()
                } else if let Some(number) = reference
                    .strip_prefix("gpu-nvidia-")
                    .filter(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
                {
                    self.fill("entity.gpu", &BTreeMap::from([("n", number.into())]))?
                } else if let Some(engine) = reference.strip_prefix("gpu-engine-") {
                    self.label(&format!("engine.{engine}"))
                        .or_else(|| self.label("engine.other"))?
                        .into()
                } else {
                    reference.clone()
                }
            }
            Quantity::Process { name, pid } => match name {
                Some(name) if !name.is_empty() => format!("{name} (PID {pid})"),
                _ => self.fill(
                    "process.unnamedPid",
                    &BTreeMap::from([("pid", pid.to_string())]),
                )?,
            },
            Quantity::Text(text) => text.clone(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn languages_have_identical_keys_and_placeholders() {
        let en = catalogue()["en"].as_object().unwrap();
        let fr = catalogue()["fr"].as_object().unwrap();
        assert_eq!(en.keys().collect::<Vec<_>>(), fr.keys().collect::<Vec<_>>());
        for (key, english) in en {
            let placeholders = |value: &str| {
                let mut keys = value
                    .split('{')
                    .skip(1)
                    .filter_map(|s| s.split_once('}').map(|(key, _)| key.to_owned()))
                    .collect::<Vec<_>>();
                keys.sort();
                keys
            };
            assert_eq!(
                placeholders(english.as_str().unwrap()),
                placeholders(fr[key].as_str().unwrap()),
                "{key}"
            );
        }
    }

    #[test]
    fn typed_values_preserve_zero_large_integers_and_free_text() {
        assert_eq!(
            Language::Fr.quantity(&Quantity::Count(u64::MAX)).unwrap(),
            u64::MAX.to_string()
        );
        assert_eq!(
            Language::Fr.quantity(&Quantity::BasisPoints(0)).unwrap(),
            "0\u{a0}%"
        );
        assert_eq!(Language::Fr.bytes(1 << 30), "1\u{a0}Gio");
        assert_eq!(Language::En.bytes(1 << 30), "1 GiB");
        assert_eq!(
            Language::Fr
                .quantity(&Quantity::Text("User {share} text".into()))
                .unwrap(),
            "User {share} text"
        );
        assert!(Language::Fr.render(&Phrase::new("not.a.key")).is_none());
        assert!(Language::Fr
            .render(&Phrase::new("fact.available_share"))
            .is_none());
    }

    #[test]
    fn phrase_rendering_does_not_mutate_machine_payloads() {
        let phrase = Phrase::new("fact.available_share").with("share", Quantity::BasisPoints(1700));
        let before = serde_json::to_value(&phrase).unwrap();
        assert_eq!(
            Language::Fr.render(&phrase).unwrap(),
            "La mémoire physique disponible était de 17\u{a0}%."
        );
        assert_eq!(serde_json::to_value(&phrase).unwrap(), before);
    }
}
