//! Selected SMBIOS fields only. Structure handles are used locally to join memory arrays.
//! Serial numbers, UUIDs, asset tags, device locators and firmware IDs are never decoded.

pub(crate) const MAX_MEMORY_SLOTS: usize = 128;
const MAX_ARRAYS: usize = 128;
const MAX_STRUCTURES: usize = 65_536;
const MAX_TABLE_BYTES: usize = 16 * 1024 * 1024;

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct MemorySlot {
    pub capacity_bytes: Option<u64>,
    pub populated: Option<bool>,
    pub speed_mt_s: Option<u64>,
    pub configured_speed_mt_s: Option<u64>,
    pub memory_type: Option<&'static str>,
    pub form_factor: Option<&'static str>,
    pub manufacturer: Option<String>,
    pub part_number: Option<String>,
    array_handle: u16,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct MemoryArray {
    pub maximum_capacity_bytes: Option<u64>,
    pub device_count: Option<u64>,
    pub system_memory: bool,
    handle: u16,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Inventory {
    pub slots: Vec<MemorySlot>,
    pub arrays: Vec<MemoryArray>,
    pub manufacturer: Option<String>,
    pub model: Option<String>,
    pub truncated: bool,
    pub arrays_known: bool,
}

fn word(body: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(body.get(at..at + 2)?.try_into().ok()?))
}
fn dword(body: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(body.get(at..at + 4)?.try_into().ok()?))
}
fn qword(body: &[u8], at: usize) -> Option<u64> {
    Some(u64::from_le_bytes(body.get(at..at + 8)?.try_into().ok()?))
}

fn text(strings: &[u8], index: Option<&u8>) -> Option<String> {
    let index = *index?;
    if index == 0 {
        return None;
    }
    let raw = strings.split(|b| *b == 0).nth(index as usize - 1)?;
    if raw.is_empty() || raw.len() > 128 {
        return None;
    }
    let value = std::str::from_utf8(raw).ok()?.trim();
    if value.is_empty()
        || value
            .chars()
            .any(|c| c.is_control() || matches!(c, '\\' | '/' | '@'))
        || [
            "unknown",
            "none",
            "not specified",
            "not available",
            "n/a",
            "default string",
            "to be filled by o.e.m.",
            "system product name",
            "system manufacturer",
        ]
        .contains(&value.to_ascii_lowercase().as_str())
    {
        return None;
    }
    Some(value.to_owned())
}

