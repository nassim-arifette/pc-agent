//! Portable conversion of NVIDIA Management Library (NVML) readings into evidence. The Windows
//! module supplies raw values and NVML return codes; everything here is deterministic and
//! fixture-tested. A missing value is reported with its reason, never as zero.
use pcagent_shared::{Evidence, Metric, Status, Window, NVML_SLOWDOWN_MASK};

/// `NVML_SUCCESS` (nvml.h).
pub const NVML_SUCCESS: i32 = 0;
const NVML_ERROR_NOT_SUPPORTED: i32 = 3;
const NVML_ERROR_NO_PERMISSION: i32 = 4;

/// One query: the raw value, or why it is missing (`None`: this NVML version does not export
/// the function; `Some(code)`: the NVML return code).
pub type Query<T> = Result<T, Option<i32>>;

/// Raw readings of one NVIDIA device, as NVML returned them.
#[derive(Debug, Clone)]
pub struct DeviceReading {
    pub index: u32,
    pub name: Option<String>,
    pub utilization_percent: Query<u32>,
    /// (total, used) bytes.
    pub memory: Query<(u64, u64)>,
    pub temperature_celsius: Query<i64>,
    pub temperature_source: &'static str,
    pub sm_clock_mhz: Query<u32>,
    pub power_milliwatts: Query<u32>,
    /// Raw clock-event reason bits; only the slowdown bits are kept as evidence.
    pub event_reasons: Query<u64>,
    pub reasons_source: &'static str,
}

/// Added once to a snapshot that holds NVML readings.
pub const LIMITATIONS: [&str; 3] = [
    "NVIDIA GPU values come from NVML and cover NVIDIA devices only; other GPUs appear only in the aggregated Windows GPU engine counters.",
    "NVML utilization covers the driver's last sample period (1/6 s to 1 s depending on the product); GPU memory used includes the driver's own reserve.",
    "Reading NVML can wake a sleeping discrete GPU on hybrid laptops; readings are taken only when a snapshot is requested.",
];

/// Generic, non-identifying entity: the NVML device index.
pub fn entity(index: u32) -> String {
    format!("gpu-nvidia-{index}")
}

pub fn describe(code: i32) -> String {
    let meaning = match code {
        1 => "NVML not initialized",
        2 => "invalid argument",
        3 => "not supported by this GPU or driver",
        4 => "permission denied",
        9 => "NVIDIA driver not loaded",
        12 => "NVML library not found",
        13 => "function not implemented by this NVML",
        15 => "the GPU fell off the bus or is inaccessible",
        999 => "unknown driver error",
        _ => "error",
    };
    format!("NVML returned {code} ({meaning})")
}

fn missing(code: Option<i32>) -> (Status, String) {
    let status = match code {
        None | Some(NVML_ERROR_NOT_SUPPORTED) => Status::Unsupported,
        Some(NVML_ERROR_NO_PERMISSION) => Status::PermissionDenied,
        Some(_) => Status::Error,
    };
    let reason = code.map_or_else(
        || "Not exported by this NVML version.".to_owned(),
        |c| format!("{}.", describe(c)),
    );
    (status, reason)
}
fn out_of_range(what: &str) -> (Status, String) {
    (
        Status::Error,
        format!("NVML returned an out-of-range {what}."),
    )
}

/// NUL-terminated device name from a fixed NVML buffer; `None` if unterminated or empty.
pub fn device_name(buffer: &[u8]) -> Option<String> {
    let end = buffer.iter().position(|&b| b == 0)?;
    let name = String::from_utf8_lossy(&buffer[..end]).trim().to_owned();
    (!name.is_empty()).then_some(name)
}

