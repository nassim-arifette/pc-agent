//! Smaller inference context without changing collection, the local evidence or the verifier.
//! Exact repeats still run the collector. Different dates, windows, statuses or values are
//! never treated as the same observation.
use pcagent_shared::{format_bytes, Metric};
use serde_json::{json, Map, Value};
use std::collections::BTreeMap;

/// The real capture includes individual tool replies over 150 KiB. At 32 KiB of retained tool
/// context, older replies inherit identical metadata from their parent; the newest stays whole.
const SUMMARY_THRESHOLD_BYTES: usize = 32 * 1024;

#[derive(Default)]
pub(crate) struct ToolContext {
    seen: BTreeMap<String, String>,
}

impl ToolContext {
    /// Records a payload the model received outside a tool reply, like the frozen selection in
    /// the first message, so a tool that returns it again gets the short form.
    pub(crate) fn seed(&mut self, cloud: &Value, citation: &str) {
        self.seen
            .entry(identity(cloud))
            .or_insert_with(|| citation.into());
    }

    pub(crate) fn message(&mut self, cloud: &Value) -> Value {
        let Some(citation) = cloud["observation_id"].as_str() else {
            return cloud.clone();
        };
        let key = identity(cloud);
        if let Some(previous) = self.seen.get(&key) {
            let mut short = Map::new();
            for field in [
                "observation_id",
                "status",
                "measured_at_unix_ms",
                "window",
                "origin",
                "current_finding_ids",
                "citable_observation_ids",
                "inspection_complete",
            ] {
                if let Some(value) = cloud.get(field) {
                    short.insert(field.into(), value.clone());
                }
            }
            short.insert("same_result_as".into(), json!(previous));
            short.insert("note".into(), json!("The tool ran again. Values, coverage and measurement times are identical to the referenced result; both citations resolve to the full local evidence."));
            return Value::Object(short);
        }
        self.seen.insert(key, citation.into());
        cloud.clone()
    }

    pub(crate) fn compact(&self, messages: &mut [Value]) {
        let indices: Vec<usize> = messages
            .iter()
            .enumerate()
            .filter(|(_, message)| message["role"] == "tool")
            .map(|(index, _)| index)
            .collect();
        let total: usize = indices
            .iter()
            .filter_map(|i| messages[*i]["content"].as_str())
            .map(str::len)
            .sum();
        if total <= SUMMARY_THRESHOLD_BYTES {
            return;
        }
        for index in indices.iter().rev().skip(1) {
            let Some(text) = messages[*index]["content"].as_str() else {
                continue;
            };
            let Ok(mut value) = serde_json::from_str::<Value>(text) else {
                continue;
            };
            if value.get("context_summary").is_some() || !value.is_object() {
                continue;
            }
            inherit_metadata(&mut value, &BTreeMap::new());
            value["context_summary"] = json!("Repeated source, origin, time and window fields inherit from the nearest enclosing object. Every raw value, unit, status, name, citation and coverage limitation is retained; full records remain local.");
            let compacted = value.to_string();
            if compacted.len() < text.len() {
                messages[*index]["content"] = json!(compacted);
            }
        }
    }
}

/// What two payloads must share to be the same result: everything but the citation bookkeeping.
fn identity(cloud: &Value) -> String {
    let mut identity = cloud.clone();
    remove_bookkeeping(&mut identity);
    identity.to_string()
}

fn remove_bookkeeping(value: &mut Value) {
    match value {
        Value::Object(object) => {
            for field in [
                "observation_id",
                "current_finding_ids",
                "citable_observation_ids",
            ] {
                object.remove(field);
            }
            object.values_mut().for_each(remove_bookkeeping);
        }
        Value::Array(values) => values.iter_mut().for_each(remove_bookkeeping),
        _ => {}
    }
}

fn inherit_metadata(value: &mut Value, inherited: &BTreeMap<String, Value>) {
    match value {
        Value::Object(object) => {
            let mut next = inherited.clone();
            for field in ["measured_at_unix_ms", "window", "origin", "source"] {
                if let Some(current) = object.get(field).cloned() {
                    if inherited.get(field) == Some(&current) {
                        object.remove(field);
                    }
                    next.insert(field.into(), current);
                }
            }
            for child in object.values_mut() {
                inherit_metadata(child, &next);
            }
        }
        Value::Array(values) => {
            for child in values {
                inherit_metadata(child, inherited);
            }
        }
        _ => {}
    }
}

