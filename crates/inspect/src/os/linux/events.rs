use super::command;
use crate::{CoverageStatus as C, Inspection};
use serde_json::{json, Value};
pub(super) fn collect(out: &mut Inspection, days: u32, limit: usize) {
    let since = format!("{} days ago", days);
    let count = limit.to_string();
    let result = command::run(
        "/usr/bin/journalctl",
        &[
            "--no-pager",
            "--quiet",
            "--output=json",
            "--output-fields=__REALTIME_TIMESTAMP,PRIORITY,_SYSTEMD_UNIT,SYSLOG_IDENTIFIER",
            "--priority=0..4",
            "--since",
            &since,
            "--lines",
            &count,
        ],
    );
    match result {
        Ok(text) => {
            let mut parsed = 0;
            for line in text.lines() {
                let Ok(v) = serde_json::from_str::<Value>(line) else {
                    continue;
                };
                let Some(priority) = v["PRIORITY"]
                    .as_str()
                    .and_then(|s| s.parse::<u8>().ok())
                    .filter(|n| *n <= 4)
                else {
                    continue;
                };
                let entity = out.entity(
                    "event",
                    v["_SYSTEMD_UNIT"]
                        .as_str()
                        .or(v["SYSLOG_IDENTIFIER"].as_str())
                        .map(str::to_owned),
                );
                let time = v["__REALTIME_TIMESTAMP"]
                    .as_str()
                    .and_then(|s| s.parse::<u64>().ok())
                    .map(|t| t / 1000);
                out.value(
                    &entity,
                    "event_priority",
                    priority,
                    "severity",
                    "journald PRIORITY",
                );
                out.value(
                    &entity,
                    "event_timestamp_unix_ms",
                    json!(time),
                    "unix_ms",
                    "journald realtime timestamp",
                );
                parsed += 1;
            }
            out.value(
                "system",
                "events_returned",
                parsed,
                "count",
                "journalctl JSON",
            );
            out.coverage("journald",C::Partial,"journalctl JSON","Only journals readable by this user and priorities 0–4 are returned, up to the requested count. Messages, hostnames, PIDs and user identifiers are not read into observations.");
        }
        Err(c) => out.coverage(
            "journald",
            c,
            "journalctl JSON",
            "Journal unavailable, unreadable, or command exceeded its read bound.",
        ),
    }
}
