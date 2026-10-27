//! Parse stated quantities and compare them with typed measurements in canonical units.
use serde_json::Value;

/// A physical dimension in a canonical unit, including energy separately from power.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Dim {
    Bytes,
    Percent,
    Millis,
    Celsius,
    Megahertz,
    Watts,
    WattHours,
    Volts,
    PerSecond,
    BytesPerSecond,
    Count,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Bound {
    Exact,
    About,
    AtMost,
    AtLeast,
}

/// A number with a unit written in a text.
#[derive(Debug, Clone)]
pub(super) struct Claim {
    pub(super) excerpt: String,
    pub(super) end: usize,
    pub(super) dim: Dim,
    /// Candidate canonical values with their tolerance: a separator or a byte unit can be read
    /// in two ways (1,234 / 1.234; GB as 10^9 or 2^30).
    pub(super) values: Vec<(f64, f64)>,
    pub(super) bound: Bound,
}

/// Relative slack for a number written as measured, and for one introduced by "about".
const EXACT_SHARE: f64 = 0.05;
const ABOUT_SHARE: f64 = 0.15;

/// A quantity found in what the model received.
#[derive(Debug, Clone, Copy)]
struct Quantity {
    dim: Dim,
    value: f64,
    /// A total others can be a share of.
    total: bool,
    /// Installed or physical memory: any memory amount can be stated as a share of it.
    memory_total: bool,
    /// Quantities of one object, measured entity or text: shares are only taken within it.
    group: usize,
}

/// Every quantity the model received, plus the simple derivations an answer may state: the
/// share of a total, its complement and what remains of a total.
pub(super) struct Known {
    pub(super) values: Vec<(Dim, f64)>,
}
impl Known {
    pub(super) fn from_received(received: &[Value], derive_shares: bool) -> Self {
        let mut found: Vec<Quantity> = Vec::new();
        let mut groups = 0;
        for value in received {
            walk(value, None, 0, &mut groups, &mut found);
        }
        let mut values: Vec<(Dim, f64)> = found.iter().map(|q| (q.dim, q.value)).collect();
        // Metric-specific battery claims must retain their role: the complement of wear is
        // retention, not another wear measurement. The collector already provides these
        // derivations with their own metric, source and status.
        if !derive_shares {
            return Self { values };
        }
        for total in found.iter().filter(|q| q.total && q.value > 0.0) {
            for part in &found {
                let related = part.group == total.group || total.memory_total;
                if part.dim != total.dim || part.value > total.value || !related {
                    continue;
                }
                let share = part.value / total.value * 100.0;
                values.push((Dim::Percent, share));
                values.push((total.dim, total.value - part.value));
            }
        }
        let complements: Vec<(Dim, f64)> = values
            .iter()
            .filter(|(d, v)| *d == Dim::Percent && *v <= 100.0)
            .map(|(_, v)| (Dim::Percent, 100.0 - v))
            .collect();
        values.extend(complements);
        Self { values }
    }
    pub(super) fn supports(&self, claim: &Claim) -> bool {
        self.values
            .iter()
            .filter(|(dim, _)| *dim == claim.dim)
            .any(|(_, known)| {
                claim.values.iter().any(|(value, tolerance)| {
                    let slack = |share: f64| tolerance.max(share * known.abs()).max(1e-9);
                    match claim.bound {
                        Bound::Exact => (value - known).abs() <= slack(EXACT_SHARE),
                        Bound::About => (value - known).abs() <= slack(ABOUT_SHARE),
                        Bound::AtMost => *known <= value + slack(EXACT_SHARE),
                        Bound::AtLeast => *known >= value - slack(EXACT_SHARE),
                    }
                })
            })
    }
}

