//! Closed, bounded data for privileged read-only system inspections.
//!
//! Process paths and process identities belong to the local helper protocol only. C4
//! projections must use opaque references and put only executable basenames in the
//! separate names map. Device IDs, volume GUIDs, user names and command lines are absent.
use super::protocol::{ElevationError, ElevationStatus};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeSet, fmt};

pub const MAX_SYSTEM_ROWS: usize = 32;
pub const MAX_PROCESS_IMAGES: usize = 16;
pub const MAX_IMAGE_PATH_BYTES: usize = 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SystemReadLimit {
    ProviderUnavailable,
    ProviderValueMissing,
    ProviderValuesInconsistent,
    EnumerationIncomplete,
    RowLimit,
    DurationLimit,
    Cancelled,
    PlatformUnsupported,
    ShadowStorageIncludesOtherSnapshots,
    RestorePointAttributionUnavailable,
    StorageCountersAreDeviceReported,
    RawSmartAttributesUnavailable,
    ProcessIdentityChanged,
    ProcessExited,
    ProcessIdentityUnavailable,
    ImagePathTooLong,
    ProtectedProcessDenied,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SystemReadSummary<T> {
    pub status: ElevationStatus,
    pub samples: Vec<T>,
    pub truncated: bool,
    pub read_errors: u32,
    pub limitations: Vec<SystemReadLimit>,
}

impl<T> SystemReadSummary<T> {
    #[cfg_attr(not(windows), allow(dead_code))]
    pub(crate) fn unavailable(status: ElevationStatus, limit: SystemReadLimit) -> Self {
        Self {
            status,
            samples: Vec::new(),
            truncated: false,
            read_errors: 0,
            limitations: vec![limit],
        }
    }

    pub fn validate(&self, maximum: usize) -> Result<(), ElevationError> {
        if self.samples.len() > maximum
            || self.limitations.len() > 16
            || self
                .limitations
                .iter()
                .enumerate()
                .any(|(index, limit)| self.limitations[..index].contains(limit))
            || (self.status == ElevationStatus::Completed
                && (self.truncated || self.read_errors != 0))
            || (matches!(
                self.status,
                ElevationStatus::Unsupported | ElevationStatus::Denied | ElevationStatus::Error
            ) && !self.samples.is_empty())
        {
            return Err(ElevationError::InvalidResponse);
        }
        Ok(())
    }
}

/// One provider association. It is not a restore point or a deletion estimate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ShadowStorageSample {
    pub used_bytes: Option<u64>,
    pub allocated_bytes: Option<u64>,
    /// UINT64_MAX means unlimited, represented separately rather than as a capacity.
    pub maximum_bytes: Option<u64>,
    pub maximum_unlimited: Option<bool>,
    pub values_consistent: Option<bool>,
}

impl ShadowStorageSample {
    pub(crate) fn from_provider(
        used: Option<u64>,
        allocated: Option<u64>,
        max: Option<u64>,
    ) -> Self {
        let maximum_unlimited = max.map(|v| v == u64::MAX);
        let maximum_bytes = max.filter(|v| *v != u64::MAX);
        let exceeds_maximum = maximum_bytes.is_some_and(|max| {
            used.is_some_and(|used| used > max)
                || allocated.is_some_and(|allocated| allocated > max)
        });
        let values_consistent = if exceeds_maximum {
            Some(false)
        } else {
            used.zip(allocated)
                .map(|(used, allocated)| used <= allocated)
        };
        Self {
            used_bytes: used,
            allocated_bytes: allocated,
            maximum_bytes,
            maximum_unlimited,
            values_consistent,
        }
    }

    pub fn validate(&self) -> Result<(), ElevationError> {
        let encoded_max = match (self.maximum_unlimited, self.maximum_bytes) {
            (Some(true), None) => Some(u64::MAX),
            (Some(false), Some(max)) if max != u64::MAX => Some(max),
            (None, None) => None,
            _ => return Err(ElevationError::InvalidResponse),
        };
        if *self != Self::from_provider(self.used_bytes, self.allocated_bytes, encoded_max) {
            return Err(ElevationError::InvalidResponse);
        }
        Ok(())
    }
}

/// Selected documented MSFT_StorageReliabilityCounter fields, without identifiers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiskReliabilitySample {
    pub temperature_celsius: Option<u8>,
    pub temperature_limit_celsius: Option<u8>,
    pub wear_percent: Option<u8>,
    pub power_on_hours: Option<u16>,
    pub read_errors_total: Option<u64>,
    pub read_errors_corrected: Option<u64>,
    pub read_errors_uncorrected: Option<u64>,
    pub write_errors_total: Option<u64>,
    pub write_errors_corrected: Option<u64>,
    pub write_errors_uncorrected: Option<u64>,
    pub start_stop_cycles: Option<u32>,
    pub load_unload_cycles: Option<u32>,
    pub maximum_read_latency_ms: Option<u64>,
    pub maximum_write_latency_ms: Option<u64>,
    pub maximum_flush_latency_ms: Option<u64>,
}

