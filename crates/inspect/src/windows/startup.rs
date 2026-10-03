use super::{com_error, error, registry::Key};
use crate::{
    parse::{logon_task_enabled, startup_enabled},
    CoverageStatus as C, Inspection,
};
use ::windows::{
    core::BSTR,
    Win32::{
        Foundation::VARIANT_BOOL,
        System::{Com::*, TaskScheduler::*, Variant::*},
    },
};
use serde_json::Value;
use std::{
    path::PathBuf,
    time::{Duration, Instant},
};
use windows_sys::Win32::System::Registry::*;

const SOURCE: &str = "Registry.Run/RunOnce/StartupApproved";
pub(super) fn collect(out: &mut Inspection) {
    for (scope, root, view, approval) in [
        ("user_64", HKEY_CURRENT_USER, KEY_WOW64_64KEY, "Run"),
        ("machine_64", HKEY_LOCAL_MACHINE, KEY_WOW64_64KEY, "Run"),
        ("machine_32", HKEY_LOCAL_MACHINE, KEY_WOW64_32KEY, "Run32"),
    ] {
        for kind in ["Run", "RunOnce"] {
            let path = format!(r"SOFTWARE\Microsoft\Windows\CurrentVersion\{kind}");
            let key = match Key::open(root, &path, view) {
                Ok(Some(k)) => k,
                Ok(None) => {
                    out.coverage(
                        &format!("{scope}_{kind}"),
                        C::Ok,
                        SOURCE,
                        "No registry startup entries.",
                    );
                    continue;
                }
                Err(e) => {
                    out.coverage(
                        &format!("{scope}_{kind}"),
                        error(e),
                        SOURCE,
                        "Startup key unreadable.",
                    );
                    continue;
                }
            };
            let approved = Key::open(
                root,
                &format!(
                    r"SOFTWARE\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\{approval}"
                ),
                KEY_WOW64_64KEY,
            )
            .ok()
            .flatten();
            match key.names(false, 500) {
                Ok((names, capped)) => {
                    let mut unknown = false;
                    for name in names {
                        let state = if kind == "RunOnce" {
                            None
                        } else {
                            startup_enabled(
                                approved
                                    .as_ref()
                                    .and_then(|k| k.bytes(&name, RRF_RT_REG_BINARY).ok().flatten())
                                    .as_deref(),
                            )
                        };
                        unknown |= state.is_none();
                        let entity = out.entity("startup", Some(name));
                        out.value(
                            &entity,
                            "startup_enabled",
                            state.map(Value::from).unwrap_or(Value::Null),
                            "boolean",
                            SOURCE,
                        );
                        out.value(&entity, "startup_kind", kind, "category", SOURCE);
                        out.value(&entity, "startup_scope", scope, "category", SOURCE);
                    }
                    out.coverage(&format!("{scope}_{kind}"),if capped||unknown {C::Partial}else{C::Ok},SOURCE,if capped {"Entry limit reached."} else if unknown {"An absent or unrecognized StartupApproved value does not prove enabled or disabled; RunOnce has no equivalent approval state."} else {"Only entry names and approval states were read; command values were never read."});
                }
                Err(e) => out.coverage(
                    &format!("{scope}_{kind}"),
                    error(e),
                    SOURCE,
                    "Startup entry enumeration failed.",
                ),
            }
        }
    }
    for (scope, env, root) in [
        ("user_folder", "APPDATA", HKEY_CURRENT_USER),
        ("common_folder", "PROGRAMDATA", HKEY_LOCAL_MACHINE),
    ] {
        let Some(base) = std::env::var_os(env) else {
            out.coverage(
                scope,
                C::Unsupported,
                "Startup folder",
                "Known local folder unavailable.",
            );
            continue;
        };
        let path = PathBuf::from(base).join(r"Microsoft\Windows\Start Menu\Programs\Startup");
        if !super::cleanup::safe_root(&path).unwrap_or(false) {
            out.coverage(
                scope,
                C::Partial,
                "Startup folder metadata",
                "Nonlocal, reparse-backed or unreadable startup folder was not traversed.",
            );
            continue;
        }
        let approved = Key::open(
            root,
            r"SOFTWARE\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\StartupFolder",
            KEY_WOW64_64KEY,
        )
        .ok()
        .flatten();
        let entries = match std::fs::read_dir(&path) {
            Ok(d) => d,
            Err(e) => {
                out.coverage(
                    scope,
                    if e.kind() == std::io::ErrorKind::NotFound {
                        C::Ok
                    } else if e.kind() == std::io::ErrorKind::PermissionDenied {
                        C::NeedsAdmin
                    } else {
                        C::Error
                    },
                    "Startup folder",
                    "Startup folder unavailable or empty.",
                );
                continue;
            }
        };
        let mut partial = false;
        for (index, item) in entries.enumerate() {
            if index >= 500 {
                partial = true;
                break;
            }
            let Ok(item) = item else {
                partial = true;
                continue;
            };
            let name = item.file_name().to_string_lossy().to_string();
            if name.eq_ignore_ascii_case("desktop.ini") {
                continue;
            }
            let state = startup_enabled(
                approved
                    .as_ref()
                    .and_then(|k| k.bytes(&name, RRF_RT_REG_BINARY).ok().flatten())
                    .as_deref(),
            );
            partial |= state.is_none();
            let entity = out.entity("startup", Some(name));
            out.value(
                &entity,
                "startup_enabled",
                state.map(Value::from).unwrap_or(Value::Null),
                "boolean",
                SOURCE,
            );
            out.value(
                &entity,
                "startup_kind",
                "startup_folder",
                "category",
                "Startup folder metadata",
            );
            out.value(
                &entity,
                "startup_scope",
                scope,
                "category",
                "Startup folder metadata",
            );
        }
        out.coverage(
            scope,
            if partial { C::Partial } else { C::Ok },
            "Startup folder metadata",
            "Shortcut contents and targets were not read; missing approval state remains unknown.",
        );
    }
    tasks(out);
}

