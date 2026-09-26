use super::{last_status, processor_count};
use crate::{parse_multi_sz, windows_family};
use pcagent_shared::{
    new_id, unix_ms, HardwareInventory, Observed, Origin, OsVersion, Status, VolumeInfo,
    VolumeKind, SCHEMA_VERSION,
};
use std::{iter::once, mem::zeroed, ptr::null_mut};
use windows_sys::Win32::{
    Foundation::ERROR_SUCCESS,
    Storage::FileSystem::{GetDiskFreeSpaceExW, GetDriveTypeW, GetLogicalDriveStringsW},
    System::{
        Registry::{
            RegGetValueW, HKEY_LOCAL_MACHINE, RRF_RT_REG_DWORD, RRF_RT_REG_SZ,
            RRF_SUBKEY_WOW6464KEY,
        },
        SystemInformation::{
            GetNativeSystemInfo, GetPhysicallyInstalledSystemMemory, PROCESSOR_ARCHITECTURE_AMD64,
            PROCESSOR_ARCHITECTURE_ARM64, PROCESSOR_ARCHITECTURE_INTEL, SYSTEM_INFO,
        },
        WindowsProgramming::{
            DRIVE_CDROM, DRIVE_FIXED, DRIVE_NO_ROOT_DIR, DRIVE_RAMDISK, DRIVE_REMOTE,
            DRIVE_REMOVABLE,
        },
    },
};

