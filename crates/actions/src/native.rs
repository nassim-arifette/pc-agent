pub use crate::os::{NativeEffects, NativeGuard};
use crate::{FileIdentity, ProtectionCheck};
use std::path::Path;

/// `X:\...` form only; UNC, device (`\\?\`), relative and drive-relative paths are rejected.
pub fn is_windows_absolute(path: &Path) -> bool {
    let Some(s) = path.to_str() else {
        return false;
    };
    let b = s.as_bytes();
    b.len() >= 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && b[2] == b'\\'
}

/// Single raw argument for `explorer.exe`. Characters that Windows forbids in file names (and that
/// could change how Explorer splits its command line) are refused rather than escaped.
pub fn explorer_select_arg(path: &Path) -> Option<String> {
    let s = path.to_str()?;
    let forbidden = |c: char| c.is_control() || "\"<>|?*".contains(c) || c == '/';
    if !is_windows_absolute(path) || s[2..].contains(':') || s.chars().any(forbidden) {
        return None;
    }
    Some(format!("/select,\"{s}\""))
}

/// Minimal fail-closed baseline until the policy crate provides user exclusions.
#[derive(Debug, Default, Clone, Copy)]
pub struct BaselineProtection;
impl ProtectionCheck for BaselineProtection {
    fn is_protected(&self, path: &Path) -> bool {
        const ROOTS: [&str; 9] = [
            "windows",
            "program files",
            "program files (x86)",
            "programdata",
            "$recycle.bin",
            "system volume information",
            "recovery",
            "boot",
            "$windows.~bt",
        ];
        let Some(s) = path.to_str().filter(|_| is_windows_absolute(path)) else {
            return true;
        };
        let parts: Vec<String> = s[3..].split('\\').map(str::to_ascii_lowercase).collect();
        parts
            .first()
            .is_none_or(|p| p.is_empty() || ROOTS.contains(&p.as_str()))
            || parts
                .iter()
                .any(|p| p.is_empty() || p == "." || p == ".." || p.ends_with(['.', ' ']))
    }
}

/// Same file-system object (volume and file id), wherever it currently is.
pub(crate) fn same_object(a: &FileIdentity, b: &FileIdentity) -> bool {
    a.volume_serial == b.volume_serial && a.file_id == b.file_id
}

/// `final_path` (optionally `\\?\`-prefixed) is an entry below `<drive>:\$Recycle.Bin\<folder>\`
/// on the drive of `original`.
pub(crate) fn in_recycle_bin(original: &Path, final_path: &str) -> bool {
    let Some(drive) = original
        .to_str()
        .filter(|_| is_windows_absolute(original))
        .and_then(|s| s.get(..3))
    else {
        return false;
    };
    let path = final_path.strip_prefix(r"\\?\").unwrap_or(final_path);
    let prefix = format!(r"{drive}$Recycle.Bin\");
    let Some(rest) = path
        .get(..prefix.len())
        .filter(|p| p.eq_ignore_ascii_case(&prefix))
        .and_then(|_| path.get(prefix.len()..))
    else {
        return false;
    };
    let parts: Vec<&str> = rest.split('\\').collect();
    parts.len() >= 2
        && parts
            .iter()
            .all(|p| !p.is_empty() && *p != "." && *p != "..")
}

/// Observations taken after the shell recycle operation returned.
#[derive(Debug, Clone, Default)]
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) struct RecycleEvidence {
    /// The operation reported success, was not aborted, and the pre-delete check passed.
    pub shell_succeeded: bool,
    /// Path of the recycle bin item reported by the shell, and the identity observed there.
    pub recycled_item: Option<(String, Option<FileIdentity>)>,
    /// Final path and identity of a handle opened on the pinned file before the operation.
    pub tracked: Option<(String, FileIdentity)>,
    /// Identity currently at the original path.
    pub original_now: Option<FileIdentity>,
}

/// Fail-closed post-check: the item the shell put in the recycle bin is the identity-checked
/// object, the handle held on that object sees it there, and the original path no longer names it.
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) fn verify_recycled(
    original: &Path,
    expected: &FileIdentity,
    evidence: &RecycleEvidence,
) -> Result<(), &'static str> {
    if !evidence.shell_succeeded {
        return Err("shell recycle operation failed or was aborted");
    }
    let Some((item_path, item_identity)) = &evidence.recycled_item else {
        return Err("shell reported no recycle bin item (possibly deleted permanently)");
    };
    if !in_recycle_bin(original, item_path) {
        return Err("reported item is not in the recycle bin of the original volume");
    }
    let unchanged = |id: &FileIdentity| {
        same_object(id, expected)
            && id.size_bytes == expected.size_bytes
            && id.modified_filetime == expected.modified_filetime
            && !id.is_directory
    };
    if !item_identity.as_ref().is_some_and(unchanged) {
        return Err("recycled item is not the identity-checked file");
    }
    match &evidence.tracked {
        Some((path, id))
            if unchanged(id)
                && path
                    .trim_start_matches(r"\\?\")
                    .eq_ignore_ascii_case(item_path.trim_start_matches(r"\\?\")) => {}
        _ => return Err("identity-checked file is not at the reported recycle bin item"),
    }
    if evidence
        .original_now
        .as_ref()
        .is_some_and(|id| same_object(id, expected))
    {
        return Err("original path still refers to the identity-checked file");
    }
    Ok(())
}
