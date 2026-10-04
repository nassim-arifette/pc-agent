//! One fixed, private E3 export, owned by the locally approved privileged helper.
//! No shell, PATH search, arbitrary arguments, inherited credentials or SRUM changes.
use super::{
    energy::{parse_export, EnergyLimit, EnergySummary, MAX_EXPORT_BYTES},
    native::{is_elevated, NativeJob},
    protocol::{ElevationError, ElevationStatus},
    system::valid_image_path,
};
use std::{
    fs::File,
    mem::size_of,
    os::windows::ffi::OsStrExt,
    path::{Path, PathBuf},
    ptr::{null, null_mut},
    time::{Duration, Instant},
};
use windows_sys::Win32::{
    Foundation::{
        CloseHandle, GetLastError, HANDLE, INVALID_HANDLE_VALUE, WAIT_OBJECT_0, WAIT_TIMEOUT,
    },
    Storage::FileSystem::{
        CreateFileW, FileAttributeTagInfo, GetFileInformationByHandleEx, FILE_ATTRIBUTE_DIRECTORY,
        FILE_ATTRIBUTE_REPARSE_POINT, FILE_ATTRIBUTE_TAG_INFO, FILE_FLAG_BACKUP_SEMANTICS,
        FILE_FLAG_OPEN_REPARSE_POINT, FILE_READ_ATTRIBUTES, FILE_SHARE_READ, FILE_SHARE_WRITE,
        OPEN_EXISTING,
    },
    System::{
        JobObjects::{
            AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
            SetInformationJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
            JOB_OBJECT_LIMIT_ACTIVE_PROCESS, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        },
        SystemInformation::{GetSystemDirectoryW, GetWindowsDirectoryW},
        Threading::{
            CreateProcessW, GetExitCodeProcess, ResumeThread, TerminateProcess,
            WaitForSingleObject, CREATE_NO_WINDOW, CREATE_SUSPENDED, CREATE_UNICODE_ENVIRONMENT,
            PROCESS_INFORMATION, STARTF_USESTDHANDLES, STARTUPINFOW,
        },
    },
};

struct Handle(HANDLE);
impl Drop for Handle {
    fn drop(&mut self) {
        // SAFETY: this module owns the valid, non-inherited handle until this drop.
        unsafe { CloseHandle(self.0) };
    }
}

fn wide_path(path: &Path) -> Result<Vec<u16>, ElevationError> {
    let units: Vec<u16> = path.as_os_str().encode_wide().collect();
    if units.is_empty()
        || units.len() > 16_000
        || units.contains(&0)
        || units.contains(&(b'"' as u16))
    {
        return Err(ElevationError::InvalidArguments);
    }
    Ok(units.into_iter().chain(Some(0)).collect())
}

fn os_directory(system: bool) -> Result<PathBuf, ElevationError> {
    let mut buffer = vec![0_u16; 1025];
    // SAFETY: bounded writable UTF-16 output; neither directory comes from inherited environment/PATH.
    let length = unsafe {
        if system {
            GetSystemDirectoryW(buffer.as_mut_ptr(), buffer.len() as u32)
        } else {
            GetWindowsDirectoryW(buffer.as_mut_ptr(), buffer.len() as u32)
        }
    } as usize;
    if length == 0 || length >= buffer.len() {
        return Err(ElevationError::Unsupported);
    }
    let text = String::from_utf16(&buffer[..length]).map_err(|_| ElevationError::Unsupported)?;
    if !valid_image_path(&text) {
        return Err(ElevationError::Unsupported);
    }
    Ok(PathBuf::from(text))
}

