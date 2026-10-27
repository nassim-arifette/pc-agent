//! Bounded read-only battery IOCTLs. Device paths and tags remain inside a local reference cache.
use crate::{parse::battery as parse, CoverageStatus as C, Inspection};
use pcagent_shared::{new_id, unix_ms, Window};
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    ptr::{null, null_mut},
    sync::{Mutex, OnceLock},
    time::{Duration, Instant},
};
use windows_sys::Win32::{
    Devices::DeviceAndDriverInstallation::{
        SetupDiDestroyDeviceInfoList, SetupDiEnumDeviceInterfaces, SetupDiGetClassDevsW,
        SetupDiGetDeviceInterfaceDetailW, DIGCF_DEVICEINTERFACE, DIGCF_PRESENT, HDEVINFO,
        SP_DEVICE_INTERFACE_DATA, SP_DEVICE_INTERFACE_DETAIL_DATA_W,
    },
    Foundation::{
        CloseHandle, GetLastError, ERROR_ACCESS_DENIED, ERROR_DEVICE_NOT_CONNECTED,
        ERROR_FILE_NOT_FOUND, ERROR_INSUFFICIENT_BUFFER, ERROR_INVALID_DATA,
        ERROR_INVALID_FUNCTION, ERROR_NOT_SUPPORTED, ERROR_NO_MORE_ITEMS, ERROR_NO_SUCH_DEVICE,
        GENERIC_READ, HANDLE, INVALID_HANDLE_VALUE,
    },
    Storage::FileSystem::{CreateFileW, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING},
    System::{
        Power::{
            BatteryEstimatedTime, BatteryInformation, BATTERY_TAG_INVALID, GUID_DEVICE_BATTERY,
            IOCTL_BATTERY_QUERY_INFORMATION, IOCTL_BATTERY_QUERY_STATUS, IOCTL_BATTERY_QUERY_TAG,
        },
        IO::DeviceIoControl,
    },
};

const SOURCE_ENUM: &str = "SetupAPI/GUID_DEVICE_BATTERY";
const SOURCE_INFO: &str = "DeviceIoControl/IOCTL_BATTERY_QUERY_INFORMATION";
const SOURCE_STATUS: &str = "DeviceIoControl/IOCTL_BATTERY_QUERY_STATUS";
const SOURCE_ESTIMATE: &str = "DeviceIoControl/BatteryEstimatedTime";
const SOURCE_DERIVED: &str = "derived/IOCTL_BATTERY_QUERY_INFORMATION+IOCTL_BATTERY_QUERY_STATUS";
const MAX_DEVICES: u32 = 8;
const MAX_PATH_BYTES: usize = 8192;
const MAX_REFERENCES: usize = 64;
const SCAN_BUDGET: Duration = Duration::from_secs(4);

#[derive(Default)]
struct References {
    entries: BTreeMap<Vec<u16>, (u32, String)>,
}
impl References {
    fn get(&mut self, path: &[u16], tag: u32) -> String {
        if let Some((known_tag, reference)) = self.entries.get(path) {
            if *known_tag == tag {
                return reference.clone();
            }
        }
        if self.entries.len() >= MAX_REFERENCES && !self.entries.contains_key(path) {
            // A new opaque reference breaks continuity safely if the local cache is exhausted.
            self.entries.clear();
        }
        let reference = new_id("battery");
        self.entries.insert(path.to_vec(), (tag, reference.clone()));
        reference
    }
    fn forget_absent(&mut self, present: &BTreeSet<Vec<u16>>) {
        self.entries.retain(|path, _| present.contains(path));
    }
}
fn references() -> &'static Mutex<References> {
    static REFERENCES: OnceLock<Mutex<References>> = OnceLock::new();
    REFERENCES.get_or_init(|| Mutex::new(References::default()))
}

