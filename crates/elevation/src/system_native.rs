//! Fixed Windows system reads for the locally approved helper. No command or network API.
use super::{
    protocol::{ElevationError, ElevationStatus},
    system::{
        policy_folder, valid_image_path, validate_process_targets, DiskReliabilitySample,
        ProcessImageSample, ProcessImageTarget, ShadowStorageSample, SystemReadLimit,
        SystemReadSummary, MAX_IMAGE_PATH_BYTES, MAX_SYSTEM_ROWS,
    },
};
use ::windows::{
    core::{BSTR, PCWSTR},
    Win32::System::{Com::*, Variant::*, Wmi::*},
};
use std::{
    mem::size_of,
    os::windows::ffi::OsStrExt,
    path::{Path, PathBuf},
    ptr::{null, null_mut},
    time::{Duration, Instant},
};
use windows_sys::Win32::{
    Foundation::{
        CloseHandle, GetLastError, ERROR_ACCESS_DENIED, ERROR_FILE_NOT_FOUND,
        ERROR_INVALID_PARAMETER, ERROR_PATH_NOT_FOUND, FILETIME, HANDLE, INVALID_HANDLE_VALUE,
        STILL_ACTIVE,
    },
    Storage::FileSystem::{
        CreateFileW, FileAttributeTagInfo, GetDriveTypeW, GetFileInformationByHandleEx,
        FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_REPARSE_POINT, FILE_ATTRIBUTE_TAG_INFO,
        FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_READ_ATTRIBUTES,
        FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
    },
    System::{
        Registry::{
            RegCloseKey, RegGetValueW, RegOpenKeyExW, HKEY, HKEY_LOCAL_MACHINE, KEY_QUERY_VALUE,
            KEY_WOW64_64KEY, RRF_NOEXPAND, RRF_RT_REG_EXPAND_SZ, RRF_RT_REG_SZ,
        },
        Threading::{
            GetExitCodeProcess, GetProcessTimes, OpenProcess, QueryFullProcessImageNameW,
            PROCESS_QUERY_LIMITED_INFORMATION,
        },
        WindowsProgramming::DRIVE_FIXED,
    },
};

fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(Some(0)).collect()
}

struct Apartment;
impl Apartment {
    fn new() -> ::windows::core::Result<Self> {
        // SAFETY: the helper owns this thread. Successful S_OK/S_FALSE is balanced on this thread.
        unsafe { CoInitializeEx(None, COINIT_MULTITHREADED).ok()? };
        Ok(Self)
    }
}
impl Drop for Apartment {
    fn drop(&mut self) {
        // SAFETY: one successful CoInitializeEx is balanced after all COM objects/errors are dropped.
        unsafe { CoUninitialize() };
    }
}

struct NativeHandle(HANDLE);
impl Drop for NativeHandle {
    fn drop(&mut self) {
        // SAFETY: the handle was opened by this module and remains owned until this drop.
        unsafe { CloseHandle(self.0) };
    }
}

struct Budget<'a> {
    started: Instant,
    duration: Duration,
    check_cancel: &'a mut dyn FnMut() -> Result<bool, ElevationError>,
}
impl<'a> Budget<'a> {
    fn new(
        duration: Duration,
        check_cancel: &'a mut dyn FnMut() -> Result<bool, ElevationError>,
    ) -> Self {
        Self {
            started: Instant::now(),
            duration,
            check_cancel,
        }
    }
    fn stop(&mut self) -> Result<Option<SystemReadLimit>, ElevationError> {
        if (self.check_cancel)()? {
            Ok(Some(SystemReadLimit::Cancelled))
        } else if self.started.elapsed() >= self.duration {
            Ok(Some(SystemReadLimit::DurationLimit))
        } else {
            Ok(None)
        }
    }
}

