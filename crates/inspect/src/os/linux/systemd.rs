use super::command;
use crate::{CoverageStatus as C, Inspection};
use serde_json::{json, Value};

pub(super) fn units(out: &mut Inspection, limit: usize, user: bool) {
    let scope = if user {
        "systemd_user_units"
    } else {
        "systemd_services"
    };
    let kind = if user { "--user" } else { "--system" };
    let text = match command::run(
        "/usr/bin/busctl",
        &[
            kind,
            "--json=short",
            "call",
            "org.freedesktop.systemd1",
            "/org/freedesktop/systemd1",
            "org.freedesktop.systemd1.Manager",
            "ListUnits",
        ],
    ) {
        Ok(s) => s,
        Err(c) => {
            out.coverage(
                scope,
                c,
                "systemd1 D-Bus ListUnits",
                "The systemd bus is unavailable or access was denied.",
            );
            return;
        }
    };
    let Ok(value) = serde_json::from_str::<Value>(&text) else {
        out.coverage(scope, C::Error, "systemd1 D-Bus", "Invalid D-Bus response.");
        return;
    };
    let Some(rows) = value["data"][0].as_array() else {
        out.coverage(
            scope,
            C::Error,
            "systemd1 D-Bus",
            "Unexpected D-Bus response shape.",
        );
        return;
    };
    let mut count = 0;
    for row in rows
        .iter()
        .filter(|r| r[0].as_str().is_some_and(|n| n.ends_with(".service")))
    {
        count += 1;
        if count > limit {
            continue;
        }
        let entity = out.entity("service", row[0].as_str().map(str::to_owned));
        let state = row[3].as_str().filter(|s| {
            matches!(
                *s,
                "active"
                    | "inactive"
                    | "failed"
                    | "activating"
                    | "deactivating"
                    | "reloading"
                    | "maintenance"
            )
        });
        out.value(
            &entity,
            "service_state",
            json!(state),
            "category",
            "systemd1 D-Bus ListUnits",
        );
        out.value(
            &entity,
            "service_scope",
            if user { "user" } else { "system" },
            "category",
            "systemd1 D-Bus",
        );
    }
    out.value(
        "system",
        "services_enumerated",
        count as u64,
        "count",
        "systemd1 D-Bus ListUnits",
    );
    out.coverage(scope,if count>limit{C::Partial}else{C::Ok},"systemd1 D-Bus ListUnits",
        if count>limit{"Result limit reached; increase the requested limit to inspect more services."}else{"Loaded service names and finite states only; unit descriptions and command lines are omitted."});
}
pub(super) fn startup(out: &mut Inspection) {
    let text = match command::run(
        "/usr/bin/systemctl",
        &[
            "--user",
            "--no-pager",
            "--no-legend",
            "list-unit-files",
            "--type=service",
        ],
    ) {
        Ok(s) => s,
        Err(c) => {
            out.coverage(
                "systemd_user_startup",
                c,
                "systemctl user manager (D-Bus)",
                "User systemd manager unavailable.",
            );
            return;
        }
    };
    for line in text.lines() {
        let mut fields = line.split_whitespace();
        let Some(name) = fields.next().filter(|n| n.ends_with(".service")) else {
            continue;
        };
        let Some(state) = fields.next().filter(|s| {
            matches!(
                *s,
                "enabled"
                    | "enabled-runtime"
                    | "disabled"
                    | "masked"
                    | "static"
                    | "indirect"
                    | "alias"
            )
        }) else {
            continue;
        };
        let entity = out.entity("startup", Some(name.into()));
        let enabled = match state {
            "enabled" | "enabled-runtime" => Some(true),
            "disabled" | "masked" => Some(false),
            _ => None,
        };
        out.value(
            &entity,
            "startup_enabled",
            json!(enabled),
            "boolean",
            "systemctl list-unit-files",
        );
        out.value(
            &entity,
            "startup_kind",
            "systemd_user",
            "category",
            "systemctl list-unit-files",
        );
        out.value(
            &entity,
            "startup_scope",
            "user",
            "category",
            "systemctl list-unit-files",
        );
    }
    out.coverage("systemd_user_startup",C::Partial,"systemctl list-unit-files","Enablement is not proof of execution at login; static/indirect units have no independent enabled state.");
}
