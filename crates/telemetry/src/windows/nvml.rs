//! NVIDIA Management Library (NVML): temperature, memory, utilization, clock, power and
//! slowdown reasons of NVIDIA GPUs. Loaded at run time from System32 only, where the NVIDIA
//! driver installs it (never from the current directory or PATH); read-only queries.
//! Prototypes, structures and constants follow nvml.h (API 13).
use crate::gpu::{self, DeviceReading, Query, NVML_SUCCESS};
use std::{
    ffi::c_void,
    mem::{size_of, transmute_copy},
    ptr::null_mut,
};
use windows_sys::Win32::{
    Foundation::{FreeLibrary, HMODULE},
    System::LibraryLoader::{GetProcAddress, LoadLibraryExW, LOAD_LIBRARY_SEARCH_SYSTEM32},
};

type Device = *mut c_void;
type NoArgs = unsafe extern "C" fn() -> i32;
type GetCount = unsafe extern "C" fn(*mut u32) -> i32;
type GetHandle = unsafe extern "C" fn(u32, *mut Device) -> i32;
type GetName = unsafe extern "C" fn(Device, *mut u8, u32) -> i32;
type GetU32 = unsafe extern "C" fn(Device, *mut u32) -> i32;
type GetU32Of = unsafe extern "C" fn(Device, u32, *mut u32) -> i32;
type GetU64 = unsafe extern "C" fn(Device, *mut u64) -> i32;
type GetMemory = unsafe extern "C" fn(Device, *mut Memory) -> i32;
type GetUtilization = unsafe extern "C" fn(Device, *mut Utilization) -> i32;
type GetTemperatureV = unsafe extern "C" fn(Device, *mut TemperatureV1) -> i32;

/// `nvmlMemory_t` (v1): total = free + used, in bytes.
#[repr(C)]
#[derive(Default)]
struct Memory {
    total: u64,
    free: u64,
    used: u64,
}
/// `nvmlUtilization_t`: percent of the last sample period.
#[repr(C)]
#[derive(Default)]
struct Utilization {
    gpu: u32,
    memory: u32,
}
/// `nvmlTemperature_v1_t`.
#[repr(C)]
struct TemperatureV1 {
    version: u32,
    sensor_type: u32,
    temperature: i32,
}
/// `NVML_STRUCT_VERSION(Temperature, 1)`: the structure size with the version in the top byte.
const TEMPERATURE_V1: u32 = size_of::<TemperatureV1>() as u32 | (1 << 24);
/// `NVML_TEMPERATURE_GPU`: the GPU die sensor.
const TEMPERATURE_GPU: u32 = 0;
/// `NVML_CLOCK_SM`.
const CLOCK_SM: u32 = 1;
/// `NVML_DEVICE_NAME_V2_BUFFER_SIZE`.
const NAME_BUFFER: usize = 96;
/// Bounds the device loop; no desktop holds this many GPUs.
const MAX_DEVICES: u32 = 16;

pub(crate) enum OpenError {
    /// No nvml.dll in System32: no NVIDIA driver, nothing to report.
    Absent,
    Failed(String),
}

pub(crate) struct Nvml {
    module: HMODULE,
    shutdown: NoArgs,
    count: GetCount,
    handle: GetHandle,
    name: Option<GetName>,
    utilization: Option<GetUtilization>,
    memory: Option<GetMemory>,
    temperature_v: Option<GetTemperatureV>,
    temperature: Option<GetU32Of>,
    clock: Option<GetU32Of>,
    power: Option<GetU32>,
    event_reasons: Option<GetU64>,
    throttle_reasons: Option<GetU64>,
}

