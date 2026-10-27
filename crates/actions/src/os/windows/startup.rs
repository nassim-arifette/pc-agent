//! Fixed HKCU Run/StartupApproved paths. A registry transaction binds revalidation and the write.
use super::{filetime, last_error, status_of, wide, OwnedHandle};
use crate::{EffectError, EffectOutcome, Refusal, StartupApproval, StartupIdentity, StartupTarget};
use pcagent_shared::Status;
use std::{
    ffi::OsStr,
    ptr::{null, null_mut},
};
use windows_sys::Win32::{
    Foundation::{
        ERROR_FILE_NOT_FOUND, ERROR_MORE_DATA, ERROR_NO_MORE_ITEMS, ERROR_SUCCESS,
        ERROR_TRANSACTIONAL_CONFLICT, FILETIME, HANDLE, INVALID_HANDLE_VALUE,
    },
    Storage::FileSystem::{CommitTransaction, CreateTransaction},
    System::Registry::{
        RegCloseKey, RegCreateKeyTransactedW, RegDeleteValueW, RegEnumValueW, RegOpenKeyExW,
        RegOpenKeyTransactedW, RegQueryInfoKeyW, RegQueryValueExW, RegSetValueExW, HKEY,
        HKEY_CURRENT_USER, KEY_QUERY_VALUE, KEY_SET_VALUE, KEY_WOW64_64KEY, REG_BINARY,
        REG_OPTION_NON_VOLATILE,
    },
};

const RUN: &str = r"SOFTWARE\Microsoft\Windows\CurrentVersion\Run";
const APPROVED: &str = r"SOFTWARE\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run";
const MAX_RUN_VALUES: u32 = 1024;

struct Key(HKEY);
impl Drop for Key {
    fn drop(&mut self) {
        // SAFETY: this wrapper owns a nonpredefined handle returned by RegOpen/CreateKey.
        unsafe {
            RegCloseKey(self.0);
        }
    }
}
fn open(
    path: &str,
    access: u32,
    transaction: Option<HANDLE>,
    create: bool,
) -> Result<Option<Key>, u32> {
    let path =
        wide(OsStr::new(path)).ok_or(windows_sys::Win32::Foundation::ERROR_INVALID_PARAMETER)?;
    let mut key = null_mut();
    // SAFETY: the only callers supply fixed HKCU paths; UTF-16 strings and outputs remain valid.
    // Transaction handles are owned by the caller, with no impersonation or elevated privileges.
    let code = unsafe {
        match (transaction, create) {
            (Some(transaction), true) => RegCreateKeyTransactedW(
                HKEY_CURRENT_USER,
                path.as_ptr(),
                0,
                null(),
                REG_OPTION_NON_VOLATILE,
                access | KEY_WOW64_64KEY,
                null(),
                &mut key,
                null_mut(),
                transaction,
                null(),
            ),
            (Some(transaction), false) => RegOpenKeyTransactedW(
                HKEY_CURRENT_USER,
                path.as_ptr(),
                0,
                access | KEY_WOW64_64KEY,
                &mut key,
                transaction,
                null(),
            ),
            (None, false) => RegOpenKeyExW(
                HKEY_CURRENT_USER,
                path.as_ptr(),
                0,
                access | KEY_WOW64_64KEY,
                &mut key,
            ),
            (None, true) => return Err(windows_sys::Win32::Foundation::ERROR_INVALID_PARAMETER),
        }
    };
    match code {
        ERROR_SUCCESS => Ok(Some(Key(key))),
        ERROR_FILE_NOT_FOUND => Ok(None),
        code => Err(code),
    }
}
fn last_write(key: &Key) -> Result<u64, Status> {
    let mut time = FILETIME {
        dwLowDateTime: 0,
        dwHighDateTime: 0,
    };
    // SAFETY: output is a valid FILETIME; every optional output is NULL. No value data is read.
    let code = unsafe {
        RegQueryInfoKeyW(
            key.0,
            null_mut(),
            null_mut(),
            null(),
            null_mut(),
            null_mut(),
            null_mut(),
            null_mut(),
            null_mut(),
            null_mut(),
            null_mut(),
            &mut time,
        )
    };
    if code == ERROR_SUCCESS {
        Ok(filetime(time))
    } else {
        Err(status_of(code))
    }
}
fn contains_name(key: &Key, expected: &str) -> Result<bool, Status> {
    for index in 0..MAX_RUN_VALUES {
        let mut name = [0_u16; 257];
        let mut length = name.len() as u32;
        // SAFETY: the fixed output is sized by length; NULL data outputs enumerate names only.
        let code = unsafe {
            RegEnumValueW(
                key.0,
                index,
                name.as_mut_ptr(),
                &mut length,
                null(),
                null_mut(),
                null_mut(),
                null_mut(),
            )
        };
        match code {
            ERROR_SUCCESS => {
                let name =
                    String::from_utf16(&name[..length as usize]).map_err(|_| Status::Error)?;
                if name.eq_ignore_ascii_case(expected) {
                    return Ok(true);
                }
            }
            ERROR_NO_MORE_ITEMS => return Ok(false),
            ERROR_MORE_DATA => return Err(Status::Unsupported),
            code => return Err(status_of(code)),
        }
    }
    Err(Status::Partial)
}
fn approval(key: Option<&Key>, value_name: &str) -> Result<StartupApproval, Status> {
    let Some(key) = key else {
        return Ok(StartupApproval::Missing);
    };
    let name = wide(OsStr::new(value_name)).ok_or(Status::Unsupported)?;
    let mut data = [0_u8; 12];
    let mut length = data.len() as u32;
    let mut value_type = 0;
    // SAFETY: bounded output and NUL-terminated name; only this approval blob is read, never Run.
    let code = unsafe {
        RegQueryValueExW(
            key.0,
            name.as_ptr(),
            null(),
            &mut value_type,
            data.as_mut_ptr(),
            &mut length,
        )
    };
    match code {
        ERROR_FILE_NOT_FOUND => Ok(StartupApproval::Missing),
        ERROR_SUCCESS if value_type == REG_BINARY && length == 12 => {
            StartupApproval::from_bytes(Some(&data))
        }
        ERROR_SUCCESS | ERROR_MORE_DATA => Err(Status::Unsupported),
        code => Err(status_of(code)),
    }
}

