//! Read existing optional sensor publishers. This adapter never starts an application, enables
//! sharing, contacts a web server, loads a sensor library/driver, or changes a sensor setting.
use super::{com_error, wide, wmi};
use crate::{
    parse::sensors::{
        self, Freshness, Header, Kind, ParseError, Readings, MAX_SAMPLES, MAX_SHARED_BYTES,
    },
    CoverageStatus as C, Inspection,
};
use serde_json::Value;
use std::{collections::BTreeMap, mem::size_of};
use windows_sys::Win32::{
    Foundation::{CloseHandle, GetLastError, HANDLE, WAIT_ABANDONED, WAIT_OBJECT_0, WAIT_TIMEOUT},
    System::{
        Memory::{
            MapViewOfFile, OpenFileMappingW, UnmapViewOfFile, VirtualQuery, FILE_MAP_READ,
            MEMORY_BASIC_INFORMATION, MEMORY_MAPPED_VIEW_ADDRESS, MEM_COMMIT, MEM_MAPPED,
            PAGE_READONLY,
        },
        Threading::{OpenMutexW, ReleaseMutex, WaitForSingleObject, SYNCHRONIZATION_SYNCHRONIZE},
    },
};

const LHM_SOURCE: &str = "WMI.root/LibreHardwareMonitor.Sensor (existing provider)";
const HWINFO_SOURCE: &str = "HWiNFO.SharedMemory2 (OpenFileMappingW FILE_MAP_READ)";
const HWINFO_MAPPING: &str = r"Global\HWiNFO_SENS_SM2";
const HWINFO_MUTEX: &str = r"Global\HWiNFO_SM2_MUTEX";
const MUTEX_WAIT_MS: u32 = 50;

/// Hook from get_power_and_thermal after the native Windows/NVML sources. Each absent supplier
/// has its own unsupported coverage and does not suppress measurements from the other suppliers.
pub(super) fn collect(out: &mut Inspection) {
    lhm(out);
    hwinfo(out);
}

fn lhm(out: &mut Inspection) {
    // Kind affects the older WMI projection only. query returns these selected raw scalar columns;
    // the bounded sensor parser supplies their fixed metrics and never projects Parent strings.
    let columns = ["SensorType", "Parent", "Value"].map(|property| wmi::Column {
        property,
        metric: "unused",
        unit: "unused",
        kind: wmi::Kind::Text,
    });
    match wmi::query(
        r"root\LibreHardwareMonitor",
        "Sensor",
        &columns,
        None,
        MAX_SAMPLES,
    ) {
        Ok((rows, partial)) => {
            let parsed = sensors::lhm_readings(&rows, partial);
            if rows.is_empty() && !partial {
                out.coverage(
                    "libre_hardware_monitor",
                    C::Unsupported,
                    LHM_SOURCE,
                    "No published sensor rows are available. The application/WMI provider may be absent or sensor monitoring disabled; nothing was started or installed.",
                );
                return;
            }
            project(out, parsed, Freshness::Unknown, None, "lhm_wmi", LHM_SOURCE);
            out.coverage(
                "libre_hardware_monitor",
                C::Partial,
                LHM_SOURCE,
                "At most 256 sensor rows and the existing WMI time bound. The legacy provider exports missing measurements as zero, so every zero is unknown. It supplies no polling timestamp: these are readings at query time with unknown measurement age, not proven current CPU/SSD temperatures. Only SensorType, Parent and Value were read; names, identifiers and historical extrema were not queried.",
            );
        }
        Err(error) => out.coverage(
            "libre_hardware_monitor",
            com_error(&error),
            LHM_SOURCE,
            "The existing LHM WMI namespace/class is absent, unavailable or denied. Some LHM versions have no WMI publisher. No installation, web request or elevation was attempted.",
        ),
    }
}

struct Handle(HANDLE);
impl Drop for Handle {
    fn drop(&mut self) {
        // SAFETY: each non-null handle is owned by one wrapper and closed exactly once.
        unsafe {
            CloseHandle(self.0);
        }
    }
}
struct MutexOwnership<'a>(&'a Handle);
impl Drop for MutexOwnership<'_> {
    fn drop(&mut self) {
        // SAFETY: this thread acquired this mutex once, including abandoned ownership.
        unsafe {
            ReleaseMutex(self.0 .0);
        }
    }
}
struct View {
    address: MEMORY_MAPPED_VIEW_ADDRESS,
    readable: usize,
}
impl Drop for View {
    fn drop(&mut self) {
        // SAFETY: successful mapping owned by this wrapper, unmapped once after all reads finish.
        unsafe {
            UnmapViewOfFile(self.address);
        }
    }
}