struct DeviceSet(HDEVINFO);
impl Drop for DeviceSet {
    fn drop(&mut self) {
        // SAFETY: the uniquely owned SetupAPI set has no remaining borrowed records.
        unsafe { SetupDiDestroyDeviceInfoList(self.0) };
    }
}
struct Device(HANDLE);
impl Drop for Device {
    fn drop(&mut self) {
        // SAFETY: this synchronous read-only device handle is uniquely owned and closed once.
        unsafe { CloseHandle(self.0) };
    }
}

fn failure(code: u32) -> C {
    match code {
        ERROR_ACCESS_DENIED => C::Denied,
        ERROR_FILE_NOT_FOUND
        | ERROR_NO_SUCH_DEVICE
        | ERROR_DEVICE_NOT_CONNECTED
        | ERROR_INVALID_FUNCTION
        | ERROR_NOT_SUPPORTED => C::Unsupported,
        _ => C::Error,
    }
}

fn device_path(set: &DeviceSet, interface: &SP_DEVICE_INTERFACE_DATA) -> Result<Vec<u16>, u32> {
    let mut required = 0;
    // SAFETY: valid set/record; null detail pointer performs the documented size query only.
    let ok = unsafe {
        SetupDiGetDeviceInterfaceDetailW(set.0, interface, null_mut(), 0, &mut required, null_mut())
    };
    // SAFETY: read immediately after the native size query.
    let code = unsafe { GetLastError() };
    if ok == 0 && code != ERROR_INSUFFICIENT_BUFFER {
        return Err(code);
    }
    let size = required as usize;
    if !(std::mem::size_of::<SP_DEVICE_INTERFACE_DETAIL_DATA_W>()..=MAX_PATH_BYTES).contains(&size)
    {
        return Err(ERROR_INVALID_DATA);
    }
    // usize storage supplies the alignment of either supported Windows ABI.
    let mut storage = vec![0_usize; size.div_ceil(std::mem::size_of::<usize>())];
    let detail = storage
        .as_mut_ptr()
        .cast::<SP_DEVICE_INTERFACE_DETAIL_DATA_W>();
    // SAFETY: the allocated aligned region is at least the requested bounded size; cbSize is
    // the ABI-specific structure size. SetupAPI writes only inside the declared buffer.
    let ok = unsafe {
        (*detail).cbSize = std::mem::size_of::<SP_DEVICE_INTERFACE_DETAIL_DATA_W>() as u32;
        SetupDiGetDeviceInterfaceDetailW(
            set.0,
            interface,
            detail,
            required,
            &mut required,
            null_mut(),
        )
    };
    if ok == 0 {
        // SAFETY: immediate error read after the failed native detail query.
        return Err(unsafe { GetLastError() });
    }
    if required as usize > size {
        return Err(ERROR_INVALID_DATA);
    }
    let offset = std::mem::offset_of!(SP_DEVICE_INTERFACE_DETAIL_DATA_W, DevicePath);
    let count = (required as usize)
        .checked_sub(offset)
        .ok_or(ERROR_INVALID_DATA)?
        / 2;
    // SAFETY: DevicePath is u16-aligned at its ABI field offset; the slice stays inside the
    // initialized allocation and the checked returned byte count. It is not serialized.
    let path = unsafe {
        std::slice::from_raw_parts(
            storage.as_ptr().cast::<u8>().add(offset).cast::<u16>(),
            count,
        )
    };
    let end = path
        .iter()
        .position(|unit| *unit == 0)
        .ok_or(ERROR_INVALID_DATA)?;
    if end == 0 {
        return Err(ERROR_INVALID_DATA);
    }
    Ok(path[..=end].to_vec())
}