const VERSION_KEY: &str = r"SOFTWARE\Microsoft\Windows NT\CurrentVersion";
const VERSION_SOURCE: &str = r"Registry.HKLM\SOFTWARE\Microsoft\Windows NT\CurrentVersion";

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(once(0)).collect()
}
fn reg_dword(name: &str) -> Option<u32> {
    let (key, value) = (wide(VERSION_KEY), wide(name));
    let mut data = 0_u32;
    let mut size = size_of_u32::<u32>();
    // SAFETY: NUL-terminated strings; data is a writable DWORD of `size` bytes; the type
    // filter guarantees a DWORD is written or an error is returned.
    let rc = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            key.as_ptr(),
            value.as_ptr(),
            RRF_RT_REG_DWORD | RRF_SUBKEY_WOW6464KEY,
            null_mut(),
            (&mut data as *mut u32).cast(),
            &mut size,
        )
    };
    (rc == ERROR_SUCCESS).then_some(data)
}
fn reg_sz(name: &str) -> Option<String> {
    let (key, value) = (wide(VERSION_KEY), wide(name));
    let mut data = [0_u16; 256];
    let mut size = size_of_u32::<[u16; 256]>();
    // SAFETY: NUL-terminated strings; data is writable for `size` bytes and RegGetValueW
    // NUL-terminates REG_SZ output or fails with ERROR_MORE_DATA.
    let rc = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            key.as_ptr(),
            value.as_ptr(),
            RRF_RT_REG_SZ | RRF_SUBKEY_WOW6464KEY,
            null_mut(),
            data.as_mut_ptr().cast(),
            &mut size,
        )
    };
    if rc != ERROR_SUCCESS {
        return None;
    }
    let units = (size as usize / 2).min(data.len());
    parse_multi_sz(&data[..units]).into_iter().next()
}
fn size_of_u32<T>() -> u32 {
    std::mem::size_of::<T>() as u32
}
fn os_version() -> Observed<OsVersion> {
    let major = reg_dword("CurrentMajorVersionNumber");
    let minor = reg_dword("CurrentMinorVersionNumber");
    let build = reg_sz("CurrentBuildNumber").and_then(|b| b.trim().parse().ok());
    match (major, minor, build) {
        (Some(major), Some(minor), Some(build)) => Observed::ok(
            OsVersion {
                major,
                minor,
                build,
                update_build_revision: reg_dword("UBR"),
                display_version: reg_sz("DisplayVersion"),
                family: windows_family(major, build).map(str::to_owned),
            },
            VERSION_SOURCE,
        ),
        _ => Observed::missing(VERSION_SOURCE, Status::Error),
    }
}
fn architecture() -> Observed<String> {
    const SOURCE: &str = "Win32.GetNativeSystemInfo";
    // SAFETY: SYSTEM_INFO is plain data; the union read is the documented architecture field.
    let arch = unsafe {
        let mut info: SYSTEM_INFO = zeroed();
        GetNativeSystemInfo(&mut info);
        info.Anonymous.Anonymous.wProcessorArchitecture
    };
    match arch {
        PROCESSOR_ARCHITECTURE_AMD64 => Observed::ok("x64".into(), SOURCE),
        PROCESSOR_ARCHITECTURE_ARM64 => Observed::ok("arm64".into(), SOURCE),
        PROCESSOR_ARCHITECTURE_INTEL => Observed::ok("x86".into(), SOURCE),
        _ => Observed::missing(SOURCE, Status::Unsupported),
    }
}
fn installed_memory() -> Observed<u64> {
    const SOURCE: &str = "Win32.GetPhysicallyInstalledSystemMemory";
    let mut kib = 0_u64;
    // SAFETY: kib is a valid u64 output.
    if unsafe { GetPhysicallyInstalledSystemMemory(&mut kib) } == 0 {
        return Observed::missing(SOURCE, last_status());
    }
    match kib.checked_mul(1024) {
        Some(bytes) if bytes > 0 => Observed::ok(bytes, SOURCE),
        _ => Observed::missing(SOURCE, Status::Error),
    }
}
fn volume(root: String) -> VolumeInfo {
    let path = wide(&root);
    // SAFETY: NUL-terminated root path; no outputs.
    let kind = match unsafe { GetDriveTypeW(path.as_ptr()) } {
        DRIVE_FIXED => VolumeKind::Fixed,
        DRIVE_REMOVABLE => VolumeKind::Removable,
        DRIVE_REMOTE => VolumeKind::Network,
        DRIVE_CDROM => VolumeKind::Optical,
        DRIVE_RAMDISK => VolumeKind::RamDisk,
        DRIVE_NO_ROOT_DIR => VolumeKind::NoRootDir,
        _ => VolumeKind::Unknown,
    };
    let mut v = VolumeInfo {
        root,
        kind,
        total_bytes: None,
        free_bytes: None,
        available_to_caller_bytes: None,
        source: "Win32.GetDiskFreeSpaceExW".into(),
        status: Status::Unsupported,
        limitations: Vec::new(),
    };
    if !matches!(kind, VolumeKind::Fixed | VolumeKind::RamDisk) {
        v.limitations.push(
            "Not probed: network, removable and optical volumes may block, wake devices or prompt."
                .into(),
        );
        return v;
    }
    let (mut avail, mut total, mut free) = (0_u64, 0_u64, 0_u64);
    // SAFETY: NUL-terminated root path; all outputs are valid u64 locations.
    if unsafe { GetDiskFreeSpaceExW(path.as_ptr(), &mut avail, &mut total, &mut free) } == 0 {
        v.status = last_status();
        return v;
    }
    v.total_bytes = Some(total);
    v.free_bytes = Some(free);
    v.available_to_caller_bytes = Some(avail);
    v.status = Status::Ok;
    v
}
fn volumes() -> Observed<Vec<VolumeInfo>> {
    const SOURCE: &str = "Win32.GetLogicalDriveStringsW";
    let mut buffer = [0_u16; 512];
    // SAFETY: the buffer is writable for its length in UTF-16 units.
    let len = unsafe { GetLogicalDriveStringsW(buffer.len() as u32, buffer.as_mut_ptr()) };
    if len == 0 {
        return Observed::missing(SOURCE, last_status());
    }
    if len as usize > buffer.len() {
        return Observed::missing(SOURCE, Status::Partial);
    }
    Observed::ok(
        parse_multi_sz(&buffer[..len as usize])
            .into_iter()
            .map(volume)
            .collect(),
        SOURCE,
    )
}

pub(crate) fn hardware_inventory() -> HardwareInventory {
    let processors = match processor_count() {
        0 => Observed::missing("Win32.GetActiveProcessorCount", Status::Error),
        n => Observed::ok(n, "Win32.GetActiveProcessorCount"),
    };
    let mut inv = HardwareInventory {
        schema_version: SCHEMA_VERSION,
        id: new_id("inventory"),
        origin: Origin::Live,
        status: Status::Ok,
        collected_at_unix_ms: unix_ms(),
        os_version: os_version(),
        architecture: architecture(),
        logical_processors: processors,
        installed_memory_bytes: installed_memory(),
        volumes: volumes(),
        gpu_adapters: Observed::missing("DXGI", Status::Unsupported),
        limitations: vec![
            "Windows family is inferred from the build number; the registry product name is not used.".into(),
            "Installed memory is firmware-reported and can exceed the physical memory usable by Windows.".into(),
            "GPU adapter enumeration is not implemented yet.".into(),
            "No serial numbers, volume serials or machine identifiers are collected.".into(),
        ],
    };
    let statuses = [
        inv.os_version.status,
        inv.architecture.status,
        inv.logical_processors.status,
        inv.installed_memory_bytes.status,
        inv.volumes.status,
    ];
    if statuses.iter().any(|s| *s != Status::Ok) {
        inv.status = Status::Partial;
    }
    inv
}