impl View {
    fn map(mapping: &Handle, bytes: usize) -> Result<Self, ReadError> {
        if !(44..=MAX_SHARED_BYTES).contains(&bytes) {
            return Err(ParseError::Limit.into());
        }
        // SAFETY: valid read-only handle; the requested view itself is bounded, with no write or
        // executable access. MapViewOfFile verifies the extent against the existing mapping.
        let address = unsafe { MapViewOfFile(mapping.0, FILE_MAP_READ, 0, 0, bytes) };
        if address.Value.is_null() {
            return Err(ReadError::os("The bounded HWiNFO read-only view could not be mapped; its declared extent may exceed the actual mapping."));
        }
        let view = Self {
            address,
            readable: bytes,
        };
        let mut region = MEMORY_BASIC_INFORMATION::default();
        // SAFETY: mapped base and initialized exact-size output. No mapped data is accessed yet.
        let queried = unsafe {
            VirtualQuery(
                view.address.Value,
                &mut region,
                size_of::<MEMORY_BASIC_INFORMATION>(),
            )
        };
        if queried != size_of::<MEMORY_BASIC_INFORMATION>()
            || region.BaseAddress != view.address.Value
            || region.State != MEM_COMMIT
            || region.Type != MEM_MAPPED
            || region.Protect != PAGE_READONLY
            || region.RegionSize < bytes
        {
            return Err(ReadError {
                status: C::Error,
                detail: "The HWiNFO view is not a committed readable mapped region large enough for the requested extent; no bytes were dereferenced.",
            });
        }
        Ok(view)
    }

    fn copy_into(&self, destination: &mut [u8]) -> Result<(), ReadError> {
        if destination.len() > self.readable {
            return Err(ParseError::Truncated.into());
        }
        // SAFETY: map verified this many bytes against both the actual mapping and VirtualQuery.
        // Destination is an owned disjoint initialized buffer; caller holds the publisher mutex.
        unsafe {
            std::ptr::copy_nonoverlapping(
                self.address.Value.cast::<u8>(),
                destination.as_mut_ptr(),
                destination.len(),
            );
        }
        Ok(())
    }
}

struct ReadError {
    status: C,
    detail: &'static str,
}
impl ReadError {
    fn os(detail: &'static str) -> Self {
        // SAFETY: called immediately after a failed Windows call on this thread.
        let code = unsafe { GetLastError() };
        Self {
            status: match code {
                2 | 3 => C::Unsupported,
                5 | 1314 => C::Denied,
                _ => C::Error,
            },
            detail,
        }
    }
}
impl From<ParseError> for ReadError {
    fn from(error: ParseError) -> Self {
        let (status, detail) = match error {
            ParseError::Inactive => (
                C::Unsupported,
                "The HWiNFO publisher marked the shared memory inactive (DEAD). No readings were used; sharing may be disabled or expired.",
            ),
            ParseError::Version => (
                C::Unsupported,
                "Unsupported HWiNFO shared-memory layout. Only published versions 1/2 and revisions 0/1 are parsed; no field offsets were guessed.",
            ),
            ParseError::Limit => (
                C::Partial,
                "HWiNFO shared memory exceeds the 4 MiB, 256 component or 4096 reading bound. No oversized table was allocated or read.",
            ),
            ParseError::ConcurrentChange => (
                C::Partial,
                "The HWiNFO header changed during the mutex-protected copy. The entire copy was discarded; no inconsistent measurements were used.",
            ),
            ParseError::Signature | ParseError::Layout | ParseError::Truncated => (
                C::Error,
                "HWiNFO shared-memory signature, section layout, component links or readable extent is invalid. No malformed measurements were used.",
            ),
        };
        Self { status, detail }
    }
}