/// One evidence item per metric and device, measured or explicitly missing.
pub fn device_evidence(d: &DeviceReading, window: &Window) -> Vec<Evidence> {
    let utilization = match d.utilization_percent {
        Ok(percent) if percent <= 100 => Ok(u64::from(percent) * 100),
        Ok(_) => Err(out_of_range("utilization")),
        Err(code) => Err(missing(code)),
    };
    let (used, total) = match d.memory {
        Ok((total, used)) if total > 0 && used <= total => (Ok(used), Ok(total)),
        Ok(_) => (
            Err(out_of_range("memory size")),
            Err(out_of_range("memory size")),
        ),
        Err(code) => (Err(missing(code)), Err(missing(code))),
    };
    let temperature = match d.temperature_celsius {
        Ok(celsius) if (0..=150).contains(&celsius) => Ok(celsius.unsigned_abs()),
        Ok(_) => Err(out_of_range("temperature")),
        Err(code) => Err(missing(code)),
    };
    let entity = entity(d.index);
    [
        (
            Metric::GpuUtilizationBasisPoints,
            "NVML.nvmlDeviceGetUtilizationRates",
            utilization,
        ),
        (
            Metric::GpuMemoryUsedBytes,
            "NVML.nvmlDeviceGetMemoryInfo",
            used,
        ),
        (
            Metric::GpuMemoryTotalBytes,
            "NVML.nvmlDeviceGetMemoryInfo",
            total,
        ),
        (
            Metric::GpuTemperatureCelsius,
            d.temperature_source,
            temperature,
        ),
        (
            Metric::GpuSmClockMegahertz,
            "NVML.nvmlDeviceGetClockInfo(NVML_CLOCK_SM)",
            d.sm_clock_mhz.map(u64::from).map_err(missing),
        ),
        (
            Metric::GpuPowerMilliwatts,
            "NVML.nvmlDeviceGetPowerUsage",
            d.power_milliwatts.map(u64::from).map_err(missing),
        ),
        (
            Metric::GpuSlowdownReasons,
            d.reasons_source,
            d.event_reasons
                .map(|bits| bits & NVML_SLOWDOWN_MASK)
                .map_err(missing),
        ),
    ]
    .into_iter()
    .map(|(metric, source, value)| match value {
        Ok(v) => Evidence::measured(metric, &entity, v, source, window.clone()),
        Err((status, why)) => Evidence::missing(metric, &entity, source, status, &why),
    })
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use pcagent_shared::{NVML_HW_THERMAL_SLOWDOWN, NVML_SW_POWER_CAP};
    fn reading() -> DeviceReading {
        DeviceReading {
            index: 0,
            name: Some("NVIDIA GeForce MX350".into()),
            utilization_percent: Ok(42),
            memory: Ok((2 << 30, 512 << 20)),
            temperature_celsius: Ok(53),
            temperature_source: "NVML.nvmlDeviceGetTemperatureV",
            sm_clock_mhz: Ok(1_354),
            power_milliwatts: Err(Some(NVML_ERROR_NOT_SUPPORTED)),
            // GPU idle (0x1) is not a slowdown; the thermal and power-cap bits are.
            event_reasons: Ok(0x1 | NVML_SW_POWER_CAP | NVML_HW_THERMAL_SLOWDOWN),
            reasons_source: "NVML.nvmlDeviceGetCurrentClocksEventReasons",
        }
    }
    fn find(evidence: &[Evidence], metric: Metric) -> &Evidence {
        evidence.iter().find(|e| e.metric == metric).unwrap()
    }
    #[test]
    fn every_metric_is_reported_once_measured_or_missing() {
        let e = device_evidence(&reading(), &Window::instant());
        assert_eq!(e.len(), 7);
        assert!(e.iter().all(|e| e.entity_ref == "gpu-nvidia-0"));
        assert_eq!(
            find(&e, Metric::GpuUtilizationBasisPoints).value,
            Some(4_200)
        );
        assert_eq!(find(&e, Metric::GpuMemoryTotalBytes).value, Some(2 << 30));
        assert_eq!(find(&e, Metric::GpuMemoryUsedBytes).value, Some(512 << 20));
        assert_eq!(find(&e, Metric::GpuTemperatureCelsius).value, Some(53));
        assert_eq!(find(&e, Metric::GpuSmClockMegahertz).value, Some(1_354));
        assert_eq!(
            find(&e, Metric::GpuSlowdownReasons).value,
            Some(NVML_SW_POWER_CAP | NVML_HW_THERMAL_SLOWDOWN)
        );
        let power = find(&e, Metric::GpuPowerMilliwatts);
        assert_eq!((power.value, power.status), (None, Status::Unsupported));
        assert!(power.limitations[0].contains("not supported"));
        assert_eq!(
            find(&e, Metric::GpuTemperatureCelsius).source,
            "NVML.nvmlDeviceGetTemperatureV"
        );
    }
    #[test]
    fn missing_functions_permissions_and_impossible_values_are_never_zero() {
        let mut r = reading();
        r.utilization_percent = Ok(101);
        r.memory = Ok((1 << 30, 2 << 30));
        r.temperature_celsius = Ok(-5);
        r.sm_clock_mhz = Err(None);
        r.event_reasons = Err(Some(NVML_ERROR_NO_PERMISSION));
        let e = device_evidence(&r, &Window::instant());
        for metric in [
            Metric::GpuUtilizationBasisPoints,
            Metric::GpuMemoryUsedBytes,
            Metric::GpuMemoryTotalBytes,
            Metric::GpuTemperatureCelsius,
        ] {
            let item = find(&e, metric);
            assert_eq!(
                (item.value, item.status),
                (None, Status::Error),
                "{metric:?}"
            );
        }
        let clock = find(&e, Metric::GpuSmClockMegahertz);
        assert_eq!(clock.status, Status::Unsupported);
        assert!(clock.limitations[0].contains("Not exported"));
        assert_eq!(
            find(&e, Metric::GpuSlowdownReasons).status,
            Status::PermissionDenied
        );
        assert!(describe(15).contains("fell off the bus"));
        assert!(describe(42).ends_with("(error)"));
    }
    #[test]
    fn device_names_need_a_terminator_and_content() {
        let mut buffer = [0_u8; 96];
        buffer[..20].copy_from_slice(b"NVIDIA GeForce MX350");
        assert_eq!(
            device_name(&buffer).as_deref(),
            Some("NVIDIA GeForce MX350")
        );
        assert_eq!(device_name(&[b'x'; 96]), None);
        assert_eq!(device_name(&[0; 96]), None);
        assert_eq!(device_name(b"  \0"), None);
    }
}
