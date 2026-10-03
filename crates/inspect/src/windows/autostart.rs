use super::{registry::Key, wide};
use crate::autostart::{command, Error};
use std::{
    path::Path,
    ptr::{null, null_mut},
};
use windows_sys::Win32::{
    Foundation::{ERROR_ACCESS_DENIED, ERROR_FILE_NOT_FOUND, ERROR_SUCCESS},
    System::Registry::*,
};

const PATH: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const VALUE: &str = "PcAgent";
fn failure(code: u32) -> Error {
    if code == ERROR_ACCESS_DENIED {
        Error::PermissionDenied
    } else {
        Error::Registry
    }
}
pub(crate) fn get() -> Result<bool, Error> {
    let key = Key::open(HKEY_CURRENT_USER, PATH, KEY_WOW64_64KEY).map_err(failure)?;
    match key {
        Some(key) => key
            .bytes(VALUE, RRF_RT_REG_SZ)
            .map(|value| {
                value.is_some_and(|bytes| bytes.get(..2).is_some_and(|first| first != [0, 0]))
            })
            .map_err(failure),
        None => Ok(false),
    }
}
pub(crate) fn set(enabled: bool, executable: &Path) -> Result<(), Error> {
    let current = std::env::current_exe()
        .map_err(|_| Error::InvalidExecutable)?
        .canonicalize()
        .map_err(|_| Error::InvalidExecutable)?;
    let supplied = executable
        .canonicalize()
        .map_err(|_| Error::InvalidExecutable)?;
    if current != supplied {
        return Err(Error::InvalidExecutable);
    }
    // canonicalize uses the extended-length prefix on Windows; a normal drive path is needed
    // by the Run command-line parser. UNC and device paths remain rejected by command().
    let path = supplied.to_str().ok_or(Error::InvalidExecutable)?;
    let path = path.strip_prefix(r"\\?\").unwrap_or(path);
    let value = command(path)?;
    let mut key = null_mut();
    // SAFETY: current-user Run key only, terminated constant path, writable key output;
    // access is limited to this application's opt-in local setting.
    let opened = unsafe {
        RegCreateKeyExW(
            HKEY_CURRENT_USER,
            wide(PATH).as_ptr(),
            0,
            null(),
            REG_OPTION_NON_VOLATILE,
            KEY_SET_VALUE | KEY_WOW64_64KEY,
            null(),
            &mut key,
            null_mut(),
        )
    };
    if opened != ERROR_SUCCESS {
        return Err(failure(opened));
    }
    let result = if enabled {
        let encoded = wide(&value);
        // SAFETY: complete terminated UTF-16 command bytes, one fixed registry value.
        unsafe {
            RegSetValueExW(
                key,
                wide(VALUE).as_ptr(),
                0,
                REG_SZ,
                encoded.as_ptr().cast(),
                (encoded.len() * 2) as u32,
            )
        }
    } else {
        // SAFETY: deletes only this application's fixed value after explicit local opt-out.
        unsafe { RegDeleteValueW(key, wide(VALUE).as_ptr()) }
    };
    // SAFETY: close the uniquely owned key returned by RegCreateKeyExW.
    unsafe {
        RegCloseKey(key);
    }
    if result == ERROR_SUCCESS || !enabled && result == ERROR_FILE_NOT_FOUND {
        Ok(())
    } else {
        Err(failure(result))
    }
}