fn index_variant(index: i32) -> VARIANT {
    let mut v = VARIANT::default();
    // SAFETY: construct the tagged integer variant used by COM's one-based collections.
    unsafe {
        let inner = &mut *v.Anonymous.Anonymous;
        inner.vt = VT_I4;
        inner.Anonymous.lVal = index;
    }
    v
}
fn tasks(out: &mut Inspection) {
    const SOURCE: &str = "TaskScheduler.COM (logon triggers only)";
    let result = (|| -> ::windows::core::Result<bool> {
        // SAFETY: local service, empty credentials, initialized COM apartment; read-only methods only.
        let root = unsafe {
            let service: ITaskService =
                CoCreateInstance(&TaskScheduler, None, CLSCTX_INPROC_SERVER)?;
            let empty = VARIANT::default();
            service.Connect(&empty, &empty, &empty, &empty)?;
            service.GetFolder(&BSTR::from("\\"))?
        };
        let mut pending = vec![root];
        let mut count = 0;
        let mut folders = 0;
        let mut partial = false;
        let start = Instant::now();
        while let Some(folder) = pending.pop() {
            folders += 1;
            if folders > 128 || count >= 2048 || start.elapsed() > Duration::from_secs(3) {
                return Ok(true);
            }
            // SAFETY: COM objects stay on this initialized thread; bounded collection indices.
            unsafe {
                match folder.GetTasks(TASK_ENUM_HIDDEN.0) {
                    Ok(tasks) => {
                        let total = tasks.Count()?;
                        for index in 1..=total {
                            count += 1;
                            if count > 2048 || start.elapsed() > Duration::from_secs(3) {
                                return Ok(true);
                            }
                            let task = match tasks.get_Item(&index_variant(index)) {
                                Ok(t) => t,
                                Err(_) => {
                                    partial = true;
                                    continue;
                                }
                            };
                            let triggers = match task.Definition().and_then(|d| d.Triggers()) {
                                Ok(t) => t,
                                Err(_) => {
                                    partial = true;
                                    continue;
                                }
                            };
                            let mut trigger_count = 0;
                            triggers.Count(&mut trigger_count)?;
                            let mut triggers_complete = (0..=100).contains(&trigger_count);
                            partial |= !triggers_complete;
                            let mut logon = false;
                            let mut active_trigger = false;
                            for i in 1..=trigger_count.min(100) {
                                let trigger = match triggers.get_Item(i) {
                                    Ok(trigger) => trigger,
                                    Err(_) => {
                                        triggers_complete = false;
                                        partial = true;
                                        continue;
                                    }
                                };
                                let mut kind = TASK_TRIGGER_TYPE2::default();
                                if trigger.Type(&mut kind).is_err() {
                                    triggers_complete = false;
                                    partial = true;
                                    continue;
                                }
                                if kind == TASK_TRIGGER_LOGON {
                                    logon = true;
                                    let mut enabled = VARIANT_BOOL::default();
                                    if trigger.Enabled(&mut enabled).is_ok() {
                                        active_trigger |= enabled.0 != 0;
                                    } else {
                                        triggers_complete = false;
                                        partial = true;
                                    }
                                }
                            }
                            if logon {
                                let entity =
                                    out.entity("startup", task.Name().ok().map(|n| n.to_string()));
                                let enabled = logon_task_enabled(
                                    task.Enabled().ok().map(|v| v.0 != 0),
                                    active_trigger,
                                    triggers_complete,
                                );
                                partial |= enabled.is_none();
                                out.value(
                                    &entity,
                                    "startup_enabled",
                                    enabled.map(Value::from).unwrap_or(Value::Null),
                                    "boolean",
                                    SOURCE,
                                );
                                out.value(
                                    &entity,
                                    "startup_kind",
                                    "scheduled_logon_task",
                                    "category",
                                    SOURCE,
                                );
                            }
                        }
                    }
                    Err(_) => partial = true,
                }
                match folder.GetFolders(0) {
                    Ok(children) => {
                        let total = children.Count()?;
                        // Include both visited and queued folders in the budget, so COM objects
                        // cannot accumulate across wide branches before the visit limit fires.
                        let remaining = 128_usize.saturating_sub(folders + pending.len());
                        if total < 0 || total as usize > remaining {
                            partial = true;
                        }
                        for i in 1..=total.min(remaining as i32) {
                            match children.get_Item(&index_variant(i)) {
                                Ok(f) => pending.push(f),
                                Err(_) => partial = true,
                            }
                        }
                    }
                    Err(_) => partial = true,
                }
            }
        }
        Ok(partial)
    })();
    match result {Ok(partial)=>out.coverage("scheduled_logon_tasks",if partial{C::Partial}else{C::Ok},SOURCE,"Read task names, enabled states and trigger types only. Protected tasks, trigger conditions and actions are not inspected."),Err(e)=>out.coverage("scheduled_logon_tasks",com_error(&e),SOURCE,"Task Scheduler could not be queried with the current account.")}
}