const SHADOW_COLUMNS: &[&str] = &["UsedSpace", "AllocatedSpace", "MaxSpace"];
const RELIABILITY_COLUMNS: &[&str] = &[
    "Temperature",
    "TemperatureMax",
    "Wear",
    "PowerOnHours",
    "ReadErrorsTotal",
    "ReadErrorsCorrected",
    "ReadErrorsUncorrected",
    "WriteErrorsTotal",
    "WriteErrorsCorrected",
    "WriteErrorsUncorrected",
    "StartStopCycleCount",
    "LoadUnloadCycleCount",
    "ReadLatencyMax",
    "WriteLatencyMax",
    "FlushLatencyMax",
];

enum FixedQuery {
    ShadowStorage,
    StorageReliability,
}
impl FixedQuery {
    fn namespace(&self) -> &'static str {
        match self {
            Self::ShadowStorage => r"root\cimv2",
            Self::StorageReliability => r"root\Microsoft\Windows\Storage",
        }
    }
    fn class(&self) -> &'static str {
        match self {
            Self::ShadowStorage => "Win32_ShadowStorage",
            Self::StorageReliability => "MSFT_StorageReliabilityCounter",
        }
    }
    fn columns(&self) -> &'static [&'static str] {
        match self {
            Self::ShadowStorage => SHADOW_COLUMNS,
            Self::StorageReliability => RELIABILITY_COLUMNS,
        }
    }
}

struct NumericRows {
    rows: Vec<Vec<Option<u64>>>,
    status: ElevationStatus,
    truncated: bool,
    errors: u32,
    limitations: Vec<SystemReadLimit>,
}
impl NumericRows {
    fn empty(status: ElevationStatus) -> Self {
        Self {
            rows: Vec::new(),
            status,
            truncated: false,
            errors: 0,
            limitations: Vec::new(),
        }
    }
}

fn com_status(error: &::windows::core::Error) -> ElevationStatus {
    match error.code().0 as u32 {
        0x80070005 | 0x80041003 | 0x80070522 => ElevationStatus::Denied,
        0x80041010 | 0x8004100e | 0x8004100c | 0x80070032 => ElevationStatus::Unsupported,
        _ => ElevationStatus::Error,
    }
}

fn numeric_variant(variant: &VARIANT) -> Option<u64> {
    // SAFETY: the VARIANT discriminant is checked first; by-reference/interface variants are refused.
    unsafe {
        let value = &variant.Anonymous.Anonymous;
        match value.vt {
            VT_UI1 => Some(u64::from(value.Anonymous.bVal)),
            VT_UI2 => Some(u64::from(value.Anonymous.uiVal)),
            VT_UI4 => Some(u64::from(value.Anonymous.ulVal)),
            VT_UI8 => Some(value.Anonymous.ullVal),
            VT_I2 => u64::try_from(value.Anonymous.iVal).ok(),
            VT_I4 => u64::try_from(value.Anonymous.lVal).ok(),
            VT_I8 => u64::try_from(value.Anonymous.llVal).ok(),
            VT_BSTR => {
                // WMI represents CIM uint64 as decimal BSTR. Bound before converting to UTF-8.
                let text = &value.Anonymous.bstrVal;
                if text.is_empty() || text.len() > 20 {
                    return None;
                }
                let text = text.to_string();
                text.bytes()
                    .all(|byte| byte.is_ascii_digit())
                    .then(|| text.parse().ok())
                    .flatten()
            }
            _ => None,
        }
    }
}

enum FixedReadError {
    Com(::windows::core::Error),
    Protocol(ElevationError),
}
impl From<::windows::core::Error> for FixedReadError {
    fn from(error: ::windows::core::Error) -> Self {
        Self::Com(error)
    }
}
impl From<ElevationError> for FixedReadError {
    fn from(error: ElevationError) -> Self {
        Self::Protocol(error)
    }
}