/// Collects quantities from a payload. The unit of a number comes from:
/// - its key (`…_bytes`, `…_basis_points`, …);
/// - the unit map of a measurements object;
/// - the `metric` field of a history;
/// - the key of a map whose values are all numbers (`windows_ms`).
///
/// Prose is not measured evidence. Only the facts of a locally computed finding are parsed
/// separately by the source resolver.
fn walk(
    value: &Value,
    unit: Option<&str>,
    group: usize,
    groups: &mut usize,
    out: &mut Vec<Quantity>,
) {
    let mut push = |dim: Dim, value: f64, name: &str, group: usize| {
        let memory = name.contains("physical_total") || name.contains("installed_memory");
        out.push(Quantity {
            dim,
            value,
            total: is_total(name),
            memory_total: memory && is_total(name),
            group,
        });
    };
    match value {
        Value::Object(map) => {
            if map
                .get("measurement_metadata_available")
                .and_then(Value::as_bool)
                == Some(false)
            {
                return;
            }
            if map.contains_key("value")
                && map.get("status").and_then(Value::as_str).is_some_and(|s| {
                    s != "ok"
                        && !(s == "partial"
                            && map
                                .get("measured_samples")
                                .and_then(Value::as_u64)
                                .is_some_and(|count| count > 0))
                })
            {
                return;
            }
            *groups += 1;
            let own = *groups;
            if let (Some(Value::Object(values)), Some(Value::Object(units))) =
                (map.get("values"), map.get("units"))
            {
                for metrics in values.values().filter_map(Value::as_object) {
                    *groups += 1;
                    for (metric, n) in metrics {
                        let unit = units.get(metric).and_then(Value::as_str).unwrap_or(metric);
                        if let (Some(n), Some((dim, factor))) = (n.as_f64(), unit_of(unit)) {
                            push(dim, n * factor, metric, *groups);
                        }
                    }
                }
            }
            let metric = map.get("metric").and_then(Value::as_str);
            let event_unit = if map.contains_key("peak")
                && map.contains_key("started_at_unix_ms")
                && map.contains_key("ended_at_unix_ms")
                && map.contains_key("metric")
            {
                map.get("unit").and_then(Value::as_str)
            } else {
                None
            };
            // C4 inspection records have a closed, typed schema. Their explicit unit applies
            // only to the numeric value of a usable measurement, never to descriptive prose.
            let record_unit = if map.contains_key("entity_ref")
                && map.contains_key("observation_id")
                && map.get("status").and_then(Value::as_str) == Some("ok")
            {
                map.get("unit").and_then(Value::as_str)
            } else {
                None
            };
            let is_record = map.contains_key("value")
                && map.contains_key("metric")
                && map.contains_key("entity_ref")
                && map.contains_key("observation_id");
            let inherit_all = unit.is_some() && map.values().all(Value::is_number);
            for (key, child) in map {
                // Provenance windows and operands cannot certify a result's quantity. A C4
                // record contributes its typed value only; its payload stays intact in Context.
                if key.ends_with("_unix_ms")
                    || matches!(key.as_str(), "window" | "inputs")
                    || (is_record && key != "value")
                {
                    continue;
                }
                if key == "values" && map.contains_key("units") {
                    continue;
                }
                let hint = if key == "value" {
                    record_unit.or(unit)
                } else if key == "peak" {
                    event_unit
                } else if inherit_all || (matches!(key.as_str(), "min" | "max") && unit.is_some()) {
                    unit
                } else if unit_of(key).is_some() {
                    Some(key.as_str())
                } else if key == "points" {
                    metric
                } else {
                    None
                };
                walk(child, hint, own, groups, out);
            }
        }
        Value::Array(items) => {
            for item in items {
                walk(item, unit, group, groups, out);
            }
        }
        Value::Number(n) => {
            if let (Some(n), Some(name)) = (n.as_f64(), unit) {
                if let Some((dim, factor)) = unit_of(name) {
                    push(dim, n * factor, name, group);
                }
            }
        }
        _ => {}
    }
}

/// The dimension and factor to the canonical unit of a payload unit or key name.
fn unit_of(name: &str) -> Option<(Dim, f64)> {
    let n = name.to_ascii_lowercase();
    let is = |unit: &str| n == unit || n.ends_with(&format!("_{unit}"));
    if n.contains("unix_ms") {
        None
    } else if is("count") {
        Some((Dim::Count, 1.0))
    } else if is("bytes_per_second") {
        Some((Dim::BytesPerSecond, 1.0))
    } else if is("bytes") {
        Some((Dim::Bytes, 1.0))
    } else if n.starts_with("basis_points") || is("basis_points") {
        Some((Dim::Percent, 0.01))
    } else if is("microseconds") {
        Some((Dim::Millis, 0.001))
    } else if is("milliseconds") || is("ms") {
        Some((Dim::Millis, 1.0))
    } else if is("seconds") {
        Some((Dim::Millis, 1_000.0))
    } else if is("celsius") {
        Some((Dim::Celsius, 1.0))
    } else if is("megahertz") {
        Some((Dim::Megahertz, 1.0))
    } else if is("milliwatt_hours") || is("mwh") {
        Some((Dim::WattHours, 0.001))
    } else if is("watt_hours") {
        Some((Dim::WattHours, 1.0))
    } else if name == "mJ" || is("millijoules") {
        Some((Dim::WattHours, 1.0 / 3_600_000.0))
    } else if name == "J" || is("joules") {
        Some((Dim::WattHours, 1.0 / 3_600.0))
    } else if is("hours") {
        Some((Dim::Millis, 3_600_000.0))
    } else if is("milliwatts") || is("mw") {
        Some((Dim::Watts, 0.001))
    } else if is("millivolts") || is("mv") {
        Some((Dim::Volts, 0.001))
    } else if is("volts") {
        Some((Dim::Volts, 1.0))
    } else if n.contains("per_second_x100") {
        Some((Dim::PerSecond, 0.01))
    } else if is("percent") {
        Some((Dim::Percent, 1.0))
    } else {
        None
    }
}

