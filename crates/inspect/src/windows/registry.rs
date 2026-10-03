use super::{error, wide};
use crate::{CoverageStatus as C, Inspection};
use serde_json::Value;
use std::ptr::null_mut;
use windows_sys::Win32::{
    Foundation::{ERROR_FILE_NOT_FOUND, ERROR_NO_MORE_ITEMS, ERROR_PATH_NOT_FOUND, ERROR_SUCCESS},
    System::Registry::*,
};

pub(super) struct Key(HKEY);
impl Drop for Key {
    fn drop(&mut self) {
        // SAFETY: this key is owned and used only until drop.
        unsafe {
            RegCloseKey(self.0);
        }
    }
}
impl Key {
    pub(super) fn open(root: HKEY, path: &str, view: u32) -> Result<Option<Self>, u32> {
        let mut key = null_mut();
        // SAFETY: key output and terminated path are valid; rights are read-only.
        let rc = unsafe { RegOpenKeyExW(root, wide(path).as_ptr(), 0, KEY_READ | view, &mut key) };
        match rc {
            ERROR_SUCCESS => Ok(Some(Self(key))),
            ERROR_FILE_NOT_FOUND | ERROR_PATH_NOT_FOUND => Ok(None),
            e => Err(e),
        }
    }
    pub(super) fn child(&self, name: &str) -> Result<Option<Self>, u32> {
        Self::open(self.0, name, 0)
    }
    pub(super) fn names(&self, subkeys: bool, limit: usize) -> Result<(Vec<String>, bool), u32> {
        let mut result = Vec::new();
        for index in 0..=limit as u32 {
            let mut name = [0_u16; 16_384];
            let mut len = name.len() as u32;
            // SAFETY: output name buffer matches capacity; all unneeded outputs are null.
            let rc = unsafe {
                if subkeys {
                    RegEnumKeyExW(
                        self.0,
                        index,
                        name.as_mut_ptr(),
                        &mut len,
                        null_mut(),
                        null_mut(),
                        null_mut(),
                        null_mut(),
                    )
                } else {
                    RegEnumValueW(
                        self.0,
                        index,
                        name.as_mut_ptr(),
                        &mut len,
                        null_mut(),
                        null_mut(),
                        null_mut(),
                        null_mut(),
                    )
                }
            };
            if rc == ERROR_NO_MORE_ITEMS {
                return Ok((result, false));
            }
            if rc != ERROR_SUCCESS {
                return Err(rc);
            }
            if result.len() == limit {
                return Ok((result, true));
            }
            result.push(String::from_utf16_lossy(&name[..len as usize]));
        }
        Ok((result, true))
    }
    pub(super) fn bytes(&self, name: &str, flags: u32) -> Result<Option<Vec<u8>>, u32> {
        let mut buffer = vec![0_u8; 16_384];
        let mut len = buffer.len() as u32;
        // SAFETY: bounded writable byte buffer, valid key and terminated value name.
        let rc = unsafe {
            RegGetValueW(
                self.0,
                std::ptr::null(),
                wide(name).as_ptr(),
                flags,
                null_mut(),
                buffer.as_mut_ptr().cast(),
                &mut len,
            )
        };
        match rc {
            ERROR_SUCCESS => {
                buffer.truncate(len as usize);
                Ok(Some(buffer))
            }
            ERROR_FILE_NOT_FOUND => Ok(None),
            e => Err(e),
        }
    }
    pub(super) fn string(&self, name: &str) -> Option<String> {
        self.bytes(name, RRF_RT_REG_SZ).ok().flatten().map(|v| {
            let units: Vec<u16> = v
                .chunks_exact(2)
                .map(|c| u16::from_le_bytes([c[0], c[1]]))
                .take_while(|c| *c != 0)
                .collect();
            String::from_utf16_lossy(&units)
        })
    }
    pub(super) fn dword(&self, name: &str) -> Option<u32> {
        self.bytes(name, RRF_RT_REG_DWORD)
            .ok()
            .flatten()
            .and_then(|v| Some(u32::from_le_bytes(v.get(..4)?.try_into().ok()?)))
    }
}

