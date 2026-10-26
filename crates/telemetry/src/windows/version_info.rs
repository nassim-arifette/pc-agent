//! Readable program names from executables' version resources (see `crate::program_names`):
//! a query-only process handle for the image path, then the documented version APIs.
//! `GetFileVersionInfoW` maps the file as data, never as code; nothing is written or executed.
use super::{image_path_units, times, OwnedHandle};
use crate::program_names::{drive_letter, name_from_tables, translations, utf16_text};
use std::{ffi::c_void, ptr::null_mut};
use windows_sys::Win32::{
    Storage::FileSystem::{
        GetDriveTypeW, GetFileVersionInfoSizeW, GetFileVersionInfoW, VerQueryValueW,
    },
    System::WindowsProgramming::{DRIVE_FIXED, DRIVE_RAMDISK, DRIVE_REMOVABLE},
};

/// Version resources are a few kilobytes; a larger one is not read.
const MAX_VERSION_BYTES: u32 = 256 * 1024;

/// The executable path of one process instance, or `None` when it refuses a query-only handle
/// or its PID now belongs to another instance.
pub(crate) fn image_path(pid: u32, created: u64) -> Option<String> {
    // The idle and System processes have no executable file.
    if pid == 0 || pid == 4 {
        return None;
    }
    let handle = OwnedHandle::open(pid).ok()?;
    // A PID can be reused: the name must come from the instance that was measured.
    if times(&handle).ok()?.0 != created {
        return None;
    }
    String::from_utf16(&image_path_units(&handle)?).ok()
}

/// The readable name in the version resource of an executable on a local drive, or `None`.
pub(crate) fn version_name(path: &str) -> Option<String> {
    // A network drive could block this worker for a long time; a bare name would be searched.
    let letter = drive_letter(path)?;
    let root: Vec<u16> = format!("{letter}:\\").encode_utf16().chain([0]).collect();
    // SAFETY: `root` is a NUL-terminated UTF-16 string that outlives the call.
    let kind = unsafe { GetDriveTypeW(root.as_ptr()) };
    if !matches!(kind, DRIVE_FIXED | DRIVE_REMOVABLE | DRIVE_RAMDISK) {
        return None;
    }
    let wide: Vec<u16> = path.encode_utf16().chain([0]).collect();
    let mut ignored = 0_u32;
    // SAFETY: `wide` is NUL-terminated and outlives the call; `ignored` is a valid output.
    let size = unsafe { GetFileVersionInfoSizeW(wide.as_ptr(), &mut ignored) };
    if size == 0 || size > MAX_VERSION_BYTES {
        return None;
    }
    let mut block = vec![0_u8; size as usize];
    // SAFETY: the buffer is writable for exactly `size` bytes and the path is NUL-terminated.
    if unsafe { GetFileVersionInfoW(wide.as_ptr(), 0, size, block.as_mut_ptr().cast()) } == 0 {
        return None;
    }
    name_from_block(&mut block)
}

/// FileDescription, else ProductName, of a version block: from the string table of its first
/// declared translation (`\VarFileInfo\Translation`), else from the fallback tables.
pub(crate) fn name_from_block(block: &mut [u8]) -> Option<String> {
    let declared = query(block, "\\VarFileInfo\\Translation")
        .map(|(start, len)| translations(&block[start..start.saturating_add(len).min(block.len())]))
        .unwrap_or_default();
    name_from_tables(&declared, |sub_block| {
        let (start, chars) = query(block, sub_block)?;
        let end = start
            .saturating_add(chars.saturating_mul(2))
            .min(block.len());
        Some(utf16_text(&block[start..end]))
    })
}

