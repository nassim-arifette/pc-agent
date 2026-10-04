//! Pure parsers for untrusted native data; covered without a Windows host.
pub(crate) mod battery;
pub(crate) mod compatibility;
pub(crate) mod driver_version;
pub(crate) mod event_fields;
pub(crate) mod sensors;
pub(crate) mod smbios;

/// StartupApproved is not a stable documented structure. Unknown encodings remain unknown.
pub(crate) fn startup_enabled(data: Option<&[u8]>) -> Option<bool> {
    let data = data?;
    if data.len() < 12 {
        return None;
    }
    match u32::from_le_bytes(data[..4].try_into().ok()?) {
        2 | 6 => Some(true),
        3 | 7 => Some(false),
        _ => None,
    }
}

/// A missing trigger beyond the scan limit can still enable a logon task.
pub(crate) fn logon_task_enabled(
    task_enabled: Option<bool>,
    active_trigger: bool,
    triggers_complete: bool,
) -> Option<bool> {
    match (task_enabled, active_trigger, triggers_complete) {
        (Some(false), _, _) | (_, false, true) => Some(false),
        (Some(true), true, _) => Some(true),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn incomplete_trigger_scan_cannot_prove_a_logon_task_disabled() {
        assert_eq!(logon_task_enabled(Some(true), false, false), None);
        assert_eq!(logon_task_enabled(Some(true), true, false), Some(true));
        assert_eq!(logon_task_enabled(Some(false), false, false), Some(false));
        assert_eq!(logon_task_enabled(Some(true), false, true), Some(false));
        assert_eq!(logon_task_enabled(None, true, true), None);
        assert_eq!(logon_task_enabled(None, false, true), Some(false));
    }
    #[test]
    fn startup_unknown_does_not_mean_enabled() {
        assert_eq!(startup_enabled(None), None);
        assert_eq!(startup_enabled(Some(&[2])), None);
        assert_eq!(
            startup_enabled(Some(&[2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0])),
            Some(true)
        );
        assert_eq!(
            startup_enabled(Some(&[3, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0])),
            Some(false)
        );
        assert_eq!(
            startup_enabled(Some(&[4, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0])),
            None
        );
    }
}