struct Read<const N: usize> {
    bytes: Result<[u8; N], u32>,
    window: Window,
}
fn read<const N: usize>(device: &Device, code: u32, input: &[u8]) -> Read<N> {
    let started = unix_ms();
    let clock = Instant::now();
    let mut output = [0_u8; N];
    let mut returned = 0;
    // SAFETY: only private fixed-size read-only IOCTLs call this helper; live device handle,
    // initialized bounded input/output and byte-count pointers, synchronous completion, and
    // no overlapped pointer outliving this stack frame. No battery setter is called.
    let ok = unsafe {
        DeviceIoControl(
            device.0,
            code,
            input.as_ptr().cast(),
            input.len() as u32,
            output.as_mut_ptr().cast(),
            N as u32,
            &mut returned,
            null_mut(),
        )
    };
    let bytes = if ok == 0 {
        // SAFETY: immediate error read after the failed native query.
        Err(unsafe { GetLastError() })
    } else if returned as usize != N {
        Err(ERROR_INVALID_DATA)
    } else {
        Ok(output)
    };
    Read {
        bytes,
        window: Window::finish(started, clock),
    }
}
fn current_tag(device: &Device) -> Result<u32, u32> {
    let result = read::<4>(device, IOCTL_BATTERY_QUERY_TAG, &0_u32.to_le_bytes());
    let tag = parse::scalar(&result.bytes?).ok_or(ERROR_INVALID_DATA)?;
    if tag == BATTERY_TAG_INVALID {
        Err(ERROR_FILE_NOT_FOUND)
    } else {
        Ok(tag)
    }
}
fn information_input(tag: u32, level: i32) -> [u8; 12] {
    let mut input = [0_u8; 12];
    input[..4].copy_from_slice(&tag.to_le_bytes());
    input[4..8].copy_from_slice(&level.to_le_bytes());
    // AtRate = 0 asks for the driver's estimate at its present drain.
    input
}
fn status_input(tag: u32) -> [u8; 20] {
    let mut input = [0_u8; 20];
    input[..4].copy_from_slice(&tag.to_le_bytes());
    // Timeout = 0 returns immediately regardless of wait conditions.
    input
}