fn is_total(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    ["total", "limit", "capacity", "installed"]
        .iter()
        .any(|w| n.contains(w))
}

const THOUSANDS_SPACES: [char; 4] = [' ', '\u{a0}', '\u{202f}', '\u{2009}'];

/// Numbers followed by a known unit. Numbers inside identifiers (`F1`, `x86`, `proc-1a2b`) and
/// numbers without a unit (versions, counts, dates) are not claims.
pub(super) fn claims(text: &str) -> Vec<Claim> {
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let (start, c) = chars[i];
        if !c.is_ascii_digit() {
            i += 1;
            continue;
        }
        let prev = i.checked_sub(1).map(|p| chars[p].1);
        let mut sign_index = i.checked_sub(1);
        while sign_index.is_some_and(|p| THOUSANDS_SPACES.contains(&chars[p].1)) {
            sign_index = sign_index.and_then(|p| p.checked_sub(1));
        }
        let negative_index = sign_index.filter(|p| {
            let (offset, sign) = chars[*p];
            let separated = *p + 1 < i;
            // A Markdown list marker is not the sign of its first number.
            let bullet = sign == '-'
                && separated
                && text[..offset]
                    .rsplit('\n')
                    .next()
                    .unwrap_or("")
                    .trim()
                    .is_empty();
            matches!(sign, '-' | '−') && !bullet && (*p == 0 || !chars[*p - 1].1.is_alphanumeric())
        });
        let joined = prev.is_some_and(|p| p.is_alphanumeric() || p == '_')
            || (matches!(prev, Some('-' | '−' | '‐' | '‑' | '‒' | '–' | '—'))
                && i >= 2
                && chars[i - 2].1.is_alphanumeric());
        let negative = negative_index.is_some() && !joined;
        let claim_start = negative_index
            .filter(|_| negative)
            .map_or(start, |p| chars[p].0);
        // Parse digit groups and their separators.
        let mut groups = vec![String::new()];
        let mut seps: Vec<char> = Vec::new();
        let mut j = i;
        while j < chars.len() {
            let ch = chars[j].1;
            if ch.is_ascii_digit() {
                groups.last_mut().expect("one group").push(ch);
                j += 1;
                continue;
            }
            let next_digits = chars[j + 1..]
                .iter()
                .take_while(|(_, c)| c.is_ascii_digit())
                .count();
            let after = chars.get(j + 1 + next_digits).map(|(_, c)| *c);
            let thousands_space = THOUSANDS_SPACES.contains(&ch)
                && next_digits == 3
                && after.is_none_or(|c| !c.is_ascii_digit());
            if (matches!(ch, '.' | ',') && next_digits > 0) || thousands_space {
                seps.push(ch);
                groups.push(String::new());
                j += 1;
                continue;
            }
            break;
        }
        // An identifier continues after the number (`4060ti`, `v2beta`): not a quantity.
        let glued = chars
            .get(j)
            .is_some_and(|(_, c)| c.is_alphanumeric() && !unit_start(text, chars[j].0));
        if joined || glued {
            i = j.max(i + 1);
            while i < chars.len() && (chars[i].1.is_alphanumeric() || chars[i].1 == '_') {
                i += 1;
            }
            continue;
        }
        let end_number = chars.get(j).map_or(text.len(), |(p, _)| *p);
        let readings = readings(&groups, &seps);
        // The unit: after optional spaces.
        let mut k = j;
        while k < chars.len() && THOUSANDS_SPACES.contains(&chars[k].1) && k < j + 2 {
            k += 1;
        }
        let unit_from = chars.get(k).map_or(text.len(), |(p, _)| *p);
        let unit_text: String = text[unit_from..]
            .chars()
            .take_while(|c| c.is_alphabetic() || matches!(c, '%' | '°' | '/' | 'µ'))
            .collect();
        let unit_end = unit_from + unit_text.len();
        let per_second = unit_text.to_lowercase().ends_with("/s")
            || text[unit_end..].to_lowercase().starts_with(" par seconde")
            || text[unit_end..].to_lowercase().starts_with(" per second");
        if let Some((dim, factors)) =
            unit(&unit_text, per_second).filter(|_| at_token_end(text, unit_end))
        {
            let bound = bound_before(&text[..claim_start]);
            let sign = if negative { -1.0 } else { 1.0 };
            let values = readings
                .iter()
                .flat_map(|(value, decimals)| {
                    factors
                        .iter()
                        .map(move |f| (sign * value * f, 0.5 * 10f64.powi(-(*decimals as i32)) * f))
                })
                .collect();
            out.push(Claim {
                excerpt: text[claim_start..unit_end.max(end_number)]
                    .trim()
                    .to_owned(),
                end: unit_end.max(end_number),
                dim,
                values,
                bound,
            });
            i = k.max(j) + unit_text.chars().count();
        } else {
            i = j.max(i + 1);
        }
    }
    out
}

