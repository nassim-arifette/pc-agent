//! Read-only captions/UI Automation for already-observed browser process instances.
use super::{handle_times, OwnedHandle};
use crate::local_browser::{
    BrowserWindowTarget, LocalBrowserWindow, LocalBrowserWindows, LocalTab,
};
use pcagent_shared::Status;
use windows::Win32::{
    Foundation::HWND as AutomationWindow,
    System::{Com::*, Variant::VARIANT},
    UI::Accessibility::*,
};
use windows_sys::Win32::{
    Foundation::{HWND, LPARAM},
    UI::WindowsAndMessaging::{
        EnumWindows, GetWindowTextLengthW, GetWindowTextW, GetWindowThreadProcessId,
        IsWindowVisible,
    },
};

struct Enumeration<'a> {
    targets: &'a [BrowserWindowTarget],
    windows: Vec<(HWND, usize, String)>,
    partial: bool,
}

fn matches(handle: &OwnedHandle, target: &BrowserWindowTarget) -> bool {
    handle_times(handle).is_ok_and(|t| {
        t.created == target.instance.creation_time_filetime && t.exited == 0 && t.created != 0
    })
}

fn owned_by(hwnd: HWND, pid: u32) -> bool {
    let mut owner = 0;
    // SAFETY: hwnd is borrowed from EnumWindows; owner is a valid output.
    unsafe { GetWindowThreadProcessId(hwnd, &mut owner) != 0 && owner == pid }
}

fn caption(hwnd: HWND) -> Option<String> {
    // SAFETY: read-only calls on a borrowed window handle and a correctly-sized UTF-16 buffer.
    // GetWindowText reads another process's cached caption, not arbitrary control contents.
    unsafe {
        let length = GetWindowTextLengthW(hwnd);
        if length <= 0 {
            return None;
        }
        let size = length.checked_add(1)?;
        let mut text = vec![0; usize::try_from(size).ok()?];
        let copied = GetWindowTextW(hwnd, text.as_mut_ptr(), size);
        if copied <= 0 || GetWindowTextLengthW(hwnd) > copied {
            return None;
        }
        Some(String::from_utf16_lossy(&text[..copied as usize]))
    }
}

unsafe extern "system" fn enumerate(hwnd: HWND, parameter: LPARAM) -> windows_sys::core::BOOL {
    // SAFETY: EnumWindows invokes this synchronously with the live exclusive Enumeration pointer.
    let enumeration = unsafe { &mut *(parameter as *mut Enumeration<'_>) };
    // SAFETY: borrowed HWND supplied by EnumWindows.
    if unsafe { IsWindowVisible(hwnd) } == 0 {
        return 1;
    }
    let mut pid = 0;
    // SAFETY: borrowed HWND and a valid output.
    unsafe { GetWindowThreadProcessId(hwnd, &mut pid) };
    let Some((index, target)) = enumeration
        .targets
        .iter()
        .enumerate()
        .find(|(_, t)| t.instance.pid == pid)
    else {
        return 1;
    };
    let Ok(handle) = OwnedHandle::open(pid) else {
        enumeration.partial = true;
        return 1;
    };
    if !matches(&handle, target) {
        enumeration.partial = true;
        return 1;
    }
    if let Some(title) = caption(hwnd) {
        if matches(&handle, target) && owned_by(hwnd, pid) {
            enumeration.windows.push((hwnd, index, title));
        } else {
            enumeration.partial = true;
        }
    }
    1
}

struct Apartment;
impl Drop for Apartment {
    fn drop(&mut self) {
        // SAFETY: this guard exists only after successful CoInitializeEx on this same thread.
        unsafe { CoUninitialize() };
    }
}

fn tab_titles(automation: &IUIAutomation2, hwnd: HWND) -> windows::core::Result<Vec<LocalTab>> {
    // SAFETY: owned COM interfaces, borrowed HWND, a typed integer VARIANT and provider-owned
    // element array. Only Name/Selection properties of TabItem controls are read; no pattern
    // invocation, focus, keyboard input, URL read or browser mutation.
    unsafe {
        let root = automation.ElementFromHandle(AutomationWindow(hwnd))?;
        let condition = automation.CreatePropertyCondition(
            UIA_ControlTypePropertyId,
            &VARIANT::from(UIA_TabItemControlTypeId.0),
        )?;
        let elements = root.FindAll(TreeScope_Descendants, &condition)?;
        let mut tabs = Vec::new();
        for i in 0..elements.Length()? {
            let element = elements.GetElement(i)?;
            tabs.push(LocalTab {
                title: element.CurrentName()?.to_string(),
                selected: element
                    .GetCurrentPatternAs::<IUIAutomationSelectionItemPattern>(
                        UIA_SelectionItemPatternId,
                    )
                    .and_then(|pattern| pattern.CurrentIsSelected())
                    .ok()
                    .map(|v| v.as_bool()),
            });
        }
        Ok(tabs)
    }
}

fn automation() -> Option<(IUIAutomation2, Apartment)> {
    // SAFETY: a balanced MTA initialization on this worker thread, then in-process UI Automation.
    unsafe {
        CoInitializeEx(None, COINIT_MULTITHREADED).ok().ok()?;
        let apartment = Apartment;
        let automation: IUIAutomation2 =
            CoCreateInstance(&CUIAutomation8, None, CLSCTX_INPROC_SERVER).ok()?;
        // Bound individual provider hangs, not the number of windows or tabs returned.
        automation.SetConnectionTimeout(2000).ok()?;
        automation.SetTransactionTimeout(2000).ok()?;
        Some((automation, apartment))
    }
}

pub(crate) fn read(targets: &[BrowserWindowTarget], include_tabs: bool) -> LocalBrowserWindows {
    let mut enumeration = Enumeration {
        targets,
        windows: Vec::new(),
        partial: false,
    };
    // SAFETY: synchronous callback; enumeration stays exclusively borrowed for the whole call.
    let ok = unsafe {
        EnumWindows(
            Some(enumerate),
            (&mut enumeration as *mut Enumeration<'_>) as LPARAM,
        )
    };
    if ok == 0 {
        return LocalBrowserWindows::unavailable(Status::Error);
    }
    let automation = include_tabs.then(automation).flatten();
    let mut output = LocalBrowserWindows::unavailable(if enumeration.partial {
        Status::Partial
    } else {
        Status::Ok
    });
    for (hwnd, index, title) in enumeration.windows {
        let target = &targets[index];
        let handle = match OwnedHandle::open(target.instance.pid) {
            Ok(handle) if matches(&handle, target) && owned_by(hwnd, target.instance.pid) => handle,
            _ => {
                output.status = Status::Partial;
                continue;
            }
        };
        let (tabs, tabs_status) = match &automation {
            Some((uia, _)) => match tab_titles(uia, hwnd) {
                Ok(tabs) if !tabs.is_empty() => (tabs, Status::Ok),
                Ok(_) => (Vec::new(), Status::Unsupported),
                Err(_) => (Vec::new(), Status::Partial),
            },
            None => (Vec::new(), Status::Unsupported),
        };
        if !matches(&handle, target) || !owned_by(hwnd, target.instance.pid) {
            output.status = Status::Partial;
            continue;
        }
        output.windows.push(LocalBrowserWindow {
            process_ref: target.instance.entity_ref.clone(),
            family: target.family,
            title,
            tabs,
            tabs_status,
        });
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_targets_cannot_read_other_application_titles() {
        assert!(read(&[], false).windows.is_empty());
    }
}
