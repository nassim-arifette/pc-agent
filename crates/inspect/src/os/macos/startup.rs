//! Startup state is distinct from running state; all labels and paths remain local.
use super::{
    command::{self, ReadOnly},
    native,
};
use crate::{CoverageStatus as C, Inspection};
use serde::Deserialize;
use std::{
    collections::BTreeMap,
    fs,
    io::{self, Read},
    os::unix::fs::OpenOptionsExt,
    path::Path,
};

const SOURCE: &str = "CFPropertyList / launchctl print-disabled";
const MAX_PLIST: u64 = 1024 * 1024;

fn disabled_agents(bytes: &[u8]) -> Option<BTreeMap<String, bool>> {
    let text = std::str::from_utf8(bytes).ok()?;
    let mut lines = text.lines().map(str::trim).filter(|line| !line.is_empty());
    if lines.next()? != "disabled services = {" {
        return None;
    }
    let mut result = BTreeMap::new();
    for line in lines.by_ref() {
        if line == "}" {
            return lines.next().is_none().then_some(result);
        }
        let (label, value) = line.split_once(" => ")?;
        let label: String = serde_json::from_str(label).ok()?;
        if label.is_empty() || label.chars().any(char::is_control) {
            return None;
        }
        let value = match value {
            "true" => true,
            "false" => false,
            _ => return None,
        };
        if result.insert(label, value).is_some() {
            return None;
        }
    }
    None
}

fn read_agent(path: &Path) -> io::Result<(String, bool)> {
    let file = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() || metadata.len() > MAX_PLIST {
        return Err(io::ErrorKind::InvalidData.into());
    }
    let mut bytes = Vec::new();
    file.take(MAX_PLIST + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_PLIST {
        return Err(io::ErrorKind::InvalidData.into());
    }
    native::launch_agent(&bytes).ok_or_else(|| io::ErrorKind::InvalidData.into())
}

fn error_kind(error: &io::Error) -> C {
    if error.kind() == io::ErrorKind::PermissionDenied {
        C::Denied
    } else {
        C::Partial
    }
}

fn agents(
    out: &mut Inspection,
    path: &Path,
    scope: &str,
    overrides: Option<&BTreeMap<String, bool>>,
) {
    let entries = match fs::read_dir(path) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            out.value(scope, "startup_entry_count", 0, "count", SOURCE);
            out.coverage(
                scope,
                C::Ok,
                SOURCE,
                "The optional LaunchAgents directory does not exist.",
            );
            return;
        }
        Err(error) => {
            out.coverage(
                scope,
                error_kind(&error),
                SOURCE,
                "LaunchAgents directory could not be enumerated.",
            );
            return;
        }
    };
    let mut count = 0;
    let mut failed = 0;
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(_) => {
                failed += 1;
                continue;
            }
        };
        if entry
            .path()
            .extension()
            .is_none_or(|extension| extension != "plist")
        {
            continue;
        }
        count += 1;
        match read_agent(&entry.path()) {
            Ok((label, disabled)) => {
                let enabled =
                    overrides.map(|entries| !entries.get(&label).copied().unwrap_or(disabled));
                let entity = out.entity("startup", Some(label));
                out.value(&entity, "startup_enabled", enabled, "boolean", SOURCE);
                out.value(&entity, "startup_kind", "launch_agent", "category", SOURCE);
                out.value(&entity, "startup_scope", scope, "category", SOURCE);
            }
            Err(_) => failed += 1,
        }
    }
    out.value(scope, "startup_entry_count", count, "count", SOURCE);
    out.value(scope, "startup_unreadable_count", failed, "count", SOURCE);
    out.coverage(scope, if failed > 0 || overrides.is_none() { C::Partial } else { C::Ok }, SOURCE,
        "LaunchAgents in the current GUI domain; enabled means not disabled by its plist/launchd override, not currently running. Malformed, linked or oversized files remain unreadable. Commands and paths are never exported.");
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LoginItem {
    name: String,
    hidden: bool,
}

fn login_items(out: &mut Inspection, bytes: &[u8]) -> Option<()> {
    let entries: Vec<LoginItem> = serde_json::from_slice(bytes).ok()?;
    if entries
        .iter()
        .any(|entry| entry.name.is_empty() || entry.name.chars().any(char::is_control))
    {
        return None;
    }
    out.value(
        "login_items",
        "startup_entry_count",
        entries.len() as u64,
        "count",
        "System Events Login Items",
    );
    for entry in entries {
        let entity = out.entity("startup", Some(entry.name));
        out.value(
            &entity,
            "startup_enabled",
            true,
            "boolean",
            "System Events Login Items",
        );
        out.value(
            &entity,
            "startup_hidden",
            entry.hidden,
            "boolean",
            "System Events Login Items",
        );
        out.value(
            &entity,
            "startup_kind",
            "login_item",
            "category",
            "System Events Login Items",
        );
        out.value(
            &entity,
            "startup_scope",
            "current_user",
            "category",
            "System Events Login Items",
        );
    }
    out.coverage("login_items", C::Partial, "System Events Login Items",
        "Legacy Open at Login entries only. Hidden means a hidden window, not disabled. Third-party SMAppService background permissions have no public enumeration API here; inspect them in System Settings.");
    Some(())
}