/// `VerQueryValueW` on a version block: the value's offset in the block and its length (bytes
/// for a binary value, UTF-16 units for a string), or `None` when it is absent or empty.
fn query(block: &mut [u8], sub_block: &str) -> Option<(usize, usize)> {
    let path: Vec<u16> = sub_block.encode_utf16().chain([0]).collect();
    let mut value: *mut c_void = null_mut();
    let mut len = 0_u32;
    // SAFETY: the block is an initialized buffer, exclusively borrowed and unmoved for the call
    // (the API may write into it, as into a GetFileVersionInfoW buffer); the sub-block path is
    // NUL-terminated; both outputs are valid locals. The returned pointer is only compared with
    // the block's bounds below, never dereferenced.
    let found = unsafe {
        VerQueryValueW(
            block.as_mut_ptr().cast_const().cast(),
            path.as_ptr(),
            &mut value,
            &mut len,
        )
    } != 0;
    if !found || value.is_null() || len == 0 {
        return None;
    }
    let start = (value as usize).checked_sub(block.as_ptr() as usize)?;
    (start < block.len()).then_some((start, len as usize))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn align(out: &mut Vec<u8>) {
        while !out.len().is_multiple_of(4) {
            out.push(0);
        }
    }

    /// One node of a compiled version resource: its length, value length and type, its
    /// NUL-terminated UTF-16 key, then its value and children, each aligned on 32 bits.
    fn node(key: &str, text: bool, value: &[u8], value_len: u16, children: &[Vec<u8>]) -> Vec<u8> {
        let mut out = vec![0_u8; 6];
        out.extend(key.encode_utf16().chain([0]).flat_map(u16::to_le_bytes));
        align(&mut out);
        out.extend_from_slice(value);
        for child in children {
            align(&mut out);
            out.extend_from_slice(child);
        }
        let len = u16::try_from(out.len()).expect("small fixture");
        out[0..2].copy_from_slice(&len.to_le_bytes());
        out[2..4].copy_from_slice(&value_len.to_le_bytes());
        out[4..6].copy_from_slice(&u16::from(text).to_le_bytes());
        out
    }

    fn string(key: &str, value: &str) -> Vec<u8> {
        let units: Vec<u16> = value.encode_utf16().chain([0]).collect();
        let bytes: Vec<u8> = units.iter().copied().flat_map(u16::to_le_bytes).collect();
        node(key, true, &bytes, units.len() as u16, &[])
    }

    /// A synthetic version block, as `GetFileVersionInfoW` returns one: string tables keyed by
    /// language and code page, an optional translation list and spare room after the resource.
    fn block(tables: &[(&str, &[(&str, &str)])], translation: Option<&[(u16, u16)]>) -> Vec<u8> {
        let mut fixed = vec![0_u8; 52];
        fixed[0..4].copy_from_slice(&0xFEEF_04BD_u32.to_le_bytes());
        fixed[4..8].copy_from_slice(&0x0001_0000_u32.to_le_bytes());
        let tables: Vec<Vec<u8>> = tables
            .iter()
            .map(|(key, strings)| {
                let strings: Vec<Vec<u8>> = strings.iter().map(|(k, v)| string(k, v)).collect();
                node(key, true, &[], 0, &strings)
            })
            .collect();
        let mut children = vec![node("StringFileInfo", true, &[], 0, &tables)];
        if let Some(pairs) = translation {
            let bytes: Vec<u8> = pairs
                .iter()
                .flat_map(|(language, code_page)| {
                    [language.to_le_bytes(), code_page.to_le_bytes()].concat()
                })
                .collect();
            let var = node("Translation", false, &bytes, bytes.len() as u16, &[]);
            children.push(node("VarFileInfo", true, &[], 0, &[var]));
        }
        let mut out = node("VS_VERSION_INFO", false, &fixed, 52, &children);
        let resource = out.len();
        out.resize(resource * 2, 0);
        out
    }

    #[test]
    fn synthetic_version_blocks_give_the_description_then_the_product_name() {
        let mut chrome = block(
            &[(
                "040904B0",
                &[
                    ("CompanyName", "Google LLC"),
                    ("FileDescription", "Google Chrome"),
                    ("ProductName", "Google Chrome Beta"),
                ],
            )],
            Some(&[(0x0409, 0x04B0)]),
        );
        assert_eq!(
            name_from_block(&mut chrome).as_deref(),
            Some("Google Chrome")
        );
        // No description in the declared French table: its product name.
        let mut french = block(
            &[
                ("040904B0", &[("FileDescription", "English Player")]),
                ("040C04B0", &[("ProductName", "Lecteur multimédia")]),
            ],
            Some(&[(0x040C, 0x04B0)]),
        );
        assert_eq!(
            name_from_block(&mut french).as_deref(),
            Some("Lecteur multimédia")
        );
        // No translation list, or one naming a missing table: the U.S. English fallbacks.
        let mut undeclared = block(&[("040904E4", &[("FileDescription", "Legacy Tool")])], None);
        assert_eq!(
            name_from_block(&mut undeclared).as_deref(),
            Some("Legacy Tool")
        );
        let mut missing = block(
            &[("040904B0", &[("FileDescription", "Fallback Name")])],
            Some(&[(0x0407, 0x04B0)]),
        );
        assert_eq!(
            name_from_block(&mut missing).as_deref(),
            Some("Fallback Name")
        );
        // The value is cleaned like any displayed name.
        let mut spoofed = block(
            &[(
                "040904B0",
                &[("FileDescription", "  Safe\u{202E}  Name\u{7} ")],
            )],
            Some(&[(0x0409, 0x04B0)]),
        );
        assert_eq!(name_from_block(&mut spoofed).as_deref(), Some("Safe Name"));
        let mut blank = block(
            &[("040904B0", &[("FileDescription", " "), ("ProductName", "")])],
            Some(&[(0x0409, 0x04B0)]),
        );
        assert_eq!(name_from_block(&mut blank), None);
        let mut nothing = vec![0_u8; 64];
        assert_eq!(name_from_block(&mut nothing), None);
    }

    #[test]
    fn native_names_need_a_matching_instance_and_a_local_absolute_path() {
        let me = std::process::id();
        let created = times(&OwnedHandle::open(me).expect("own process"))
            .unwrap()
            .0;
        let path = image_path(me, created).expect("own image path");
        assert!(drive_letter(&path).is_some(), "{path}");
        assert!(path.to_lowercase().ends_with(".exe"));
        // Another creation time is another instance, even with the same PID.
        assert_eq!(image_path(me, created + 1), None);
        assert_eq!(image_path(0, 0), None);
        assert_eq!(image_path(4, 0), None);
        let windows = std::env::var("SystemRoot").expect("SystemRoot");
        let explorer = format!(r"{windows}\explorer.exe");
        assert!(version_name(&explorer).is_some_and(|name| !name.is_empty()));
        for refused in ["explorer.exe", r"\\localhost\c$\Windows\explorer.exe"] {
            assert_eq!(version_name(refused), None, "{refused}");
        }
        // The cache is filled through the same reads.
        let names = crate::ProgramNames::new();
        let first = names.name(me, created);
        assert_eq!(names.name(me, created), first);
    }
}
