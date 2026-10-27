//! Local desktop clicks only. Model-originated actions still require the broker's permit.
use crate::{web_url_allowed, EffectError};
use pcagent_shared::Status;
use std::{
    path::Path,
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

fn error(status: Status) -> EffectError {
    EffectError::new(status, "The desktop could not open the selected item.")
}

fn web_command(url: &str) -> Result<Command, EffectError> {
    if !web_url_allowed(url) {
        return Err(error(Status::Unsupported));
    }
    #[cfg(target_os = "macos")]
    let mut command = Command::new("/usr/bin/open");
    #[cfg(target_os = "linux")]
    let mut command = Command::new("/usr/bin/xdg-open");
    command.arg(url);
    Ok(command)
}

fn reveal_command(path: &Path) -> Result<Command, EffectError> {
    if !path.is_absolute()
        || path.components().any(|part| {
            matches!(
                part,
                std::path::Component::ParentDir | std::path::Component::CurDir
            )
        })
    {
        return Err(error(Status::Unsupported));
    }
    let metadata = path.symlink_metadata().map_err(|_| error(Status::Error))?;
    #[cfg(target_os = "macos")]
    {
        let _ = metadata;
        let mut command = Command::new("/usr/bin/open");
        // Reveal, never launch a selected executable or app bundle.
        command.arg("-R").arg(path);
        Ok(command)
    }
    #[cfg(target_os = "linux")]
    {
        // xdg-open would execute/open a file. Only its containing directory is safe to reveal.
        let directory = if metadata.is_dir() {
            path
        } else {
            path.parent().ok_or_else(|| error(Status::Unsupported))?
        };
        let mut command = Command::new("/usr/bin/xdg-open");
        command.arg(directory);
        Ok(command)
    }
}

fn run(mut command: Command) -> Result<(), EffectError> {
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let mut child = command.spawn().map_err(|_| error(Status::Error))?;
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => return Ok(()),
            Ok(Some(_)) => return Err(error(Status::Error)),
            Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(20)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(error(Status::Error));
            }
        }
    }
}

pub fn open_web_page(url: &str) -> Result<(), EffectError> {
    run(web_command(url)?)
}

pub fn show_in_explorer(path: &Path) -> Result<(), EffectError> {
    run(reveal_command(path)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn web_launcher_accepts_only_a_single_validated_https_argument() {
        let command = web_command("https://example.org/path?q=value").unwrap();
        assert_eq!(
            command.get_args().collect::<Vec<_>>(),
            ["https://example.org/path?q=value"]
        );
        for url in [
            "file:///bin/sh",
            "javascript:alert(1)",
            "https://u:p@example.org",
            "-a",
            "https://example.org/;open",
        ] {
            assert!(web_command(url).is_err());
        }
    }

    #[test]
    fn reveal_refuses_relative_paths_and_never_opens_a_file_as_an_application() {
        assert!(reveal_command(Path::new("relative")).is_err());
        assert!(reveal_command(Path::new("/usr/../bin/sh")).is_err());
        let command = reveal_command(Path::new("/bin/sh")).unwrap();
        let args = command.get_args().collect::<Vec<_>>();
        #[cfg(target_os = "macos")]
        assert_eq!(args, ["-R", "/bin/sh"]);
        #[cfg(target_os = "linux")]
        assert_eq!(args, ["/bin"]);
    }
}
