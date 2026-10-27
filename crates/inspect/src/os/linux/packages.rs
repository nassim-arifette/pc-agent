use super::command;
use crate::{CoverageStatus as C, Inspection, WindowsUpdateMode};
use serde_json::json;

pub(super) fn installed(out: &mut Inspection, limit: usize) {
    let result = command::run(
        "/usr/bin/dpkg-query",
        &["-W", "-f=${binary:Package}\\t${Version}\\n"],
    )
    .or_else(|c| {
        if c == C::Unsupported {
            command::run(
                "/usr/bin/rpm",
                &["-qa", "--qf", "%{NAME}\\t%{VERSION}-%{RELEASE}\\n"],
            )
        } else {
            Err(c)
        }
    });
    match result {
        Ok(text) => {
            let mut count = 0;
            for line in text.lines() {
                let Some((name, version)) = line.split_once('\t') else {
                    continue;
                };
                count += 1;
                if count > limit {
                    continue;
                }
                let entity = out.entity("app", Some(name.into()));
                out.name_value(
                    &entity,
                    "app_version",
                    Some(version.into()),
                    "Local package database",
                );
            }
            out.value(
                "system",
                "installed_packages",
                count as u64,
                "count",
                "Local package database",
            );
            out.coverage("installed_apps",C::Partial,"dpkg-query/rpm","System package database only, including libraries; manually installed applications may not appear. Rows respect the requested result limit.");
        }
        Err(c) => out.coverage(
            "installed_apps",
            c,
            "dpkg-query/rpm",
            "No readable supported package database.",
        ),
    }
}
pub(super) fn updates(out: &mut Inspection, limit: usize, mode: WindowsUpdateMode) {
    if mode == WindowsUpdateMode::None {
        out.coverage(
            "pending_updates",
            C::Unsupported,
            "Local package caches",
            "Update check not requested.",
        );
        return;
    }
    for (scope, program, args) in [
        ("apt", "/usr/bin/apt", vec!["list", "--upgradable"]),
        ("dnf", "/usr/bin/dnf", vec!["--cacheonly", "check-update"]),
        (
            "flatpak",
            "/usr/bin/flatpak",
            vec![
                "remote-ls",
                "--cached",
                "--updates",
                "--columns=application",
            ],
        ),
    ] {
        match command::run(program, &args) {
            Ok(text) => {
                let mut count = 0;
                for line in text.lines() {
                    let name = match scope {
                        "apt" => line.split_once('/').map(|(n, _)| n),
                        "dnf" => line
                            .split_whitespace()
                            .next()
                            .filter(|n| n.contains('.') && !n.ends_with(':')),
                        _ => line.split_whitespace().next().filter(|n| n.contains('.')),
                    };
                    let Some(name) = name.filter(|n| {
                        n.bytes()
                            .all(|b| b.is_ascii_alphanumeric() || b"+-._:".contains(&b))
                    }) else {
                        continue;
                    };
                    count += 1;
                    if count > limit {
                        continue;
                    }
                    let entity = out.entity("update", Some(name.into()));
                    out.value(
                        &entity,
                        "update_pending",
                        true,
                        "boolean",
                        "Local package cache",
                    );
                    out.value(
                        &entity,
                        "update_manager",
                        scope,
                        "category",
                        "Local package cache",
                    );
                }
                out.value(
                    "system",
                    &format!("{scope}_pending_updates"),
                    json!(count),
                    "count",
                    "Local package cache",
                );
                out.coverage(scope,C::Partial,"Offline package metadata","No network refresh, install or upgrade. Counts reflect cached metadata only, not repository freshness; rows respect the requested limit.");
            }
            Err(c) => out.coverage(
                scope,
                c,
                "Offline package metadata",
                "Package manager absent or cache unreadable.",
            ),
        }
    }
}
