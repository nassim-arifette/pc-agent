//! Pure parsers for untrusted native data; covered without a Windows host.

/// StartupApproved is not a stable documented structure. Interpret only observed states;
/// unknown/new encodings remain unknown rather than incorrectly declaring an entry enabled.
pub(crate) fn startup_enabled(data: Option<&[u8]>) -> Option<bool> {
    let data = data?;
    if data.len() < 12 {
        return None;
    }
    match u32::from_le_bytes(data[..4].try_into().ok()?) {
        2 | 6 => Some(true),
        3 | 7 => Some(false),
        _ => None,
    }
}

/// A missing trigger beyond the scan limit can still enable a logon task.
pub(crate) fn logon_task_enabled(
    task_enabled: Option<bool>,
    active_trigger: bool,
    triggers_complete: bool,
) -> Option<bool> {
    match (task_enabled, active_trigger, triggers_complete) {
        (Some(false), _, _) | (_, false, true) => Some(false),
        (Some(true), true, _) => Some(true),
        _ => None,
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct MemorySlot {
    pub capacity_bytes: Option<u64>,
    pub populated: bool,
    pub speed_mhz: Option<u64>,
}

pub(crate) const MAX_MEMORY_SLOTS: usize = 128;

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct MemorySlots {
    pub slots: Vec<MemorySlot>,
    pub truncated: bool,
}

/// SMBIOS type 17 memory devices. No serial numbers, asset tags or UUIDs are inspected.
pub(crate) fn memory_slots(raw: &[u8]) -> Result<MemorySlots, &'static str> {
    if raw.len() < 8 {
        return Err("Truncated raw SMBIOS header.");
    }
    let len = u32::from_le_bytes(raw[4..8].try_into().unwrap()) as usize;
    let table = raw
        .get(8..8_usize.checked_add(len).ok_or("Invalid table size.")?)
        .ok_or("Truncated SMBIOS table.")?;
    let mut at = 0;
    let mut slots = Vec::new();
    while at < table.len() {
        let head = table.get(at..at + 4).ok_or("Truncated structure header.")?;
        let size = head[1] as usize;
        if size < 4 {
            return Err("Invalid structure length.");
        }
        let body = table
            .get(at..at + size)
            .ok_or("Truncated structure body.")?;
        if head[0] == 17 {
            if slots.len() == MAX_MEMORY_SLOTS {
                return Ok(MemorySlots {
                    slots,
                    truncated: true,
                });
            }
            if size < 0x15 {
                return Err("Truncated memory device.");
            }
            let short = u16::from_le_bytes(body[0x0c..0x0e].try_into().unwrap());
            let capacity_bytes = match short {
                0 => Some(0),
                0xffff => None,
                0x7fff if size >= 0x20 => Some(
                    u32::from_le_bytes(body[0x1c..0x20].try_into().unwrap()) as u64 & 0x7fff_ffff,
                )
                .and_then(|n| n.checked_mul(1024 * 1024)),
                0x7fff => None,
                n if n & 0x8000 != 0 => Some((n as u64 & 0x7fff) * 1024),
                n => Some(n as u64 * 1024 * 1024),
            };
            let speed = u16::from_le_bytes(
                body.get(0x15..0x17)
                    .and_then(|v| v.try_into().ok())
                    .unwrap_or([0, 0]),
            );
            slots.push(MemorySlot {
                capacity_bytes,
                populated: short != 0,
                speed_mhz: (speed != 0 && speed != 0xffff).then_some(speed as u64),
            });
        }
        let strings = at + size;
        let end = table
            .get(strings..)
            .ok_or("Invalid string table.")?
            .windows(2)
            .position(|w| w == [0, 0])
            .ok_or("Unterminated string table.")?;
        at = strings + end + 2;
        if head[0] == 127 {
            break;
        }
    }
    Ok(MemorySlots {
        slots,
        truncated: false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn incomplete_trigger_scan_cannot_prove_a_logon_task_disabled() {
        assert_eq!(logon_task_enabled(Some(true), false, false), None);
        assert_eq!(logon_task_enabled(Some(true), true, false), Some(true));
        assert_eq!(logon_task_enabled(Some(false), false, false), Some(false));
        assert_eq!(logon_task_enabled(Some(true), false, true), Some(false));
        assert_eq!(logon_task_enabled(None, true, true), None);
        assert_eq!(logon_task_enabled(None, false, true), Some(false));
    }

    #[test]
    fn startup_unknown_does_not_mean_enabled() {
        assert_eq!(startup_enabled(None), None);
        assert_eq!(startup_enabled(Some(&[2])), None);
        assert_eq!(
            startup_enabled(Some(&[2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0])),
            Some(true)
        );
        assert_eq!(
            startup_enabled(Some(&[3, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0])),
            Some(false)
        );
        assert_eq!(
            startup_enabled(Some(&[4, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0])),
            None
        );
    }
    #[test]
    fn smbios_memory_excludes_identifiers_and_rejects_truncation() {
        let mut data = vec![0; 8 + 0x20 + 2];
        data[4..8].copy_from_slice(&(0x22_u32).to_le_bytes());
        data[8] = 17;
        data[9] = 0x20;
        data[20..22].copy_from_slice(&8192_u16.to_le_bytes());
        data[29..31].copy_from_slice(&3200_u16.to_le_bytes());
        assert_eq!(
            memory_slots(&data).unwrap().slots,
            vec![MemorySlot {
                capacity_bytes: Some(8 * 1024 * 1024 * 1024),
                populated: true,
                speed_mhz: Some(3200)
            }]
        );
        data.pop();
        assert!(memory_slots(&data).is_err());
    }

    #[test]
    fn smbios_caps_memory_devices_and_distinguishes_an_exact_fit() {
        fn table(count: usize) -> Vec<u8> {
            let mut data = vec![0; 8 + count * 0x22];
            data[4..8].copy_from_slice(&((count * 0x22) as u32).to_le_bytes());
            for device in data[8..].chunks_exact_mut(0x22) {
                device[0] = 17;
                device[1] = 0x20;
                device[0x0c..0x0e].copy_from_slice(&8192_u16.to_le_bytes());
            }
            data
        }

        let exact = memory_slots(&table(MAX_MEMORY_SLOTS)).unwrap();
        assert_eq!(exact.slots.len(), MAX_MEMORY_SLOTS);
        assert!(!exact.truncated);
        let over = memory_slots(&table(MAX_MEMORY_SLOTS + 1)).unwrap();
        assert_eq!(over.slots, exact.slots);
        assert!(over.truncated);
    }
}
