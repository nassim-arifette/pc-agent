//! Pure C5 projection of supplied Windows inspection observations.
use crate::{PrivacyContext, Sharing, ToolOutput};
use pcagent_inspect::{CoverageStatus, Inspection};
use pcagent_shared::Status;
use serde_json::{json, Map, Value};
use std::collections::BTreeSet;

const MAX_CLOUD_OBSERVATIONS: usize = 500;

pub fn project(inspection: &Inspection, sharing: Sharing, privacy: &PrivacyContext) -> ToolOutput {
    let retained =
        &inspection.observations[..inspection.observations.len().min(MAX_CLOUD_OBSERVATIONS)];
    let omitted = inspection.observations.len() - retained.len();
    let status = if omitted > 0 {
        Status::Partial
    } else {
        inspection.status
    };
    // Bound the projection before serialization; keep the native inspection intact for local use.
    let mut cloud = json!({
        "status": status,
        "measured_at_unix_ms": inspection.measured_at_unix_ms,
        "window": inspection.window,
        "origin": inspection.origin,
        "observations": retained,
        "coverage": inspection.coverage,
    });
    // Names are never embedded in records. This map can be removed in full when a later turn
    // changes from complete to anonymized sharing.
    if sharing == Sharing::Complete {
        let mut references: BTreeSet<&str> =
            retained.iter().map(|o| o.entity_ref.as_str()).collect();
        for observation in retained {
            if observation.unit == "opaque_ref" {
                if let Some(reference) = observation.value.as_str() {
                    references.insert(reference);
                }
            }
        }
        let names: Map<String, Value> = inspection
            .names
            .iter()
            .filter(|(id, _)| {
                references.contains(id.as_str())
                    || id
                        .strip_suffix("-display")
                        .is_some_and(|entity| references.contains(entity))
            })
            .filter_map(|(id, name)| Some((id.clone(), Value::from(privacy.shared_name(name)?))))
            .collect();
        if !names.is_empty() {
            cloud["shared_names"] = Value::Object(names);
        }
    }
    let mut limitations: Vec<String> = inspection
        .coverage
        .iter()
        .filter(|c| c.status != CoverageStatus::Ok)
        .map(|c| format!("{}: {}", c.scope, c.detail))
        .collect();
    if omitted > 0 {
        let detail = format!(
            "Cloud result limited to {} observations; {omitted} observations omitted. The local inspection remains complete at its native coverage level.",
            retained.len()
        );
        cloud["projection"] = json!({
            "truncated": true,
            "observation_limit": MAX_CLOUD_OBSERVATIONS,
            "total_observations": inspection.observations.len(),
            "returned_observations": retained.len(),
            "omitted_observations": omitted,
        });
        cloud["coverage"]
            .as_array_mut()
            .expect("coverage array")
            .push(json!({
                "scope": "cloud_projection",
                "status": CoverageStatus::Partial,
                "source": "Agent bounded projection",
                "detail": detail,
            }));
        limitations.push(format!("cloud_projection: {detail}"));
    }
    privacy.sanitize_value(&mut cloud);
    ToolOutput {
        status,
        inspection_complete: retained.iter().any(|o| {
            matches!(o.status, CoverageStatus::Ok | CoverageStatus::Partial) && !o.value.is_null()
        }),
        cloud,
        evidence: Vec::new(),
        findings: Vec::new(),
        processes: Vec::new(),
        storage_items: Vec::new(),
        limitations,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::system::TOOLS;

    fn fixture(count: usize) -> Inspection {
        let window = json!({"started_at_unix_ms":1,"finished_at_unix_ms":1,"duration_ms":0});
        serde_json::from_value(json!({
            "status":"ok", "measured_at_unix_ms":1, "window":window,
            "origin":"synthetic_fixture", "coverage":[],
            "observations": (0..count).map(|index| json!({
                "observation_id": format!("measurement-{index}"),
                "measured_at_unix_ms": 1, "window":window,
                "entity_ref": format!("entity-{index}"),
                "metric":"enabled", "value":true, "unit":"boolean", "status":"ok",
                "source":"fixture", "origin":"synthetic_fixture",
            })).collect::<Vec<_>>(),
        }))
        .unwrap()
    }
    #[test]
    fn all_schemas_are_closed_and_queries_cannot_be_injected() {
        for spec in TOOLS {
            assert_eq!(
                spec.definition()["function"]["parameters"]["additionalProperties"],
                false
            );
            assert!(spec.validate(r#"{"command":"whoami"}"#).is_err());
        }
        let query = TOOLS.iter().find(|t| t.name == "query_system").unwrap();
        for bad in [
            r#"{"query":"SELECT * FROM Win32_Process"}"#,
            r#"{"query":"HKEY_USERS"}"#,
            r#"{"query":"pagefile","limit":101}"#,
        ] {
            assert!(query.validate(bad).is_err());
        }
        assert!(query.validate(r#"{"query":"pagefile","limit":10}"#).is_ok());
    }
    #[test]
    fn names_are_consent_gated_and_sanitized() {
        let mut inspection = fixture(1);
        inspection
            .names
            .insert("entity-0".into(), "Example Service".into());
        let privacy =
            PrivacyContext::from_identifiers("C:\\Users\\Alice", "Alice", "EXAMPLE-PC", &[]);
        let anonymized = project(&inspection, Sharing::Anonymized, &privacy);
        assert!(anonymized.cloud.get("shared_names").is_none());
        let complete = project(&inspection, Sharing::Complete, &privacy);
        assert_eq!(
            complete.cloud["shared_names"]["entity-0"],
            "Example Service"
        );
    }

    #[test]
    fn cloud_limit_preserves_local_data_and_only_retains_referenced_names() {
        let mut inspection = fixture(MAX_CLOUD_OBSERVATIONS + 1);
        for index in 0..inspection.observations.len() {
            inspection
                .names
                .insert(format!("entity-{index}"), format!("Program {index}"));
        }
        inspection
            .names
            .insert("entity-0-display".into(), "Display name".into());
        inspection
            .names
            .insert("entity-0-publisher".into(), "Publisher".into());
        inspection
            .names
            .insert("orphan".into(), "Unused name".into());
        inspection.observations[0].unit = "opaque_ref".into();
        inspection.observations[0].value = Value::from("entity-0-publisher");
        let before = serde_json::to_value(&inspection).unwrap();
        let names_before = inspection.names.clone();
        let privacy = PrivacyContext::from_identifiers("", "", "", &[]);
        let output = project(&inspection, Sharing::Complete, &privacy);
        assert_eq!(
            output.cloud["observations"].as_array().unwrap().len(),
            MAX_CLOUD_OBSERVATIONS
        );
        assert_eq!(output.status, Status::Partial);
        assert_eq!(output.cloud["status"], "partial");
        assert_eq!(output.cloud["projection"]["omitted_observations"], 1);
        assert_eq!(output.cloud["coverage"][0]["status"], "partial");
        assert_eq!(output.limitations.len(), 1);
        let names = output.cloud["shared_names"].as_object().unwrap();
        assert_eq!(names.len(), MAX_CLOUD_OBSERVATIONS + 2);
        assert!(names.contains_key("entity-0-display"));
        assert!(names.contains_key("entity-0-publisher"));
        assert!(!names.contains_key("entity-500"));
        assert!(!names.contains_key("orphan"));
        assert_eq!(serde_json::to_value(&inspection).unwrap(), before);
        assert_eq!(inspection.names, names_before);

        inspection.observations.truncate(MAX_CLOUD_OBSERVATIONS);
        let exact = project(&inspection, Sharing::Anonymized, &privacy);
        assert_eq!(exact.status, Status::Ok);
        assert!(exact.cloud.get("projection").is_none());
        assert!(exact.cloud.get("shared_names").is_none());
    }
}
