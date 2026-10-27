use super::*;
use crate::{BrowserManager, BrowserManagerTarget};
use windows_sys::Win32::{
    System::Threading::QueryFullProcessImageNameW,
    UI::WindowsAndMessaging::{
        GetClassNameW, SendMessageTimeoutW, SMTO_ABORTIFHUNG, SMTO_BLOCK, WM_COMMAND,
    },
};

fn executable(handle: &OwnedHandle) -> Result<PathBuf, Status> {
    let mut text = vec![0_u16; 32768];
    let mut length = text.len() as u32;
    // SAFETY: query-only process handle; the writable buffer and its length agree.
    if unsafe { QueryFullProcessImageNameW(handle.0, 0, text.as_mut_ptr(), &mut length) } == 0 {
        return Err(status_of(last_error()));
    }
    let path = PathBuf::from(String::from_utf16_lossy(&text[..length as usize]));
    if !crate::is_windows_absolute(&path) {
        return Err(Status::Partial);
    }
    Ok(path)
}

pub(crate) fn observe(process: &ProcessTarget) -> Result<Option<BrowserManagerTarget>, Status> {
    let Some((handle, current)) = open_process(process.identity.pid)? else {
        return Ok(None);
    };
    if !current.same_instance(&process.identity) {
        return Ok(None);
    }
    let path = executable(&handle)?;
    let Some(destination) = path
        .file_name()
        .and_then(|s| s.to_str())
        .and_then(BrowserManager::from_executable)
    else {
        return Ok(None);
    };
    let Some(identity) = observe_file(&path)? else {
        return Ok(None);
    };
    let target = BrowserManagerTarget {
        process: process.clone(),
        executable: FileTarget { path, identity },
        destination,
    };
    // SAFETY: the retained query/synchronize handle is valid; an exited process is never reused.
    if !target.valid() || unsafe { WaitForSingleObject(handle.0, 0) } == WAIT_OBJECT_0 {
        return Ok(None);
    }
    Ok(Some(target))
}

pub(crate) fn open(target: &BrowserManagerTarget) -> Result<EffectOutcome, EffectError> {
    let err = |s| EffectError::new(s, "browser identity could not be verified");
    let Some((process, current)) = open_process(target.process.identity.pid).map_err(err)? else {
        return Ok(EffectOutcome::Refused(Refusal::TargetMissing));
    };
    if !target.valid()
        || !current.same_instance(&target.process.identity)
        || executable(&process).map_err(err)? != target.executable.path
    {
        return Ok(EffectOutcome::Refused(Refusal::ProcessReplaced));
    }
    // Pin the reviewed executable against replacement or writes during dispatch. Unlike file
    // mutation actions this read-only action may target a browser installed in Program Files.
    let file = open_with(
        &target.executable.path,
        FILE_READ_ATTRIBUTES,
        FILE_SHARE_READ,
    )
    .map_err(|code| err(status_of(code)))?;
    if identity(&file, &target.executable.path).map_err(err)? != target.executable.identity {
        return Ok(EffectOutcome::Refused(Refusal::TargetReplaced));
    }
    let mut search = WindowSearch {
        pid: current.pid,
        windows: Vec::new(),
    };
    // SAFETY: synchronous enumeration with a live stack-owned search object.
    unsafe {
        EnumWindows(
            Some(collect_window),
            &mut search as *mut WindowSearch as LPARAM,
        );
    }
    if search.windows.is_empty() {
        return Ok(EffectOutcome::Refused(Refusal::NoTopLevelWindow));
    }
    if target.destination == BrowserManager::FirefoxProcesses {
        // No shell or PATH lookup, no inherited profile command line, only fixed arguments.
        Command::new(&target.executable.path)
            .args(["-new-tab", "about:processes"])
            .spawn()
            .map_err(|_| EffectError::new(Status::Error, "Firefox manager request failed"))?;
        return Ok(EffectOutcome::Done(Some(
            "requested Firefox about:processes in a new tab; no process was closed".into(),
        )));
    }
    for window in search.windows {
        let mut class = [0_u16; 128];
        let mut pid = 0;
        // SAFETY: Win32 validates HWNDs; writable output buffers are valid for these calls.
        let count = unsafe {
            GetWindowThreadProcessId(window, &mut pid);
            GetClassNameW(window, class.as_mut_ptr(), class.len() as i32)
        };
        if pid != current.pid
            || count <= 0
            || String::from_utf16_lossy(&class[..count as usize]) != "Chrome_WidgetWin_1"
        {
            continue;
        }
        // Chromium's fixed IDC_TASK_MANAGER (chrome/app/chrome_command_ids.h), routed by
        // HWNDMessageHandler::OnCommand -> BrowserView::ExecuteWindowsCommand. Targeted message,
        // never global keyboard injection; the model cannot choose the command or the window.
        let mut result = 0;
        // SAFETY: scalar-only WM_COMMAND; no pointers cross processes. The retained process and
        // rechecked HWND ownership restrict the request; hung receivers time out.
        if unsafe {
            SendMessageTimeoutW(
                window,
                WM_COMMAND,
                40006,
                0,
                SMTO_ABORTIFHUNG | SMTO_BLOCK,
                2000,
                &mut result,
            )
        } != 0
        {
            return Ok(EffectOutcome::Done(Some(
                "requested browser Task Manager; no process was closed; confirm in the browser"
                    .into(),
            )));
        }
    }
    Err(EffectError::new(
        Status::Partial,
        "no responsive supported browser window",
    ))
}