fn memory_type(value: u8) -> Option<&'static str> {
    Some(match value {
        0x03 => "dram",
        0x0f => "sdram",
        0x11 => "rdram",
        0x12 => "ddr",
        0x13 => "ddr2",
        0x14 => "ddr2_fb_dimm",
        0x18 => "ddr3",
        0x19 => "fbd2",
        0x1a => "ddr4",
        0x1b => "lpddr",
        0x1c => "lpddr2",
        0x1d => "lpddr3",
        0x1e => "lpddr4",
        0x1f => "logical_nonvolatile",
        0x20 => "hbm",
        0x21 => "hbm2",
        0x22 => "ddr5",
        0x23 => "lpddr5",
        0x24 => "hbm3",
        _ => return None,
    })
}
fn form_factor(value: u8) -> Option<&'static str> {
    Some(match value {
        3 => "simm",
        4 => "sip",
        5 => "chip",
        6 => "dip",
        7 => "zip",
        8 => "proprietary_card",
        9 => "dimm",
        10 => "tsop",
        11 => "row_of_chips",
        12 => "rimm",
        13 => "so_dimm",
        14 => "srimm",
        15 => "fb_dimm",
        16 => "die",
        17 => "camm",
        _ => return None,
    })
}
pub(crate) fn mounting_hint(form: Option<&str>) -> Option<&'static str> {
    match form {
        Some("chip" | "row_of_chips" | "die" | "tsop") => Some("likely_soldered"),
        Some("dimm" | "so_dimm" | "camm" | "simm" | "rimm" | "srimm" | "fb_dimm") => Some("module"),
        _ => None,
    }
}
fn speed(body: &[u8], at: usize, extended_at: usize) -> Option<u64> {
    match word(body, at)? {
        0 => None,
        0xffff => dword(body, extended_at)
            .filter(|v| *v > 0xffff && v & 0x8000_0000 == 0)
            .map(u64::from),
        n => Some(n as u64),
    }
}
fn slot(body: &[u8], strings: &[u8]) -> Result<MemorySlot, &'static str> {
    if body.len() < 0x15 {
        return Err("Truncated memory device.");
    }
    let short = word(body, 0x0c).ok_or("Missing memory size.")?;
    let capacity_bytes = match short {
        0 => Some(0),
        0xffff => None,
        0x7fff => dword(body, 0x1c)
            .filter(|n| *n != 0 && n & 0x8000_0000 == 0)
            .and_then(|n| (n as u64).checked_mul(1024 * 1024)),
        n if n & 0x8000 != 0 => Some((n as u64 & 0x7fff) * 1024),
        n => Some(n as u64 * 1024 * 1024),
    };
    Ok(MemorySlot {
        capacity_bytes,
        populated: match short {
            0 => Some(false),
            0xffff => None,
            _ => Some(true),
        },
        speed_mt_s: speed(body, 0x15, 0x54),
        configured_speed_mt_s: speed(body, 0x20, 0x58),
        memory_type: memory_type(body[0x12]),
        form_factor: form_factor(body[0x0e]),
        manufacturer: text(strings, body.get(0x17)),
        part_number: text(strings, body.get(0x1a)),
        array_handle: word(body, 4).ok_or("Missing memory array reference.")?,
    })
}
fn array(body: &[u8]) -> Result<MemoryArray, &'static str> {
    if body.len() < 0x0f {
        return Err("Truncated physical memory array.");
    }
    let maximum_capacity_bytes = match dword(body, 7).ok_or("Missing array capacity.")? {
        0 => None,
        0x8000_0000 => qword(body, 0x0f).filter(|n| *n != 0),
        n if n < 0x8000_0000 => Some(n as u64 * 1024),
        _ => None,
    };
    Ok(MemoryArray {
        maximum_capacity_bytes,
        device_count: word(body, 0x0d).filter(|n| *n != 0).map(u64::from),
        system_memory: body[5] == 3,
        handle: word(body, 2).ok_or("Missing array handle.")?,
    })
}

