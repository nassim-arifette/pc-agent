//! Selected sensor fields from optional, already-running providers. Provider names, labels and
//! hardware identifiers are never retained in the parsed result, including Debug output.
use serde_json::Value;
use std::{collections::BTreeMap, ops::Range};

pub(crate) const MAX_SHARED_BYTES: usize = 4 * 1024 * 1024;
pub(crate) const MAX_COMPONENTS: usize = 256;
pub(crate) const MAX_READINGS: usize = 4096;
pub(crate) const MAX_SAMPLES: usize = 256;
pub(crate) const MAX_SAMPLE_AGE_MS: u64 = 15_000;
const BASE_HEADER_BYTES: usize = 44;
const EXTENDED_HEADER_BYTES: usize = 48;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Component {
    Cpu,
    Gpu,
    Motherboard,
    Memory,
    Storage,
    Battery,
    PowerSupply,
    System,
    Unknown,
}
impl Component {
    pub(crate) fn category(self) -> &'static str {
        match self {
            Self::Cpu => "cpu",
            Self::Gpu => "gpu",
            Self::Motherboard => "motherboard",
            Self::Memory => "memory",
            Self::Storage => "storage",
            Self::Battery => "battery",
            Self::PowerSupply => "power_supply",
            Self::System => "system",
            Self::Unknown => "unknown",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    Temperature,
    Voltage,
    Fan,
    Current,
    Power,
    Clock,
    Load,
    ThermalThrottling,
}
impl Kind {
    pub(crate) fn metric(self) -> &'static str {
        match self {
            Self::Temperature => "sensor_temperature_celsius",
            Self::Voltage => "sensor_voltage_volts",
            Self::Fan => "sensor_fan_speed_rpm",
            Self::Current => "sensor_current_amperes",
            Self::Power => "sensor_power_watts",
            Self::Clock => "sensor_clock_megahertz",
            Self::Load => "sensor_load_percent",
            Self::ThermalThrottling => "sensor_thermal_throttling",
        }
    }
    pub(crate) fn unit(self) -> &'static str {
        match self {
            Self::Temperature => "degrees_celsius",
            Self::Voltage => "volts",
            Self::Fan => "revolutions_per_minute",
            Self::Current => "amperes",
            Self::Power => "watts",
            Self::Clock => "megahertz",
            Self::Load => "percent",
            Self::ThermalThrottling => "boolean",
        }
    }
    fn valid(self, value: f64) -> Option<f64> {
        if !value.is_finite() {
            return None;
        }
        let range = match self {
            Self::Temperature => -273.15..=500.0,
            Self::Voltage => -1_000.0..=1_000.0,
            Self::Fan => 0.0..=200_000.0,
            Self::Current | Self::Power => -1_000_000.0..=1_000_000.0,
            Self::Clock => 0.0..=1_000_000.0,
            // CPU utility may exceed 100%; this is an encoding sanity bound, not a health limit.
            Self::Load => 0.0..=10_000.0,
            Self::ThermalThrottling => {
                return (value == 0.0 || value == 1.0).then_some(value);
            }
        };
        range.contains(&value).then_some(value)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Role {
    Unspecified,
    Package,
    Die,
    DieOrControl,
    Core,
    Hotspot,
    Memory,
}
impl Role {
    pub(crate) fn category(self) -> &'static str {
        match self {
            Self::Unspecified => "unspecified",
            Self::Package => "package",
            Self::Die => "die",
            Self::DieOrControl => "die_or_control",
            Self::Core => "core",
            Self::Hotspot => "hotspot",
            Self::Memory => "memory",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Sample {
    /// A table index used only locally to generate an opaque component reference.
    pub(crate) component: usize,
    pub(crate) category: Component,
    pub(crate) kind: Kind,
    pub(crate) role: Role,
    pub(crate) value: Option<f64>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Readings {
    pub(crate) samples: Vec<Sample>,
    pub(crate) truncated: bool,
    pub(crate) unsupported_readings: usize,
    pub(crate) unknown_values: usize,
}

/// LHM v0.9.3 WMI exposes no polling timestamp and projects a missing float as zero. A zero
/// therefore cannot distinguish a real stopped fan/idle reading from an absent measurement.
/// The only queried properties are SensorType, Parent and Value, in that order.
pub(crate) fn lhm_readings(rows: &[Vec<Value>], partial: bool) -> Readings {
    let mut parsed = Readings {
        truncated: partial || rows.len() > MAX_SAMPLES,
        ..Readings::default()
    };
    let mut parents = BTreeMap::new();
    let mut next_component = 0;
    for row in rows.iter().take(MAX_SAMPLES) {
        let kind = match row.first().and_then(Value::as_str) {
            Some("Temperature") => Kind::Temperature,
            Some("Voltage") => Kind::Voltage,
            Some("Fan") => Kind::Fan,
            Some("Current") => Kind::Current,
            Some("Power") => Kind::Power,
            Some("Clock") => Kind::Clock,
            Some("Load") => Kind::Load,
            _ => {
                parsed.unsupported_readings += 1;
                continue;
            }
        };
        let parent = row
            .get(1)
            .and_then(Value::as_str)
            .filter(|p| p.len() <= 256);
        let category = parent.map(lhm_component).unwrap_or(Component::Unknown);
        let component = if let Some(parent) = parent {
            *parents.entry(parent).or_insert_with(|| {
                let index = next_component;
                next_component += 1;
                index
            })
        } else {
            let index = next_component;
            next_component += 1;
            index
        };
        let value = row
            .get(2)
            .and_then(Value::as_f64)
            .filter(|v| *v != 0.0)
            .and_then(|v| kind.valid(v));
        parsed.unknown_values += usize::from(value.is_none());
        parsed.samples.push(Sample {
            component,
            category,
            kind,
            role: Role::Unspecified,
            value,
        });
    }
    parsed
}

fn lhm_component(parent: &str) -> Component {
    match parent.strip_prefix('/').and_then(|p| p.split('/').next()) {
        Some("intelcpu" | "amdcpu") => Component::Cpu,
        Some("nvidiagpu" | "amdgpu" | "intelgpu" | "gpu-nvidia" | "gpu-amd" | "gpu-intel") => {
            Component::Gpu
        }
        Some("lpc" | "motherboard" | "superio") => Component::Motherboard,
        Some("ram" | "memory") => Component::Memory,
        Some("hdd" | "ssd" | "nvme" | "storage") => Component::Storage,
        Some("battery") => Component::Battery,
        Some("psu" | "corsairpsu") => Component::PowerSupply,
        _ => Component::Unknown,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Freshness {
    Fresh,
    Stale,
    Unknown,
    Future,
}
impl Freshness {
    pub(crate) fn category(self) -> &'static str {
        match self {
            Self::Fresh => "fresh",
            Self::Stale => "stale",
            Self::Unknown => "unknown",
            Self::Future => "future",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ParseError {
    Truncated,
    Inactive,
    Signature,
    Version,
    Limit,
    Layout,
    ConcurrentChange,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Section {
    range: Range<usize>,
    stride: usize,
    count: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Header {
    pub(crate) bytes: usize,
    pub(crate) extent: usize,
    pub(crate) version: u32,
    pub(crate) revision: u32,
    pub(crate) polled_at_unix_ms: Option<u64>,
    pub(crate) age_ms: Option<u64>,
    pub(crate) freshness: Freshness,
    sensors: Section,
    readings: Section,
}

fn u32_at(bytes: &[u8], at: usize) -> Result<u32, ParseError> {
    let data = bytes.get(at..at + 4).ok_or(ParseError::Truncated)?;
    Ok(u32::from_le_bytes(data.try_into().expect("four bytes")))
}
fn i64_at(bytes: &[u8], at: usize) -> Result<i64, ParseError> {
    let data = bytes.get(at..at + 8).ok_or(ParseError::Truncated)?;
    Ok(i64::from_le_bytes(data.try_into().expect("eight bytes")))
}
fn f64_at(bytes: &[u8], at: usize) -> Result<f64, ParseError> {
    let data = bytes.get(at..at + 8).ok_or(ParseError::Truncated)?;
    Ok(f64::from_le_bytes(data.try_into().expect("eight bytes")))
}

fn section(
    bytes: &[u8],
    descriptor: usize,
    stride: usize,
    limit: usize,
    header_bytes: usize,
    available: usize,
) -> Result<Section, ParseError> {
    let start = u32_at(bytes, descriptor)? as usize;
    let declared_stride = u32_at(bytes, descriptor + 4)? as usize;
    let count = u32_at(bytes, descriptor + 8)? as usize;
    if count > limit {
        return Err(ParseError::Limit);
    }
    if count == 0 {
        // Empty sections need not allocate a table, but cannot describe bytes beyond the view.
        if start > available || (declared_stride != 0 && declared_stride != stride) {
            return Err(ParseError::Layout);
        }
        return Ok(Section {
            range: start..start,
            stride,
            count,
        });
    }
    if start < header_bytes || declared_stride != stride {
        return Err(ParseError::Layout);
    }
    let end = start
        .checked_add(stride.checked_mul(count).ok_or(ParseError::Limit)?)
        .ok_or(ParseError::Limit)?;
    if end > MAX_SHARED_BYTES {
        return Err(ParseError::Limit);
    }
    if end > available {
        return Err(ParseError::Truncated);
    }
    Ok(Section {
        range: start..end,
        stride,
        count,
    })
}

/// Layout from the author's public hwisenssm2.h: packed v1/v2, revisions 0/1 only. Read the
/// header before allocating or copying the declared extent. `available` comes from VirtualQuery.
pub(crate) fn hwinfo_header(
    bytes: &[u8],
    available: usize,
    now_unix_ms: u64,
) -> Result<Header, ParseError> {
    match bytes.get(..4) {
        Some(b"HWiS") => {}
        Some(b"DEAD") => return Err(ParseError::Inactive),
        Some(_) => return Err(ParseError::Signature),
        None => return Err(ParseError::Truncated),
    }
    let version = u32_at(bytes, 4)?;
    let revision = u32_at(bytes, 8)?;
    if !(1..=2).contains(&version) || revision > 1 {
        return Err(ParseError::Version);
    }
    let header_bytes = if revision == 0 {
        BASE_HEADER_BYTES
    } else {
        EXTENDED_HEADER_BYTES
    };
    if bytes.len() < header_bytes || available < header_bytes {
        return Err(ParseError::Truncated);
    }
    let (sensor_stride, reading_stride) = if version == 1 { (264, 316) } else { (392, 460) };
    let sensors = section(
        bytes,
        20,
        sensor_stride,
        MAX_COMPONENTS,
        header_bytes,
        available,
    )?;
    let readings = section(
        bytes,
        32,
        reading_stride,
        MAX_READINGS,
        header_bytes,
        available,
    )?;
    if sensors.count != 0
        && readings.count != 0
        && sensors.range.start < readings.range.end
        && readings.range.start < sensors.range.end
    {
        return Err(ParseError::Layout);
    }
    let extent = header_bytes.max(sensors.range.end).max(readings.range.end);
    if extent > MAX_SHARED_BYTES {
        return Err(ParseError::Limit);
    }
    let poll_seconds = i64_at(bytes, 12)?;
    let polled_at_unix_ms = u64::try_from(poll_seconds)
        .ok()
        .filter(|seconds| *seconds != 0)
        .and_then(|seconds| seconds.checked_mul(1000));
    let age_ms = polled_at_unix_ms.and_then(|at| now_unix_ms.checked_sub(at));
    let freshness = match (polled_at_unix_ms, age_ms) {
        (None, _) => Freshness::Unknown,
        (Some(_), None) => Freshness::Future,
        (_, Some(age)) if age > MAX_SAMPLE_AGE_MS => Freshness::Stale,
        _ => Freshness::Fresh,
    };
    Ok(Header {
        bytes: header_bytes,
        extent,
        version,
        revision,
        polled_at_unix_ms,
        age_ms,
        freshness,
        sensors,
        readings,
    })
}

/// A cooperating publisher uses the documented mutex. This second check also rejects a header
/// changed between the preliminary read, bounded copy, and final read while holding that mutex.
pub(crate) fn coherent_copy(
    before: &[u8],
    snapshot: &[u8],
    after: &[u8],
    header_bytes: usize,
) -> Result<(), ParseError> {
    if !matches!(header_bytes, BASE_HEADER_BYTES | EXTENDED_HEADER_BYTES) {
        return Err(ParseError::Layout);
    }
    let first = before.get(..header_bytes).ok_or(ParseError::Truncated)?;
    let copied = snapshot.get(..header_bytes).ok_or(ParseError::Truncated)?;
    let last = after.get(..header_bytes).ok_or(ParseError::Truncated)?;
    if first == copied && copied == last {
        Ok(())
    } else {
        Err(ParseError::ConcurrentChange)
    }
}

fn c_string(bytes: &[u8]) -> Option<&[u8]> {
    let end = bytes.iter().position(|b| *b == 0)?;
    Some(&bytes[..end])
}

fn indexed_component(name: &[u8], prefix: &[u8]) -> bool {
    let Some(tail) = name.strip_prefix(prefix) else {
        return false;
    };
    let Some(end) = tail.iter().position(|b| *b == b']') else {
        return false;
    };
    (1..=3).contains(&end)
        && tail[..end].iter().all(u8::is_ascii_digit)
        && tail.get(end + 1).is_some_and(|b| *b == b':')
}
fn hwinfo_component(name: &[u8]) -> Component {
    if name == b"CPU" || indexed_component(name, b"CPU [#") {
        Component::Cpu
    } else if name == b"GPU" || indexed_component(name, b"GPU [#") {
        Component::Gpu
    } else if name.starts_with(b"S.M.A.R.T.:") || name.starts_with(b"Drive:") {
        Component::Storage
    } else if name == b"Memory Timings" || name.starts_with(b"Memory:") {
        Component::Memory
    } else if name.starts_with(b"Battery:") {
        Component::Battery
    } else if name.starts_with(b"Motherboard:") {
        Component::Motherboard
    } else if name.starts_with(b"System:") {
        Component::System
    } else {
        Component::Unknown
    }
}

fn hwinfo_kind(reading_type: u32, label: Option<&[u8]>) -> Option<Kind> {
    Some(match reading_type {
        1 => Kind::Temperature,
        2 => Kind::Voltage,
        3 => Kind::Fan,
        4 => Kind::Current,
        5 => Kind::Power,
        6 => Kind::Clock,
        7 => Kind::Load,
        8 if matches!(
            label,
            Some(
                b"Core Thermal Throttling"
                    | b"Package/Ring Thermal Throttling"
                    | b"Thermal Throttling (HTC)"
                    | b"Thermal Throttling (PROCHOT CPU)"
                    | b"Thermal Throttling (PROCHOT EXT)"
            )
        ) =>
        {
            Kind::ThermalThrottling
        }
        _ => return None,
    })
}
fn hwinfo_role(label: Option<&[u8]>) -> Role {
    match label {
        Some(b"CPU Package" | b"CPU Package Power") => Role::Package,
        Some(b"CPU Die (average)") => Role::Die,
        Some(b"CPU (Tctl/Tdie)") => Role::DieOrControl,
        Some(b"Core Temperatures" | b"Core Max" | b"GPU Temperature") => Role::Core,
        Some(b"GPU Hot Spot Temperature" | b"GPU Hotspot Temperature") => Role::Hotspot,
        Some(b"GPU Memory Junction Temperature" | b"GPU Memory Temperature") => Role::Memory,
        _ => Role::Unspecified,
    }
}

fn hwinfo_value(kind: Kind, unit: Option<&[u8]>, raw: f64) -> Option<f64> {
    let unit = unit?;
    let value = match (kind, unit) {
        (Kind::Temperature, b"C" | b"\xb0C" | b"\xc2\xb0C") => raw,
        (Kind::Temperature, b"F" | b"\xb0F" | b"\xc2\xb0F") => (raw - 32.0) * 5.0 / 9.0,
        (Kind::Voltage, b"V")
        | (Kind::Current, b"A")
        | (Kind::Power, b"W")
        | (Kind::Clock, b"MHz")
        | (Kind::Load, b"%")
        | (Kind::Fan, b"RPM") => raw,
        (Kind::Voltage, b"mV") | (Kind::Current, b"mA") | (Kind::Power, b"mW") => raw / 1000.0,
        (Kind::Power, b"kW") | (Kind::Clock, b"GHz") => raw * 1000.0,
        (Kind::Clock, b"kHz") => raw / 1000.0,
        (Kind::ThermalThrottling, b"Yes/No") => raw,
        _ => return None,
    };
    kind.valid(value)
}

pub(crate) fn hwinfo_readings(bytes: &[u8], header: &Header) -> Result<Readings, ParseError> {
    if bytes.len() < header.extent {
        return Err(ParseError::Truncated);
    }
    let mut parsed = Readings::default();
    for index in 0..header.readings.count {
        if parsed.samples.len() == MAX_SAMPLES {
            parsed.truncated = true;
            break;
        }
        let start = header.readings.range.start + index * header.readings.stride;
        let reading = &bytes[start..start + header.readings.stride];
        let label = c_string(&reading[12..140]);
        let Some(kind) = hwinfo_kind(u32_at(reading, 0)?, label) else {
            parsed.unsupported_readings += 1;
            continue;
        };
        let component = u32_at(reading, 4)? as usize;
        if component >= header.sensors.count {
            return Err(ParseError::Layout);
        }
        let sensor_start = header.sensors.range.start + component * header.sensors.stride;
        let category = c_string(&bytes[sensor_start + 8..sensor_start + 136])
            .map(hwinfo_component)
            .unwrap_or(Component::Unknown);
        let unit = if header.version == 2 {
            // UTF-8 unit from v2, followed by the original ANSI field if the new one is empty.
            c_string(&reading[444..460])
                .filter(|unit| !unit.is_empty())
                .or_else(|| c_string(&reading[268..284]))
        } else {
            c_string(&reading[268..284])
        };
        let value = if header.freshness == Freshness::Fresh {
            hwinfo_value(kind, unit, f64_at(reading, 284)?)
        } else {
            None
        };
        parsed.unknown_values += usize::from(value.is_none());
        parsed.samples.push(Sample {
            component,
            category,
            kind,
            role: hwinfo_role(label),
            value,
        });
    }
    Ok(parsed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const NOW: u64 = 1_800_000_000_000;

    fn put_u32(bytes: &mut [u8], at: usize, value: u32) {
        bytes[at..at + 4].copy_from_slice(&value.to_le_bytes());
    }
    fn put_string(bytes: &mut [u8], at: usize, text: &[u8]) {
        bytes[at..at + text.len()].copy_from_slice(text);
    }
    fn fixture(version: u32, revision: u32, count: usize) -> Vec<u8> {
        let header = if revision == 0 { 44 } else { 48 };
        let sensor = if version == 1 { 264 } else { 392 };
        let reading = if version == 1 { 316 } else { 460 };
        let readings_at = header + sensor;
        let mut bytes = vec![0; readings_at + reading * count];
        bytes[..4].copy_from_slice(b"HWiS");
        put_u32(&mut bytes, 4, version);
        put_u32(&mut bytes, 8, revision);
        bytes[12..20].copy_from_slice(&((NOW / 1000 - 1) as i64).to_le_bytes());
        put_u32(&mut bytes, 20, header as u32);
        put_u32(&mut bytes, 24, sensor as u32);
        put_u32(&mut bytes, 28, 1);
        put_u32(&mut bytes, 32, readings_at as u32);
        put_u32(&mut bytes, 36, reading as u32);
        put_u32(&mut bytes, 40, count as u32);
        if revision == 1 {
            put_u32(&mut bytes, 44, 2000);
        }
        put_string(&mut bytes, header + 8, b"CPU [#0]: PRIVATE_CPU_SERIAL");
        put_string(&mut bytes, header + 136, b"PRIVATE_USER_SENSOR_NAME");
        if version == 2 {
            put_string(&mut bytes, header + 264, b"PRIVATE_UTF8_SENSOR_NAME");
        }
        for i in 0..count {
            let at = readings_at + reading * i;
            put_u32(&mut bytes, at, 1);
            put_u32(&mut bytes, at + 4, 0);
            put_u32(&mut bytes, at + 8, 0xdead_beef);
            put_string(&mut bytes, at + 12, b"CPU Package");
            put_string(&mut bytes, at + 140, b"PRIVATE_USER_READING_NAME");
            put_string(&mut bytes, at + 268, b"\xb0C");
            bytes[at + 284..at + 292].copy_from_slice(&65.5_f64.to_le_bytes());
            // Min, max and average have no known accumulation window and are never exported.
            bytes[at + 292..at + 300].copy_from_slice(&1.0_f64.to_le_bytes());
            bytes[at + 300..at + 308].copy_from_slice(&100.0_f64.to_le_bytes());
            bytes[at + 308..at + 316].copy_from_slice(&70.0_f64.to_le_bytes());
            if version == 2 {
                put_string(&mut bytes, at + 316, b"PRIVATE_UTF8_READING_NAME");
                put_string(&mut bytes, at + 444, b"\xc2\xb0C");
            }
        }
        bytes
    }

    #[test]
    fn lhm_zero_missing_and_unrecognised_values_stay_unknown() {
        let rows = vec![
            vec![json!("Fan"), json!("/intelcpu/0"), json!(0)],
            vec![json!("Temperature"), json!("/intelcpu/0"), json!(65.25)],
            vec![json!("Power"), json!("/gpu-nvidia/PRIVATE_ID"), Value::Null],
            vec![json!("Voltage"), json!("/lpc/PRIVATE_ID"), json!(-12)],
            vec![
                json!("Temperature"),
                json!("/ssd/PRIVATE_SERIAL"),
                json!(9999),
            ],
            vec![json!("SecretType"), json!("/PRIVATE_PROFILE"), json!(123)],
            vec![json!("Temperature")],
        ];
        let parsed = lhm_readings(&rows, false);
        assert_eq!(parsed.samples.len(), 6);
        assert_eq!(parsed.unknown_values, 4);
        assert_eq!(parsed.unsupported_readings, 1);
        assert_eq!(parsed.samples[0].value, None);
        assert_eq!(parsed.samples[1].value, Some(65.25));
        assert_eq!(parsed.samples[0].component, parsed.samples[1].component);
        assert_eq!(parsed.samples[2].category, Component::Gpu);
        assert_eq!(parsed.samples[3].value, Some(-12.0));
        let debug = format!("{parsed:?}");
        assert!(!debug.contains("PRIVATE") && !debug.contains("SecretType"));
    }

    #[test]
    fn lhm_does_not_parse_strings_as_measurements_and_caps_rows() {
        let rows = vec![vec![json!("Load"), Value::Null, json!("42")]; MAX_SAMPLES + 1];
        let parsed = lhm_readings(&rows, false);
        assert_eq!(parsed.samples.len(), MAX_SAMPLES);
        assert_eq!(parsed.unknown_values, MAX_SAMPLES);
        assert!(parsed.truncated);
        assert!(lhm_readings(&[], true).truncated);
    }

    #[test]
    fn both_known_layouts_keep_only_selected_values_and_categories() {
        for version in 1..=2 {
            for revision in 0..=1 {
                let bytes = fixture(version, revision, 1);
                let header = hwinfo_header(&bytes, bytes.len(), NOW).unwrap();
                assert_eq!(header.freshness, Freshness::Fresh);
                assert_eq!(header.age_ms, Some(1000));
                assert_eq!(header.polled_at_unix_ms, Some(NOW - 1000));
                assert_eq!(header.extent, bytes.len());
                coherent_copy(&bytes, &bytes, &bytes, header.bytes).unwrap();
                let parsed = hwinfo_readings(&bytes, &header).unwrap();
                assert_eq!(parsed.samples[0].value, Some(65.5));
                assert_eq!(parsed.samples[0].category, Component::Cpu);
                assert_eq!(parsed.samples[0].role, Role::Package);
                assert!(!format!("{parsed:?}").contains("PRIVATE"));
                assert_eq!(parsed.samples.len(), 1);
            }
        }
    }

    #[test]
    fn old_future_unknown_and_overflowing_timestamps_cannot_yield_current_values() {
        for (seconds, freshness) in [
            ((NOW / 1000 - 16) as i64, Freshness::Stale),
            ((NOW / 1000 + 1) as i64, Freshness::Future),
            (0, Freshness::Unknown),
            (-1, Freshness::Unknown),
            (i64::MAX, Freshness::Unknown),
        ] {
            let mut bytes = fixture(2, 1, 1);
            bytes[12..20].copy_from_slice(&seconds.to_le_bytes());
            let header = hwinfo_header(&bytes, bytes.len(), NOW).unwrap();
            assert_eq!(header.freshness, freshness);
            assert_eq!(
                hwinfo_readings(&bytes, &header).unwrap().samples[0].value,
                None
            );
        }
        let mut bytes = fixture(1, 0, 1);
        bytes[12..20].copy_from_slice(&((NOW / 1000 - 15) as i64).to_le_bytes());
        assert_eq!(
            hwinfo_header(&bytes, bytes.len(), NOW).unwrap().freshness,
            Freshness::Fresh
        );
    }

    #[test]
    fn inactive_unknown_versions_malformed_sections_and_large_counts_are_rejected() {
        let original = fixture(1, 1, 1);
        for (at, value, expected) in [
            (0, u32::from_le_bytes(*b"DEAD"), ParseError::Inactive),
            (0, 0, ParseError::Signature),
            (4, 3, ParseError::Version),
            (8, 2, ParseError::Version),
            (20, 0, ParseError::Layout),
            (24, 265, ParseError::Layout),
            (28, (MAX_COMPONENTS + 1) as u32, ParseError::Limit),
            (32, u32::MAX, ParseError::Limit),
            (32, 48, ParseError::Layout),
            (36, 315, ParseError::Layout),
            (40, (MAX_READINGS + 1) as u32, ParseError::Limit),
        ] {
            let mut bytes = original.clone();
            put_u32(&mut bytes, at, value);
            assert_eq!(
                hwinfo_header(&bytes, bytes.len(), NOW),
                Err(expected),
                "field {at}"
            );
        }
        for length in 0..48 {
            assert!(hwinfo_header(&original[..length], original.len(), NOW).is_err());
        }
        assert_eq!(
            hwinfo_header(&original, original.len() - 1, NOW),
            Err(ParseError::Truncated)
        );
        let header = hwinfo_header(&original, original.len(), NOW).unwrap();
        assert_eq!(
            hwinfo_readings(&original[..original.len() - 1], &header),
            Err(ParseError::Truncated)
        );
    }

    #[test]
    fn concurrent_header_changes_and_invalid_component_links_are_rejected() {
        let bytes = fixture(2, 1, 1);
        let header = hwinfo_header(&bytes, bytes.len(), NOW).unwrap();
        let mut changed = bytes.clone();
        changed[12] ^= 1;
        assert_eq!(
            coherent_copy(&bytes, &bytes, &changed, header.bytes),
            Err(ParseError::ConcurrentChange)
        );
        assert_eq!(
            coherent_copy(&bytes, &changed, &bytes, header.bytes),
            Err(ParseError::ConcurrentChange)
        );
        assert_eq!(
            coherent_copy(&bytes[..20], &bytes, &bytes, header.bytes),
            Err(ParseError::Truncated)
        );
        let mut invalid = bytes.clone();
        put_u32(&mut invalid, header.readings.range.start + 4, 1);
        assert_eq!(hwinfo_readings(&invalid, &header), Err(ParseError::Layout));
    }

    #[test]
    fn units_are_exact_finite_and_converted_without_using_display_names() {
        assert_eq!(
            hwinfo_value(Kind::Temperature, Some(b"\xc2\xb0F"), 212.0),
            Some(100.0)
        );
        assert_eq!(hwinfo_value(Kind::Fan, Some(b"RPM"), 0.0), Some(0.0));
        assert_eq!(hwinfo_value(Kind::Power, Some(b"mW"), 65000.0), Some(65.0));
        assert_eq!(hwinfo_value(Kind::Clock, Some(b"GHz"), 3.2), Some(3200.0));
        assert_eq!(hwinfo_value(Kind::Load, Some(b"%"), f64::NAN), None);
        assert_eq!(hwinfo_value(Kind::Fan, Some(b"RPM"), -1.0), None);
        assert_eq!(
            hwinfo_value(Kind::Temperature, Some(b"PRIVATE_UNIT"), 65.0),
            None
        );
        assert_eq!(hwinfo_value(Kind::Temperature, None, 65.0), None);
        assert_eq!(
            hwinfo_value(Kind::ThermalThrottling, Some(b"Yes/No"), 1.0),
            Some(1.0)
        );
        assert_eq!(
            hwinfo_value(Kind::ThermalThrottling, Some(b"Yes/No"), 2.0),
            None
        );
    }

    #[test]
    fn exact_count_and_extent_limits_are_accepted_without_wrapping_or_unbounded_output() {
        let bytes = fixture(2, 1, MAX_READINGS);
        let header = hwinfo_header(&bytes, bytes.len(), NOW).unwrap();
        let readings = hwinfo_readings(&bytes, &header).unwrap();
        assert_eq!(readings.samples.len(), MAX_SAMPLES);
        assert!(readings.truncated);

        let mut components = fixture(2, 1, 0);
        let end = 48 + 392 * MAX_COMPONENTS;
        components.resize(end, 0);
        put_u32(&mut components, 28, MAX_COMPONENTS as u32);
        put_u32(&mut components, 32, end as u32);
        let header = hwinfo_header(&components, components.len(), NOW).unwrap();
        assert_eq!(header.extent, end);
        assert!(hwinfo_readings(&components, &header)
            .unwrap()
            .samples
            .is_empty());

        let original = fixture(2, 1, 1);
        let reading = original[440..].to_vec();
        let mut maximal = original;
        maximal.resize(MAX_SHARED_BYTES, 0);
        let at = MAX_SHARED_BYTES - 460;
        maximal[at..].copy_from_slice(&reading);
        put_u32(&mut maximal, 32, at as u32);
        let header = hwinfo_header(&maximal, MAX_SHARED_BYTES, NOW).unwrap();
        assert_eq!(header.extent, MAX_SHARED_BYTES);
        assert_eq!(
            hwinfo_readings(&maximal, &header).unwrap().samples[0].value,
            Some(65.5)
        );
        put_u32(&mut maximal, 32, at as u32 + 1);
        assert_eq!(
            hwinfo_header(&maximal, MAX_SHARED_BYTES, NOW),
            Err(ParseError::Limit)
        );
    }

    #[test]
    fn nonfinite_and_nonterminated_fields_remain_unknown_and_readings_are_capped() {
        let mut bytes = fixture(2, 1, MAX_SAMPLES + 1);
        let header = hwinfo_header(&bytes, bytes.len(), NOW).unwrap();
        let at = header.readings.range.start;
        bytes[at + 284..at + 292].copy_from_slice(&f64::INFINITY.to_le_bytes());
        bytes[at + 444..at + 460].fill(b'X');
        bytes[at + 268..at + 284].fill(b'Y');
        let parsed = hwinfo_readings(&bytes, &header).unwrap();
        assert_eq!(parsed.samples.len(), MAX_SAMPLES);
        assert!(parsed.truncated);
        assert_eq!(parsed.samples[0].value, None);
        assert_eq!(parsed.unknown_values, 1);
        let mut zero = fixture(1, 0, 0);
        put_u32(&mut zero, 28, 0);
        let header = hwinfo_header(&zero, zero.len(), NOW).unwrap();
        assert!(hwinfo_readings(&zero, &header).unwrap().samples.is_empty());
    }
}