fn hwinfo(out: &mut Inspection) {
    match read_hwinfo() {
        Ok((header, readings)) => {
            let partial = readings.truncated
                || readings.unsupported_readings != 0
                || readings.unknown_values != 0
                || readings
                    .samples
                    .iter()
                    .any(|s| s.category == sensors::Component::Unknown)
                || header.freshness != Freshness::Fresh;
            let provider = project(
                out,
                readings,
                header.freshness,
                header.polled_at_unix_ms,
                "hwinfo_shared_memory",
                HWINFO_SOURCE,
            );
            out.value(
                &provider,
                "sensor_sample_age_seconds",
                header
                    .age_ms
                    .map(|age| Value::from(age as f64 / 1000.0))
                    .unwrap_or(Value::Null),
                "seconds",
                HWINFO_SOURCE,
            );
            out.value(
                &provider,
                "sensor_last_poll_unix_ms",
                header
                    .polled_at_unix_ms
                    .map(Value::from)
                    .unwrap_or(Value::Null),
                "unix_milliseconds",
                HWINFO_SOURCE,
            );
            out.coverage(
                "hwinfo_shared_memory",
                if partial { C::Partial } else { C::Ok },
                HWINFO_SOURCE,
                "Existing local mapping only, read-only under the publisher mutex with a 50 ms acquisition bound and unchanged header before/after the bounded copy. Versions 1/2, revisions 0/1, at most 4 MiB/256 components/4096 readings examined/256 readings returned. Only current finite values with recognized units and a last poll at most 15 seconds old are used; stale, future-dated or undated values are unknown. Sensor/device names, IDs, user labels and min/max/average values are discarded. Unknown component categories are not inferred from model names; CPU package temperature, PSU power or a thermal-throttling cause is never inferred from another reading.",
            );
        }
        Err(error) => out.coverage(
            "hwinfo_shared_memory",
            error.status,
            HWINFO_SOURCE,
            error.detail,
        ),
    }
}

fn read_hwinfo() -> Result<(Header, Readings), ReadError> {
    // SAFETY: fixed local mapping name, read access only, no handle inheritance and no creation.
    let mapping = unsafe { OpenFileMappingW(FILE_MAP_READ, 0, wide(HWINFO_MAPPING).as_ptr()) };
    if mapping.is_null() {
        return Err(ReadError::os("HWiNFO sensor shared memory is absent or unreadable. The application/sensors window may not be running, sharing may be disabled or expired. Nothing was started, enabled or installed."));
    }
    let mapping = Handle(mapping);
    // SAFETY: fixed mutex name, only synchronization access, no creation or security changes.
    let mutex = unsafe { OpenMutexW(SYNCHRONIZATION_SYNCHRONIZE, 0, wide(HWINFO_MUTEX).as_ptr()) };
    if mutex.is_null() {
        return Err(ReadError::os("The HWiNFO publisher mutex is absent or unreadable. A snapshot cannot be synchronized; no unsynchronized readings were used."));
    }
    let mutex = Handle(mutex);
    // SAFETY: valid owned mutex handle; fixed short timeout, never an unbounded wait.
    let wait = unsafe { WaitForSingleObject(mutex.0, MUTEX_WAIT_MS) };
    let _ownership = match wait {
        WAIT_OBJECT_0 => MutexOwnership(&mutex),
        WAIT_ABANDONED => {
            let _ownership = MutexOwnership(&mutex);
            return Err(ReadError {
                status: C::Partial,
                detail: "The HWiNFO publisher mutex was abandoned. Possibly interrupted writes were discarded and mutex ownership was released.",
            });
        }
        WAIT_TIMEOUT => {
            return Err(ReadError {
                status: C::Partial,
                detail: "The HWiNFO publisher mutex was busy for 50 ms. No unsynchronized readings were used.",
            });
        }
        _ => return Err(ReadError::os("The HWiNFO synchronization wait failed.")),
    };
    let mut header_view = View::map(&mapping, 44)?;
    let mut before = [0_u8; 48];
    header_view.copy_into(&mut before[..44])?;
    let now = pcagent_shared::unix_ms();
    let header = match sensors::hwinfo_header(&before[..44], MAX_SHARED_BYTES, now) {
        // Revision 1 needs its additional four header bytes. The preliminary fixed-size view
        // deliberately works for a valid revision-0 mapping as small as 44 bytes too.
        Err(ParseError::Truncated) => {
            drop(header_view);
            header_view = View::map(&mapping, 48)?;
            header_view.copy_into(&mut before)?;
            sensors::hwinfo_header(&before, MAX_SHARED_BYTES, now)?
        }
        other => other?,
    };
    drop(header_view);
    // The bounded complete view validates the declared extent against the actual object;
    // a publisher claiming more bytes than it allocated cannot pass this step.
    let view = View::map(&mapping, header.extent)?;
    let mut bytes = vec![0_u8; header.extent];
    view.copy_into(&mut bytes)?;
    let mut after = [0_u8; 48];
    view.copy_into(&mut after[..header.bytes])?;
    sensors::coherent_copy(&before, &bytes, &after, header.bytes)?;
    // Release the publisher mutex and OS handles before parsing names/units in the owned copy.
    drop(view);
    drop(_ownership);
    drop(mutex);
    drop(mapping);
    let readings = sensors::hwinfo_readings(&bytes, &header)?;
    Ok((header, readings))
}

