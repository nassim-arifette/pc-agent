//! Present device driver properties only. Never requests instance IDs, hardware IDs or paths.
use super::{error, updates, wide};
use crate::{
    parse::driver_version::{display_text, filetime_ms, DriverVersion},
    CoverageStatus as C, Inspection, WindowsUpdateMode,
};
use serde_json::Value;
use std::{
    ptr::{null, null_mut},
    time::{Duration, Instant},
};
use windows_sys::{
    core::GUID,
    Win32::{
        Devices::{DeviceAndDriverInstallation::*, Properties::*},
        Foundation::{FreeLibrary, GetLastError, DEVPROPKEY, ERROR_NO_MORE_ITEMS, HMODULE},
        System::LibraryLoader::{GetProcAddress, LoadLibraryExW, LOAD_LIBRARY_SEARCH_SYSTEM32},
    },
};

const SOURCE: &str = "Win32.SetupDiGetClassDevsW/SetupDiGetDevicePropertyW (present devices only)";
const CLASSES: &[(&str, GUID)] = &[
    (
        "display",
        GUID::from_u128(0x4d36e968_e325_11ce_bfc1_08002be10318),
    ),
    (
        "network",
        GUID::from_u128(0x4d36e972_e325_11ce_bfc1_08002be10318),
    ),
    (
        "storage",
        GUID::from_u128(0x4d36e96a_e325_11ce_bfc1_08002be10318),
    ),
    (
        "storage",
        GUID::from_u128(0x4d36e97b_e325_11ce_bfc1_08002be10318),
    ),
    (
        "storage",
        GUID::from_u128(0x4d36e967_e325_11ce_bfc1_08002be10318),
    ),
    (
        "audio",
        GUID::from_u128(0x4d36e96c_e325_11ce_bfc1_08002be10318),
    ),
    (
        "bluetooth",
        GUID::from_u128(0xe0cbf06c_cd8b_4647_bb8a_263b43f0f974),
    ),
    (
        "system",
        GUID::from_u128(0x4d36e97d_e325_11ce_bfc1_08002be10318),
    ),
];

