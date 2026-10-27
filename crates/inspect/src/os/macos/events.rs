use super::command::{self, ReadOnly};
use crate::{CoverageStatus as C, Inspection};
use pcagent_shared::{unix_ms, Window};
use serde::Deserialize;

const SOURCE: &str = "macOS log show --style json (error/fault predicate)";

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Event {
    #[serde(default)]
    message_type: Option<String>,
    #[serde(default)]
    timestamp: Option<String>,
}

fn project(bytes: &[u8], window: Window, limit: usize) -> Inspection {
    let mut out = Inspection::new();
    out.measured_at_unix_ms = window.finished_at_unix_ms;
    out.window = window;
    let rows: Vec<Event> = match serde_json::from_slice(bytes) {
        Ok(rows) => rows,
        Err(_) => {
            out.coverage("unified_log", C::Error, SOURCE, "The native JSON log response was incomplete or invalid; no zero count was inferred.");
            return out.finish();
        }
    };
    let mut groups: [Vec<u64>; 2] = [Vec::new(), Vec::new()];
    let mut unknown = 0_u64;
    for row in rows {
        let group = match row.message_type.as_deref() {
            Some("Error") => 0,
            Some("Fault") => 1,
            _ => {
                unknown += 1;
                continue;
            }
        };
        let timestamp = row.timestamp.and_then(|timestamp| {
            chrono::DateTime::parse_from_str(&timestamp, "%Y-%m-%d %H:%M:%S%.f%z")
                .ok()
                .and_then(|date| u64::try_from(date.timestamp_millis()).ok())
        });
        match timestamp {
            Some(timestamp)
                if timestamp >= out.window.started_at_unix_ms
                    && timestamp <= out.window.finished_at_unix_ms =>
            {
                groups[group].push(timestamp)
            }
            _ => unknown += 1,
        }
    }
    let mut remaining = limit;
    for (category, mut times) in ["error", "fault"].into_iter().zip(groups) {
        times.sort_unstable_by(|a, b| b.cmp(a));
        let entity = out.entity("events", None);
        out.value(
            &entity,
            "event_source",
            "macos_unified_log",
            "category",
            SOURCE,
        );
        out.value(&entity, "event_severity", category, "category", SOURCE);
        out.value(&entity, "event_count", times.len() as u64, "count", SOURCE);
        if unknown > 0 {
            out.observations.last_mut().expect("count inserted").status = C::Partial;
        }
        times.truncate(remaining);
        remaining -= times.len();
        out.value(
            &entity,
            "latest_occurrences_unix_ms",
            serde_json::json!(times),
            "unix_ms",
            SOURCE,
        );
    }
    out.value("events", "unclassified_records", unknown, "count", SOURCE);
    out.coverage("unified_log", C::Partial, SOURCE,
        "Counts describe only retained error/fault records visible to this caller, not every failure on the machine. At most the requested number of occurrence timestamps is shown. Message text, processes, paths, subsystems, activity IDs and identifiers are discarded.");
    out.finish()
}

pub(super) fn collect(days: u32, limit: usize) -> Inspection {
    let end_seconds = unix_ms() / 1000;
    let start_seconds = end_seconds.saturating_sub(u64::from(days) * 86_400);
    let window = Window {
        started_at_unix_ms: start_seconds * 1000,
        finished_at_unix_ms: end_seconds * 1000,
        duration_ms: (end_seconds - start_seconds) * 1000,
    };
    match command::run(ReadOnly::Events {
        start_seconds,
        end_seconds,
    }) {
        Ok(bytes) => project(&bytes, window, limit),
        Err(command::Failure(status)) => {
            let mut out = Inspection::new();
            out.window = window;
            out.coverage("unified_log", status, SOURCE,
                "The log read was denied, failed or exceeded its 10-second/8-MiB budget. No incomplete output was interpreted as an empty history; narrow the requested period.");
            out.finish()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn window() -> Window {
        Window {
            started_at_unix_ms: 0,
            finished_at_unix_ms: 10_000,
            duration_ms: 10_000,
        }
    }

    #[test]
    fn only_categories_counts_and_requested_timestamps_leave_the_parser() {
        let result = project(br#"[
            {"messageType":"Error","timestamp":"1970-01-01 00:00:01.125000+0000","eventMessage":"secret /Users/alice/document","processID":123},
            {"messageType":"Fault","timestamp":"1970-01-01 00:00:02.000000+0000","senderImageUUID":"private"}
        ]"#, window(), 1);
        let json = serde_json::to_string(&result).unwrap();
        for forbidden in ["secret", "alice", "private", "processID"] {
            assert!(!json.contains(forbidden));
        }
        assert!(result.names.is_empty());
        let occurrences = result
            .observations
            .iter()
            .filter(|o| o.metric == "latest_occurrences_unix_ms")
            .map(|o| o.value.as_array().unwrap().len())
            .sum::<usize>();
        assert_eq!(occurrences, 1);
        assert_eq!(
            result
                .observations
                .iter()
                .find(|o| o.metric == "latest_occurrences_unix_ms")
                .unwrap()
                .value,
            serde_json::json!([1125])
        );
    }

    #[test]
    fn corrupt_and_out_of_window_records_never_establish_complete_zero_counts() {
        assert!(project(b"[{", window(), 10).observations.is_empty());
        let result = project(br#"[{"messageType":"Error","timestamp":"bogus"},{"messageType":"Fault","timestamp":"1970-01-01 00:00:11+0000"}]"#, window(), 10);
        assert!(result
            .observations
            .iter()
            .filter(|o| o.metric == "event_count")
            .all(|o| o.status == C::Partial));
        assert_eq!(
            result
                .observations
                .iter()
                .find(|o| o.metric == "unclassified_records")
                .unwrap()
                .value,
            2
        );
    }
}
