use super::command::{self, ReadOnly};
use crate::{CoverageStatus as C, Inspection};

const SOURCE: &str = "launchctl list (current caller bootstrap domain)";

fn project(text: &str, limit: usize) -> Inspection {
    let mut out = Inspection::new();
    let mut lines = text.lines();
    if lines.next() != Some("PID\tStatus\tLabel") {
        out.coverage(
            "services",
            C::Error,
            SOURCE,
            "The launchctl table format was not recognized.",
        );
        return out.finish();
    }
    let mut scanned = 0_u64;
    let mut returned = 0;
    let mut unreadable = 0_u64;
    for line in lines.filter(|line| !line.trim().is_empty()) {
        scanned += 1;
        let mut columns = line.splitn(3, '\t');
        let (Some(pid), Some(status), Some(label)) =
            (columns.next(), columns.next(), columns.next())
        else {
            unreadable += 1;
            continue;
        };
        let pid = if pid == "-" {
            Some(None)
        } else {
            pid.parse::<u32>().ok().filter(|pid| *pid > 0).map(Some)
        };
        let status = status.parse::<i32>().ok();
        let (Some(pid), Some(status)) = (pid, status) else {
            unreadable += 1;
            continue;
        };
        if label.is_empty() || label.chars().any(char::is_control) {
            unreadable += 1;
            continue;
        }
        if returned >= limit {
            continue;
        }
        let entity = out.entity("service", Some(label.to_owned()));
        out.value(
            &entity,
            "service_state",
            if pid.is_some() {
                "running"
            } else {
                "not_running"
            },
            "category",
            SOURCE,
        );
        out.value(&entity, "last_exit_status", status, "exit_status", SOURCE);
        returned += 1;
    }
    out.value("services", "enumerated_count", scanned, "count", SOURCE);
    out.value("services", "unreadable_count", unreadable, "count", SOURCE);
    out.value(
        "services",
        "returned_count",
        returned as u64,
        "count",
        SOURCE,
    );
    out.coverage(
        "services",
        if unreadable > 0 || returned as u64 != scanned { C::Partial } else { C::Ok },
        SOURCE,
        "Current caller bootstrap domain only; not a system-wide daemon inventory. Labels are local display names; no command, path or environment is exported. Not running does not mean disabled.",
    );
    out.finish()
}

fn user_services(limit: usize) -> Inspection {
    match command::run(ReadOnly::Services) {
        Ok(bytes) => match std::str::from_utf8(&bytes) {
            Ok(text) => project(text, limit),
            Err(_) => {
                let mut out = Inspection::new();
                out.coverage(
                    "services",
                    C::Error,
                    SOURCE,
                    "The launchctl response was not valid UTF-8.",
                );
                out.finish()
            }
        },
        Err(command::Failure(status)) => {
            let mut out = Inspection::new();
            out.coverage("services", status, SOURCE,
                "The read-only command was denied, unavailable, failed or exceeded its 10-second/8-MiB output budget; no partial table was interpreted as complete.");
            out.finish()
        }
    }
}