fn read_numeric_rows(
    query: FixedQuery,
    budget: &mut Budget<'_>,
) -> Result<NumericRows, ElevationError> {
    let mut rows = NumericRows::empty(ElevationStatus::Completed);
    if let Some(stop) = budget.stop()? {
        rows.status = if stop == SystemReadLimit::Cancelled {
            ElevationStatus::Cancelled
        } else {
            ElevationStatus::TimedOut
        };
        rows.limitations.push(stop);
        return Ok(rows);
    }
    let apartment = match Apartment::new() {
        Ok(apartment) => apartment,
        Err(error) => {
            rows.status = com_status(&error);
            rows.limitations.push(SystemReadLimit::ProviderUnavailable);
            return Ok(rows);
        }
    };
    // All errors and interfaces remain inside this apartment's scope. Only copied integers escape.
    let result = (|| -> Result<(), FixedReadError> {
        let empty = BSTR::new();
        let wql = format!(
            "SELECT {} FROM {}",
            query.columns().join(","),
            query.class()
        );
        // SAFETY: initialized apartment; fixed local namespace and allowlisted WQL only. Empty credentials.
        let enumerator = unsafe {
            let locator: IWbemLocator = CoCreateInstance(&WbemLocator, None, CLSCTX_INPROC_SERVER)?;
            let service = locator.ConnectServer(
                &BSTR::from(query.namespace()),
                &empty,
                &empty,
                &empty,
                WBEM_FLAG_CONNECT_USE_MAX_WAIT.0,
                &empty,
                None,
            )?;
            CoSetProxyBlanket(
                &service,
                10,
                0,
                PCWSTR::null(),
                RPC_C_AUTHN_LEVEL_CALL,
                RPC_C_IMP_LEVEL_IMPERSONATE,
                None,
                EOAC_NONE,
            )?;
            service.ExecQuery(
                &BSTR::from("WQL"),
                &BSTR::from(wql),
                WBEM_FLAG_FORWARD_ONLY | WBEM_FLAG_RETURN_IMMEDIATELY,
                None,
            )?
        };
        loop {
            // The worker watchdog and parent own the outer hard deadline even if COM blocks.
            if let Some(stop) = budget.stop()? {
                rows.status = if stop == SystemReadLimit::Cancelled {
                    ElevationStatus::Cancelled
                } else {
                    ElevationStatus::TimedOut
                };
                rows.limitations.push(stop);
                break;
            }
            let mut objects = [None];
            let mut returned = 0;
            let remaining = budget.duration.saturating_sub(budget.started.elapsed());
            let timeout_ms = remaining.as_millis().clamp(1, 100) as i32;
            // SAFETY: initialized optional object output and row count; timeout is bounded to 100 ms.
            let hr = unsafe { enumerator.Next(timeout_ms, &mut objects, &mut returned) };
            if hr.is_err() {
                if rows.rows.is_empty() {
                    hr.ok()?;
                }
                rows.errors += 1;
                rows.status = ElevationStatus::Partial;
                rows.limitations
                    .push(SystemReadLimit::EnumerationIncomplete);
                break;
            }
            if returned == 0 {
                if hr.0 == WBEM_S_TIMEDOUT.0 {
                    continue;
                }
                break;
            }
            if rows.rows.len() == MAX_SYSTEM_ROWS {
                rows.truncated = true;
                rows.status = ElevationStatus::Partial;
                rows.limitations.push(SystemReadLimit::RowLimit);
                break;
            }
            let Some(object) = objects[0].take() else {
                rows.errors += 1;
                rows.status = ElevationStatus::Partial;
                rows.limitations
                    .push(SystemReadLimit::EnumerationIncomplete);
                break;
            };
            let mut row = Vec::with_capacity(query.columns().len());
            for column in query.columns() {
                let mut value = VARIANT::default();
                // SAFETY: fixed property name, initialized VARIANT output and no optional raw outputs.
                let read =
                    unsafe { object.Get(PCWSTR(wide(column).as_ptr()), 0, &mut value, None, None) };
                row.push(if read.is_ok() {
                    numeric_variant(&value)
                } else {
                    None
                });
                // SAFETY: the owned returned VARIANT is cleared once after its scalar has been copied.
                unsafe {
                    let _ = VariantClear(&mut value);
                }
            }
            rows.rows.push(row);
        }
        Ok(())
    })();
    match result {
        Err(FixedReadError::Com(error)) => {
            rows.status = com_status(&error);
            rows.limitations.push(SystemReadLimit::ProviderUnavailable);
            if !rows.rows.is_empty() {
                rows.status = ElevationStatus::Partial;
                rows.errors += 1;
            }
            // Drop error before CoUninitialize: it can retain thread-affine IErrorInfo.
            drop(error);
        }
        Err(FixedReadError::Protocol(error)) => return Err(error),
        Ok(()) => {}
    }
    drop(apartment);
    Ok(rows)
}

