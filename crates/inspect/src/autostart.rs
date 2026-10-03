//! Local application preference, deliberately separate from the read-only inspection registry.
//! Only an explicit local UI setting may call `set`; no model tool exposes this mutation.
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    Unsupported,
    InvalidExecutable,
    PermissionDenied,
    Registry,
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Unsupported => "Windows autostart is unavailable on this platform.",
            Self::InvalidExecutable => {
                "Autostart requires this running application's absolute executable path."
            }
            Self::PermissionDenied => {
                "Windows denied access to the current user's autostart setting."
            }
            Self::Registry => "Windows could not read or update the autostart setting.",
        })
    }
}
impl std::error::Error for Error {}

pub fn get() -> Result<bool, Error> {
    #[cfg(windows)]
    {
        crate::windows::autostart::get()
    }
    #[cfg(not(windows))]
    {
        Err(Error::Unsupported)
    }
}
pub fn set(enabled: bool, executable: &Path) -> Result<(), Error> {
    #[cfg(windows)]
    {
        crate::windows::autostart::set(enabled, executable)
    }
    #[cfg(not(windows))]
    {
        let _ = (enabled, executable);
        Err(Error::Unsupported)
    }
}

#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) fn command(path: &str) -> Result<String, Error> {
    // Explicit drive paths only. Registry Run uses a command-line parser; quoting one path
    // with a fixed switch avoids a command interpreter and argument injection.
    let bytes = path.as_bytes();
    if bytes.len() < 7
        || !bytes[0].is_ascii_alphabetic()
        || bytes[1] != b':'
        || !matches!(bytes[2], b'\\' | b'/')
        || !path.to_ascii_lowercase().ends_with(".exe")
        || path.chars().any(|c| c == '"' || c.is_control())
        || path.len() > 2048
    {
        return Err(Error::InvalidExecutable);
    }
    let value = format!("\"{path}\" --background");
    if value.encode_utf16().count() > 260 {
        return Err(Error::InvalidExecutable);
    }
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn run_command_contains_only_quoted_executable_and_fixed_switch() {
        assert_eq!(
            command(r"C:\Program Files\PC Agent\pcagent-gui.exe").unwrap(),
            r#""C:\Program Files\PC Agent\pcagent-gui.exe" --background"#
        );
        for path in [
            r"pcagent.exe",
            r"C:pcagent.exe",
            r"\\server\pcagent.exe",
            r"C:\agent.cmd",
            "C:\\agent.exe\" --other.exe",
            "C:\\agent\n.exe",
        ] {
            assert_eq!(command(path), Err(Error::InvalidExecutable));
        }
    }
}