/// Deny ancestor replacement and deny executable replacement/writes during the launch.
fn pin_program(system: &Path) -> Result<Vec<Handle>, ElevationError> {
    let text = system
        .to_str()
        .filter(|value| valid_image_path(value))
        .ok_or(ElevationError::Unsupported)?;
    let mut path = PathBuf::from(format!("{}\\", &text[..2]));
    let mut pinned = Vec::new();
    for component in std::iter::once("")
        .chain(text[3..].split('\\'))
        .chain(std::iter::once("powercfg.exe"))
    {
        if !component.is_empty() {
            path.push(component);
        }
        let program = component == "powercfg.exe";
        let name = wide_path(&path)?;
        // SAFETY: allowlisted absolute inbox path; reparse metadata is inspected on the retained handle.
        // Ancestors deny delete sharing. The executable also denies write sharing throughout CreateProcessW.
        let raw = unsafe {
            CreateFileW(
                name.as_ptr(),
                FILE_READ_ATTRIBUTES,
                FILE_SHARE_READ | if program { 0 } else { FILE_SHARE_WRITE },
                null(),
                OPEN_EXISTING,
                FILE_FLAG_OPEN_REPARSE_POINT
                    | if program {
                        0
                    } else {
                        FILE_FLAG_BACKUP_SEMANTICS
                    },
                null_mut(),
            )
        };
        if raw == INVALID_HANDLE_VALUE {
            return Err(std::io::Error::last_os_error().into());
        }
        let handle = Handle(raw);
        let mut attributes = FILE_ATTRIBUTE_TAG_INFO::default();
        // SAFETY: exact writable metadata structure and size for this retained handle.
        if unsafe {
            GetFileInformationByHandleEx(
                handle.0,
                FileAttributeTagInfo,
                (&mut attributes as *mut FILE_ATTRIBUTE_TAG_INFO).cast(),
                size_of::<FILE_ATTRIBUTE_TAG_INFO>() as u32,
            )
        } == 0
        {
            return Err(std::io::Error::last_os_error().into());
        }
        if attributes.FileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0
            || (attributes.FileAttributes & FILE_ATTRIBUTE_DIRECTORY != 0) == program
        {
            return Err(ElevationError::InsecureJob);
        }
        pinned.push(handle);
    }
    Ok(pinned)
}

/// The command's only variable is the NativeJob-owned fixed export path.
fn command_line(program: &Path, output: &Path) -> Result<Vec<u16>, ElevationError> {
    let program = wide_path(program)?;
    let output = wide_path(output)?;
    let quote = b'"' as u16;
    let mut command = Vec::with_capacity(program.len() + output.len() + 32);
    command.push(quote);
    command.extend_from_slice(&program[..program.len() - 1]);
    command.extend("\" /srumutil /output \"".encode_utf16());
    command.extend_from_slice(&output[..output.len() - 1]);
    command.extend("\" /csv".encode_utf16());
    if command.len() >= 32_767 {
        return Err(ElevationError::InvalidArguments);
    }
    command.push(0);
    Ok(command)
}

