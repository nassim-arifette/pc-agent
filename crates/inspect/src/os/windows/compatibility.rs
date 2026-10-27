use super::{error, registry::Key};
use crate::{parse::compatibility as parse, CoverageStatus as C, Inspection};
use serde_json::Value;
use windows_sys::Win32::System::Registry::{HKEY_LOCAL_MACHINE, KEY_WOW64_64KEY};

const SOURCE: &str = "Registry.AppCompatFlags safeguard indicators (read-only)";
fn status(key: &Key) -> Option<u32> {
    key.dword("GStatus")
        .or_else(|| key.string("GStatus")?.parse().ok())
}
pub(super) fn collect(out: &mut Inspection) {
    let summary = Key::open(
        HKEY_LOCAL_MACHINE,
        r"SOFTWARE\Microsoft\Windows NT\CurrentVersion\AppCompatFlags\Appraiser\GWX",
        KEY_WOW64_64KEY,
    );
    match summary {
        Ok(Some(key)) => {
            out.value(
                "system",
                "safeguard_hold_reported",
                parse::hold_status(status(&key))
                    .map(Value::from)
                    .unwrap_or(Value::Null),
                "boolean",
                SOURCE,
            );
            if let Some(value) = out.observations.last_mut().filter(|v| !v.value.is_null()) {
                value.status = C::Partial;
            }
            out.coverage("safeguard_summary", C::Partial, SOURCE, "GStatus 0 means a hold is reported; 2 means no hold is reported. Appraiser registry values may be stale and cannot prove current upgrade eligibility.");
        }
        Ok(None) => out.coverage(
            "safeguard_summary",
            C::Unsupported,
            SOURCE,
            "Appraiser summary key absent; safeguard state remains unknown.",
        ),
        Err(code) => out.coverage(
            "safeguard_summary",
            error(code),
            SOURCE,
            "Appraiser summary key unreadable.",
        ),
    }
    let key = match Key::open(
        HKEY_LOCAL_MACHINE,
        r"SOFTWARE\Microsoft\Windows NT\CurrentVersion\AppCompatFlags\TargetVersionUpgradeExperienceIndicators",
        KEY_WOW64_64KEY,
    ) {
        Ok(Some(key)) => key,
        Ok(None) => {
            out.coverage("safeguard_targets", C::Unsupported, SOURCE, "Target-version indicator key absent; missing data is not evidence that no hold exists.");
            return;
        }
        Err(code) => {
            out.coverage(
                "safeguard_targets",
                error(code),
                SOURCE,
                "Target-version indicators unreadable.",
            );
            return;
        }
    };
    let (names, capped) = match key.names(true, 64) {
        Ok(value) => value,
        Err(code) => {
            out.coverage(
                "safeguard_targets",
                error(code),
                SOURCE,
                "Target-version enumeration failed.",
            );
            return;
        }
    };
    let mut emitted = 0;
    let mut unreadable = false;
    for name in names.into_iter().filter(|name| parse::target_name(name)) {
        let target = match key.child(&name) {
            Ok(Some(target)) => target,
            _ => {
                unreadable = true;
                continue;
            }
        };
        let entity = out.entity("upgrade-target", None);
        out.value(&entity, "safeguard_target_version", name, "version", SOURCE);
        out.value(
            &entity,
            "safeguard_hold_reported",
            parse::hold_status(status(&target))
                .map(Value::from)
                .unwrap_or(Value::Null),
            "boolean",
            SOURCE,
        );
        let ids = target
            .string("GatedBlockId")
            .or_else(|| target.dword("GatedBlockId").map(|id| id.to_string()));
        let ids = parse::block_ids(ids.as_deref());
        out.value(
            &entity,
            "safeguard_block_ids",
            if ids.is_empty() {
                Value::Null
            } else {
                serde_json::json!(ids)
            },
            "safeguard_id",
            SOURCE,
        );
        let reason = target.string("GatedBlockReason");
        out.value(
            &entity,
            "safeguard_block_reason",
            parse::reason(reason.as_deref())
                .map(Value::from)
                .unwrap_or(Value::Null),
            "category",
            SOURCE,
        );
        emitted += 1;
    }
    out.coverage("safeguard_targets", if emitted == 0 && !unreadable && !capped { C::Unsupported } else { C::Partial }, SOURCE,
        if capped || unreadable { "Target scan was capped or entries were unreadable. Values are local appraiser evidence and may be stale." }
        else { "Existing release-named targets are discovered rather than assuming GE24H2/GE25H2. Only GStatus, numeric safeguard IDs and finite reason categories are read. Missing IDs/reasons and outdated indicators remain uncertain." });
}
