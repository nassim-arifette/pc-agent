use super::{error, events, registry::Key, wide, wmi};
use crate::{CoverageStatus as C, Inspection};
use pcagent_shared::{new_id, Status};
use serde_json::Value;
use std::{
    collections::BTreeMap,
    ptr::{null, null_mut},
};
use windows_sys::Win32::{
    Foundation::{GetLastError, LocalFree},
    System::{Power::*, Registry::*, Services::*, SystemInformation::GetTickCount64},
};

pub(super) fn version(out: &mut Inspection) {
    const SOURCE: &str = "Registry.Windows NT/CurrentVersion";
    match Key::open(
        HKEY_LOCAL_MACHINE,
        r"SOFTWARE\Microsoft\Windows NT\CurrentVersion",
        KEY_WOW64_64KEY,
    ) {
        Ok(Some(key)) => {
            for (field, metric) in [
                ("CurrentBuildNumber", "windows_build"),
                ("DisplayVersion", "windows_display_version"),
            ] {
                out.value(
                    "system",
                    metric,
                    key.string(field)
                        .filter(|v| {
                            v.len() < 32 && v.chars().all(|c| c.is_ascii_alphanumeric() || c == '.')
                        })
                        .map(Value::from)
                        .unwrap_or(Value::Null),
                    "version",
                    SOURCE,
                );
            }
            for (field, metric) in [
                ("CurrentMajorVersionNumber", "windows_major"),
                ("CurrentMinorVersionNumber", "windows_minor"),
                ("UBR", "windows_update_build_revision"),
            ] {
                out.value(
                    "system",
                    metric,
                    key.dword(field).map(Value::from).unwrap_or(Value::Null),
                    "version",
                    SOURCE,
                );
            }
            out.coverage(
                "windows_version",
                C::Ok,
                SOURCE,
                "Build/version fields only; product IDs and registered user fields were not read.",
            );
        }
        Ok(None) => out.coverage(
            "windows_version",
            C::Unsupported,
            SOURCE,
            "Version registry key missing.",
        ),
        Err(e) => out.coverage(
            "windows_version",
            error(e),
            SOURCE,
            "Version registry key unreadable.",
        ),
    }
}
pub(super) fn state(out: &mut Inspection) {
    // SAFETY: GetTickCount64 has no pointers or side effects.
    out.value(
        "system",
        "uptime_seconds",
        unsafe { GetTickCount64() } / 1000,
        "seconds",
        "Win32.GetTickCount64 (includes sleep/hibernation)",
    );
    version(out);
    super::compatibility::collect(out);
    const SOURCE: &str = "Registry reboot markers";
    let mut unknown = false;
    let mut pending = false;
    for (metric, path) in [
        (
            "component_servicing_reboot_pending",
            r"SOFTWARE\Microsoft\Windows\CurrentVersion\Component Based Servicing\RebootPending",
        ),
        (
            "windows_update_reboot_pending",
            r"SOFTWARE\Microsoft\Windows\CurrentVersion\WindowsUpdate\Auto Update\RebootRequired",
        ),
    ] {
        match Key::open(HKEY_LOCAL_MACHINE, path, KEY_WOW64_64KEY) {
            Ok(v) => {
                pending |= v.is_some();
                out.value("system", metric, v.is_some(), "boolean", SOURCE);
            }
            Err(e) => {
                unknown = true;
                out.coverage(metric, error(e), SOURCE, "Reboot marker unreadable.");
            }
        }
    }
    match Key::open(
        HKEY_LOCAL_MACHINE,
        r"SYSTEM\CurrentControlSet\Control\Session Manager",
        KEY_WOW64_64KEY,
    ) {
        Ok(Some(k)) => match k
            .bytes("PendingFileRenameOperations", RRF_RT_REG_MULTI_SZ)
            .map(|value| {
                value.is_some_and(|bytes| bytes.get(..2).is_some_and(|first| first != [0, 0]))
            }) {
            Ok(v) => {
                pending |= v;
                out.value("system", "file_rename_reboot_pending", v, "boolean", SOURCE);
            }
            Err(e) => {
                unknown = true;
                out.coverage(
                    "file_rename_reboot_pending",
                    error(e),
                    SOURCE,
                    "Pending-rename value metadata unreadable.",
                );
            }
        },
        _ => unknown = true,
    }
    out.value(
        "system",
        "reboot_pending",
        if pending {
            Value::Bool(true)
        } else if unknown {
            Value::Null
        } else {
            Value::Bool(false)
        },
        "boolean",
        SOURCE,
    );
    out.coverage("pending_reboot",if unknown{C::Partial}else{C::Ok},SOURCE,"Common servicing/update/rename markers. Pending file paths are discarded after testing for an empty list; reboot age is unknown.");
    match Key::open(
        HKEY_LOCAL_MACHINE,
        r"SYSTEM\CurrentControlSet\Control\Session Manager\Power",
        KEY_WOW64_64KEY,
    ) {
        Ok(Some(k)) => {
            let value = k.dword("HiberbootEnabled");
            out.value(
                "system",
                "fast_startup_configured",
                value.map(|v| Value::Bool(v != 0)).unwrap_or(Value::Null),
                "boolean",
                "Registry.HiberbootEnabled",
            );
            out.coverage("fast_startup",if value.is_some(){C::Ok}else{C::Partial},"Registry.HiberbootEnabled","Configuration only; eligibility also depends on firmware and hibernation availability.");
        }
        _ => out.coverage(
            "fast_startup",
            C::Partial,
            "Registry.HiberbootEnabled",
            "Fast startup configuration unavailable.",
        ),
    }
    wmi::named(out, "last_updates", 100);
    super::updates::collect(out);
    out.coverage("update_inventory_limits",C::Partial,"WMI.Win32_QuickFixEngineering","Lists installed servicing hotfixes only; not all feature/Store/driver updates. Installation dates may be missing or locale-specific. No update-in-progress conclusion is inferred from this list.");
    wmi::named(out, "pagefile", 32);
}