pub(crate) fn shadow_storage(
    duration: Duration,
    check_cancel: &mut dyn FnMut() -> Result<bool, ElevationError>,
) -> Result<SystemReadSummary<ShadowStorageSample>, ElevationError> {
    let raw = read_numeric_rows(
        FixedQuery::ShadowStorage,
        &mut Budget::new(duration, check_cancel),
    )?;
    let mut result = SystemReadSummary {
        status: raw.status,
        samples: raw
            .rows
            .iter()
            .map(|row| ShadowStorageSample::from_provider(row[0], row[1], row[2]))
            .collect(),
        truncated: raw.truncated,
        read_errors: raw.errors,
        limitations: raw.limitations,
    };
    result.limitations.extend([
        SystemReadLimit::ShadowStorageIncludesOtherSnapshots,
        SystemReadLimit::RestorePointAttributionUnavailable,
    ]);
    if result
        .samples
        .iter()
        .any(|sample| sample.values_consistent != Some(true) || sample.maximum_unlimited.is_none())
    {
        if result.status == ElevationStatus::Completed {
            result.status = ElevationStatus::Partial;
        }
        result
            .limitations
            .push(SystemReadLimit::ProviderValueMissing);
    }
    if result
        .samples
        .iter()
        .any(|sample| sample.values_consistent == Some(false))
    {
        result
            .limitations
            .push(SystemReadLimit::ProviderValuesInconsistent);
    }
    Ok(result)
}

pub(crate) fn disk_reliability(
    duration: Duration,
    check_cancel: &mut dyn FnMut() -> Result<bool, ElevationError>,
) -> Result<SystemReadSummary<DiskReliabilitySample>, ElevationError> {
    let raw = read_numeric_rows(
        FixedQuery::StorageReliability,
        &mut Budget::new(duration, check_cancel),
    )?;
    if raw.rows.is_empty()
        && matches!(
            raw.status,
            ElevationStatus::Denied | ElevationStatus::Unsupported | ElevationStatus::Error
        )
    {
        return Ok(SystemReadSummary::unavailable(
            raw.status,
            SystemReadLimit::ProviderUnavailable,
        ));
    }
    let mut result = SystemReadSummary {
        status: raw.status,
        samples: raw
            .rows
            .iter()
            .map(|row| DiskReliabilitySample {
                temperature_celsius: row[0].and_then(|v| u8::try_from(v).ok()),
                temperature_limit_celsius: row[1].and_then(|v| u8::try_from(v).ok()),
                wear_percent: row[2].and_then(|v| u8::try_from(v).ok()),
                power_on_hours: row[3].and_then(|v| u16::try_from(v).ok()),
                read_errors_total: row[4],
                read_errors_corrected: row[5],
                read_errors_uncorrected: row[6],
                write_errors_total: row[7],
                write_errors_corrected: row[8],
                write_errors_uncorrected: row[9],
                start_stop_cycles: row[10].and_then(|v| u32::try_from(v).ok()),
                load_unload_cycles: row[11].and_then(|v| u32::try_from(v).ok()),
                maximum_read_latency_ms: row[12],
                maximum_write_latency_ms: row[13],
                maximum_flush_latency_ms: row[14],
            })
            .collect(),
        truncated: raw.truncated,
        read_errors: raw.errors,
        limitations: raw.limitations,
    };
    result.limitations.extend([
        SystemReadLimit::StorageCountersAreDeviceReported,
        SystemReadLimit::RawSmartAttributesUnavailable,
    ]);
    if result.samples.is_empty() && result.status == ElevationStatus::Completed {
        result.status = ElevationStatus::Unsupported;
        result
            .limitations
            .push(SystemReadLimit::ProviderUnavailable);
    } else if result
        .samples
        .iter()
        .any(DiskReliabilitySample::has_missing)
    {
        if result.status == ElevationStatus::Completed {
            result.status = ElevationStatus::Partial;
        }
        result
            .limitations
            .push(SystemReadLimit::ProviderValueMissing);
    }
    if result
        .samples
        .iter()
        .any(|sample| sample.values_consistent() == Some(false))
    {
        if result.status == ElevationStatus::Completed {
            result.status = ElevationStatus::Partial;
        }
        result
            .limitations
            .push(SystemReadLimit::ProviderValuesInconsistent);
    }
    Ok(result)
}

