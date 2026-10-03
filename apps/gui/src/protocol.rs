//! Portable C2/C3 payload projection, separate from Tauri and from frontend rendering.
use serde_json::{json, Value};

pub fn identity_matches(legacy: u64, stable: &str, id: Option<u64>, turn_id: Option<&str>) -> bool {
    (id.is_some() || turn_id.is_some())
        && id.is_none_or(|id| id == legacy)
        && turn_id.is_none_or(|id| id == stable)
}

/// The same stable ID is returned at start, used by every event and persisted on completion.
/// Keep the legacy report as well so existing clients can reopen the stored conversation.
pub fn turn(id: &str, question: &str, asked_at: u64, report: &Value) -> Value {
    json!({"id":id, "question":question, "asked_at_unix_ms":asked_at,
        "answer":report["answer"], "unverified":report["answer"]["unverified"].as_array().cloned().unwrap_or_default(),
        "observations":report["observations"].as_array().cloned().unwrap_or_default(),
        "tool_events":report["tool_events"].as_array().cloned().unwrap_or_default(),
        "usage":report["usage"], "cost":report["usage"]["cost"], "status":report["execution"],
        "error":report["error"], "report":report})
}

pub fn step(turn_id: &str, at: u64, step: &impl serde::Serialize) -> Value {
    json!({"turn_id":turn_id,"at_unix_ms":at,"step":step})
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn stale_mixed_or_absent_identifiers_cannot_control_a_turn() {
        assert!(identity_matches(2, "turn-current", Some(2), None));
        assert!(identity_matches(
            2,
            "turn-current",
            None,
            Some("turn-current")
        ));
        for (id, turn) in [
            (None, None),
            (Some(1), None),
            (None, Some("turn-old")),
            (Some(1), Some("turn-current")),
        ] {
            assert!(!identity_matches(2, "turn-current", id, turn));
        }
    }
    #[test]
    fn completed_turn_payload_preserves_verification_cost_and_tool_events() {
        let report = json!({"answer":{"text":"Measured answer","unverified":[{"kind":"unsupported_number"}]},
            "execution":"completed","observations":[{"citation":"O1"}],"tool_events":[{"tool":"get_services"}],
            "usage":{"cost":{"usd":"0.01"}},"error":null});
        let value = turn("turn-stable", "Question", 7, &report);
        assert_eq!(value["id"], "turn-stable");
        assert_eq!(value["unverified"][0]["kind"], "unsupported_number");
        assert_eq!(value["cost"]["usd"], "0.01");
        assert_eq!(value["tool_events"][0]["tool"], "get_services");
        assert_eq!(value["report"], report);
        assert_eq!(
            step("turn-stable", 8, &json!({"kind":"tool_started"}))["turn_id"],
            "turn-stable"
        );
    }
}