struct Service(SC_HANDLE);
impl Drop for Service {
    fn drop(&mut self) {
        // SAFETY: owned SCM/service handle, closed once.
        unsafe {
            CloseServiceHandle(self.0);
        }
    }
}
pub(super) fn services(out: &mut Inspection, limit: usize) {
    const SOURCE: &str = "Win32.EnumServicesStatusExW/QueryServiceConfigW";
    let list = pcagent_telemetry::service_processes();
    let capped = list.services.len() > limit;
    // SAFETY: local SCM read/connect access, no modifications requested.
    let scm = unsafe { OpenSCManagerW(null(), null(), SC_MANAGER_CONNECT) };
    let manager = (!scm.is_null()).then_some(Service(scm));
    let mut incomplete = false;
    let mut processes = BTreeMap::new();
    for service in list.services.into_iter().take(limit) {
        let entity = out.entity("service", Some(service.service_name.clone()));
        out.names
            .insert(format!("{entity}-display"), service.display_name);
        out.value(&entity, "service_state", service.state, "category", SOURCE);
        out.value(
            &entity,
            "service_has_process",
            service.pid != 0,
            "boolean",
            SOURCE,
        );
        if service.pid != 0 {
            let process = processes
                .entry(service.pid)
                .or_insert_with(|| new_id("service-host"));
            out.value(
                &entity,
                "service_process_ref",
                process.as_str(),
                "opaque_ref",
                SOURCE,
            );
        }
        let start_type = manager.as_ref().and_then(|manager| {
            // SAFETY: existing service name from SCM, read-only configuration access.
            let handle = unsafe {
                OpenServiceW(
                    manager.0,
                    wide(&service.service_name).as_ptr(),
                    SERVICE_QUERY_CONFIG,
                )
            };
            if handle.is_null() {
                return None;
            }
            let handle = Service(handle);
            let mut buffer = [0_usize; 1024];
            let mut needed = 0;
            // SAFETY: aligned 8KiB buffer (maximum documented config size); only the numeric start
            // type is read. Binary paths, accounts and dependencies are never dereferenced.
            unsafe {
                if QueryServiceConfigW(
                    handle.0,
                    buffer.as_mut_ptr().cast(),
                    std::mem::size_of_val(&buffer) as u32,
                    &mut needed,
                ) == 0
                {
                    None
                } else {
                    Some((*buffer.as_ptr().cast::<QUERY_SERVICE_CONFIGW>()).dwStartType)
                }
            }
        });
        incomplete |= start_type.is_none();
        out.value(
            &entity,
            "service_start_type",
            match start_type {
                Some(SERVICE_AUTO_START) => Value::from("automatic"),
                Some(SERVICE_DEMAND_START) => Value::from("manual"),
                Some(SERVICE_DISABLED) => Value::from("disabled"),
                Some(SERVICE_BOOT_START) => Value::from("boot"),
                Some(SERVICE_SYSTEM_START) => Value::from("system"),
                _ => Value::Null,
            },
            "category",
            SOURCE,
        );
    }
    out.coverage("services",if capped||incomplete||list.status!=Status::Ok{C::Partial}else{C::Ok},SOURCE,if capped{"Result limit reached; some services are omitted."}else if incomplete{"Some service configurations are unreadable without additional rights."}else{"Visible Win32 services only; SCM may omit services whose query-status access is denied. Process references group shared hosts within this observation."});
    events::service_failures(out);
}