fn filetime(value: FILETIME) -> u64 {
    u64::from(value.dwLowDateTime) | (u64::from(value.dwHighDateTime) << 32)
}

fn verify_instance(handle: &NativeHandle, expected: u64) -> Result<(), SystemReadLimit> {
    let mut code = 0;
    // SAFETY: live query-only process handle and initialized output.
    if unsafe { GetExitCodeProcess(handle.0, &mut code) } == 0 {
        return Err(SystemReadLimit::ProcessIdentityUnavailable);
    }
    let mut created = FILETIME::default();
    let mut exited = FILETIME::default();
    let mut kernel = FILETIME::default();
    let mut user = FILETIME::default();
    // SAFETY: same retained process handle; all FILETIME outputs have their exact native layout.
    if unsafe { GetProcessTimes(handle.0, &mut created, &mut exited, &mut kernel, &mut user) } == 0
    {
        return Err(SystemReadLimit::ProcessIdentityUnavailable);
    }
    if filetime(created) != expected {
        return Err(SystemReadLimit::ProcessIdentityChanged);
    }
    // A process can exit with 259, so the exit FILETIME must also remain zero.
    if code != STILL_ACTIVE as u32 || filetime(exited) != 0 {
        return Err(SystemReadLimit::ProcessExited);
    }
    Ok(())
}

fn unavailable_image(
    target: ProcessImageTarget,
    status: ElevationStatus,
    limitation: SystemReadLimit,
) -> ProcessImageSample {
    ProcessImageSample {
        target,
        status,
        image_path: None,
        limitation: Some(limitation),
    }
}