pub(super) fn apps(out: &mut Inspection, limit: usize) {
    const PATH: &str = r"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall";
    const SOURCE: &str = "Registry.Uninstall (HKLM/HKCU, 64/32-bit views)";
    let mut emitted = 0;
    for (scope, root, view) in [
        ("machine_64", HKEY_LOCAL_MACHINE, KEY_WOW64_64KEY),
        ("machine_32", HKEY_LOCAL_MACHINE, KEY_WOW64_32KEY),
        ("user_64", HKEY_CURRENT_USER, KEY_WOW64_64KEY),
    ] {
        let key = match Key::open(root, PATH, view) {
            Ok(Some(k)) => k,
            Ok(None) => {
                out.coverage(scope, C::Ok, SOURCE, "Uninstall key absent.");
                continue;
            }
            Err(e) => {
                out.coverage(
                    scope,
                    error(e),
                    SOURCE,
                    "Uninstall entries could not be read.",
                );
                continue;
            }
        };
        let (names, capped) = match key.names(true, 2_000) {
            Ok(v) => v,
            Err(e) => {
                out.coverage(scope, error(e), SOURCE, "Subkey enumeration failed.");
                continue;
            }
        };
        let mut missing = false;
        let mut truncated = capped;
        for name in names {
            let entry = match key.child(&name) {
                Ok(Some(k)) => k,
                _ => {
                    missing = true;
                    continue;
                }
            };
            let Some(display) = entry.string("DisplayName") else {
                continue;
            };
            if emitted >= limit {
                truncated = true;
                break;
            }
            emitted += 1;
            let app = out.entity("app", Some(display));
            if let Some(publisher) = entry.string("Publisher") {
                out.names.insert(format!("{app}-publisher"), publisher);
                out.value(
                    &app,
                    "publisher_ref",
                    format!("{app}-publisher"),
                    "opaque_ref",
                    SOURCE,
                );
            }
            out.value(
                &app,
                "estimated_size_bytes",
                entry
                    .dword("EstimatedSize")
                    .map(|v| v as u64 * 1024)
                    .map(Value::from)
                    .unwrap_or(Value::Null),
                "bytes",
                SOURCE,
            );
            out.value(
                &app,
                "version",
                entry
                    .string("DisplayVersion")
                    .filter(|s| {
                        s.len() < 100
                            && s.chars()
                                .all(|c| c.is_ascii_alphanumeric() || ".-_+ ()".contains(c))
                    })
                    .map(Value::from)
                    .unwrap_or(Value::Null),
                "version",
                SOURCE,
            );
            out.value(
                &app,
                "install_date",
                entry
                    .string("InstallDate")
                    .filter(|s| s.len() == 8 && s.bytes().all(|b| b.is_ascii_digit()))
                    .map(Value::from)
                    .unwrap_or(Value::Null),
                "yyyymmdd",
                SOURCE,
            );
            out.value(&app, "installation_scope", scope, "category", SOURCE);
        }
        out.coverage(scope,if missing || truncated { C::Partial } else { C::Ok },SOURCE, if truncated { "Result limit reached; sizes are publisher estimates, not measured disk usage." } else if missing { "Some entries were unreadable; sizes are publisher estimates." } else { "Installer registrations; sizes are publisher estimates. Portable and Store applications may be absent." });
    }
    out.coverage(
        "store_and_portable_apps",
        C::Unsupported,
        SOURCE,
        "Uninstall keys do not provide a complete inventory of Store or portable applications.",
    );
    out.coverage("application_registration_semantics",C::Ok,SOURCE,"HKCU Software is shared between registry views and is enumerated once. InstallDate can mean the most recent repair or servicing date rather than the original installation date.");
}
