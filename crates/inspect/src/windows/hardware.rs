use super::{error, registry::Key, wide, wmi};
use crate::{parse::memory_slots, CoverageStatus as C, Inspection};
use serde_json::Value;
use std::ptr::{null, null_mut};
use windows_sys::Win32::{
    Foundation::{CloseHandle, GetLastError, INVALID_HANDLE_VALUE},
    Storage::FileSystem::*,
    System::{
        Ioctl::*, Registry::*, SystemInformation::GetSystemFirmwareTable, IO::DeviceIoControl,
    },
};

pub(super) fn collect(out: &mut Inspection) {
    memory(out);
    wmi::named(out, "processor", 16);
    wmi::named(out, "storage_devices", 64);
    disks(out);
    graphics(out);
}
fn memory(out: &mut Inspection) {
    const SOURCE: &str = "Win32.GetSystemFirmwareTable(RSMB), SMBIOS type 17";
    // SAFETY: zero-length query for the required raw firmware table size.
    let len = unsafe { GetSystemFirmwareTable(u32::from_be_bytes(*b"RSMB"), 0, null_mut(), 0) };
    if len == 0 || len > 16 * 1024 * 1024 {
        out.coverage(
            "memory_slots",
            C::Unsupported,
            SOURCE,
            "Firmware table unavailable or exceeds the 16MiB read bound.",
        );
        return;
    }
    let mut buffer = vec![0_u8; len as usize];
    // SAFETY: allocated buffer is writable for exactly the size passed to Windows.
    let read = unsafe {
        GetSystemFirmwareTable(
            u32::from_be_bytes(*b"RSMB"),
            0,
            buffer.as_mut_ptr().cast(),
            len,
        )
    };
    if read == 0 || read > len {
        out.coverage(
            "memory_slots",
            C::Partial,
            SOURCE,
            "Firmware table changed or could not be read.",
        );
        return;
    }
    match memory_slots(&buffer[..read as usize]) {
        Ok(parsed) => {
            let slots = parsed.slots;
            let free = slots.iter().filter(|s| !s.populated).count();
            let count = slots.len();
            for slot in slots {
                let entity = out.entity("memory-slot", None);
                out.value(
                    &entity,
                    "memory_populated",
                    slot.populated,
                    "boolean",
                    SOURCE,
                );
                out.value(
                    &entity,
                    "memory_capacity_bytes",
                    slot.capacity_bytes.map(Value::from).unwrap_or(Value::Null),
                    "bytes",
                    SOURCE,
                );
                out.value(
                    &entity,
                    "memory_speed_megahertz",
                    slot.speed_mhz.map(Value::from).unwrap_or(Value::Null),
                    "megahertz",
                    SOURCE,
                );
            }
            if count > 0 {
                let counts_start = out.observations.len();
                out.value("system", "memory_slot_count", count as u64, "count", SOURCE);
                out.value(
                    "system",
                    "memory_free_slot_count",
                    free as u64,
                    "count",
                    SOURCE,
                );
                if parsed.truncated {
                    for observation in &mut out.observations[counts_start..] {
                        observation.status = C::Partial;
                    }
                }
            }
            if parsed.truncated {
                out.coverage("memory_slots", C::Partial, SOURCE, "The 128-device SMBIOS limit was reached; slot and free-slot counts are lower bounds. No serials, asset tags or UUIDs are parsed.");
            } else {
                out.coverage("memory_slots",if count>0{C::Ok}else{C::Unsupported},SOURCE,"Firmware-reported memory devices; soldered memory and virtual machines may not expose usable slots. No serials, asset tags or UUIDs are parsed.");
            }
        }
        Err(detail) => out.coverage("memory_slots", C::Partial, SOURCE, detail),
    }
}