/// The raw Windows header is eight bytes. Parse only selected types/fields; old structures
/// omit optional fields rather than providing zero defaults. Speeds are transfer rates (MT/s).
pub(crate) fn inventory(raw: &[u8]) -> Result<Inventory, &'static str> {
    let len = dword(raw, 4).ok_or("Truncated raw SMBIOS header.")? as usize;
    if len > MAX_TABLE_BYTES {
        return Err("SMBIOS table exceeds the 16MiB bound.");
    }
    let table = raw.get(8..8 + len).ok_or("Truncated SMBIOS table.")?;
    let mut result = Inventory {
        slots: Vec::new(),
        arrays: Vec::new(),
        manufacturer: None,
        model: None,
        truncated: false,
        arrays_known: false,
    };
    let mut at = 0;
    let mut structures = 0;
    while at < table.len() {
        if structures == MAX_STRUCTURES {
            result.truncated = true;
            break;
        }
        structures += 1;
        let head = table.get(at..at + 4).ok_or("Truncated structure header.")?;
        let size = head[1] as usize;
        if size < 4 {
            return Err("Invalid structure length.");
        }
        let body = table
            .get(at..at + size)
            .ok_or("Truncated structure body.")?;
        let string_start = at + size;
        let rest = table.get(string_start..).ok_or("Invalid string table.")?;
        let string_len = rest
            .windows(2)
            .position(|w| w == [0, 0])
            .ok_or("Unterminated string table.")?;
        let strings = &rest[..string_len + 1];
        match head[0] {
            1 if size >= 8 => {
                if result.manufacturer.is_none() {
                    result.manufacturer = text(strings, body.get(4));
                }
                if result.model.is_none() {
                    result.model = text(strings, body.get(5));
                }
            }
            16 => {
                result.arrays_known = true;
                if result.arrays.len() < MAX_ARRAYS {
                    result.arrays.push(array(body)?);
                } else {
                    result.truncated = true;
                }
            }
            17 => {
                if result.slots.len() < MAX_MEMORY_SLOTS {
                    result.slots.push(slot(body, strings)?);
                } else {
                    result.truncated = true;
                }
            }
            _ => {}
        }
        at = string_start + string_len + 2;
        if head[0] == 127 {
            break;
        }
    }
    if result.arrays_known {
        result.slots.retain(|slot| {
            result
                .arrays
                .iter()
                .any(|array| array.system_memory && array.handle == slot.array_handle)
        });
        result.arrays.retain(|array| array.system_memory);
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn structure(kind: u8, len: usize, handle: u16, strings: &[&str]) -> Vec<u8> {
        let mut data = vec![0; len];
        data[0] = kind;
        data[1] = len as u8;
        data[2..4].copy_from_slice(&handle.to_le_bytes());
        for string in strings {
            data.extend_from_slice(string.as_bytes());
            data.push(0);
        }
        if strings.is_empty() {
            data.push(0);
        }
        data.push(0);
        data
    }
    fn raw(parts: &[Vec<u8>]) -> Vec<u8> {
        let mut data = vec![0; 8];
        let len: usize = parts.iter().map(Vec::len).sum();
        data[4..8].copy_from_slice(&(len as u32).to_le_bytes());
        for part in parts {
            data.extend_from_slice(part);
        }
        data
    }
    fn memory(len: usize) -> Vec<u8> {
        let mut data = structure(
            17,
            len,
            2,
            &["RAM Maker", "PRIVATE-SERIAL", "PRIVATE-ASSET", "PART-DDR5"],
        );
        data[4..6].copy_from_slice(&1_u16.to_le_bytes());
        data[0x0c..0x0e].copy_from_slice(&8192_u16.to_le_bytes());
        data[0x0e] = 13;
        data[0x12] = 0x22;
        if len >= 0x17 {
            data[0x15..0x17].copy_from_slice(&5600_u16.to_le_bytes());
        }
        if len >= 0x1b {
            data[0x17] = 1;
            data[0x18] = 2;
            data[0x19] = 3;
            data[0x1a] = 4;
        }
        if len >= 0x22 {
            data[0x20..0x22].copy_from_slice(&4800_u16.to_le_bytes());
        }
        data
    }
    #[test]
    fn decodes_selected_type_1_16_17_fields_without_decoding_identifiers() {
        let mut system = structure(1, 0x1b, 0, &["PC Maker", "Model A", "PRIVATE-PC-SERIAL"]);
        system[4] = 1;
        system[5] = 2;
        system[7] = 3;
        system[8..24].fill(0xff);
        let mut array = structure(16, 0x17, 1, &[]);
        array[5] = 3;
        array[7..11].copy_from_slice(&(64_u32 * 1024 * 1024).to_le_bytes());
        array[0x0d..0x0f].copy_from_slice(&2_u16.to_le_bytes());
        let parsed = inventory(&raw(&[system, array, memory(0x22)])).unwrap();
        assert_eq!(parsed.manufacturer.as_deref(), Some("PC Maker"));
        assert_eq!(parsed.model.as_deref(), Some("Model A"));
        assert_eq!(
            parsed.arrays[0].maximum_capacity_bytes,
            Some(64 * 1024 * 1024 * 1024)
        );
        assert_eq!(parsed.arrays[0].device_count, Some(2));
        assert_eq!(parsed.slots[0].memory_type, Some("ddr5"));
        assert_eq!(parsed.slots[0].form_factor, Some("so_dimm"));
        assert_eq!(parsed.slots[0].configured_speed_mt_s, Some(4800));
        assert_eq!(parsed.slots[0].part_number.as_deref(), Some("PART-DDR5"));
        assert!(!format!("{parsed:?}").contains("PRIVATE"));
    }
    #[test]
    fn old_structures_unknown_sizes_and_extended_fields_are_explicit() {
        let mut old = memory(0x15);
        old[0x0c..0x0e].copy_from_slice(&0xffff_u16.to_le_bytes());
        let parsed = inventory(&raw(&[old])).unwrap();
        assert_eq!(parsed.slots[0].populated, None);
        assert_eq!(parsed.slots[0].capacity_bytes, None);
        assert_eq!(parsed.slots[0].speed_mt_s, None);
        assert_eq!(parsed.slots[0].configured_speed_mt_s, None);
        let mut extended = memory(0x5c);
        extended[0x0c..0x0e].copy_from_slice(&0x7fff_u16.to_le_bytes());
        extended[0x1c..0x20].copy_from_slice(&65_536_u32.to_le_bytes());
        extended[0x20..0x22].copy_from_slice(&0xffff_u16.to_le_bytes());
        extended[0x58..0x5c].copy_from_slice(&70_000_u32.to_le_bytes());
        let parsed = inventory(&raw(&[extended])).unwrap();
        assert_eq!(
            parsed.slots[0].capacity_bytes,
            Some(64 * 1024 * 1024 * 1024)
        );
        assert_eq!(parsed.slots[0].configured_speed_mt_s, Some(70_000));
    }
    #[test]
    fn extended_array_capacity_uses_bytes_and_video_arrays_are_excluded() {
        let mut array = structure(16, 0x17, 1, &[]);
        array[5] = 3;
        array[7..11].copy_from_slice(&0x8000_0000_u32.to_le_bytes());
        array[0x0f..0x17].copy_from_slice(&(4_u64 * 1024 * 1024 * 1024 * 1024).to_le_bytes());
        let parsed = inventory(&raw(&[array.clone(), memory(0x22)])).unwrap();
        assert_eq!(
            parsed.arrays[0].maximum_capacity_bytes,
            Some(4 * 1024 * 1024 * 1024 * 1024)
        );
        array[5] = 4;
        let parsed = inventory(&raw(&[array, memory(0x22)])).unwrap();
        assert!(parsed.slots.is_empty());
    }
    #[test]
    fn malformed_tables_and_unknown_codes_do_not_create_measurements() {
        let mut data = raw(&[memory(0x22)]);
        data.pop();
        assert!(inventory(&data).is_err());
        let mut device = memory(0x22);
        device[0x12] = 0xff;
        device[0x0e] = 0xff;
        device[0x20..0x22].fill(0);
        let parsed = inventory(&raw(&[device])).unwrap();
        assert_eq!(parsed.slots[0].memory_type, None);
        assert_eq!(parsed.slots[0].form_factor, None);
        assert_eq!(parsed.slots[0].configured_speed_mt_s, None);
        assert_eq!(mounting_hint(Some("so_dimm")), Some("module"));
        assert_eq!(mounting_hint(Some("row_of_chips")), Some("likely_soldered"));
        assert_eq!(mounting_hint(None), None);
    }
    #[test]
    fn device_limit_distinguishes_an_exact_fit() {
        let exact = raw(&(0..MAX_MEMORY_SLOTS)
            .map(|_| memory(0x22))
            .collect::<Vec<_>>());
        let parsed = inventory(&exact).unwrap();
        assert_eq!(parsed.slots.len(), MAX_MEMORY_SLOTS);
        assert!(!parsed.truncated);
        let over = raw(&(0..=MAX_MEMORY_SLOTS)
            .map(|_| memory(0x22))
            .collect::<Vec<_>>());
        assert!(inventory(&over).unwrap().truncated);
    }
}