/// Whether a unit starts at this byte offset, so that `3Go` still reads as a quantity.
fn unit_start(text: &str, at: usize) -> bool {
    let word: String = text[at..]
        .chars()
        .take_while(|c| c.is_alphabetic() || matches!(c, '%' | '°' | '/' | 'µ'))
        .collect();
    at_token_end(text, at + word.len())
        && unit(&word, word.to_lowercase().ends_with("/s")).is_some()
}

fn at_token_end(text: &str, at: usize) -> bool {
    text[at..]
        .chars()
        .next()
        .is_none_or(|c| !c.is_alphanumeric() && c != '_')
}

/// Possible values of a written number with their count of decimals.
fn readings(groups: &[String], seps: &[char]) -> Vec<(f64, usize)> {
    let digits = |gs: &[String]| gs.concat();
    let parse = |s: &str| s.parse::<f64>().ok();
    let Some(&last) = seps.last() else {
        return parse(&groups[0]).map(|v| vec![(v, 0)]).unwrap_or_default();
    };
    let integer = parse(&digits(groups)).map(|v| (v, 0));
    if THOUSANDS_SPACES.contains(&last) {
        return integer.into_iter().collect();
    }
    let tail = groups.last().expect("a group per separator");
    let head = &groups[..groups.len() - 1];
    let decimal = parse(&format!("{}.{}", digits(head), tail)).map(|v| (v, tail.len()));
    let same = seps.iter().filter(|&&s| s == last).count();
    // "1,234" or "1.234": thousands or three decimals. "1,5" or "12.25": decimals. "1.234.567":
    // thousands.
    let grouped =
        tail.len() == 3 && head.iter().skip(1).all(|g| g.len() == 3) && head[0].len() <= 3;
    match (grouped, same > 1) {
        (true, true) => integer.into_iter().collect(),
        (true, false) => integer.into_iter().chain(decimal).collect(),
        (false, _) => decimal.into_iter().collect(),
    }
}

const BINARY: [f64; 5] = [
    1.0,
    1024.0,
    1_048_576.0,
    1_073_741_824.0,
    1_099_511_627_776.0,
];
const DECIMAL: [f64; 5] = [1.0, 1e3, 1e6, 1e9, 1e12];