pub(crate) fn observe(value_name: &str) -> Result<Option<StartupIdentity>, Status> {
    if !crate::startup::valid_name(value_name) {
        return Err(Status::Unsupported);
    }
    let Some(run) = open(RUN, KEY_QUERY_VALUE, None, false).map_err(status_of)? else {
        return Ok(None);
    };
    let before = last_write(&run)?;
    if !contains_name(&run, value_name)? {
        return Ok(None);
    }
    let approved = open(APPROVED, KEY_QUERY_VALUE, None, false).map_err(status_of)?;
    let approval = approval(approved.as_ref(), value_name)?;
    if before != last_write(&run)? {
        return Err(Status::Partial);
    }
    Ok(Some(StartupIdentity {
        run_key_last_write_filetime: before,
        approval,
    }))
}

fn error(status: Status) -> EffectError {
    EffectError::new(
        status,
        "user startup transaction could not be completed; no fallback write was attempted",
    )
}
pub(crate) fn apply(target: &StartupTarget) -> Result<EffectOutcome, EffectError> {
    if !crate::startup::valid_name(target.value_name()) || !target.desired_approval().known() {
        return Ok(EffectOutcome::Refused(Refusal::StartupEntryChanged));
    }
    // SAFETY: default isolation/security, no identifier or description, bounded five-second timeout.
    // Closing the owned handle before commit automatically rolls back all uncommitted changes.
    let transaction = unsafe { CreateTransaction(null_mut(), null_mut(), 0, 0, 0, 5000, null()) };
    if transaction == INVALID_HANDLE_VALUE || transaction.is_null() {
        return Err(error(status_of(last_error())));
    }
    let transaction = OwnedHandle(transaction);
    let Some(run) = open(RUN, KEY_QUERY_VALUE, Some(transaction.0), false)
        .map_err(|code| error(status_of(code)))?
    else {
        return Ok(EffectOutcome::Refused(Refusal::TargetMissing));
    };
    if last_write(&run).map_err(error)? != target.identity().run_key_last_write_filetime
        || !contains_name(&run, target.value_name()).map_err(error)?
    {
        return Ok(EffectOutcome::Refused(Refusal::StartupEntryChanged));
    }
    let approved = open(
        APPROVED,
        KEY_QUERY_VALUE | KEY_SET_VALUE,
        Some(transaction.0),
        true,
    )
    .map_err(|code| error(status_of(code)))?
    .ok_or_else(|| error(Status::Error))?;
    if approval(Some(&approved), target.value_name()).map_err(error)? != target.identity().approval
    {
        return Ok(EffectOutcome::Refused(Refusal::StartupEntryChanged));
    }
    let name = wide(OsStr::new(target.value_name())).ok_or_else(|| error(Status::Unsupported))?;
    // SAFETY: exact bounded desired value from the broker's validated, approved local target.
    // The handle is transactional, so a conflicting external change rolls this write back.
    let code = unsafe {
        match target.desired_approval() {
            StartupApproval::Missing => RegDeleteValueW(approved.0, name.as_ptr()),
            StartupApproval::Binary(bytes) => RegSetValueExW(
                approved.0,
                name.as_ptr(),
                0,
                REG_BINARY,
                bytes.as_ptr(),
                bytes.len() as u32,
            ),
        }
    };
    if code != ERROR_SUCCESS {
        return Err(error(status_of(code)));
    }
    drop(approved);
    drop(run);
    // SAFETY: transaction is valid and owns the sole transaction handle, all registry calls finished.
    if unsafe { CommitTransaction(transaction.0) } == 0 {
        let code = last_error();
        if code == ERROR_TRANSACTIONAL_CONFLICT {
            return Ok(EffectOutcome::Refused(Refusal::StartupEntryChanged));
        }
        return Err(error(status_of(code)));
    }
    Ok(EffectOutcome::Done(Some(
        if target.desired_approval().enabled() == Some(false) {
            "user startup approval disabled"
        } else {
            "previous user startup approval restored"
        }
        .into(),
    )))
}