pub(crate) fn process_images(
    targets: &[ProcessImageTarget],
    duration: Duration,
    check_cancel: &mut dyn FnMut() -> Result<bool, ElevationError>,
) -> Result<SystemReadSummary<ProcessImageSample>, ElevationError> {
    validate_process_targets(targets)?;
    let mut budget = Budget::new(duration, check_cancel);
    let mut result = SystemReadSummary {
        status: ElevationStatus::Completed,
        samples: Vec::new(),
        truncated: false,
        read_errors: 0,
        limitations: Vec::new(),
    };
    for target in targets.iter().copied() {
        if let Some(stop) = budget.stop()? {
            result.status = if stop == SystemReadLimit::Cancelled {
                ElevationStatus::Cancelled
            } else {
                ElevationStatus::TimedOut
            };
            result.limitations.push(stop);
            break;
        }
        // SAFETY: fixed bounded PID and query-only access; no debug privilege is enabled.
        let raw = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, target.pid) };
        if raw.is_null() {
            // SAFETY: immediately read the error of this thread's failed OpenProcess.
            let error = unsafe { GetLastError() };
            let (status, limitation) = if error == ERROR_ACCESS_DENIED {
                (
                    ElevationStatus::Denied,
                    SystemReadLimit::ProtectedProcessDenied,
                )
            } else if error == ERROR_INVALID_PARAMETER {
                (ElevationStatus::Partial, SystemReadLimit::ProcessExited)
            } else {
                (
                    ElevationStatus::Partial,
                    SystemReadLimit::ProcessIdentityUnavailable,
                )
            };
            result
                .samples
                .push(unavailable_image(target, status, limitation));
            continue;
        }
        let handle = NativeHandle(raw);
        if let Err(limit) = verify_instance(&handle, target.creation_time_filetime) {
            result
                .samples
                .push(unavailable_image(target, ElevationStatus::Partial, limit));
            continue;
        }
        // The response bound is enforced after UTF-16 decoding, not merely by UTF-16 unit count.
        let mut path = vec![0_u16; MAX_IMAGE_PATH_BYTES + 1];
        let mut length = path.len() as u32;
        // SAFETY: retained query-only handle and writable UTF-16 buffer of the supplied capacity.
        if unsafe { QueryFullProcessImageNameW(handle.0, 0, path.as_mut_ptr(), &mut length) } == 0 {
            // SAFETY: immediately read the failed query's error.
            let error = unsafe { GetLastError() };
            result.samples.push(unavailable_image(
                target,
                if error == ERROR_ACCESS_DENIED {
                    ElevationStatus::Denied
                } else {
                    ElevationStatus::Partial
                },
                if error == ERROR_ACCESS_DENIED {
                    SystemReadLimit::ProtectedProcessDenied
                } else if error == 122 {
                    SystemReadLimit::ImagePathTooLong
                } else {
                    SystemReadLimit::ProcessIdentityUnavailable
                },
            ));
            continue;
        }
        let Some(text) = path
            .get(..length as usize)
            .and_then(|path| String::from_utf16(path).ok())
            .filter(|text| valid_image_path(text))
        else {
            result.samples.push(unavailable_image(
                target,
                ElevationStatus::Partial,
                SystemReadLimit::ImagePathTooLong,
            ));
            continue;
        };
        if let Err(limit) = verify_instance(&handle, target.creation_time_filetime) {
            result
                .samples
                .push(unavailable_image(target, ElevationStatus::Partial, limit));
            continue;
        }
        result.samples.push(ProcessImageSample {
            target,
            status: ElevationStatus::Completed,
            image_path: Some(text),
            limitation: None,
        });
    }
    if result.status == ElevationStatus::Completed
        && result
            .samples
            .iter()
            .any(|sample| sample.status != ElevationStatus::Completed)
    {
        result.status = ElevationStatus::Partial;
    }
    Ok(result)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DeliveryCacheSource {
    DocumentedDefault,
    GroupPolicyFolder,
    PolicySelectsDriveOnly,
    PolicyMalformed,
    PolicyReadDenied,
    PolicyReadFailed,
    CandidateUnsafe,
}

/// Kept alive during scanning so a validated ancestor cannot be swapped for a junction.
pub(crate) struct DeliveryCacheResolution {
    pub path: Option<PathBuf>,
    pub source: DeliveryCacheSource,
    pub status: ElevationStatus,
    _pinned_directories: Vec<NativeHandle>,
}

struct PolicyKey(HKEY);
impl Drop for PolicyKey {
    fn drop(&mut self) {
        // SAFETY: this module owns the opened read-only registry key.
        unsafe { RegCloseKey(self.0) };
    }
}