fn disks(out: &mut Inspection) {
    const SOURCE: &str = "Win32.IOCTL_STORAGE_QUERY_PROPERTY";
    let mut seen = 0;
    let mut denied = false;
    for index in 0..32 {
        // SAFETY: fixed device path, zero desired access, shared read/write, OPEN_EXISTING only.
        let file = unsafe {
            CreateFileW(
                wide(&format!(r"\\.\PhysicalDrive{index}")).as_ptr(),
                0,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                null(),
                OPEN_EXISTING,
                0,
                null_mut(),
            )
        };
        if file == INVALID_HANDLE_VALUE {
            // SAFETY: immediate error from CreateFileW.
            let code = unsafe { GetLastError() };
            denied |= code == 5;
            continue;
        }
        struct File(windows_sys::Win32::Foundation::HANDLE);
        impl Drop for File {
            fn drop(&mut self) {
                // SAFETY: uniquely owned valid device handle.
                unsafe {
                    CloseHandle(self.0);
                }
            }
        }
        let file = File(file);
        let mut query = STORAGE_PROPERTY_QUERY {
            PropertyId: StorageDeviceProperty,
            QueryType: PropertyStandardQuery,
            AdditionalParameters: [0],
        };
        let mut buffer = [0_u64; 512];
        let mut read = 0;
        // SAFETY: valid fixed query and aligned bounded descriptor output; read-only IOCTL.
        let ok = unsafe {
            DeviceIoControl(
                file.0,
                IOCTL_STORAGE_QUERY_PROPERTY,
                (&mut query as *mut STORAGE_PROPERTY_QUERY).cast(),
                std::mem::size_of_val(&query) as u32,
                buffer.as_mut_ptr().cast(),
                std::mem::size_of_val(&buffer) as u32,
                &mut read,
                null_mut(),
            )
        };
        if ok == 0 || read < std::mem::size_of::<STORAGE_DEVICE_DESCRIPTOR>() as u32 {
            denied = true;
            continue;
        }
        // SAFETY: successful native call wrote a complete aligned STORAGE_DEVICE_DESCRIPTOR.
        let descriptor = unsafe { &*buffer.as_ptr().cast::<STORAGE_DEVICE_DESCRIPTOR>() };
        let entity = out.entity("disk", None);
        seen += 1;
        out.value(
            &entity,
            "disk_bus_type",
            match descriptor.BusType {
                7 => "usb",
                11 => "sata",
                17 => "nvme",
                1 => "scsi",
                3 => "ata",
                8 => "raid",
                14 => "virtual",
                15 => "file_backed_virtual",
                _ => "other",
            },
            "category",
            SOURCE,
        );
        query.PropertyId = StorageDeviceSeekPenaltyProperty;
        let mut seek = DEVICE_SEEK_PENALTY_DESCRIPTOR::default();
        read = 0;
        // SAFETY: same owned handle and valid fixed property query; correctly sized output.
        let ok = unsafe {
            DeviceIoControl(
                file.0,
                IOCTL_STORAGE_QUERY_PROPERTY,
                (&mut query as *mut STORAGE_PROPERTY_QUERY).cast(),
                std::mem::size_of_val(&query) as u32,
                (&mut seek as *mut DEVICE_SEEK_PENALTY_DESCRIPTOR).cast(),
                std::mem::size_of_val(&seek) as u32,
                &mut read,
                null_mut(),
            )
        };
        out.value(
            &entity,
            "disk_seek_penalty",
            if ok != 0 && read >= std::mem::size_of_val(&seek) as u32 {
                Value::Bool(seek.IncursSeekPenalty)
            } else {
                Value::Null
            },
            "boolean",
            SOURCE,
        );
    }
    out.coverage("storage_device_properties",if denied{C::Partial}else if seen==0{C::Unsupported}else{C::Ok},SOURCE,"Read-only properties for at most 32 physical-drive device numbers. Seek penalty distinguishes rotational behavior, not all device technologies; serial/model identifier offsets are not read.");
}
fn graphics(out: &mut Inspection) {
    const SOURCE: &str = "Registry.Display adapter class";
    let key = match Key::open(
        HKEY_LOCAL_MACHINE,
        r"SYSTEM\CurrentControlSet\Control\Class\{4d36e968-e325-11ce-bfc1-08002be10318}",
        KEY_WOW64_64KEY,
    ) {
        Ok(Some(k)) => k,
        Ok(None) => {
            out.coverage(
                "graphics_drivers",
                C::Unsupported,
                SOURCE,
                "Display class key absent.",
            );
            return;
        }
        Err(e) => {
            out.coverage(
                "graphics_drivers",
                error(e),
                SOURCE,
                "Display class key unreadable.",
            );
            return;
        }
    };
    let mut partial = false;
    match key.names(true, 100) {
        Ok((names, capped)) => {
            partial |= capped;
            for name in names
                .into_iter()
                .filter(|n| n.len() == 4 && n.bytes().all(|b| b.is_ascii_digit()))
            {
                let entry = match key.child(&name) {
                    Ok(Some(k)) => k,
                    _ => {
                        partial = true;
                        continue;
                    }
                };
                let entity = out.entity("gpu-adapter", entry.string("DriverDesc"));
                for (field, metric) in [
                    ("DriverVersion", "graphics_driver_version"),
                    ("DriverDate", "graphics_driver_date"),
                ] {
                    let value = entry.string(field).filter(|s| {
                        s.len() < 64 && s.chars().all(|c| c.is_ascii_digit() || "./- :".contains(c))
                    });
                    partial |= value.is_none();
                    out.value(
                        &entity,
                        metric,
                        value.map(Value::from).unwrap_or(Value::Null),
                        "version_or_date",
                        SOURCE,
                    );
                }
            }
        }
        Err(_) => partial = true,
    }
    out.coverage("graphics_drivers",if partial{C::Partial}else{C::Ok},SOURCE,"Driver description, version and date only; registry may retain disconnected adapters. No device instance IDs are read.");
}
