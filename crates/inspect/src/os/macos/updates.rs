use super::command::{self, ReadOnly};
use crate::{CoverageStatus as C, Inspection};

const SOURCE: &str = "softwareupdate --list (read-only available update scan)";

fn version(text: &str) -> Option<&str> {
    (!text.is_empty()
        && text.len() <= 64
        && text
            .split('.')
            .all(|part| !part.is_empty() && part.bytes().all(|c| c.is_ascii_digit())))
    .then_some(text)
}

fn project(text: &str) -> Inspection {
    let mut out = Inspection::new();
    let mut count = 0_u64;
    let mut current = None;
    let mut complete = true;
    let mut has_details = false;
    for line in text.lines().map(str::trim) {
        if line.starts_with("* Label: ") {
            if current.is_some() && !has_details {
                complete = false;
            }
            current = Some(out.entity("update", None));
            has_details = false;
            count += 1;
            continue;
        }
        let Some(entity) = &current else { continue };
        if !line.starts_with("Title: ") {
            continue;
        }
        has_details = true;
        let fields: Vec<_> = line.split(", ").collect();
        let field = |prefix: &str| {
            fields
                .iter()
                .find_map(|field| field.strip_prefix(prefix))
                .map(|v| v.trim_end_matches(','))
        };
        let recommended = match field("Recommended: ") {
            Some("YES") => Some(true),
            Some("NO") => Some(false),
            _ => None,
        };
        let restart = field("Action: ").and_then(|value| match value {
            "restart" => Some(true),
            "none" => Some(false),
            _ => None,
        });
        let size = field("Size: ")
            .and_then(|value| value.strip_suffix("KiB"))
            .and_then(|number| number.trim().parse::<u64>().ok())
            .and_then(|number| number.checked_mul(1024));
        out.value(
            entity,
            "update_version",
            field("Version: ").and_then(version).map(str::to_owned),
            "version",
            SOURCE,
        );
        out.value(entity, "update_recommended", recommended, "boolean", SOURCE);
        out.value(
            entity,
            "update_requires_restart",
            restart,
            "boolean",
            SOURCE,
        );
        out.value(entity, "update_download_bytes", size, "bytes", SOURCE);
        complete &= recommended.is_some();
    }
    if current.is_some() && !has_details {
        complete = false;
    }
    if count == 0
        && !text
            .lines()
            .any(|line| line.trim() == "No new software available.")
    {
        out.coverage("macos_updates", C::Error, SOURCE,
            "No recognized update records or explicit no-update result; an empty or changed response does not prove the system is up to date.");
        return out.finish();
    }
    out.value("system", "available_update_count", count, "count", SOURCE);
    if !complete {
        out.observations.last_mut().expect("count inserted").status = C::Partial;
    }
    out.coverage("macos_updates", if complete { C::Ok } else { C::Partial }, SOURCE,
        "Read-only scan of update availability, not installation. It may contact Apple's configured update service. Labels/titles and raw command output are not exported; absent fields stay unknown. Updates from other vendors are outside this source.");
    out.finish()
}

pub(super) fn collect() -> Inspection {
    match command::run(ReadOnly::Updates) {
        Ok(bytes) => match std::str::from_utf8(&bytes) {
            Ok(text) => project(text),
            Err(_) => {
                let mut out = Inspection::new();
                out.coverage(
                    "macos_updates",
                    C::Error,
                    SOURCE,
                    "The native response was not valid UTF-8.",
                );
                out.finish()
            }
        },
        Err(command::Failure(status)) => {
            let mut out = Inspection::new();
            out.coverage("macos_updates", status, SOURCE,
                "The read-only update scan was denied, unavailable, failed or exceeded its 10-second/8-MiB budget; no clean bill of health was inferred.");
            out.finish()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_no_update_differs_from_empty_or_unrecognized_response() {
        assert_eq!(
            project("No new software available.\n").observations[0].value,
            0
        );
        assert!(project("").observations.is_empty());
        assert!(project("server unavailable").observations.is_empty());
    }

    #[test]
    fn only_typed_fields_survive_and_units_are_checked() {
        let result = project("* Label: private-user-id\nTitle: private-title, Version: 26.6.2, Size: 12KiB, Recommended: YES, Action: restart,\n");
        let json = serde_json::to_string(&result).unwrap();
        assert!(!json.contains("private"));
        assert!(result
            .observations
            .iter()
            .any(|o| o.metric == "update_download_bytes" && o.value == 12288));
        assert!(result
            .observations
            .iter()
            .any(|o| o.metric == "update_requires_restart" && o.value == true));
        assert_eq!(version("name@example.org"), None);
        assert_eq!(version("1..2"), None);
        let missing = project("* Label: private\n");
        assert_eq!(missing.status, pcagent_shared::Status::Partial);
    }
}