pub(super) fn collect() -> Inspection {
    let mut out = Inspection::new();
    let overrides = match command::run(ReadOnly::DisabledAgents) {
        Ok(bytes) => {
            let entries = disabled_agents(&bytes);
            if entries.is_none() {
                out.coverage(
                    "launchd_overrides",
                    C::Error,
                    SOURCE,
                    "Unknown launchctl response; enabled states remain unknown.",
                );
            }
            entries
        }
        Err(command::Failure(status)) => {
            out.coverage(
                "launchd_overrides",
                status,
                SOURCE,
                "GUI-domain overrides could not be read; enabled states remain unknown.",
            );
            None
        }
    };
    if let Some(home) = native::home_directory() {
        agents(
            &mut out,
            &home.join("Library/LaunchAgents"),
            "user_launch_agents",
            overrides.as_ref(),
        );
    } else {
        out.coverage(
            "user_launch_agents",
            C::Error,
            SOURCE,
            "Current-user home directory is unavailable.",
        );
    }
    agents(
        &mut out,
        Path::new("/Library/LaunchAgents"),
        "local_launch_agents",
        overrides.as_ref(),
    );
    agents(
        &mut out,
        Path::new("/System/Library/LaunchAgents"),
        "system_launch_agents",
        overrides.as_ref(),
    );
    match command::run(ReadOnly::LoginItems) {
        Ok(bytes) if login_items(&mut out, &bytes).is_some() => {}
        Ok(_) => out.coverage("login_items", C::Error, "System Events Login Items", "Unknown response; no login state inferred."),
        Err(command::Failure(status)) => out.coverage("login_items", status, "System Events Login Items",
            "Automation permission may be required. Denied, failed or timed-out reads are not an empty login-item list."),
    }
    out.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disabled_parser_rejects_incomplete_ambiguous_and_new_formats() {
        let values = disabled_agents(
            b"disabled services = {\n \"first\" => true\n \"second\" => false\n}\n",
        )
        .unwrap();
        assert!(values["first"]);
        assert!(!values["second"]);
        for invalid in [
            "",
            "disabled services = {\n",
            "disabled services = {\n\"x\" => true\n\"x\" => false\n}",
            "disabled services = {\n\"x\" => maybe\n}",
            "disabled services = {\n}\nextra",
        ] {
            assert!(disabled_agents(invalid.as_bytes()).is_none());
        }
    }

    #[test]
    fn plist_reads_only_label_and_typed_disabled_state() {
        let bytes = br#"<?xml version="1.0"?><plist version="1.0"><dict>
          <key>Label</key><string>private.name</string><key>Disabled</key><true/>
          <key>ProgramArguments</key><array><string>/private/person/secret</string></array>
        </dict></plist>"#;
        assert_eq!(
            native::launch_agent(bytes),
            Some(("private.name".into(), true))
        );
        assert!(
            native::launch_agent(b"<plist><dict><key>Disabled</key><true/></dict></plist>")
                .is_none()
        );
        assert!(native::launch_agent(b"not a plist").is_none());
    }

    #[test]
    fn hidden_login_items_are_enabled_and_names_are_never_implicitly_exported() {
        let mut out = Inspection::new();
        login_items(&mut out, br#"[{"name":"private.person","hidden":true}]"#).unwrap();
        assert!(out
            .observations
            .iter()
            .any(|o| o.metric == "startup_enabled" && o.value == true));
        assert!(!serde_json::to_string(&out)
            .unwrap()
            .contains("private.person"));
        assert!(login_items(
            &mut out,
            br#"[{"name":"x","hidden":false,"path":"/secret"}]"#
        )
        .is_none());
    }

    #[test]
    fn linked_and_oversized_plists_are_not_read() {
        let root = std::env::temp_dir().join(pcagent_shared::new_id("launch-fixture"));
        fs::create_dir(&root).unwrap();
        let file = root.join("large.plist");
        fs::File::create(&file)
            .unwrap()
            .set_len(MAX_PLIST + 1)
            .unwrap();
        assert!(read_agent(&file).is_err());
        std::os::unix::fs::symlink(&file, root.join("link.plist")).unwrap();
        assert!(read_agent(&root.join("link.plist")).is_err());
        fs::remove_dir_all(root).unwrap();
    }
}