fn value(
    out: &mut Inspection,
    reference: &str,
    metric: (&str, &str, &str),
    value: impl Into<Value>,
    window: &Window,
    partial: bool,
) {
    out.value(reference, metric.0, value, metric.1, metric.2);
    if let Some(observation) = out.observations.last_mut() {
        observation.measured_at_unix_ms = window.finished_at_unix_ms;
        observation.window = window.clone();
        if partial && !observation.value.is_null() {
            observation.status = C::Partial;
        }
    }
}
fn project(
    out: &mut Inspection,
    reference: &str,
    facts: &parse::Facts,
    info: &Window,
    status: &Window,
    estimate: &Window,
    combined: &Window,
) {
    let capacity_partial = facts.capacities_consistent == Some(false);
    let state_partial = facts.state_consistent == Some(false);
    macro_rules! observation {
        ($metric:literal, $value:expr, $unit:literal, $source:expr, $window:expr, $partial:expr) => {
            value(
                out,
                reference,
                ($metric, $unit, $source),
                serde_json::json!($value),
                $window,
                $partial,
            );
        };
    }
    observation!(
        "battery_kind",
        facts.kind.map(parse::Kind::text),
        "category",
        SOURCE_INFO,
        info,
        false
    );
    observation!(
        "battery_capacity_units",
        facts.units.map(parse::Units::text),
        "category",
        SOURCE_INFO,
        info,
        false
    );
    observation!(
        "battery_rechargeable",
        facts.rechargeable,
        "boolean",
        SOURCE_INFO,
        info,
        false
    );
    observation!(
        "battery_design_capacity_mwh",
        facts.design_capacity_mwh,
        "milliwatt_hours",
        SOURCE_INFO,
        info,
        capacity_partial
    );
    observation!(
        "battery_full_charge_capacity_mwh",
        facts.full_charge_capacity_mwh,
        "milliwatt_hours",
        SOURCE_INFO,
        info,
        capacity_partial
    );
    observation!(
        "battery_critical_bias_mwh",
        facts.critical_bias_mwh,
        "milliwatt_hours",
        SOURCE_INFO,
        info,
        false
    );
    observation!(
        "battery_cycle_count",
        facts.cycle_count,
        "count",
        SOURCE_INFO,
        info,
        false
    );
    observation!(
        "battery_capacities_consistent",
        facts.capacities_consistent,
        "boolean",
        SOURCE_DERIVED,
        combined,
        false
    );
    observation!(
        "battery_capacity_retention_percent",
        facts.capacity_retention_percent,
        "percent",
        SOURCE_DERIVED,
        combined,
        false
    );
    observation!(
        "battery_estimated_wear_percent",
        facts.estimated_wear_percent,
        "percent",
        SOURCE_DERIVED,
        combined,
        false
    );
    observation!(
        "battery_remaining_capacity_mwh",
        facts.remaining_capacity_mwh,
        "milliwatt_hours",
        SOURCE_STATUS,
        status,
        capacity_partial
    );
    observation!(
        "battery_charge_percent",
        facts.charge_percent,
        "percent",
        SOURCE_DERIVED,
        combined,
        false
    );
    observation!(
        "battery_power_rate_mw",
        facts.power_rate_mw,
        "milliwatts",
        SOURCE_STATUS,
        status,
        state_partial
    );
    observation!(
        "battery_discharge_power_mw",
        facts.discharge_power_mw,
        "milliwatts",
        SOURCE_DERIVED,
        combined,
        false
    );
    observation!(
        "battery_charge_power_mw",
        facts.charge_power_mw,
        "milliwatts",
        SOURCE_DERIVED,
        combined,
        false
    );
    observation!(
        "battery_voltage_mv",
        facts.voltage_mv,
        "millivolts",
        SOURCE_STATUS,
        status,
        false
    );
    observation!(
        "battery_charge_state",
        facts.charge_state.map(parse::ChargeState::text),
        "category",
        SOURCE_STATUS,
        status,
        state_partial
    );
    observation!(
        "battery_on_ac",
        facts.on_ac,
        "boolean",
        SOURCE_STATUS,
        status,
        state_partial
    );
    observation!(
        "battery_critical",
        facts.critical,
        "boolean",
        SOURCE_STATUS,
        status,
        state_partial
    );
    observation!(
        "battery_state_consistent",
        facts.state_consistent,
        "boolean",
        SOURCE_STATUS,
        status,
        false
    );
    observation!(
        "battery_estimated_runtime_windows_seconds",
        facts.runtime_windows_seconds,
        "seconds",
        SOURCE_ESTIMATE,
        estimate,
        false
    );
    observation!(
        "battery_estimated_runtime_at_current_rate_seconds",
        facts.runtime_at_current_rate_seconds,
        "seconds",
        SOURCE_DERIVED,
        combined,
        false
    );
}