pub(super) struct InstalledDriver {
    pub reference: String,
    pub class: &'static str,
    pub provider: Option<String>,
    pub model: Option<String>,
    pub version: Option<DriverVersion>,
}
struct DeviceSet(HDEVINFO);
impl Drop for DeviceSet {
    fn drop(&mut self) {
        // SAFETY: the owned SetupAPI set is destroyed once after its device records are unused.
        unsafe {
            SetupDiDestroyDeviceInfoList(self.0);
        }
    }
}
fn property(
    set: &DeviceSet,
    device: &SP_DEVINFO_DATA,
    key: &DEVPROPKEY,
) -> Option<(DEVPROPTYPE, Vec<u8>)> {
    let mut kind = 0;
    let mut data = vec![0_u8; 4096];
    let mut required = 0;
    // SAFETY: live read-only device set/record; fixed property key, bounded writable buffer and
    // valid type/length outputs. No property setter or identifier lookup is invoked.
    let ok = unsafe {
        SetupDiGetDevicePropertyW(
            set.0,
            device,
            key,
            &mut kind,
            data.as_mut_ptr(),
            data.len() as u32,
            &mut required,
            0,
        )
    };
    if ok == 0 || required as usize > data.len() {
        return None;
    }
    data.truncate(required as usize);
    Some((kind, data))
}
fn text_property(set: &DeviceSet, device: &SP_DEVINFO_DATA, key: &DEVPROPKEY) -> Option<String> {
    let (kind, data) = property(set, device, key)?;
    if kind != DEVPROP_TYPE_STRING || data.len() % 2 != 0 {
        return None;
    }
    let units = data
        .chunks_exact(2)
        .map(|v| u16::from_le_bytes([v[0], v[1]]))
        .collect::<Vec<_>>();
    let end = units.iter().position(|unit| *unit == 0)?;
    let value = String::from_utf16(&units[..end]).ok()?;
    display_text(&value)
}
fn date_property(set: &DeviceSet, device: &SP_DEVINFO_DATA) -> Option<u64> {
    let (kind, data) = property(set, device, &DEVPKEY_Device_DriverDate)?;
    if kind != DEVPROP_TYPE_FILETIME || data.len() != 8 {
        return None;
    }
    filetime_ms(u64::from_le_bytes(data.try_into().ok()?))
}
pub(super) fn collect(
    out: &mut Inspection,
    limit: usize,
    windows_update: WindowsUpdateMode,
) -> Option<updates::SearchCleanup> {
    let started = Instant::now();
    let mut drivers = Vec::new();
    let mut scanned = 0;
    let mut partial = false;
    let mut bounded = false;
    for (class, guid) in CLASSES {
        if drivers.len() >= limit || scanned >= 512 || started.elapsed() > Duration::from_secs(4) {
            bounded = true;
            break;
        }
        // SAFETY: fixed class GUID, null enumerator/window, local present-device enumeration.
        let set = unsafe { SetupDiGetClassDevsW(guid, null(), null_mut(), DIGCF_PRESENT) };
        if set == -1 {
            // SAFETY: immediate last-error read after a failed native call.
            let code = unsafe { GetLastError() };
            out.coverage(
                &format!("installed_drivers_{class}"),
                error(code),
                SOURCE,
                "Present device class could not be enumerated.",
            );
            partial = true;
            continue;
        }
        let set = DeviceSet(set);
        for index in 0..=512 {
            if drivers.len() >= limit
                || scanned >= 512
                || started.elapsed() > Duration::from_secs(4)
            {
                bounded = true;
                break;
            }
            let mut device = SP_DEVINFO_DATA {
                cbSize: std::mem::size_of::<SP_DEVINFO_DATA>() as u32,
                ..Default::default()
            };
            // SAFETY: owned set, bounded index and correctly initialized writable record.
            if unsafe { SetupDiEnumDeviceInfo(set.0, index, &mut device) } == 0 {
                // SAFETY: immediate error read after the native call.
                if unsafe { GetLastError() } != ERROR_NO_MORE_ITEMS {
                    partial = true;
                }
                break;
            }
            scanned += 1;
            let provider = text_property(&set, &device, &DEVPKEY_Device_DriverProvider);
            // DeviceDesc comes from the INF; FriendlyName can contain a user-chosen device name.
            let model = text_property(&set, &device, &DEVPKEY_Device_DeviceDesc);
            let version = text_property(&set, &device, &DEVPKEY_Device_DriverVersion)
                .as_deref()
                .and_then(DriverVersion::parse);
            let date = date_property(&set, &device);
            partial |= provider.is_none() || model.is_none() || version.is_none() || date.is_none();
            let reference = out.entity("device", None);
            out.value(&reference, "driver_class", *class, "category", SOURCE);
            out.name_value(&reference, "driver_provider_ref", provider.clone(), SOURCE);
            out.name_value(&reference, "driver_model_ref", model.clone(), SOURCE);
            out.value(
                &reference,
                "driver_version",
                version
                    .map(DriverVersion::text)
                    .map(Value::from)
                    .unwrap_or(Value::Null),
                "version",
                SOURCE,
            );
            out.value(
                &reference,
                "driver_date_unix_ms",
                date.map(Value::from).unwrap_or(Value::Null),
                "unix_ms",
                SOURCE,
            );
            drivers.push(InstalledDriver {
                reference,
                class,
                provider,
                model,
                version,
            });
        }
    }
    out.coverage("installed_drivers", if partial || bounded { C::Partial } else { C::Ok }, SOURCE,
        if bounded { "The result, 512-device examination or four-second native scan bound was reached. Some present devices/classes are omitted; driver fields remain unknown if absent." }
        else { "Present display, network, storage, system, audio and Bluetooth devices only. Provider/version/date and INF device description are read; instance IDs, hardware IDs, serials, friendly names and paths are never requested." });
    nvidia_versions(out, &drivers);
    updates::driver_offers(out, &drivers, limit, windows_update)
}

