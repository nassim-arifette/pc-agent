use super::{command, number, read};
use crate::{CoverageStatus as C, Inspection};
use serde_json::{json, Value};
use std::path::Path;

pub(super) fn collect(out: &mut Inspection) {
    let dmi = Path::new("/sys/class/dmi/id");
    for (file, metric) in [
        ("sys_vendor", "system_manufacturer"),
        ("product_name", "system_model"),
        ("bios_version", "bios_version"),
    ] {
        out.name_value(
            "system",
            metric,
            read(&dmi.join(file)).map(|s| s.trim().to_string()),
            "DMI sysfs allowlist (no serials)",
        );
    }
    let Ok(entries) = std::fs::read_dir("/sys/block") else {
        out.coverage(
            "storage_devices",
            C::Unsupported,
            "sysfs block",
            "Block class unavailable.",
        );
        return;
    };
    let mut partial = false;
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with("loop")
            || name.starts_with("ram")
            || !name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
        {
            continue;
        }
        let path = entry.path();
        let entity = out.entity("disk", None);
        let model = read(&path.join("device/model"))
            .map(|s| s.trim().to_string())
            .or_else(|| {
                let path = format!("/sys/block/{name}");
                let text = command::run(
                    "/usr/bin/udevadm",
                    &["info", "--query=property", "--path", &path],
                )
                .ok()?;
                text.lines()
                    .find_map(|l| l.strip_prefix("ID_MODEL=").map(str::to_owned))
            });
        out.name_value(&entity, "disk_model", model, "sysfs block/udev ID_MODEL");
        out.value(
            &entity,
            "disk_size_bytes",
            json!(number(&path.join("size")).and_then(|n| n.checked_mul(512))),
            "bytes",
            "sysfs block size (512-byte sectors)",
        );
        out.value(
            &entity,
            "disk_rotational",
            json!(number(&path.join("queue/rotational")).map(|n| n != 0)),
            "boolean",
            "sysfs block queue",
        );
        out.value(
            &entity,
            "disk_removable",
            json!(number(&path.join("removable")).map(|n| n != 0)),
            "boolean",
            "sysfs block",
        );
        let device = format!("/dev/{name}");
        match command::run("/usr/sbin/smartctl", &["--all", "--json", &device]) {
            Ok(text) => match serde_json::from_str::<Value>(&text) {
                Ok(v) => {
                    for (metric, value, unit) in [
                        (
                            "disk_health_passed",
                            v["smart_status"]["passed"].as_bool().map(Value::from),
                            "boolean",
                        ),
                        (
                            "disk_temperature_celsius",
                            v["temperature"]["current"].as_i64().map(Value::from),
                            "celsius",
                        ),
                        (
                            "disk_power_on_hours",
                            v["power_on_time"]["hours"].as_u64().map(Value::from),
                            "hours",
                        ),
                        (
                            "disk_percentage_used",
                            v["nvme_smart_health_information_log"]["percentage_used"]
                                .as_u64()
                                .map(Value::from),
                            "percent",
                        ),
                    ] {
                        partial |= value.is_none();
                        out.value(
                            &entity,
                            metric,
                            value.unwrap_or(Value::Null),
                            unit,
                            "smartctl JSON positive numeric allowlist",
                        );
                    }
                }
                Err(_) => partial = true,
            },
            Err(c) => {
                partial = true;
                out.coverage("smart",if c==C::Denied{C::NeedsAdmin}else{c},"smartctl read-only","SMART unavailable; install smartmontools or run the explicitly approved privileged disk-health task. No device data was substituted.");
            }
        }
    }
    out.coverage("hardware",if partial{C::Partial}else{C::Ok},"DMI/sysfs/udev/SMART","Firmware and block-device metadata only; serial numbers, UUIDs and SMART free-form strings are omitted.");
}