fn collect_device(out: &mut Inspection, path: &[u16]) -> bool {
    // SAFETY: SetupAPI supplied a checked NUL-terminated interface path. Only GENERIC_READ
    // access is requested; no create, delete, write or overlapped device operation is enabled.
    let handle = unsafe {
        CreateFileW(
            path.as_ptr(),
            GENERIC_READ,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            null(),
            OPEN_EXISTING,
            0,
            null_mut(),
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        // SAFETY: immediate error read after the failed read-only open.
        out.coverage(
            "battery_device",
            failure(unsafe { GetLastError() }),
            SOURCE_ENUM,
            "A present battery interface could not be opened for read-only queries.",
        );
        return false;
    }
    let device = Device(handle);
    let tag = match current_tag(&device) {
        Ok(tag) => tag,
        Err(code) => {
            if let Ok(mut cache) = references().lock() {
                cache.entries.remove(path);
            }
            out.coverage(
                "battery_tag",
                failure(code),
                SOURCE_INFO,
                "No current battery tag was available; no battery values were fabricated.",
            );
            return false;
        }
    };
    let started = unix_ms();
    let clock = Instant::now();
    let info = read::<{ parse::INFORMATION_SIZE }>(
        &device,
        IOCTL_BATTERY_QUERY_INFORMATION,
        &information_input(tag, BatteryInformation),
    );
    let status =
        read::<{ parse::STATUS_SIZE }>(&device, IOCTL_BATTERY_QUERY_STATUS, &status_input(tag));
    let information = info
        .bytes
        .as_ref()
        .ok()
        .and_then(|bytes| parse::information(bytes));
    let state = status
        .bytes
        .as_ref()
        .ok()
        .and_then(|bytes| parse::status(bytes));
    let estimated = if state.is_some_and(|status| status.power_state & 2 != 0) {
        Some(read::<4>(
            &device,
            IOCTL_BATTERY_QUERY_INFORMATION,
            &information_input(tag, BatteryEstimatedTime),
        ))
    } else {
        None
    };
    // A changed/removed battery invalidates this entire snapshot, including static fields.
    if current_tag(&device).ok() != Some(tag) {
        if let Ok(mut cache) = references().lock() {
            cache.entries.remove(path);
        }
        out.coverage(
            "battery_coherence",
            C::Partial,
            SOURCE_INFO,
            "The battery tag changed or could not be rechecked; this device snapshot was omitted.",
        );
        return false;
    }
    let combined = Window::finish(started, clock);
    let reference = match references().lock() {
        Ok(mut cache) => cache.get(path, tag),
        Err(_) => {
            out.coverage(
                "battery_reference",
                C::Error,
                SOURCE_ENUM,
                "The local opaque-reference cache was unavailable; no unstable point was returned.",
            );
            return false;
        }
    };
    let estimate_value = estimated
        .as_ref()
        .and_then(|read| read.bytes.as_ref().ok())
        .and_then(|bytes| parse::scalar(bytes));
    let facts = parse::normalize(information, state, estimate_value);
    project(
        out,
        &reference,
        &facts,
        &info.window,
        &status.window,
        estimated
            .as_ref()
            .map(|read| &read.window)
            .unwrap_or(&status.window),
        &combined,
    );
    let mut partial = false;
    for (scope, source, code) in [
        ("battery_information", SOURCE_INFO, info.bytes.err()),
        ("battery_status", SOURCE_STATUS, status.bytes.err()),
        (
            "battery_runtime",
            SOURCE_ESTIMATE,
            estimated.and_then(|read| read.bytes.err()),
        ),
    ] {
        if let Some(code) = code {
            out.coverage(scope, failure(code), source,
                "This fixed numeric battery query was unavailable; its dependent values remain null.");
            partial = true;
        }
    }
    if facts.capacities_consistent == Some(false) || facts.state_consistent == Some(false) {
        out.coverage("battery_consistency", C::Partial, SOURCE_DERIVED,
            "Capacity bounds or power flags/rate disagree; readings are retained with explicit consistency flags, and dependent estimates are null.");
        partial = true;
    }
    out.coverage("battery_measurements", if partial { C::Partial } else { C::Ok }, SOURCE_STATUS,
        "One battery only. Optional absent values are null; power is signed mW and capacities are mWh only when absolute. No implicit multi-battery sum or per-application energy attribution.");
    true
}

pub(super) fn collect(out: &mut Inspection) {
    let started = unix_ms();
    let clock = Instant::now();
    // SAFETY: fixed battery-interface GUID, no enumerator/window; local present devices only.
    let set = unsafe {
        SetupDiGetClassDevsW(
            &GUID_DEVICE_BATTERY,
            null(),
            null_mut(),
            DIGCF_PRESENT | DIGCF_DEVICEINTERFACE,
        )
    };
    if set == -1 {
        // SAFETY: immediate error read after the failed SetupAPI query.
        out.coverage(
            "battery_devices",
            failure(unsafe { GetLastError() }),
            SOURCE_ENUM,
            "Present battery interfaces could not be enumerated.",
        );
        return;
    }
    let set = DeviceSet(set);
    let mut present = BTreeSet::new();
    let mut found = 0;
    let mut complete = false;
    let mut partial = false;
    for index in 0..=MAX_DEVICES {
        if clock.elapsed() > SCAN_BUDGET {
            partial = true;
            break;
        }
        let mut interface = SP_DEVICE_INTERFACE_DATA {
            cbSize: std::mem::size_of::<SP_DEVICE_INTERFACE_DATA>() as u32,
            ..Default::default()
        };
        // SAFETY: live owned set, bounded index, fixed interface GUID and initialized output.
        if unsafe {
            SetupDiEnumDeviceInterfaces(set.0, null(), &GUID_DEVICE_BATTERY, index, &mut interface)
        } == 0
        {
            // SAFETY: immediate last-error read after enumeration stopped.
            let code = unsafe { GetLastError() };
            complete = code == ERROR_NO_MORE_ITEMS;
            partial |= !complete;
            break;
        }
        if index == MAX_DEVICES {
            partial = true;
            break;
        }
        found += 1;
        match device_path(&set, &interface) {
            Ok(path) => {
                present.insert(path.clone());
                partial |= !collect_device(out, &path);
            }
            Err(code) => {
                out.coverage(
                    "battery_interface",
                    failure(code),
                    SOURCE_ENUM,
                    "A bounded battery interface path could not be read; this device was omitted.",
                );
                partial = true;
            }
        }
    }
    if complete && !partial {
        if let Ok(mut cache) = references().lock() {
            cache.forget_absent(&present);
        }
    }
    devices_coverage(out, found, complete, partial);
    out.window = Window::finish(started, clock);
    out.measured_at_unix_ms = out.window.finished_at_unix_ms;
}

/// The same read runs on the shared inspection worker and on the live window's battery-only
/// worker, so this text names no particular worker or bound: the model receives it verbatim.
const DEVICES_NONE: &str = "No present battery interface was reported by Windows.";
const DEVICES_BOUNDS: &str = "At most eight interfaces and a four-second scan budget. Device paths/tags are used only locally; names, manufacturers, serials and unique IDs are never queried. A stalled driver is subject to its caller's time bound, which does not cancel native I/O.";

fn devices_coverage(out: &mut Inspection, found: u32, complete: bool, partial: bool) {
    let none = found == 0 && complete;
    let status = if none {
        C::Unsupported
    } else if partial || !complete {
        C::Partial
    } else {
        C::Ok
    };
    out.coverage(
        "battery_devices",
        status,
        SOURCE_ENUM,
        if none { DEVICES_NONE } else { DEVICES_BOUNDS },
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows_sys::Win32::System::Power::{
        BATTERY_INFORMATION, BATTERY_QUERY_INFORMATION, BATTERY_STATUS, BATTERY_WAIT_STATUS,
    };

    #[test]
    fn wire_layouts_and_control_codes_are_fixed_read_only() {
        assert_eq!(
            std::mem::size_of::<BATTERY_INFORMATION>(),
            parse::INFORMATION_SIZE
        );
        assert_eq!(std::mem::size_of::<BATTERY_STATUS>(), parse::STATUS_SIZE);
        assert_eq!(std::mem::size_of::<BATTERY_QUERY_INFORMATION>(), 12);
        assert_eq!(std::mem::size_of::<BATTERY_WAIT_STATUS>(), 20);
        for code in [
            IOCTL_BATTERY_QUERY_TAG,
            IOCTL_BATTERY_QUERY_INFORMATION,
            IOCTL_BATTERY_QUERY_STATUS,
        ] {
            assert_eq!((code >> 14) & 3, 1);
        }
        assert_eq!(&status_input(19)[4..], &[0; 16]);
        assert_eq!(&information_input(19, BatteryEstimatedTime)[8..], &[0; 4]);
    }
    #[test]
    fn opaque_references_survive_ticks_and_break_on_tag_change_absence_or_bound() {
        let mut cache = References::default();
        let path = [1, 2, 0];
        let first = cache.get(&path, 7);
        assert_eq!(cache.get(&path, 7), first);
        let second = cache.get(&path, 8);
        assert_ne!(second, first);
        cache.forget_absent(&BTreeSet::new());
        assert_ne!(cache.get(&path, 8), second);
        for index in 0..MAX_REFERENCES as u16 {
            cache.get(&[index + 10, 0], 1);
        }
        assert!(cache.entries.len() <= MAX_REFERENCES);
        assert!(first.starts_with("battery-"));
    }
    #[test]
    fn projection_retains_native_windows_units_unknowns_and_excludes_local_keys() {
        let facts = parse::normalize(
            Some(parse::Information {
                capabilities: 0x8000_0000,
                technology: 1,
                design_capacity: 60_000,
                full_charge_capacity: 48_000,
                critical_bias: 500,
                cycle_count: 0,
            }),
            Some(parse::Status {
                power_state: 2,
                capacity: 24_000,
                voltage: 11_700,
                rate: -12_000,
            }),
            Some(6_800),
        );
        let info = Window {
            started_at_unix_ms: 100,
            finished_at_unix_ms: 101,
            duration_ms: 1,
        };
        let status = Window {
            started_at_unix_ms: 102,
            finished_at_unix_ms: 103,
            duration_ms: 1,
        };
        let estimate = Window {
            started_at_unix_ms: 104,
            finished_at_unix_ms: 105,
            duration_ms: 1,
        };
        let combined = Window {
            started_at_unix_ms: 100,
            finished_at_unix_ms: 106,
            duration_ms: 6,
        };
        let mut out = Inspection::new();
        project(
            &mut out,
            "battery-00000000-0000-4000-8000-000000000000",
            &facts,
            &info,
            &status,
            &estimate,
            &combined,
        );
        let remaining = out
            .observations
            .iter()
            .find(|o| o.metric == "battery_remaining_capacity_mwh")
            .unwrap();
        assert_eq!(remaining.measured_at_unix_ms, 103);
        assert_eq!(remaining.window.started_at_unix_ms, 102);
        assert_eq!(remaining.window.duration_ms, 1);
        assert_eq!(remaining.source, SOURCE_STATUS);
        assert_eq!(remaining.unit, "milliwatt_hours");
        let estimated = out
            .observations
            .iter()
            .find(|o| o.metric == "battery_estimated_runtime_windows_seconds")
            .unwrap();
        assert_eq!(estimated.measured_at_unix_ms, 105);
        let rate = out
            .observations
            .iter()
            .find(|o| o.metric == "battery_power_rate_mw")
            .unwrap();
        assert_eq!(rate.value, -12_000);
        assert_eq!(rate.unit, "milliwatts");
        assert!(out
            .observations
            .iter()
            .find(|o| o.metric == "battery_cycle_count")
            .unwrap()
            .value
            .is_null());
        assert!(out.names.is_empty());
        let serialized = serde_json::to_string(&out).unwrap();
        assert!(!serialized.contains("DevicePath"));
        assert!(!serialized.contains("BatteryTag"));
        assert!(!serialized.contains("serial"));
    }
    #[test]
    fn device_coverage_names_no_particular_worker_bound() {
        // Synthetic: the coverage a live read carries, serialized, without opening any device.
        for (found, complete, partial, status) in [
            (0, true, false, C::Unsupported),
            (1, true, false, C::Ok),
            (1, true, true, C::Partial),
            (0, false, true, C::Partial),
        ] {
            let mut out = Inspection::new();
            devices_coverage(&mut out, found, complete, partial);
            let read = out.finish();
            assert_eq!(read.coverage[0].status, status);
            let serialized = serde_json::to_string(&read).unwrap();
            for stale in ["15-second", "single-worker", "15 seconds"] {
                assert!(!serialized.contains(stale), "{stale} in {serialized}");
            }
        }
        assert!(DEVICES_BOUNDS.contains("caller's time bound"));
    }
}
