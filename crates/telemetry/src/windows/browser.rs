//! Read-only, ephemeral command-line classification. No raw arguments cross this module.
use super::{filetime, OwnedHandle};
use crate::browser::{self, BrowserClassification};
use ::windows::{
    core::{BSTR, PCWSTR},
    Win32::System::{Com::*, Variant::*, Wmi::*},
};
use pcagent_shared::Status;
use std::{
    ffi::c_void,
    mem::{size_of, zeroed},
};
use windows_sys::Win32::{
    Foundation::{LocalFree, FILETIME, HANDLE},
    System::Threading::GetProcessTimes,
    UI::Shell::CommandLineToArgvW,
};

// ProcessCommandLineInformation is the read-only information class 60 on Windows 8.1+.
#[link(name = "ntdll")]
unsafe extern "system" {
    fn NtQueryInformationProcess(
        handle: HANDLE,
        class: u32,
        buffer: *mut c_void,
        size: u32,
        returned: *mut u32,
    ) -> i32;
}

#[repr(C)]
#[derive(Clone, Copy)]
struct UnicodeString {
    length: u16,
    maximum_length: u16,
    buffer: usize,
}

fn matches(handle: &OwnedHandle, expected: u64) -> bool {
    // SAFETY: live query-only handle and four initialized FILETIME outputs.
    unsafe {
        let (mut created, mut exited, mut kernel, mut user): (
            FILETIME,
            FILETIME,
            FILETIME,
            FILETIME,
        ) = (zeroed(), zeroed(), zeroed(), zeroed());
        GetProcessTimes(handle.0, &mut created, &mut exited, &mut kernel, &mut user) != 0
            && expected != 0
            && filetime(created) == expected
            && filetime(exited) == 0
    }
}

fn tokenize_and_classify(image: &str, text: &[u16]) -> Option<BrowserClassification> {
    if text.is_empty() || text.contains(&0) {
        return None;
    }
    let terminated: Vec<_> = text.iter().copied().chain(Some(0)).collect();
    let mut count = 0;
    // SAFETY: terminated is NUL-terminated; count is writable. LocalFree owns the returned block.
    let argv = unsafe { CommandLineToArgvW(terminated.as_ptr(), &mut count) };
    if argv.is_null() || count <= 0 {
        return None;
    }
    let mut args = Vec::new();
    for index in 0..count as usize {
        // SAFETY: CommandLineToArgvW returns `count` NUL-terminated pointers in its owned block.
        let arg = unsafe {
            let p = *argv.add(index);
            let mut len = 0;
            while *p.add(len) != 0 {
                len += 1;
            }
            String::from_utf16_lossy(std::slice::from_raw_parts(p, len))
        };
        args.push(arg);
    }
    let result = browser::classify(image, &args);
    // SAFETY: argv was allocated by CommandLineToArgvW and is released exactly once.
    unsafe {
        LocalFree(argv.cast());
    }
    Some(result)
}

fn native(handle: &OwnedHandle, image: &str) -> Option<BrowserClassification> {
    // UNICODE_STRING length is a u16 byte count; this buffer fits the whole legal command line.
    let mut words = vec![0_u64; (65536 + size_of::<UnicodeString>()).div_ceil(8)];
    let mut returned = 0;
    // SAFETY: aligned writable buffer with its real size, query-only handle and valid size output.
    let status = unsafe {
        NtQueryInformationProcess(
            handle.0,
            60,
            words.as_mut_ptr().cast(),
            (words.len() * 8) as u32,
            &mut returned,
        )
    };
    if status < 0 {
        return None;
    }
    let available = usize::try_from(returned).ok()?.min(words.len() * 8);
    if available < size_of::<UnicodeString>() {
        return None;
    }
    // SAFETY: the buffer is aligned, initialized and holds at least the header.
    let header = unsafe { words.as_ptr().cast::<UnicodeString>().read() };
    let start = header.buffer.checked_sub(words.as_ptr() as usize)?;
    let bytes = usize::from(header.length);
    if bytes == 0
        || bytes % 2 != 0
        || header.buffer % 2 != 0
        || header.length > header.maximum_length
        || start < size_of::<UnicodeString>()
        || start.checked_add(bytes)? > available
    {
        return None;
    }
    // SAFETY: the pointer is aligned and the entire UTF-16 slice is inside the initialized buffer.
    let text = unsafe { std::slice::from_raw_parts(header.buffer as *const u16, bytes / 2) };
    let result = tokenize_and_classify(image, text);
    words.fill(0);
    result
}