/// # Safety
/// `T` must be the `unsafe extern "C" fn` type of the exported symbol's nvml.h prototype, and
/// `name` must be NUL-terminated.
unsafe fn symbol<T: Copy>(module: HMODULE, name: &[u8]) -> Option<T> {
    debug_assert_eq!(name.last(), Some(&0));
    debug_assert_eq!(size_of::<T>(), size_of::<usize>());
    // SAFETY: `name` is NUL-terminated and `module` is a loaded module handle.
    let address = unsafe { GetProcAddress(module, name.as_ptr()) }?;
    // SAFETY: function pointers have one size; the caller names the matching prototype.
    Some(unsafe { transmute_copy::<_, T>(&address) })
}

/// Runs one query when its function exists; the value counts only on `NVML_SUCCESS`.
fn call<F: Copy, T>(function: Option<F>, run: impl FnOnce(F) -> (i32, T)) -> Query<T> {
    let function = function.ok_or(None)?;
    match run(function) {
        (NVML_SUCCESS, value) => Ok(value),
        (code, _) => Err(Some(code)),
    }
}

impl Nvml {
    pub(crate) fn open() -> Result<Self, OpenError> {
        let file: Vec<u16> = "nvml.dll\0".encode_utf16().collect();
        // SAFETY: NUL-terminated name and no file handle; only System32 is searched.
        let module =
            unsafe { LoadLibraryExW(file.as_ptr(), null_mut(), LOAD_LIBRARY_SEARCH_SYSTEM32) };
        if module.is_null() {
            return Err(OpenError::Absent);
        }
        // SAFETY: each name is paired with its nvml.h prototype.
        let required = unsafe {
            (
                symbol::<NoArgs>(module, b"nvmlInit_v2\0"),
                symbol::<NoArgs>(module, b"nvmlShutdown\0"),
                symbol::<GetCount>(module, b"nvmlDeviceGetCount_v2\0"),
                symbol::<GetHandle>(module, b"nvmlDeviceGetHandleByIndex_v2\0"),
            )
        };
        let (Some(init), Some(shutdown), Some(count), Some(handle)) = required else {
            // SAFETY: the module was loaded above and nothing from it is kept.
            unsafe { FreeLibrary(module) };
            return Err(OpenError::Failed(
                "nvml.dll lacks the NVML v2 initialization functions".into(),
            ));
        };
        // SAFETY: nvmlInit_v2 takes no arguments.
        let code = unsafe { init() };
        if code != NVML_SUCCESS {
            // SAFETY: initialization failed, so there is nothing to shut down.
            unsafe { FreeLibrary(module) };
            return Err(OpenError::Failed(gpu::describe(code)));
        }
        // SAFETY: each name is paired with its nvml.h prototype.
        unsafe {
            Ok(Self {
                module,
                shutdown,
                count,
                handle,
                name: symbol(module, b"nvmlDeviceGetName\0"),
                utilization: symbol(module, b"nvmlDeviceGetUtilizationRates\0"),
                memory: symbol(module, b"nvmlDeviceGetMemoryInfo\0"),
                temperature_v: symbol(module, b"nvmlDeviceGetTemperatureV\0"),
                temperature: symbol(module, b"nvmlDeviceGetTemperature\0"),
                clock: symbol(module, b"nvmlDeviceGetClockInfo\0"),
                power: symbol(module, b"nvmlDeviceGetPowerUsage\0"),
                event_reasons: symbol(module, b"nvmlDeviceGetCurrentClocksEventReasons\0"),
                throttle_reasons: symbol(module, b"nvmlDeviceGetCurrentClocksThrottleReasons\0"),
            })
        }
    }

    /// Readings of every device, or why the device list could not be read.
    pub(crate) fn read(&self) -> Result<Vec<DeviceReading>, String> {
        let mut count = 0_u32;
        // SAFETY: valid output pointer.
        let code = unsafe { (self.count)(&mut count) };
        if code != NVML_SUCCESS {
            return Err(gpu::describe(code));
        }
        Ok((0..count.min(MAX_DEVICES))
            .map(|index| self.device(index))
            .collect())
    }

