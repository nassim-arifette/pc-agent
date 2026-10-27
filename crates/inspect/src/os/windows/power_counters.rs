use super::wide;
use crate::{CoverageStatus as C, Inspection};
use pcagent_shared::{unix_ms, Window};
use serde_json::Value;
use std::{
    ptr::{null, null_mut},
    time::{Duration, Instant},
};
use windows_sys::Win32::System::Performance::*;
const FORMAT: u32 = PDH_FMT_DOUBLE | 0x0000_8000; // PDH_FMT_NOCAP100, from pdh.h.
const PATHS: [&str; 4] = [
    r"\Processor Information(_Total)\% Performance Limit",
    r"\Processor Information(_Total)\% Processor Performance",
    r"\Processor Information(_Total)\Processor Frequency",
    r"\Thermal Zone Information(*)\Temperature",
];
const METRICS: [&str; 3] = [
    "cpu_performance_limit_basis_points",
    "cpu_processor_performance_basis_points",
    "cpu_frequency_megahertz",
];
struct Query(PDH_HQUERY);
impl Drop for Query {
    fn drop(&mut self) {
        // SAFETY: uniquely owned query closes its registered counters.
        unsafe {
            PdhCloseQuery(self.0);
        }
    }
}
fn scalar(value: &PDH_FMT_COUNTERVALUE) -> Option<f64> {
    if !matches!(value.CStatus, PDH_CSTATUS_VALID_DATA | PDH_CSTATUS_NEW_DATA) {
        return None;
    }
    // SAFETY: callers requested PDH_FMT_DOUBLE and checked the native counter status.
    let number = unsafe { value.Anonymous.doubleValue };
    number.is_finite().then_some(number)
}
fn celsius(kelvin: f64) -> Option<f64> {
    (200.0..=450.0).contains(&kelvin).then_some(kelvin - 273.15)
}
pub(super) fn collect(out: &mut Inspection) {
    const SOURCE: &str = "PDH.Processor Information/Thermal Zone Information";
    let mut raw = null_mut();
    // SAFETY: local live datasource, valid query output, no log or remote server.
    if unsafe { PdhOpenQueryW(null(), 0, &mut raw) } != 0 {
        out.coverage(
            "power_counters",
            C::Unsupported,
            SOURCE,
            "Performance query unavailable.",
        );
        return;
    }
    let query = Query(raw);
    let mut counters = Vec::new();
    for path in PATHS {
        let mut handle = null_mut();
        // SAFETY: fixed English counter name, owned query, valid counter output.
        let code = unsafe { PdhAddEnglishCounterW(query.0, wide(path).as_ptr(), 0, &mut handle) };
        counters.push((code == 0).then_some(handle));
    }
    let started = unix_ms();
    let timer = Instant::now();
    // SAFETY: read-only collection on the live owned query.
    let first = unsafe { PdhCollectQueryData(query.0) };
    std::thread::sleep(Duration::from_secs(1));
    // SAFETY: a second collection provides the required rate baseline.
    let second = unsafe { PdhCollectQueryData(query.0) };
    let previous = out.window.clone();
    out.window = Window::finish(started, timer);
    for (index, metric) in METRICS.iter().enumerate() {
        let number = counters[index]
            .and_then(|counter| {
                let mut value = PDH_FMT_COUNTERVALUE::default();
                // SAFETY: double format selected; initialized fixed-size output; owned live counter.
                let code =
                    unsafe { PdhGetFormattedCounterValue(counter, FORMAT, null_mut(), &mut value) };
                (code == 0 && first == 0 && second == 0)
                    .then(|| scalar(&value))
                    .flatten()
            })
            .filter(|n| *n >= 0.0)
            .map(|n| if index < 2 { n * 100.0 } else { n });
        let source = format!("PDH.{}", PATHS[index]);
        out.value(
            "system",
            metric,
            number.map(Value::from).unwrap_or(Value::Null),
            if index < 2 {
                "basis_points"
            } else {
                "megahertz"
            },
            &source,
        );
        out.coverage(metric,if number.is_some(){C::Ok}else{C::Unsupported},&source,"Two collections one second apart. Frequency/performance limits are indicators; they do not alone establish thermal throttling.");
    }
    let mut temperatures = Vec::new();
    let mut complete = false;
    if let Some(counter) = counters[3] {
        let mut bytes = 0;
        let mut count = 0;
        // SAFETY: size-only query; no output buffer is passed.
        let code = unsafe {
            PdhGetFormattedCounterArrayW(counter, FORMAT, &mut bytes, &mut count, null_mut())
        };
        if code == PDH_MORE_DATA && bytes > 0 && bytes <= 1024 * 1024 {
            let mut buffer = vec![0_u64; (bytes as usize).div_ceil(8)];
            let capacity = std::mem::size_of_val(buffer.as_slice());
            // SAFETY: u64 storage has native item alignment, and capacity covers the requested bytes.
            let code = unsafe {
                PdhGetFormattedCounterArrayW(
                    counter,
                    FORMAT,
                    &mut bytes,
                    &mut count,
                    buffer.as_mut_ptr().cast(),
                )
            };
            if code == 0
                && bytes as usize <= capacity
                && (count as usize) <= capacity / std::mem::size_of::<PDH_FMT_COUNTERVALUE_ITEM_W>()
            {
                // SAFETY: successful call and count verified against the initialized output storage.
                let items = unsafe {
                    std::slice::from_raw_parts(
                        buffer.as_ptr().cast::<PDH_FMT_COUNTERVALUE_ITEM_W>(),
                        count as usize,
                    )
                };
                complete = count <= 64;
                for item in items.iter().take(64) {
                    if let Some(temperature) = scalar(&item.FmtValue).and_then(celsius) {
                        temperatures.push(temperature);
                    } else {
                        complete = false;
                    }
                }
            }
        }
    }
    for temperature in &temperatures {
        let entity = out.entity("thermal-zone", None);
        out.value(
            &entity,
            "thermal_zone_temperature_celsius",
            *temperature,
            "degrees_celsius",
            "PDH.Thermal Zone Information.Temperature (Kelvin converted locally)",
        );
    }
    out.coverage("thermal_zone_counters",if complete&&!temperatures.is_empty(){C::Ok}else if temperatures.is_empty(){C::Unsupported}else{C::Partial},SOURCE,"At most 64 thermal zones. These are firmware zones, not guaranteed CPU package temperatures; missing or invalid readings remain unknown.");
    out.window = previous;
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn thermal_conversion_rejects_absent_and_nonphysical_values() {
        assert_eq!(celsius(0.0), None);
        assert_eq!(celsius(f64::NAN), None);
        assert!((celsius(300.0).unwrap() - 26.85).abs() < 0.001);
    }
    #[test]
    fn formatted_counter_status_is_checked_before_value() {
        let mut value = PDH_FMT_COUNTERVALUE {
            CStatus: PDH_CSTATUS_NO_INSTANCE,
            ..Default::default()
        };
        assert_eq!(scalar(&value), None);
        value.CStatus = PDH_CSTATUS_VALID_DATA;
        value.Anonymous.doubleValue = 42.5;
        assert_eq!(scalar(&value), Some(42.5));
    }
}
