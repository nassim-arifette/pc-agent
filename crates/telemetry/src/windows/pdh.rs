//! Performance Data Helper counters (pdh.dll): one query, English counter paths, two
//! collections at least `MIN_RATE_WINDOW` apart, formatted values validated by `CStatus`.
use crate::counters::{
    CounterReadings, Instances, Sampled, Unavailable, MIN_RATE_WINDOW, SRC_CORES, SRC_DISK_IDLE,
    SRC_DISK_QUEUE, SRC_DISK_READ, SRC_DISK_WRITE, SRC_GPU, SRC_HARD_FAULTS,
};
use pcagent_shared::{unix_ms, Status, Window};
use std::{mem::size_of, ptr::null, time::Duration};
use windows_sys::Win32::System::Performance::{
    PdhAddEnglishCounterW, PdhCloseQuery, PdhCollectQueryData, PdhGetFormattedCounterArrayW,
    PdhGetFormattedCounterValue, PdhOpenQueryW, PDH_CSTATUS_BAD_COUNTERNAME, PDH_CSTATUS_NEW_DATA,
    PDH_CSTATUS_NO_COUNTER, PDH_CSTATUS_NO_COUNTERNAME, PDH_CSTATUS_NO_INSTANCE,
    PDH_CSTATUS_NO_OBJECT, PDH_CSTATUS_VALID_DATA, PDH_FMT_COUNTERVALUE,
    PDH_FMT_COUNTERVALUE_ITEM_W, PDH_FMT_DOUBLE, PDH_HCOUNTER, PDH_HQUERY, PDH_MORE_DATA,
    PDH_NO_DATA,
};

/// `PDH_FMT_NOCAP100` from pdh.h (not exported by windows-sys): values above 100 are not capped,
/// so overshoot stays visible and is judged by the portable conversion.
const PDH_FMT_NOCAP100: u32 = 0x0000_8000;
const FORMAT: u32 = PDH_FMT_DOUBLE | PDH_FMT_NOCAP100;
/// Upper bound for one wildcard array (GPU Engine has one instance per process and engine).
const MAX_ARRAY_BYTES: u32 = 16 * 1024 * 1024;

const PATHS: [&str; 7] = [
    r"\PhysicalDisk(_Total)\Avg. Disk sec/Read",
    r"\PhysicalDisk(_Total)\Avg. Disk sec/Write",
    r"\PhysicalDisk(_Total)\% Idle Time",
    r"\PhysicalDisk(_Total)\Current Disk Queue Length",
    r"\Memory\Pages Input/sec",
    r"\Processor Information(*)\% Processor Utility",
    r"\GPU Engine(*)\Utilization Percentage",
];
const SOURCES: [&str; 7] = [
    SRC_DISK_READ,
    SRC_DISK_WRITE,
    SRC_DISK_IDLE,
    SRC_DISK_QUEUE,
    SRC_HARD_FAULTS,
    SRC_CORES,
    SRC_GPU,
];

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}
fn failed(code: u32, what: &str) -> Unavailable {
    let status = match code {
        PDH_CSTATUS_NO_OBJECT
        | PDH_CSTATUS_NO_COUNTER
        | PDH_CSTATUS_NO_INSTANCE
        | PDH_CSTATUS_NO_COUNTERNAME
        | PDH_CSTATUS_BAD_COUNTERNAME => Status::Unsupported,
        _ => Status::Error,
    };
    let reason = if status == Status::Unsupported {
        format!("{what}: counter or instance not present on this system (0x{code:08X}).")
    } else {
        format!("{what} failed (0x{code:08X}).")
    };
    Unavailable::new(status, reason)
}
fn valid(cstatus: u32) -> bool {
    cstatus == PDH_CSTATUS_VALID_DATA || cstatus == PDH_CSTATUS_NEW_DATA
}

#[derive(Clone, Copy)]
struct Baseline {
    at: Duration,
    unix_ms: u64,
}