/// The dimension of a written unit and its factors to the canonical unit.
fn unit(written: &str, per_second: bool) -> Option<(Dim, Vec<f64>)> {
    let lower = written.to_lowercase();
    let base = lower.trim_end_matches("/s");
    let bytes = |power: usize| vec![DECIMAL[power], BINARY[power]];
    let byte_unit = match base {
        "o" | "b" | "octet" | "octets" | "byte" | "bytes" => Some(vec![1.0]),
        "ko" | "kb" | "kio" | "kib" => Some(bytes(1)),
        "mo" | "mb" | "mio" | "mib" => Some(bytes(2)),
        "go" | "gb" | "gio" | "gib" => Some(bytes(3)),
        "to" | "tb" | "tio" | "tib" => Some(bytes(4)),
        _ => None,
    };
    if per_second {
        return match (byte_unit, base) {
            (Some(factors), _) => Some((Dim::BytesPerSecond, factors)),
            (None, "" | "page" | "pages" | "e" | "fautes" | "faults") => {
                Some((Dim::PerSecond, vec![1.0]))
            }
            _ => None,
        };
    }
    if let Some(factors) = byte_unit {
        return Some((Dim::Bytes, factors));
    }
    let one = |dim: Dim, factor: f64| Some((dim, vec![factor]));
    // The prefixes are case-sensitive: mJ and MJ differ by a billion.
    match written {
        "mJ" => return one(Dim::WattHours, 1.0 / 3_600_000.0),
        "MJ" => return one(Dim::WattHours, 1_000_000.0 / 3_600.0),
        "J" => return one(Dim::WattHours, 1.0 / 3_600.0),
        "kJ" => return one(Dim::WattHours, 1_000.0 / 3_600.0),
        _ => {}
    }
    match lower.as_str() {
        "%" => one(Dim::Percent, 1.0),
        "ms" => one(Dim::Millis, 1.0),
        "µs" | "us" => one(Dim::Millis, 0.001),
        "s" | "sec" | "seconde" | "secondes" | "second" | "seconds" => one(Dim::Millis, 1_000.0),
        "min" | "minute" | "minutes" => one(Dim::Millis, 60_000.0),
        "heure" | "heures" | "hour" | "hours" => one(Dim::Millis, 3_600_000.0),
        "°c" | "°" | "degrés" | "degres" | "degrees" => one(Dim::Celsius, 1.0),
        "mhz" => one(Dim::Megahertz, 1.0),
        "ghz" => one(Dim::Megahertz, 1_000.0),
        "w" | "watt" | "watts" => one(Dim::Watts, 1.0),
        "mw" => one(Dim::Watts, 0.001),
        "wh" => one(Dim::WattHours, 1.0),
        "mwh" => one(Dim::WattHours, 0.001),
        "joule" | "joules" => one(Dim::WattHours, 1.0 / 3_600.0),
        "millijoule" | "millijoules" => one(Dim::WattHours, 1.0 / 3_600_000.0),
        "v" | "volt" | "volts" => one(Dim::Volts, 1.0),
        "mv" | "millivolt" | "millivolts" => one(Dim::Volts, 0.001),
        "processes" | "processus" | "services" | "applications" | "apps" | "programs"
        | "programmes" | "slots" | "emplacements" | "modules" | "files" | "fichiers"
        | "folders" | "dossiers" | "entries" | "entrées" | "cycles" => one(Dim::Count, 1.0),
        _ => None,
    }
}

fn ends_with_word(before: &str, words: &[&str]) -> bool {
    let lower = before.trim_end().to_lowercase();
    words.iter().any(|w| {
        lower.ends_with(w)
            && lower[..lower.len() - w.len()]
                .chars()
                .next_back()
                .is_none_or(|c| !c.is_alphanumeric())
    })
}

fn bound_before(before: &str) -> Bound {
    const AT_MOST: [&str; 14] = [
        "moins de",
        "less than",
        "under",
        "below",
        "au plus",
        "at most",
        "inférieur à",
        "inférieure à",
        "sous",
        "<",
        "≤",
        "jusqu'à",
        "up to",
        "max",
    ];
    const AT_LEAST: [&str; 14] = [
        "plus de",
        "more than",
        "over",
        "above",
        "au moins",
        "at least",
        "supérieur à",
        "supérieure à",
        ">",
        "≥",
        "dépasse",
        "exceeds",
        "beyond",
        "au-delà de",
    ];
    const ABOUT: [&str; 14] = [
        "environ",
        "about",
        "around",
        "approximately",
        "approx.",
        "~",
        "≈",
        "près de",
        "roughly",
        "nearly",
        "presque",
        "almost",
        "autour de",
        "de l'ordre de",
    ];
    if ends_with_word(before, &AT_MOST) {
        Bound::AtMost
    } else if ends_with_word(before, &AT_LEAST) {
        Bound::AtLeast
    } else if ends_with_word(before, &ABOUT) {
        Bound::About
    } else {
        Bound::Exact
    }
}
