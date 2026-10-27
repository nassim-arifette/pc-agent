use super::read;
use crate::{CoverageStatus as C, Inspection};
use serde_json::json;
use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
};

#[derive(Default)]
pub(super) struct Desktop {
    pub name: Option<String>,
    pub hidden: Option<bool>,
    pub application: bool,
    pub conditional: bool,
}
pub(super) fn parse(text: &str) -> Option<Desktop> {
    let mut result = Desktop::default();
    let mut group = false;
    let mut found = false;
    let mut keys = BTreeSet::new();
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            group = line == "[Desktop Entry]";
            if group && found {
                return None;
            }
            found |= group;
            continue;
        }
        if !group || line.starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        if matches!(key, "Type" | "Name" | "Hidden") && !keys.insert(key) {
            return None;
        }
        match key {
            "Type" => result.application = value == "Application",
            "Name" => result.name = Some(value.into()),
            "Hidden" => {
                result.hidden = Some(match value {
                    "true" => true,
                    "false" => false,
                    _ => return None,
                })
            }
            "OnlyShowIn" | "NotShowIn" | "TryExec" | "AutostartCondition" => {
                result.conditional = true
            }
            _ => {}
        }
    }
    (found && result.application).then_some(result)
}
pub(super) fn config_dir() -> Option<PathBuf> {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| {
            std::env::var_os("HOME")
                .map(|h| PathBuf::from(h).join(".config"))
                .filter(|p| p.is_absolute())
        })
}
pub(super) fn collect(out: &mut Inspection) {
    let mut roots = Vec::new();
    if let Some(config) = config_dir() {
        roots.push((config.join("autostart"), "user_xdg"));
    }
    let system = std::env::var_os("XDG_CONFIG_DIRS").unwrap_or_else(|| "/etc/xdg".into());
    roots.extend(
        std::env::split_paths(&system)
            .filter(|p| p.is_absolute())
            .map(|p| (p.join("autostart"), "system_xdg")),
    );
    let mut seen = BTreeSet::new();
    for (root, scope) in roots {
        directory(out, &root, scope, &mut seen);
    }
    super::systemd::startup(out);
}
fn directory(
    out: &mut Inspection,
    root: &Path,
    scope: &str,
    seen: &mut BTreeSet<std::ffi::OsString>,
) {
    let entries = match std::fs::read_dir(root) {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            out.coverage(scope, C::Ok, "XDG autostart", "No autostart directory.");
            return;
        }
        Err(_) => {
            out.coverage(
                scope,
                C::Denied,
                "XDG autostart",
                "Autostart directory unreadable.",
            );
            return;
        }
    };
    let mut partial = false;
    for entry in entries {
        let Ok(entry) = entry else {
            partial = true;
            continue;
        };
        if entry.path().extension().is_none_or(|e| e != "desktop")
            || !seen.insert(entry.file_name())
        {
            continue;
        }
        let Some(desktop) = read(&entry.path()).as_deref().and_then(parse) else {
            partial = true;
            continue;
        };
        let entity = out.entity(
            "startup",
            Some(entry.file_name().to_string_lossy().into_owned()),
        );
        out.name_value(
            &entity,
            "startup_display_name",
            desktop.name,
            "XDG Desktop Entry",
        );
        out.value(
            &entity,
            "startup_enabled",
            json!(!desktop.hidden.unwrap_or(false)),
            "boolean",
            "XDG Desktop Entry.Hidden",
        );
        out.value(
            &entity,
            "startup_kind",
            "xdg_autostart",
            "category",
            "XDG autostart",
        );
        out.value(&entity, "startup_scope", scope, "category", "XDG autostart");
        partial |= desktop.conditional;
    }
    out.coverage(scope,if partial{C::Partial}else{C::Ok},"XDG autostart",
        "User entries override matching system filenames. Hidden is the disabled state, not proof of actual execution; desktop/condition filters may apply. Exec and arguments stay local and are never observations.");
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn desktop_projection_never_keeps_exec() {
        let d=parse("[Desktop Entry]\nType=Application\nName=Fixture\nExec=/home/private/run --secret\nHidden=false\n").unwrap();
        assert_eq!(d.hidden, Some(false));
        assert_eq!(d.name.as_deref(), Some("Fixture"));
        assert!(parse("[Desktop Entry]\nType=Application\nHidden=maybe\n").is_none());
        assert!(parse("[Desktop Entry]\nType=Application\nHidden=false\nHidden=true\n").is_none());
    }
}