fn cache_policy() -> Result<Option<String>, u32> {
    let mut key = null_mut();
    // SAFETY: fixed HKLM path and 64-bit read-only view; valid key output.
    let status = unsafe {
        RegOpenKeyExW(
            HKEY_LOCAL_MACHINE,
            wide(r"SOFTWARE\Policies\Microsoft\Windows\DeliveryOptimization").as_ptr(),
            0,
            KEY_QUERY_VALUE | KEY_WOW64_64KEY,
            &mut key,
        )
    };
    match status {
        ERROR_FILE_NOT_FOUND | ERROR_PATH_NOT_FOUND => return Ok(None),
        0 => {}
        error => return Err(error),
    }
    let key = PolicyKey(key);
    let mut bytes = vec![0_u8; 2 * (MAX_IMAGE_PATH_BYTES + 1)];
    let mut length = bytes.len() as u32;
    // SAFETY: fixed value name and bounded output. General environment expansion is prohibited.
    let status = unsafe {
        RegGetValueW(
            key.0,
            null(),
            wide("DOModifyCacheDrive").as_ptr(),
            RRF_RT_REG_SZ | RRF_RT_REG_EXPAND_SZ | RRF_NOEXPAND,
            null_mut(),
            bytes.as_mut_ptr().cast(),
            &mut length,
        )
    };
    if status == ERROR_FILE_NOT_FOUND {
        return Ok(None);
    }
    if status != 0 {
        return Err(status);
    }
    let length = length as usize;
    if length < 2 || length > bytes.len() || !length.is_multiple_of(2) {
        return Err(13);
    }
    let units: Vec<u16> = bytes[..length]
        .chunks_exact(2)
        .map(|chunk| u16::from_le_bytes([chunk[0], chunk[1]]))
        .collect();
    if units.last() != Some(&0) || units[..units.len() - 1].contains(&0) {
        return Err(13);
    }
    String::from_utf16(&units[..units.len() - 1])
        .map(Some)
        .map_err(|_| 13)
}

fn pin_cache_ancestors(path: &Path) -> Result<Vec<NativeHandle>, u32> {
    let text = path
        .to_str()
        .filter(|text| valid_image_path(text))
        .ok_or(13_u32)?;
    let root = format!("{}\\", &text[..2]);
    // SAFETY: terminated local drive root. Mapped/network/removable roots are excluded.
    if unsafe { GetDriveTypeW(wide(&root).as_ptr()) } != DRIVE_FIXED {
        return Err(50);
    }
    let mut current = PathBuf::from(root);
    let mut handles = Vec::new();
    for component in std::iter::once("").chain(text[3..].split('\\')) {
        if !component.is_empty() {
            current.push(component);
        }
        let name: Vec<u16> = current.as_os_str().encode_wide().chain(Some(0)).collect();
        // SAFETY: validated absolute fixed-drive path. Metadata only; reparse points are not followed;
        // no delete sharing, and ancestor handles remain pinned until the inventory finishes.
        let raw = unsafe {
            CreateFileW(
                name.as_ptr(),
                FILE_READ_ATTRIBUTES,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                null(),
                OPEN_EXISTING,
                FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
                null_mut(),
            )
        };
        if raw == INVALID_HANDLE_VALUE {
            // SAFETY: immediately read this failed file-open error.
            let error = unsafe { GetLastError() };
            if matches!(error, ERROR_FILE_NOT_FOUND | ERROR_PATH_NOT_FOUND) {
                break;
            }
            return Err(error);
        }
        let handle = NativeHandle(raw);
        let mut attributes = FILE_ATTRIBUTE_TAG_INFO::default();
        // SAFETY: retained handle and exact metadata output size; no file content is read.
        if unsafe {
            GetFileInformationByHandleEx(
                handle.0,
                FileAttributeTagInfo,
                (&mut attributes as *mut FILE_ATTRIBUTE_TAG_INFO).cast(),
                size_of::<FILE_ATTRIBUTE_TAG_INFO>() as u32,
            )
        } == 0
        {
            // SAFETY: immediately read the metadata query error.
            return Err(unsafe { GetLastError() });
        }
        if attributes.FileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0
            || attributes.FileAttributes & FILE_ATTRIBUTE_DIRECTORY == 0
        {
            return Err(13);
        }
        handles.push(handle);
    }
    Ok(handles)
}