struct Writer {
    job: Option<Handle>,
    process: Handle,
    thread: Handle,
    exited: bool,
}
impl Writer {
    fn spawn(
        program: &Path,
        output: &Path,
        system: &Path,
        windows: &Path,
    ) -> Result<Self, ElevationError> {
        // SAFETY: private unnamed job; no descriptor/handle inheritance or globally shared job name.
        let raw_job = unsafe { CreateJobObjectW(null(), null()) };
        if raw_job.is_null() {
            return Err(std::io::Error::last_os_error().into());
        }
        let job = Handle(raw_job);
        let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        limits.BasicLimitInformation.LimitFlags =
            JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE | JOB_OBJECT_LIMIT_ACTIVE_PROCESS;
        limits.BasicLimitInformation.ActiveProcessLimit = 1;
        // SAFETY: valid owned job and exact information structure; closing the last handle kills its child.
        if unsafe {
            SetInformationJobObject(
                job.0,
                JobObjectExtendedLimitInformation,
                (&limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
                size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            )
        } == 0
        {
            return Err(std::io::Error::last_os_error().into());
        }
        let name = wide_path(program)?;
        let mut command = command_line(program, output)?;
        let directory = wide_path(system)?;
        let root = windows.to_str().ok_or(ElevationError::Unsupported)?;
        let private = output
            .parent()
            .and_then(Path::to_str)
            .ok_or(ElevationError::InsecureJob)?;
        // Construct a minimal environment; never copy API keys, PATH or any inherited user value.
        let environment: Vec<u16> =
            format!("SystemRoot={root}\0TEMP={private}\0TMP={private}\0windir={root}\0\0")
                .encode_utf16()
                .collect();
        let startup = STARTUPINFOW {
            cb: size_of::<STARTUPINFOW>() as u32,
            dwFlags: STARTF_USESTDHANDLES,
            // Null standard handles intentionally suppress provider output; CSV is the sole result.
            ..Default::default()
        };
        let mut information = PROCESS_INFORMATION::default();
        // SAFETY: fixed pinned inbox executable, bounded mutable command, private output and explicit
        // environment. No inherited handles. Suspended creation prevents any code running before assignment.
        if unsafe {
            CreateProcessW(
                name.as_ptr(),
                command.as_mut_ptr(),
                null(),
                null(),
                0,
                CREATE_SUSPENDED | CREATE_NO_WINDOW | CREATE_UNICODE_ENVIRONMENT,
                environment.as_ptr().cast(),
                directory.as_ptr(),
                &startup,
                &mut information,
            )
        } == 0
        {
            return Err(std::io::Error::last_os_error().into());
        }
        let writer = Self {
            job: Some(job),
            process: Handle(information.hProcess),
            thread: Handle(information.hThread),
            exited: false,
        };
        // SAFETY: the only created child remains suspended and is assigned to this private lifetime job.
        if unsafe { AssignProcessToJobObject(writer.job.as_ref().unwrap().0, writer.process.0) }
            == 0
        {
            return Err(std::io::Error::last_os_error().into());
        }
        // SAFETY: the child is already contained in a kill-on-close job; this is its owned primary thread.
        if unsafe { ResumeThread(writer.thread.0) } == u32::MAX {
            return Err(std::io::Error::last_os_error().into());
        }
        Ok(writer)
    }
    fn poll(&mut self) -> Result<Option<u32>, ElevationError> {
        // SAFETY: owned live child handle and a fixed bounded wait. Signalling proves the process exited.
        match unsafe { WaitForSingleObject(self.process.0, 20) } {
            WAIT_TIMEOUT => Ok(None),
            WAIT_OBJECT_0 => {
                self.exited = true;
                let mut code = 0;
                // SAFETY: retained, signalled child handle and initialized exit-code output.
                if unsafe { GetExitCodeProcess(self.process.0, &mut code) } == 0 {
                    return Err(std::io::Error::last_os_error().into());
                }
                Ok(Some(code))
            }
            _ => {
                // SAFETY: immediately read this wait failure's native error on the same thread.
                Err(ElevationError::LaunchFailed(unsafe { GetLastError() }))
            }
        }
    }
}
impl Drop for Writer {
    fn drop(&mut self) {
        if !self.exited {
            // SAFETY: terminate only the child created here, including assignment/resume failure.
            // Job close also covers watchdog/helper termination and forbids an orphaned writer.
            unsafe {
                let _ = TerminateProcess(self.process.0, 125);
            }
            drop(self.job.take());
            // SAFETY: bounded wait on our own child only; its result is not represented as successful exit.
            unsafe {
                let _ = WaitForSingleObject(self.process.0, 1000);
            }
        }
    }
}

fn stop(
    started: Instant,
    duration: Duration,
    cancel: &mut dyn FnMut() -> Result<bool, ElevationError>,
) -> Result<Option<EnergySummary>, ElevationError> {
    if cancel()? {
        Ok(Some(EnergySummary::unavailable(
            ElevationStatus::Cancelled,
            EnergyLimit::Cancelled,
        )))
    } else if started.elapsed() >= duration {
        Ok(Some(EnergySummary::unavailable(
            ElevationStatus::TimedOut,
            EnergyLimit::DurationLimit,
        )))
    } else {
        Ok(None)
    }
}

pub(crate) fn collect(
    job: &NativeJob,
    duration: Duration,
    cancel: &mut dyn FnMut() -> Result<bool, ElevationError>,
) -> Result<EnergySummary, ElevationError> {
    let started = Instant::now();
    if let Some(summary) = stop(started, duration, cancel)? {
        return Ok(summary);
    }
    if !is_elevated()? {
        return Ok(EnergySummary::unavailable(
            ElevationStatus::Denied,
            EnergyLimit::ExportUnavailable,
        ));
    }
    let system = os_directory(true)?;
    let windows = os_directory(false)?;
    let _program_pins = pin_program(&system)?;
    let program = system.join("powercfg.exe");
    let (output, mut file): (PathBuf, File) = job.energy_output()?;
    if let Some(summary) = stop(started, duration, cancel)? {
        return Ok(summary);
    }
    let mut writer = Writer::spawn(&program, &output, &system, &windows)?;
    loop {
        if let Some(summary) = stop(started, duration, cancel)? {
            return Ok(summary);
        }
        if file.metadata()?.len() > MAX_EXPORT_BYTES as u64 {
            return Ok(EnergySummary::unavailable(
                ElevationStatus::Partial,
                EnergyLimit::ExportSizeLimit,
            ));
        }
        if let Some(code) = writer.poll()? {
            if code != 0 {
                return Ok(EnergySummary::unavailable(
                    ElevationStatus::Error,
                    EnergyLimit::ExportFailed,
                ));
            }
            break;
        }
    }
    drop(writer);
    if let Some(summary) = stop(started, duration, cancel)? {
        return Ok(summary);
    }
    let bytes = job.energy_bytes(&mut file, MAX_EXPORT_BYTES)?;
    let mut summary = parse_export(&bytes, job.owner_sid());
    if summary.status == ElevationStatus::Partial {
        summary
            .limitations
            .push(EnergyLimit::NativeAccuracyUnvalidated);
    }
    summary.validate()?;
    Ok(summary)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn the_only_provider_command_is_fixed_and_does_not_search_path() {
        let command = command_line(
            Path::new(r"C:\Windows\System32\powercfg.exe"),
            Path::new(r"D:\Private jobs\pcagent-fixture\energy.csv"),
        )
        .unwrap();
        let text = String::from_utf16(&command[..command.len() - 1]).unwrap();
        assert_eq!(
            text,
            r#""C:\Windows\System32\powercfg.exe" /srumutil /output "D:\Private jobs\pcagent-fixture\energy.csv" /csv"#
        );
        assert!(command_line(
            Path::new("bad\"program.exe"),
            Path::new(r"D:\safe\energy.csv")
        )
        .is_err());
        assert!(command_line(Path::new("powercfg.exe"), Path::new("bad\0output")).is_err());
    }
    #[test]
    fn cancellation_takes_precedence_over_the_export_deadline() {
        let value = stop(Instant::now(), Duration::ZERO, &mut || Ok(true))
            .unwrap()
            .unwrap();
        assert_eq!(value.status, ElevationStatus::Cancelled);
        let value = stop(Instant::now(), Duration::ZERO, &mut || Ok(false))
            .unwrap()
            .unwrap();
        assert_eq!(value.status, ElevationStatus::TimedOut);
    }
    #[test]
    fn private_export_pin_permits_in_place_writer_and_rejects_replacement() {
        // Controlled file fixture only. No E3 command, UAC prompt, SRUM/database or service is touched.
        use super::super::protocol::{ElevatedTask, ElevationRequest};
        use std::io::Write;
        let request =
            ElevationRequest::new(ElevatedTask::SystemCaches, Duration::from_secs(10)).unwrap();
        let job = NativeJob::create(&request).unwrap();
        let (output, mut pinned) = job.energy_output().unwrap();
        {
            let mut writer = std::fs::OpenOptions::new()
                .write(true)
                .truncate(true)
                .open(&output)
                .unwrap();
            writer.write_all(b"synthetic E3 fixture").unwrap();
        }
        assert!(std::fs::remove_file(&output).is_err());
        assert_eq!(
            job.energy_bytes(&mut pinned, 128).unwrap(),
            b"synthetic E3 fixture"
        );
        drop(pinned);
        drop(job);
        assert!(!output.exists());
    }
}