    fn device(&self, index: u32) -> DeviceReading {
        let mut device: Device = null_mut();
        // SAFETY: valid output pointer; `index` is below the reported count.
        let code = unsafe { (self.handle)(index, &mut device) };
        if code != NVML_SUCCESS || device.is_null() {
            let code = Some(code);
            return DeviceReading {
                index,
                name: None,
                utilization_percent: Err(code),
                memory: Err(code),
                temperature_celsius: Err(code),
                temperature_source: "NVML.nvmlDeviceGetHandleByIndex_v2",
                sm_clock_mhz: Err(code),
                power_milliwatts: Err(code),
                event_reasons: Err(code),
                reasons_source: "NVML.nvmlDeviceGetHandleByIndex_v2",
            };
        }
        // SAFETY (every query below): `device` is a live handle from NVML and each output
        // pointer or buffer is valid, correctly sized, and initialized as nvml.h requires.
        let name = call(self.name, |f| {
            let mut buffer = [0_u8; NAME_BUFFER];
            let code = unsafe { f(device, buffer.as_mut_ptr(), NAME_BUFFER as u32) };
            (code, gpu::device_name(&buffer))
        })
        .ok()
        .flatten();
        let utilization = call(self.utilization, |f| {
            let mut u = Utilization::default();
            (unsafe { f(device, &mut u) }, u.gpu)
        });
        let memory = call(self.memory, |f| {
            let mut m = Memory::default();
            (unsafe { f(device, &mut m) }, (m.total, m.used))
        });
        let current = call(self.temperature_v, |f| {
            let mut t = TemperatureV1 {
                version: TEMPERATURE_V1,
                sensor_type: TEMPERATURE_GPU,
                temperature: 0,
            };
            (unsafe { f(device, &mut t) }, i64::from(t.temperature))
        });
        let (temperature_celsius, temperature_source) = match current {
            Ok(v) => (Ok(v), "NVML.nvmlDeviceGetTemperatureV"),
            // Older drivers only have the deprecated query.
            Err(_) if self.temperature.is_some() => (
                call(self.temperature, |f| {
                    let mut celsius = 0_u32;
                    let code = unsafe { f(device, TEMPERATURE_GPU, &mut celsius) };
                    (code, i64::from(celsius))
                }),
                "NVML.nvmlDeviceGetTemperature",
            ),
            Err(code) => (Err(code), "NVML.nvmlDeviceGetTemperatureV"),
        };
        let sm_clock_mhz = call(self.clock, |f| {
            let mut mhz = 0_u32;
            (unsafe { f(device, CLOCK_SM, &mut mhz) }, mhz)
        });
        let power_milliwatts = call(self.power, |f| {
            let mut mw = 0_u32;
            (unsafe { f(device, &mut mw) }, mw)
        });
        let reasons = |function: Option<GetU64>| {
            call(function, |f| {
                let mut bits = 0_u64;
                (unsafe { f(device, &mut bits) }, bits)
            })
        };
        let (event_reasons, reasons_source) = match reasons(self.event_reasons) {
            Ok(bits) => (Ok(bits), "NVML.nvmlDeviceGetCurrentClocksEventReasons"),
            Err(_) if self.throttle_reasons.is_some() => (
                reasons(self.throttle_reasons),
                "NVML.nvmlDeviceGetCurrentClocksThrottleReasons",
            ),
            Err(code) => (Err(code), "NVML.nvmlDeviceGetCurrentClocksEventReasons"),
        };
        DeviceReading {
            index,
            name,
            utilization_percent: utilization,
            memory,
            temperature_celsius,
            temperature_source,
            sm_clock_mhz,
            power_milliwatts,
            event_reasons,
            reasons_source,
        }
    }
}

impl Drop for Nvml {
    fn drop(&mut self) {
        // SAFETY: NVML was initialized in `open`; the library is released only after shutdown
        // and no function pointer outlives `self`.
        unsafe {
            (self.shutdown)();
            FreeLibrary(self.module);
        }
    }
}