pub(crate) struct Pdh {
    query: PDH_HQUERY,
    counters: Vec<Result<PDH_HCOUNTER, Unavailable>>,
    baseline: Option<Baseline>,
}
// SAFETY: the query handle is owned by this value and only used through `&mut self`; PDH query
// handles are not bound to the creating thread.
unsafe impl Send for Pdh {}
// SAFETY: no method takes `&self`, so shared references cannot reach the handles.
unsafe impl Sync for Pdh {}
impl Drop for Pdh {
    fn drop(&mut self) {
        // SAFETY: the query was opened by `open`, is owned exclusively and closes its counters.
        unsafe {
            PdhCloseQuery(self.query);
        }
    }
}
impl Pdh {
    pub(crate) fn open() -> Result<Self, Unavailable> {
        let mut query: PDH_HQUERY = std::ptr::null_mut();
        // SAFETY: null data source selects real-time data; `query` is a valid output pointer.
        let code = unsafe { PdhOpenQueryW(null(), 0, &mut query) };
        if code != 0 {
            return Err(failed(code, "PdhOpenQueryW"));
        }
        let counters = PATHS
            .iter()
            .map(|path| {
                let wpath = wide(path);
                let mut counter: PDH_HCOUNTER = std::ptr::null_mut();
                // SAFETY: the query is open; `wpath` is NUL-terminated and outlives the call;
                // `counter` is a valid output pointer.
                let code = unsafe { PdhAddEnglishCounterW(query, wpath.as_ptr(), 0, &mut counter) };
                if code == 0 {
                    Ok(counter)
                } else {
                    Err(failed(code, "PdhAddEnglishCounterW"))
                }
            })
            .collect();
        Ok(Self {
            query,
            counters,
            baseline: None,
        })
    }
    fn collect(&mut self) -> Result<(), Unavailable> {
        // SAFETY: the query handle is open and owned by `self`.
        match unsafe { PdhCollectQueryData(self.query) } {
            0 => Ok(()),
            PDH_NO_DATA => Err(Unavailable::new(
                Status::Unsupported,
                "No performance counter in the query is available on this system.",
            )),
            code => Err(failed(code, "PdhCollectQueryData")),
        }
    }
    /// Age of the pending first collection, if any.
    pub(crate) fn baseline_age(&self, now: Duration) -> Option<Duration> {
        self.baseline.and_then(|b| now.checked_sub(b.at))
    }
    /// Starts a new window at `now`.
    pub(crate) fn start(&mut self, now: Duration) {
        self.baseline = self.collect().ok().map(|()| Baseline {
            at: now,
            unix_ms: unix_ms(),
        });
    }
    /// Second collection; on success it also becomes the start of the next window.
    pub(crate) fn read(&mut self, now: Duration) -> Result<(CounterReadings, Window), Unavailable> {
        let Some(base) = self.baseline else {
            self.start(now);
            return Err(Unavailable::new(
                Status::Partial,
                "Rate counters need two collections; the first one was taken now.",
            ));
        };
        let elapsed = now.checked_sub(base.at).unwrap_or_default();
        if elapsed < MIN_RATE_WINDOW {
            return Err(Unavailable::new(
                Status::Partial,
                "Counter window shorter than one second; the pending first collection is kept.",
            ));
        }
        if let Err(u) = self.collect() {
            self.baseline = None;
            return Err(u);
        }
        let finished = unix_ms();
        self.baseline = Some(Baseline {
            at: now,
            unix_ms: finished,
        });
        let window = Window {
            started_at_unix_ms: base.unix_ms,
            finished_at_unix_ms: finished,
            duration_ms: u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX),
        };
        let readings = CounterReadings {
            disk_read_seconds: self.single(0),
            disk_write_seconds: self.single(1),
            disk_idle_percent: self.single(2),
            disk_queue_length: self.single(3),
            pages_input_per_second: self.single(4),
            core_utility_percent: self.array(5),
            gpu_engine_percent: self.array(6),
        };
        Ok((readings, window))
    }
    fn counter(&self, i: usize) -> Result<PDH_HCOUNTER, Unavailable> {
        self.counters[i].clone()
    }
    fn single(&self, i: usize) -> Sampled<f64> {
        let counter = self.counter(i)?;
        let mut value = PDH_FMT_COUNTERVALUE::default();
        // SAFETY: the counter belongs to the open query; the type output is optional (null) and
        // `value` is a valid output structure.
        let code = unsafe {
            PdhGetFormattedCounterValue(counter, FORMAT, std::ptr::null_mut(), &mut value)
        };
        if code != 0 && !valid(value.CStatus) {
            let status = if value.CStatus == 0 {
                code
            } else {
                value.CStatus
            };
            return Err(failed(status, SOURCES[i]));
        }
        if !valid(value.CStatus) {
            return Err(Unavailable::new(
                Status::Partial,
                format!("Counter data status 0x{:08X} is not valid.", value.CStatus),
            ));
        }
        // SAFETY: PDH_FMT_DOUBLE was requested and CStatus is valid, so doubleValue is set.
        Ok(unsafe { value.Anonymous.doubleValue })
    }
    fn array(&self, i: usize) -> Sampled<Instances> {
        let counter = self.counter(i)?;
        for _ in 0..3 {
            let (mut bytes, mut count) = (0_u32, 0_u32);
            // SAFETY: a null buffer with size 0 asks PDH for the required size.
            let code = unsafe {
                PdhGetFormattedCounterArrayW(
                    counter,
                    FORMAT,
                    &mut bytes,
                    &mut count,
                    std::ptr::null_mut(),
                )
            };
            if code == 0 && count == 0 {
                return Ok(Instances::default());
            }
            if code != PDH_MORE_DATA {
                return Err(failed(code, SOURCES[i]));
            }
            if bytes == 0 || bytes > MAX_ARRAY_BYTES {
                return Err(Unavailable::new(
                    Status::Partial,
                    "Counter array size is zero or exceeds the collection bound.",
                ));
            }
            let item = size_of::<PDH_FMT_COUNTERVALUE_ITEM_W>();
            let len = (bytes as usize).div_ceil(item);
            let mut buffer = vec![PDH_FMT_COUNTERVALUE_ITEM_W::default(); len];
            let capacity = u32::try_from(len * item).unwrap_or(bytes);
            let mut bytes = capacity;
            // SAFETY: `buffer` is aligned for the item type and `bytes` is its size in bytes;
            // PDH writes items followed by their names inside that buffer.
            let code = unsafe {
                PdhGetFormattedCounterArrayW(
                    counter,
                    FORMAT,
                    &mut bytes,
                    &mut count,
                    buffer.as_mut_ptr(),
                )
            };
            if code == PDH_MORE_DATA {
                continue;
            }
            if code != 0 {
                return Err(failed(code, SOURCES[i]));
            }
            let base = buffer.as_ptr() as usize;
            let end = base + len * item;
            let n = (count as usize).min(len);
            let mut out = Instances::default();
            for entry in &buffer[..n] {
                let name = read_name(entry.szName, base, end);
                match name {
                    Some(name) if valid(entry.FmtValue.CStatus) => {
                        // SAFETY: PDH_FMT_DOUBLE was requested and CStatus is valid.
                        let v = unsafe { entry.FmtValue.Anonymous.doubleValue };
                        out.valid.push((name, v));
                    }
                    _ => out.invalid += 1,
                }
            }
            return Ok(out);
        }
        Err(Unavailable::new(
            Status::Partial,
            "Counter instances kept changing while the array was read.",
        ))
    }
}

/// NUL-terminated UTF-16 name that must lie inside the array buffer `[base, end)`.
fn read_name(ptr: *const u16, base: usize, end: usize) -> Option<String> {
    let start = ptr as usize;
    if ptr.is_null() || start < base || start >= end || !start.is_multiple_of(2) {
        return None;
    }
    let max = (end - start) / 2;
    // SAFETY: `[start, start + 2 * max)` lies inside the live, initialized buffer.
    let units = unsafe { std::slice::from_raw_parts(ptr, max) };
    let len = units.iter().position(|&u| u == 0)?;
    String::from_utf16(&units[..len]).ok()
}