pub(crate) fn delivery_cache(windows_directory: &Path) -> DeliveryCacheResolution {
    let empty = |source, status| DeliveryCacheResolution {
        path: None,
        source,
        status,
        _pinned_directories: Vec::new(),
    };
    let Some(windows) = windows_directory
        .to_str()
        .filter(|text| valid_image_path(text))
    else {
        return empty(
            DeliveryCacheSource::CandidateUnsafe,
            ElevationStatus::Denied,
        );
    };
    let drive = windows.chars().next().unwrap().to_ascii_uppercase();
    let policy = match cache_policy() {
        Ok(policy) => policy,
        Err(ERROR_ACCESS_DENIED) => {
            return empty(
                DeliveryCacheSource::PolicyReadDenied,
                ElevationStatus::Denied,
            )
        }
        Err(_) => {
            return empty(
                DeliveryCacheSource::PolicyReadFailed,
                ElevationStatus::Partial,
            )
        }
    };
    let (path, source) = match policy {
        None => (windows_directory.join(r"ServiceProfiles\NetworkService\AppData\Local\Microsoft\Windows\DeliveryOptimization\Cache"), DeliveryCacheSource::DocumentedDefault),
        Some(value) => {
            let Some(folder) = policy_folder(&value, drive) else {
                let drive_only = value.len() == 2 && value.as_bytes()[0].is_ascii_alphabetic() && value.as_bytes()[1] == b':'
                    || value.eq_ignore_ascii_case("%SYSTEMDRIVE%");
                return empty(if drive_only { DeliveryCacheSource::PolicySelectsDriveOnly } else { DeliveryCacheSource::PolicyMalformed }, ElevationStatus::Partial);
            };
            (PathBuf::from(folder), DeliveryCacheSource::GroupPolicyFolder)
        }
    };
    match pin_cache_ancestors(&path) {
        Ok(handles) => DeliveryCacheResolution {
            path: Some(path),
            source,
            // A policy/default is a location candidate, not confirmation of DoSvc's effective state.
            status: ElevationStatus::Partial,
            _pinned_directories: handles,
        },
        Err(ERROR_ACCESS_DENIED) => empty(
            DeliveryCacheSource::CandidateUnsafe,
            ElevationStatus::Denied,
        ),
        Err(_) => empty(
            DeliveryCacheSource::CandidateUnsafe,
            ElevationStatus::Partial,
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wmi_selected_columns_never_request_machine_identifiers_or_paths() {
        for query in [FixedQuery::ShadowStorage, FixedQuery::StorageReliability] {
            for column in query.columns() {
                assert!(![
                    "DeviceId",
                    "ObjectId",
                    "UniqueId",
                    "SerialNumber",
                    "Volume",
                    "DiffVolume",
                    "Name",
                    "Manufacturer",
                    "PSComputerName"
                ]
                .contains(column));
            }
        }
    }

    #[test]
    fn wmi_uint64_strings_are_exact_and_not_coerced_from_floating_point() {
        // SAFETY: these owned test VARIANTs use their initialized, matching scalar discriminants.
        let mut value = unsafe {
            let mut value = VARIANT::default();
            // ManuallyDrop is transparent; explicit writes initialize the selected generated
            // union member without attempting an implicit DerefMut on that union field.
            let inner = std::ptr::addr_of_mut!(value.Anonymous.Anonymous).cast::<VARIANT_0_0>();
            (*inner).vt = VT_BSTR;
            std::ptr::addr_of_mut!((*inner).Anonymous.bstrVal).write(std::mem::ManuallyDrop::new(
                BSTR::from("18446744073709551615"),
            ));
            value
        };
        assert_eq!(numeric_variant(&value), Some(u64::MAX));
        // SAFETY: the owned BSTR VARIANT is cleared once after its scalar was copied.
        unsafe {
            VariantClear(&mut value).unwrap();
        }
        let value = VARIANT::from(12.5_f64);
        assert_eq!(numeric_variant(&value), None);
        let value = VARIANT::from(-1_i32);
        assert_eq!(numeric_variant(&value), None);
    }
}