/// Add display values to both current collectors and older recorded scenarios. Unknown scales
/// and unavailable values are left untouched, rather than guessing their unit or a zero.
pub(crate) fn add_readable_values(value: &mut Value) {
    match value {
        Value::Object(object) => {
            if let (Some(values), Some(units)) = (object.get("values"), object.get("units")) {
                if let (Some(values), Some(units)) = (values.as_object(), units.as_object()) {
                    let readable: Map<String, Value> = values
                        .iter()
                        .filter_map(|(entity, metrics)| {
                            let metrics: Map<String, Value> = metrics
                                .as_object()?
                                .iter()
                                .filter_map(|(metric, raw)| {
                                    let amount = raw.as_u64()?;
                                    let display = serde_json::from_value::<Metric>(json!(metric))
                                        .ok()
                                        .map(|known| known.format_value(amount))
                                        .or_else(|| {
                                            readable_unit(
                                                amount as f64,
                                                units.get(metric)?.as_str()?,
                                            )
                                        })?;
                                    Some((metric.clone(), json!(display)))
                                })
                                .collect();
                            Some((entity.clone(), Value::Object(metrics)))
                        })
                        .collect();
                    if !readable.is_empty() {
                        object.insert("readable_values".into(), Value::Object(readable));
                    }
                }
            }
            if object.get("status").is_none_or(|status| status == "ok") {
                if let (Some(amount), Some(unit)) = (
                    object.get("value").and_then(Value::as_f64),
                    object.get("unit").and_then(Value::as_str),
                ) {
                    if let Some(display) = readable_unit(amount, unit) {
                        object.insert("display_value".into(), json!(display));
                    }
                }
            }
            for child in object.values_mut() {
                add_readable_values(child);
            }
        }
        Value::Array(values) => values.iter_mut().for_each(add_readable_values),
        _ => {}
    }
}

fn readable_unit(amount: f64, unit: &str) -> Option<String> {
    if !amount.is_finite() {
        return None;
    }
    Some(match unit {
        "bytes" if amount >= 0.0 && amount < u64::MAX as f64 => format_bytes(amount as u64),
        "bytes_per_second" if amount >= 0.0 && amount < u64::MAX as f64 => {
            format!("{}/s", format_bytes(amount as u64))
        }
        "percent" => format!("{amount:.1}%"),
        "basis_points" | "basis_points_of_total_cpu" | "percent_x100" => {
            format!("{:.1}%", amount / 100.0)
        }
        "microseconds" => format!("{:.1} ms", amount / 1000.0),
        "milliseconds" => format!("{amount:.1} ms"),
        "seconds" => format!("{amount:.1} s"),
        "pages_per_second_x100" => format!("{:.1} pages/s", amount / 100.0),
        "celsius" | "degrees_celsius" => format!("{amount:.1} °C"),
        "milliwatts" => format!("{:.1} W", amount / 1000.0),
        "milliwatt_hours" => format!("{:.2} Wh", amount / 1000.0),
        "millivolts" => format!("{:.2} V", amount / 1000.0),
        "megahertz" => format!("{amount:.1} MHz"),
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn raw_values_survive_scaling_and_missing_values_have_no_display() {
        let mut payload = json!({"measurements":{"values":{"system":{"hard_fault_pages_per_second_x100":238300,"cpu_busy_basis_points":8100}},
            "units":{"hard_fault_pages_per_second_x100":"pages_per_second_x100","cpu_busy_basis_points":"basis_points"}},
            "observations":[{"value":2147483648_u64,"unit":"bytes","status":"ok"},
                {"value":null,"unit":"celsius","status":"unsupported"}]});
        add_readable_values(&mut payload);
        assert_eq!(
            payload["measurements"]["values"]["system"]["hard_fault_pages_per_second_x100"],
            238300
        );
        assert_eq!(
            payload["measurements"]["readable_values"]["system"]
                ["hard_fault_pages_per_second_x100"],
            "2383.0 pages/s"
        );
        assert_eq!(payload["observations"][0]["display_value"], "2.00 GiB");
        assert!(payload["observations"][1].get("display_value").is_none());
    }

    #[test]
    fn only_identical_dates_windows_coverage_and_values_reference_an_earlier_result() {
        let mut context = ToolContext::default();
        let first = json!({"observation_id":"O1","measured_at_unix_ms":100,"window":{"duration_ms":1000},"status":"ok","value":50});
        assert_eq!(context.message(&first), first);
        let mut repeated = first.clone();
        repeated["observation_id"] = json!("O2");
        assert_eq!(context.message(&repeated)["same_result_as"], "O1");
        for (field, value) in [
            ("measured_at_unix_ms", json!(101)),
            ("window", json!({"duration_ms":2000})),
            ("status", json!("partial")),
            ("value", json!(51)),
        ] {
            let mut fresh = repeated.clone();
            fresh[field] = value;
            assert!(context.message(&fresh).get("same_result_as").is_none());
        }
    }

    #[test]
    fn older_large_context_preserves_raw_evidence_and_newest_reply() {
        let row = json!({"measured_at_unix_ms":100,"window":{"duration_ms":3000},"origin":"live",
            "metric":"temperature","unit":"celsius","value":52,"status":"ok","source":"fixture"});
        let cloud = json!({"observation_id":"O1","measured_at_unix_ms":100,"window":{"duration_ms":3000},"origin":"live",
            "observations":vec![row;300],"coverage":[{"status":"partial","detail":"missing sensor"}]});
        let newest = json!({"role":"tool","content":cloud.to_string()});
        let mut messages = vec![newest.clone(), newest.clone()];
        ToolContext::default().compact(&mut messages);
        assert_eq!(messages[1], newest);
        let summary: Value =
            serde_json::from_str(messages[0]["content"].as_str().unwrap()).unwrap();
        assert_eq!(summary["observations"][0]["value"], 52);
        assert_eq!(summary["observations"][0]["unit"], "celsius");
        assert_eq!(summary["measured_at_unix_ms"], 100);
        assert_eq!(summary["coverage"], cloud["coverage"]);
        assert!(messages[0]["content"].as_str().unwrap().len() < cloud.to_string().len());
    }
}