fn nvidia_versions(out: &mut Inspection, drivers: &[InstalledDriver]) {
    let nvidia = drivers
        .iter()
        .filter(|driver| {
            driver.class == "display"
                && driver.provider.as_deref().is_some_and(|provider| {
                    provider.eq_ignore_ascii_case("NVIDIA")
                        || provider.eq_ignore_ascii_case("NVIDIA Corporation")
                })
        })
        .collect::<Vec<_>>();
    if nvidia.is_empty() {
        return;
    }
    let nvml = nvml_driver_version();
    for driver in nvidia {
        let converted = driver.version.and_then(DriverVersion::nvidia_public);
        out.value(
            &driver.reference,
            "nvidia_driver_public_version",
            converted.clone().map(Value::from).unwrap_or(Value::Null),
            "version",
            "Local NVIDIA Windows version conversion",
        );
        out.value(
            &driver.reference,
            "nvidia_driver_nvml_version",
            nvml.clone().map(Value::from).unwrap_or(Value::Null),
            "version",
            "NVML.nvmlSystemGetDriverVersion",
        );
        let agreement = match (&converted, &nvml) {
            (Some(a), Some(b)) => Value::Bool(a == b),
            _ => Value::Null,
        };
        out.value(
            &driver.reference,
            "nvidia_driver_version_agreement",
            agreement,
            "boolean",
            "SetupAPI and NVML cross-check",
        );
    }
    out.coverage("nvidia_driver_cross_check", if nvml.is_some() { C::Ok } else { C::Unsupported }, "NVML.nvmlSystemGetDriverVersion", "NVML is read from System32 only. A missing result or disagreement does not validate the converted version; multiple adapters can use different installed packages.");
}

fn nvml_driver_version() -> Option<String> {
    type NoArgs = unsafe extern "C" fn() -> i32;
    type Version = unsafe extern "C" fn(*mut u8, u32) -> i32;
    struct Library(HMODULE);
    impl Drop for Library {
        fn drop(&mut self) {
            // SAFETY: uniquely owned loaded library, with no surviving symbols.
            unsafe {
                FreeLibrary(self.0);
            }
        }
    }
    // SAFETY: a terminated library name, no file handle, and System32-only search prevents
    // loading a same-named binary from the current directory or user PATH.
    let library = unsafe {
        LoadLibraryExW(
            wide("nvml.dll").as_ptr(),
            null_mut(),
            LOAD_LIBRARY_SEARCH_SYSTEM32,
        )
    };
    if library.is_null() {
        return None;
    }
    let library = Library(library);
    // SAFETY: fixed NUL-terminated exports paired with the exact nvml.h prototypes. Symbols
    // are used only while the library remains loaded.
    let (init, shutdown, version): (NoArgs, NoArgs, Version) = unsafe {
        let init = GetProcAddress(library.0, c"nvmlInit_v2".as_ptr().cast())?;
        let shutdown = GetProcAddress(library.0, c"nvmlShutdown".as_ptr().cast())?;
        let version = GetProcAddress(library.0, c"nvmlSystemGetDriverVersion".as_ptr().cast())?;
        (
            std::mem::transmute::<unsafe extern "system" fn() -> isize, NoArgs>(init),
            std::mem::transmute::<unsafe extern "system" fn() -> isize, NoArgs>(shutdown),
            std::mem::transmute::<unsafe extern "system" fn() -> isize, Version>(version),
        )
    };
    // SAFETY: initialization has no parameters; a failed initialization is not shut down.
    if unsafe { init() } != 0 {
        return None;
    }
    let mut buffer = [0_u8; 128];
    // SAFETY: NVML receives a writable bounded buffer; shutdown releases this init reference.
    let code = unsafe {
        let code = version(buffer.as_mut_ptr(), buffer.len() as u32);
        shutdown();
        code
    };
    if code != 0 {
        return None;
    }
    let end = buffer.iter().position(|b| *b == 0)?;
    let value = std::str::from_utf8(&buffer[..end]).ok()?;
    if value.is_empty()
        || value.len() > 32
        || !value
            .split('.')
            .all(|part| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit()))
    {
        return None;
    }
    Some(value.to_owned())
}