fn project(
    out: &mut Inspection,
    parsed: Readings,
    freshness: Freshness,
    sample_at: Option<u64>,
    provider_category: &str,
    source: &str,
) -> String {
    let provider = out.entity("sensor-provider", None);
    out.value(
        &provider,
        "sensor_provider",
        provider_category,
        "category",
        source,
    );
    out.value(
        &provider,
        "sensor_sample_freshness",
        freshness.category(),
        "category",
        source,
    );
    out.value(
        &provider,
        "sensor_sample_time_known",
        sample_at.is_some(),
        "boolean",
        source,
    );
    if sample_at.is_none() {
        out.value(
            &provider,
            "sensor_sample_age_seconds",
            Value::Null,
            "seconds",
            source,
        );
    }
    out.value(
        &provider,
        "sensor_readings_truncated",
        parsed.truncated,
        "boolean",
        source,
    );
    out.value(
        &provider,
        "sensor_unsupported_reading_count",
        parsed.unsupported_readings as u64,
        "count",
        source,
    );
    out.value(
        &provider,
        "sensor_unknown_value_count",
        parsed.unknown_values as u64,
        "count",
        source,
    );
    let mut components = BTreeMap::new();
    for sample in parsed.samples {
        let component = components.entry(sample.component).or_insert_with(|| {
            let component = out.entity("sensor-component", None);
            out.value(
                &component,
                "sensor_component_category",
                sample.category.category(),
                "category",
                source,
            );
            out.value(
                &component,
                "sensor_provider_ref",
                provider.as_str(),
                "opaque_ref",
                source,
            );
            component
        });
        let sensor = out.entity("sensor", None);
        out.value(
            &sensor,
            "sensor_component_ref",
            component.as_str(),
            "opaque_ref",
            source,
        );
        out.value(
            &sensor,
            "sensor_role",
            sample.role.category(),
            "category",
            source,
        );
        let value = match (sample.kind, sample.value) {
            (Kind::ThermalThrottling, Some(value)) => Value::Bool(value == 1.0),
            (_, Some(value)) => Value::from(value),
            (_, None) => Value::Null,
        };
        out.value(
            &sensor,
            sample.kind.metric(),
            value,
            sample.kind.unit(),
            source,
        );
        let observation = out.observations.last_mut().expect("value was appended");
        if let Some(measured_at) = sample_at {
            // Current HWiNFO values retain their actual polling time, not the query/read time.
            observation.measured_at_unix_ms = measured_at;
        }
        if !observation.value.is_null() && freshness != Freshness::Fresh {
            observation.status = C::Partial;
        }
    }
    provider
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse::sensors::{Component, Role, Sample};

    #[test]
    fn synchronize_only_mutex_handle_releases_ownership_for_another_thread() {
        use windows_sys::Win32::System::Threading::CreateMutexW;

        // This fixture creates its own local mutex; it never opens the HWiNFO publisher mutex.
        let name = wide(&format!(
            r"Local\{}",
            pcagent_shared::new_id("pcagent-sensor-mutex-fixture")
        ));
        // SAFETY: null security attributes, no initial ownership and a terminated unique name.
        let created = unsafe { CreateMutexW(std::ptr::null(), 0, name.as_ptr()) };
        assert!(!created.is_null());
        let _created = Handle(created);
        // SAFETY: the fixture's live named mutex is opened with only the documented wait access.
        let opened = unsafe { OpenMutexW(SYNCHRONIZATION_SYNCHRONIZE, 0, name.as_ptr()) };
        assert!(!opened.is_null());
        let opened = Handle(opened);
        // SAFETY: owned mutex handle, bounded wait on an initially unowned fixture.
        assert_eq!(
            unsafe { WaitForSingleObject(opened.0, MUTEX_WAIT_MS) },
            WAIT_OBJECT_0
        );
        let ownership = MutexOwnership(&opened);
        drop(ownership);

        // A second thread must acquire it: waiting again on this thread would also succeed for
        // an incorrectly retained recursive ownership and would not test ReleaseMutex.
        std::thread::spawn(move || {
            // SAFETY: terminated fixture name; the original created handle keeps it alive.
            let handle = unsafe { OpenMutexW(SYNCHRONIZATION_SYNCHRONIZE, 0, name.as_ptr()) };
            assert!(!handle.is_null());
            let handle = Handle(handle);
            // SAFETY: owned handle and fixed short wait on the fixture, no supplier involved.
            assert_eq!(
                unsafe { WaitForSingleObject(handle.0, MUTEX_WAIT_MS) },
                WAIT_OBJECT_0
            );
            let _ownership = MutexOwnership(&handle);
        })
        .join()
        .expect("fixture thread completed");
    }

    #[test]
    fn projection_preserves_sample_time_unknowns_and_fixed_categories_without_names() {
        let mut out = Inspection::new();
        let readings = Readings {
            samples: vec![
                Sample {
                    component: 0,
                    category: Component::Cpu,
                    kind: Kind::Temperature,
                    role: Role::Package,
                    value: Some(65.5),
                },
                Sample {
                    component: 0,
                    category: Component::Cpu,
                    kind: Kind::Fan,
                    role: Role::Unspecified,
                    value: None,
                },
                Sample {
                    component: 1,
                    category: Component::Storage,
                    kind: Kind::Temperature,
                    role: Role::Unspecified,
                    value: Some(41.0),
                },
            ],
            unknown_values: 1,
            ..Readings::default()
        };
        project(
            &mut out,
            readings,
            Freshness::Fresh,
            Some(123_000),
            "hwinfo_shared_memory",
            HWINFO_SOURCE,
        );
        let temperatures: Vec<_> = out
            .observations
            .iter()
            .filter(|o| o.metric == "sensor_temperature_celsius")
            .collect();
        assert_eq!(temperatures.len(), 2);
        assert_eq!(temperatures[0].value, 65.5);
        assert!(temperatures
            .iter()
            .all(|o| o.measured_at_unix_ms == 123_000));
        assert!(out
            .observations
            .iter()
            .find(|o| o.metric == "sensor_fan_speed_rpm")
            .unwrap()
            .value
            .is_null());
        let categories: Vec<_> = out
            .observations
            .iter()
            .filter(|o| o.metric == "sensor_component_category")
            .map(|o| &o.value)
            .collect();
        assert_eq!(
            categories,
            vec![&Value::from("cpu"), &Value::from("storage")]
        );
        assert!(out.names.is_empty());
        let encoded = serde_json::to_value(out).unwrap().to_string();
        for forbidden in [
            "dwSensor",
            "dwReading",
            "szLabel",
            "PRIVATE",
            "Parent",
            "sensor_names",
        ] {
            assert!(!encoded.contains(forbidden));
        }
    }

    #[test]
    fn undated_wmi_values_are_partial_and_zero_is_not_reintroduced() {
        let mut out = Inspection::new();
        let readings = sensors::lhm_readings(
            &[
                vec![
                    Value::from("Temperature"),
                    Value::from("/intelcpu/PRIVATE_ID"),
                    Value::from(50.0),
                ],
                vec![
                    Value::from("Fan"),
                    Value::from("/intelcpu/PRIVATE_ID"),
                    Value::from(0),
                ],
            ],
            false,
        );
        project(
            &mut out,
            readings,
            Freshness::Unknown,
            None,
            "lhm_wmi",
            LHM_SOURCE,
        );
        let temperature = out
            .observations
            .iter()
            .find(|o| o.metric == "sensor_temperature_celsius")
            .unwrap();
        assert_eq!(temperature.status, C::Partial);
        assert_eq!(temperature.value, 50.0);
        let fan = out
            .observations
            .iter()
            .find(|o| o.metric == "sensor_fan_speed_rpm")
            .unwrap();
        assert!(fan.value.is_null());
        assert_eq!(fan.status, C::Unsupported);
        assert!(!serde_json::to_string(&out).unwrap().contains("PRIVATE_ID"));
    }
}