pub(super) fn power(out: &mut Inspection) {
    const SOURCE: &str = "Win32.GetSystemPowerStatus";
    let mut status = SYSTEM_POWER_STATUS::default();
    // SAFETY: valid initialized fixed-size output structure.
    if unsafe { GetSystemPowerStatus(&mut status) } != 0 {
        out.value(
            "system",
            "ac_power",
            match status.ACLineStatus {
                0 => Value::Bool(false),
                1 => Value::Bool(true),
                _ => Value::Null,
            },
            "boolean",
            SOURCE,
        );
        out.value(
            "system",
            "battery_present",
            if status.BatteryFlag == 255 {
                Value::Null
            } else {
                Value::Bool(status.BatteryFlag & 128 == 0)
            },
            "boolean",
            SOURCE,
        );
        out.value(
            "system",
            "battery_charge_basis_points",
            if status.BatteryLifePercent <= 100 {
                Value::from(status.BatteryLifePercent as u64 * 100)
            } else {
                Value::Null
            },
            "basis_points",
            SOURCE,
        );
        out.value(
            "system",
            "battery_saver",
            status.SystemStatusFlag != 0,
            "boolean",
            SOURCE,
        );
        out.coverage(
            "battery",
            C::Ok,
            SOURCE,
            "Unavailable charge or line status is left unknown; desktop PCs may have no battery.",
        );
    } else {
        // SAFETY: last error read immediately after failed call.
        out.coverage(
            "battery",
            error(unsafe { GetLastError() }),
            SOURCE,
            "Power status unavailable.",
        );
    }
    let mut scheme = null_mut();
    // SAFETY: writable allocated-GUID pointer output; read-only local power policy query.
    let rc = unsafe { PowerGetActiveScheme(null_mut(), &mut scheme) };
    if rc == 0 && !scheme.is_null() {
        // SAFETY: allocated GUID from successful PowerGetActiveScheme; copy before LocalFree.
        let guid = unsafe { *scheme };
        let plan = match (guid.data1, guid.data2, guid.data3, guid.data4) {
            (0x381b4222, 0xf694, 0x41f0, [0x96, 0x85, 0xff, 0x5b, 0xb2, 0x60, 0xdf, 0x2e]) => {
                "balanced"
            }
            (0x8c5e7fda, 0xe8bf, 0x4a96, [0x9a, 0x85, 0xa6, 0xe2, 0x3a, 0x8c, 0x63, 0x5c]) => {
                "high_performance"
            }
            (0xa1841308, 0x3541, 0x4fab, [0xbc, 0x81, 0xf7, 0x15, 0x56, 0xf2, 0x0b, 0x4a]) => {
                "power_saver"
            }
            _ => "custom",
        };
        // SAFETY: free the allocation using the allocator documented by PowerGetActiveScheme.
        unsafe {
            LocalFree(scheme.cast());
        }
        out.value(
            "system",
            "power_plan",
            plan,
            "category",
            "Win32.PowerGetActiveScheme",
        );
        out.coverage(
            "power_plan",
            C::Ok,
            "Win32.PowerGetActiveScheme",
            "Built-in plans identified locally; custom plan GUIDs are not sent.",
        );
    } else {
        out.coverage(
            "power_plan",
            error(rc),
            "Win32.PowerGetActiveScheme",
            "Active power plan unavailable.",
        );
    }
    super::power_counters::collect(out);
    wmi::named(out, "thermal_zones", 32);
    let snapshot = pcagent_telemetry::Collector::new().sample_system();
    let mut found = false;
    for evidence in snapshot
        .evidence
        .into_iter()
        .filter(|e| format!("{:?}", e.metric).starts_with("Gpu"))
    {
        found |= evidence.value.is_some();
        let metric = serde_json::to_value(evidence.metric).unwrap_or(Value::Null);
        if let Some(metric) = metric.as_str() {
            out.value(
                &evidence.entity_ref,
                metric,
                evidence.value.map(Value::from).unwrap_or(Value::Null),
                &evidence.unit,
                &evidence.source,
            );
        }
    }
    out.coverage("gpu_sensors",if found{C::Ok}else{C::Unsupported},"NVML / Windows GPU counters","NVIDIA sensor availability depends on driver support; unsupported sensors remain unknown. ACPI zones are not necessarily CPU package temperatures.");
    super::sensors::collect(out);
    super::battery::collect(out);
}