impl DiskReliabilitySample {
    pub fn validate(&self) -> Result<(), ElevationError> {
        // The provider reports UInt8 Wear; 100 is the estimated wear limit, not the maximum
        // encodable percentage. Above-limit values must remain visible rather than become unknown.
        Ok(())
    }

    pub fn values_consistent(&self) -> Option<bool> {
        let read = self
            .read_errors_total
            .zip(self.read_errors_corrected)
            .zip(self.read_errors_uncorrected)
            .map(|((total, corrected), uncorrected)| {
                corrected
                    .checked_add(uncorrected)
                    .is_some_and(|sum| sum <= total)
            });
        let write = self
            .write_errors_total
            .zip(self.write_errors_corrected)
            .zip(self.write_errors_uncorrected)
            .map(|((total, corrected), uncorrected)| {
                corrected
                    .checked_add(uncorrected)
                    .is_some_and(|sum| sum <= total)
            });
        match (read, write) {
            (Some(false), _) | (_, Some(false)) => Some(false),
            (Some(true), Some(true)) => Some(true),
            _ => None,
        }
    }

    #[cfg(windows)]
    pub(crate) fn has_missing(&self) -> bool {
        self.temperature_celsius.is_none()
            || self.temperature_limit_celsius.is_none()
            || self.wear_percent.is_none()
            || self.power_on_hours.is_none()
            || self.read_errors_total.is_none()
            || self.read_errors_corrected.is_none()
            || self.read_errors_uncorrected.is_none()
            || self.write_errors_total.is_none()
            || self.write_errors_corrected.is_none()
            || self.write_errors_uncorrected.is_none()
            || self.start_stop_cycles.is_none()
            || self.load_unload_cycles.is_none()
            || self.maximum_read_latency_ms.is_none()
            || self.maximum_write_latency_ms.is_none()
            || self.maximum_flush_latency_ms.is_none()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProcessImageTarget {
    pub pid: u32,
    /// Native FILETIME, used only locally to reject PID reuse.
    pub creation_time_filetime: u64,
}

pub fn validate_process_targets(targets: &[ProcessImageTarget]) -> Result<(), ElevationError> {
    let mut seen = BTreeSet::new();
    if targets.is_empty() || targets.len() > MAX_PROCESS_IMAGES {
        return Err(ElevationError::InvalidArguments);
    }
    for target in targets {
        if target.pid == 0 || target.creation_time_filetime == 0 || !seen.insert(target.pid) {
            return Err(ElevationError::InvalidArguments);
        }
    }
    Ok(())
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProcessImageSample {
    pub target: ProcessImageTarget,
    pub status: ElevationStatus,
    /// Local protocol data; never send the complete path to the model.
    pub image_path: Option<String>,
    pub limitation: Option<SystemReadLimit>,
}

impl fmt::Debug for ProcessImageSample {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ProcessImageSample")
            .field("status", &self.status)
            .field("image_available", &self.image_path.is_some())
            .field("limitation", &self.limitation)
            .finish_non_exhaustive()
    }
}

impl ProcessImageSample {
    pub fn validate(&self) -> Result<(), ElevationError> {
        validate_process_targets(&[self.target]).map_err(|_| ElevationError::InvalidResponse)?;
        match &self.image_path {
            Some(path)
                if self.status == ElevationStatus::Completed
                    && self.limitation.is_none()
                    && valid_image_path(path) =>
            {
                Ok(())
            }
            None if self.status != ElevationStatus::Completed && self.limitation.is_some() => {
                Ok(())
            }
            _ => Err(ElevationError::InvalidResponse),
        }
    }

    /// Executable display name only, to be handled by the existing C5 names map.
    pub fn executable_basename(&self) -> Option<&str> {
        self.image_path.as_deref()?.rsplit('\\').next()
    }
}

pub(crate) fn valid_image_path(path: &str) -> bool {
    if path.len() > MAX_IMAGE_PATH_BYTES
        || path.len() < 4
        || !path.as_bytes()[0].is_ascii_alphabetic()
        || path.as_bytes()[1..3] != *b":\\"
        || path
            .chars()
            .any(|c| c.is_control() || "/<>\"|?*".contains(c))
    {
        return false;
    }
    path[3..].split('\\').all(|part| {
        let stem = part.split('.').next().unwrap_or("").to_ascii_uppercase();
        !part.is_empty()
            && part != "."
            && part != ".."
            && !part.contains(':')
            && !part.ends_with([' ', '.'])
            && !["CON", "PRN", "AUX", "NUL", "CONIN$", "CONOUT$"].contains(&stem.as_str())
            && !(stem.len() == 4
                && (stem.starts_with("COM") || stem.starts_with("LPT"))
                && stem.as_bytes()[3].is_ascii_digit())
    })
}

/// Decode only the fixed DOModifyCacheDrive value, without general environment expansion.
/// `windows_drive` comes from GetWindowsDirectoryW, never an inherited environment variable.
#[cfg_attr(not(any(windows, test)), allow(dead_code))]
pub(crate) fn policy_folder(value: &str, windows_drive: char) -> Option<String> {
    if value.len() > MAX_IMAGE_PATH_BYTES || value.is_empty() {
        return None;
    }
    let expanded = if value
        .get(..13)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("%SYSTEMDRIVE%"))
    {
        format!("{}:{}", windows_drive, &value[13..])
    } else {
        value.to_owned()
    };
    if expanded.contains('%') {
        return None;
    }
    // A drive-only policy selects a drive, not a documented cache directory. Do not guess.
    if expanded.len() == 2 && expanded.as_bytes()[1] == b':' {
        return None;
    }
    let expanded = expanded.trim_end_matches('\\');
    if !valid_image_path(expanded) {
        return None;
    }
    Some(expanded.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_known_counter_inconsistency_survives_other_missing_fields() {
        let mut sample: DiskReliabilitySample = serde_json::from_value(serde_json::json!({
            "temperature_celsius":null,"temperature_limit_celsius":null,"wear_percent":101,
            "power_on_hours":null,"read_errors_total":1,"read_errors_corrected":1,
            "read_errors_uncorrected":1,"write_errors_total":null,"write_errors_corrected":null,
            "write_errors_uncorrected":null,"start_stop_cycles":null,"load_unload_cycles":null,
            "maximum_read_latency_ms":null,"maximum_write_latency_ms":null,"maximum_flush_latency_ms":null
        })).unwrap();
        sample.validate().unwrap();
        assert_eq!(sample.values_consistent(), Some(false));
        sample.read_errors_total = Some(u64::MAX);
        sample.read_errors_corrected = Some(u64::MAX);
        assert_eq!(sample.values_consistent(), Some(false));
        sample.read_errors_uncorrected = Some(0);
        assert_eq!(sample.values_consistent(), None);
        sample.write_errors_total = Some(0);
        sample.write_errors_corrected = Some(0);
        sample.write_errors_uncorrected = Some(0);
        assert_eq!(sample.values_consistent(), Some(true));
    }

    #[test]
    fn shadow_storage_distinguishes_unlimited_missing_and_inconsistent() {
        let zero = ShadowStorageSample::from_provider(Some(0), Some(0), Some(0));
        assert_eq!(zero.values_consistent, Some(true));
        zero.validate().unwrap();
        let unlimited = ShadowStorageSample::from_provider(Some(3), Some(8), Some(u64::MAX));
        assert_eq!(unlimited.maximum_unlimited, Some(true));
        assert_eq!(unlimited.maximum_bytes, None);
        unlimited.validate().unwrap();
        let inconsistent = ShadowStorageSample::from_provider(Some(9), Some(8), None);
        assert_eq!(inconsistent.values_consistent, Some(false));
        inconsistent.validate().unwrap();
        assert_eq!(
            ShadowStorageSample::from_provider(None, Some(8), None).values_consistent,
            None
        );
        assert_eq!(
            ShadowStorageSample::from_provider(None, Some(8), Some(7)).values_consistent,
            Some(false)
        );
        assert_eq!(
            ShadowStorageSample::from_provider(Some(8), None, Some(7)).values_consistent,
            Some(false)
        );
    }

    #[test]
    fn process_requests_bind_creation_time_and_bound_total_paths() {
        let target = ProcessImageTarget {
            pid: 25,
            creation_time_filetime: 1234,
        };
        validate_process_targets(&[target]).unwrap();
        assert!(validate_process_targets(&[]).is_err());
        assert!(validate_process_targets(&[target, target]).is_err());
        assert!(validate_process_targets(&[ProcessImageTarget { pid: 0, ..target }]).is_err());
        assert!(validate_process_targets(&[ProcessImageTarget {
            creation_time_filetime: 0,
            ..target
        }])
        .is_err());
        let value = ProcessImageSample {
            target,
            status: ElevationStatus::Completed,
            image_path: Some(r"C:\Users\private-user\Application\app.exe".into()),
            limitation: None,
        };
        value.validate().unwrap();
        assert_eq!(value.executable_basename(), Some("app.exe"));
        assert!(!format!("{value:?}").contains("private-user"));
        let mut invalid = value;
        invalid.status = ElevationStatus::Partial;
        assert!(invalid.validate().is_err());
    }

    #[test]
    fn policy_paths_never_expand_user_environment_or_guess_drive_layouts() {
        assert_eq!(
            policy_folder(r"D:\DOCache", 'C'),
            Some(r"D:\DOCache".into())
        );
        assert_eq!(
            policy_folder(r"%SYSTEMDRIVE%\DOCache", 'C'),
            Some(r"C:\DOCache".into())
        );
        for value in [
            r"D:",
            r"D:\",
            r"%USERPROFILE%\DOCache",
            r"C:\..\private",
            r"\\server\cache",
            r"\\?\C:\cache",
            r"C:\cache:stream",
            r"C:\cache\..",
            r"C:\cache.\child",
            r"C:\NUL\cache",
            "ééééééé",
        ] {
            assert_eq!(policy_folder(value, 'C'), None, "{value}");
        }
        assert!(serde_json::from_str::<ProcessImageTarget>(
            r#"{"pid":1,"creation_time_filetime":2,"path":"anything"}"#
        )
        .is_err());
    }
}