fn system_project(text: &str, limit: usize) -> Inspection {
    const SOURCE: &str = "launchctl print system";
    let mut out = Inspection::new();
    let mut lines = text.lines();
    let found = lines.by_ref().any(|line| line.trim() == "services = {");
    let (mut closed, mut scanned, mut malformed, mut returned) = (false, 0_u64, 0_u64, 0_usize);
    if found {
        for line in lines {
            let line = line.trim();
            if line == "}" {
                closed = true;
                break;
            }
            if line.is_empty() {
                continue;
            }
            scanned += 1;
            let fields: Vec<_> = line.split_whitespace().collect();
            if fields.len() != 3 {
                malformed += 1;
                continue;
            }
            let Some(pid) = fields[0].parse::<u32>().ok() else {
                malformed += 1;
                continue;
            };
            let exit = if fields[1] == "-" {
                Some(None)
            } else {
                fields[1].parse::<i32>().ok().map(Some)
            };
            let Some(exit) = exit else {
                malformed += 1;
                continue;
            };
            if returned >= limit {
                continue;
            }
            let entity = out.entity("service", Some(fields[2].into()));
            out.value(&entity, "service_scope", "system", "category", SOURCE);
            out.value(
                &entity,
                "service_state",
                if pid > 0 { "running" } else { "not_running" },
                "category",
                SOURCE,
            );
            out.value(&entity, "last_exit_status", exit, "exit_status", SOURCE);
            returned += 1;
        }
    }
    if !found || !closed {
        out = Inspection::new();
        out.coverage(
            "system_services",
            C::Error,
            SOURCE,
            "Unrecognized or incomplete launchctl response; no complete inventory inferred.",
        );
        return out.finish();
    }
    out.value(
        "system_services",
        "enumerated_count",
        scanned,
        "count",
        SOURCE,
    );
    out.value(
        "system_services",
        "unreadable_count",
        malformed,
        "count",
        SOURCE,
    );
    out.value(
        "system_services",
        "returned_count",
        returned as u64,
        "count",
        SOURCE,
    );
    out.coverage("system_services", if malformed > 0 || returned as u64 != scanned { C::Partial } else { C::Ok }, SOURCE,
        "System bootstrap services only; labels remain local, PID is reduced to running state. An absent exit status stays unknown. New launchctl formats fail closed.");
    out.finish()
}

pub(super) fn collect(limit: usize) -> Inspection {
    let mut out = user_services(limit.div_ceil(2));
    let returned = out.names.len();
    let system = match command::run(ReadOnly::SystemServices) {
        Ok(bytes) => std::str::from_utf8(&bytes)
            .ok()
            .map(|text| system_project(text, limit.saturating_sub(returned))),
        Err(command::Failure(status)) => {
            out.coverage(
                "system_services",
                status,
                "launchctl print system",
                "System domain read failed, was denied, unavailable or exceeded its budget.",
            );
            return out.finish();
        }
    };
    if let Some(system) = system {
        out.observations.extend(system.observations);
        out.names.extend(system.names);
        out.coverage.extend(system.coverage);
    } else {
        out.coverage(
            "system_services",
            C::Error,
            "launchctl print system",
            "Invalid UTF-8 response.",
        );
    }
    out.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn system_table_excludes_unrelated_output_and_keeps_unknown_exit_status() {
        let result = system_project("system = {\n environment = {\n PRIVATE=secret\n }\n services = {\n 0 - private.label\n 42 0 running.label\n }\n paths = /private/home\n}", 10);
        assert_eq!(result.names.len(), 2);
        assert!(result
            .observations
            .iter()
            .any(|o| o.metric == "last_exit_status" && o.value.is_null()));
        let json = serde_json::to_string(&result).unwrap();
        for secret in ["secret", "private.label", "running.label", "/private/home"] {
            assert!(!json.contains(secret));
        }
        assert!(system_project("services = {\n 0 - label", 10)
            .observations
            .is_empty());
    }

    #[test]
    fn labels_and_pids_stay_local_and_nonrunning_does_not_mean_disabled() {
        let result = project(
            "PID\tStatus\tLabel\n-\t0\tprivate.person.agent\n42\t-9\tother\n",
            10,
        );
        let json = serde_json::to_string(&result).unwrap();
        assert!(!json.contains("private.person"));
        assert!(!json.contains("\"value\":42"));
        assert_eq!(result.names.len(), 2);
        assert!(result.observations.iter().any(|o| o.value == "not_running"));
    }

    #[test]
    fn malformed_rows_and_requested_limit_are_explicitly_partial() {
        let result = project(
            "PID\tStatus\tLabel\n-\t0\tfirst\nnope\t0\tbad\n4\t0\tlast\n",
            1,
        );
        assert_eq!(result.status, pcagent_shared::Status::Partial);
        assert_eq!(
            result
                .observations
                .iter()
                .find(|o| o.metric == "unreadable_count")
                .unwrap()
                .value,
            1
        );
        assert_eq!(result.names.len(), 1);
        assert!(project("not a table", 10).observations.is_empty());
        assert_eq!(
            project("PID\tStatus\tLabel\n", 10).status,
            pcagent_shared::Status::Ok
        );
    }
}