struct Apartment;
impl Apartment {
    fn new() -> Option<Self> {
        // SAFETY: scoped initialization on the collecting thread, balanced only on success.
        unsafe {
            CoInitializeEx(None, COINIT_MULTITHREADED).ok().ok()?;
        }
        Some(Self)
    }
}
impl Drop for Apartment {
    fn drop(&mut self) {
        // SAFETY: this instance represents exactly one successful initialization on this thread.
        unsafe {
            CoUninitialize();
        }
    }
}

struct Wmi {
    service: IWbemServices,
    _apartment: Apartment,
}
impl Wmi {
    fn new() -> Option<Self> {
        let apartment = Apartment::new()?;
        let empty = BSTR::new();
        // SAFETY: COM initialized, strings owned through the calls; no remote server or credentials.
        let service = unsafe {
            let locator: IWbemLocator =
                CoCreateInstance(&WbemLocator, None, CLSCTX_INPROC_SERVER).ok()?;
            let service = locator
                .ConnectServer(
                    &BSTR::from("root\\cimv2"),
                    &empty,
                    &empty,
                    &empty,
                    WBEM_FLAG_CONNECT_USE_MAX_WAIT.0,
                    &empty,
                    None,
                )
                .ok()?;
            CoSetProxyBlanket(
                &service,
                10,
                0,
                PCWSTR::null(),
                RPC_C_AUTHN_LEVEL_CALL,
                RPC_C_IMP_LEVEL_IMPERSONATE,
                None,
                EOAC_NONE,
            )
            .ok()?;
            service
        };
        Some(Self {
            service,
            _apartment: apartment,
        })
    }

    fn classify(&self, pid: u32, image: &str) -> Option<BrowserClassification> {
        // SAFETY: retained COM interface; fixed WQL except an integer PID, initialized outputs.
        unsafe {
            let query = self
                .service
                .ExecQuery(
                    &BSTR::from("WQL"),
                    &BSTR::from(format!(
                        "SELECT CommandLine FROM Win32_Process WHERE ProcessId={pid}"
                    )),
                    WBEM_FLAG_FORWARD_ONLY | WBEM_FLAG_RETURN_IMMEDIATELY,
                    None,
                )
                .ok()?;
            let mut objects = [None];
            let mut returned = 0;
            query.Next(500, &mut objects, &mut returned).ok().ok()?;
            let object = objects[0].take()?;
            let mut value = VARIANT::default();
            let read = object.Get(
                ::windows::core::w!("CommandLine"),
                0,
                &mut value,
                None,
                None,
            );
            let result = if read.is_ok() && value.Anonymous.Anonymous.vt == VT_BSTR {
                let text = &value.Anonymous.Anonymous.Anonymous.bstrVal;
                tokenize_and_classify(image, text)
            } else {
                None
            };
            // Clear the only provider-owned raw value before returning the closed classification.
            let _ = VariantClear(&mut value);
            result
        }
    }
}

/// A collection-scoped connection, opened lazily only if the native API cannot read arguments.
#[derive(Default)]
pub(super) struct Classifier {
    wmi: Option<Option<Wmi>>,
}
impl Classifier {
    pub(super) fn classify(
        &mut self,
        pid: u32,
        created: u64,
        image: &str,
    ) -> BrowserClassification {
        let handle = match OwnedHandle::open(pid) {
            Ok(handle) => handle,
            Err(status) => return browser::unavailable(image, status),
        };
        if !matches(&handle, created) {
            return browser::unavailable(image, Status::Stale);
        }
        let result = native(&handle, image).or_else(|| {
            self.wmi
                .get_or_insert_with(Wmi::new)
                .as_ref()?
                .classify(pid, image)
        });
        if !matches(&handle, created) {
            return browser::unavailable(image, Status::Stale);
        }
        result.unwrap_or_else(|| browser::unavailable(image, Status::Partial))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_quoting_does_not_turn_titles_or_paths_into_switches() {
        let text: Vec<u16> = r#""C:\Program Files\Browser\chrome.exe" --type=renderer "--title=PRIVATE --type=gpu-process" "#.encode_utf16().collect();
        let result = tokenize_and_classify("chrome.exe", &text).unwrap();
        assert_eq!(
            result.process_type,
            crate::browser::BrowserProcessType::Renderer
        );
        assert!(!format!("{result:?}").contains("PRIVATE"));
    }

    #[test]
    fn native_current_process_and_identity_revalidation() {
        let rows = super::super::sysproc::query().unwrap();
        let own = rows.iter().find(|p| p.pid == std::process::id()).unwrap();
        let handle = OwnedHandle::open(own.pid).unwrap();
        assert!(native(&handle, "ordinary.exe").is_some());
        let mut classifier = Classifier::default();
        assert_eq!(
            classifier
                .classify(own.pid, own.created + 1, "chrome.exe")
                .status,
            Status::Stale
        );
    }
}
