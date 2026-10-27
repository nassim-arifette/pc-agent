//! Current-user TEMP and shader adapters. Inventory is read-only; approved files go to the bin.
use crate::{
    is_native_absolute, same_object, BaselineProtection, FileTarget, Guard, ProtectionCheck,
    Refusal,
};
use pcagent_shared::Status;
use serde::{Deserialize, Serialize};
use std::{
    collections::HashSet,
    fs,
    path::{Path, PathBuf},
    time::{Duration, SystemTime},
};

pub const MAX_USER_CLEANUP_FILES: usize = 64;
pub const MAX_USER_CLEANUP_ENTRIES: usize = 1024;
pub const MIN_TEMP_AGE: Duration = Duration::from_secs(7 * 24 * 60 * 60);
const MAX_DEPTH: usize = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UserCleanupKind {
    UserTemp,
    NvidiaDxCache,
    Direct3dShaderCache,
    LinuxCache,
}
impl UserCleanupKind {
    pub fn from_ref(reference: &str) -> Option<Self> {
        match reference {
            "user.temp" => Some(Self::UserTemp),
            "nvidia.dxcache" => Some(Self::NvidiaDxCache),
            "windows.d3dscache" => Some(Self::Direct3dShaderCache),
            "linux.cache" => Some(Self::LinuxCache),
            _ => None,
        }
    }
    pub fn adapter_id(self) -> &'static str {
        match self {
            Self::UserTemp => "user.temp",
            Self::NvidiaDxCache => "nvidia.dxcache",
            Self::Direct3dShaderCache => "windows.d3dscache",
            Self::LinuxCache => "linux.cache",
        }
    }
}

/// Exact approved metadata. Paths, identities and file names remain local.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecycleCleanupTarget {
    kind: UserCleanupKind,
    root: FileTarget,
    directories: Vec<FileTarget>,
    files: Vec<FileTarget>,
    bytes: Option<u64>,
}
impl RecycleCleanupTarget {
    pub fn kind(&self) -> UserCleanupKind {
        self.kind
    }
    pub fn root(&self) -> &FileTarget {
        &self.root
    }
    pub fn files(&self) -> &[FileTarget] {
        &self.files
    }
    pub fn bytes(&self) -> Option<u64> {
        self.bytes
    }
    /// Split into independently journaled files; additions to a cache are never implicitly approved.
    pub(crate) fn split(self) -> impl Iterator<Item = Self> {
        let Self {
            kind,
            root,
            directories,
            files,
            ..
        } = self;
        files.into_iter().map(move |file| Self {
            kind,
            root: root.clone(),
            directories: directories
                .iter()
                .filter(|directory| within_root(&directory.path, &file.path))
                .cloned()
                .collect(),
            bytes: Some(file.identity.size_bytes),
            files: vec![file],
        })
    }
    pub(crate) fn validate_structure(&self) -> Result<(), Refusal> {
        if !is_native_absolute(&self.root.path)
            || BaselineProtection.is_protected(&self.root.path)
            || !self.root.identity.is_directory
            || self.root.identity.is_reparse_point
            || self.files.is_empty()
            || self.files.len() > MAX_USER_CLEANUP_FILES
            || self.directories.len() > MAX_USER_CLEANUP_ENTRIES
        {
            return Err(Refusal::AdapterPreconditionFailed);
        }
        let mut objects = HashSet::new();
        for file in &self.files {
            if !within_root(&self.root.path, &file.path)
                || !covered_ancestors(&self.root.path, &file.path, &self.directories)
                || file.identity.is_directory
                || file.identity.is_reparse_point
                || BaselineProtection.is_protected(&file.path)
                || !objects.insert((file.identity.volume_serial, file.identity.file_id))
            {
                return Err(Refusal::AdapterPreconditionFailed);
            }
        }
        for directory in &self.directories {
            if !within_root(&self.root.path, &directory.path)
                || !directory.identity.is_directory
                || directory.identity.is_reparse_point
                || BaselineProtection.is_protected(&directory.path)
            {
                return Err(Refusal::AdapterPreconditionFailed);
            }
        }
        let bytes = self
            .files
            .iter()
            .try_fold(0_u64, |sum, file| sum.checked_add(file.identity.size_bytes));
        if self.bytes != bytes {
            return Err(Refusal::AdapterPreconditionFailed);
        }
        Ok(())
    }
    pub(crate) fn validate_directories(
        &self,
        guard: &dyn Guard,
        protection: &dyn ProtectionCheck,
    ) -> Result<(), Refusal> {
        self.validate_structure()?;
        for directory in std::iter::once(&self.root).chain(&self.directories) {
            if protection.is_protected(&directory.path) {
                return Err(Refusal::Protected);
            }
            let observed = guard
                .observe_file(&directory.path)
                .map_err(|status| Refusal::Unverifiable { status })?
                .ok_or(Refusal::TargetMissing)?;
            if observed.is_reparse_point {
                return Err(Refusal::ReparsePoint);
            }
            // Directory timestamps change while approved files are recycled; bind the object.
            if !observed.is_directory || !same_object(&observed, &directory.identity) {
                return Err(Refusal::TargetReplaced);
            }
        }
        Ok(())
    }
}

fn within_root(root: &Path, path: &Path) -> bool {
    if cfg!(unix) && root.is_absolute() {
        return path != root && path.starts_with(root) && is_native_absolute(path);
    }
    let (Some(root), Some(path)) = (root.to_str(), path.to_str()) else {
        return false;
    };
    let root = root.trim_end_matches('\\');
    path.get(..root.len())
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case(root))
        && path.as_bytes().get(root.len()) == Some(&b'\\')
        && is_native_absolute(Path::new(path))
}

fn covered_ancestors(root: &Path, file: &Path, directories: &[FileTarget]) -> bool {
    if cfg!(unix) && root.is_absolute() {
        for parent in file.ancestors().skip(1) {
            if parent == root {
                return true;
            }
            if !directories.iter().any(|directory| directory.path == parent) {
                return false;
            }
        }
        return false;
    }
    let (Some(root), Some(mut path)) = (root.to_str(), file.to_str()) else {
        return false;
    };
    let root = root.trim_end_matches('\\');
    for _ in 0..=MAX_DEPTH {
        let Some(separator) = path.rfind('\\') else {
            return false;
        };
        path = &path[..separator];
        if path.eq_ignore_ascii_case(root) {
            return true;
        }
        if !directories.iter().any(|directory| {
            directory
                .path
                .to_str()
                .is_some_and(|candidate| candidate.eq_ignore_ascii_case(path))
        }) {
            return false;
        }
    }
    false
}

/// A trusted adapter whose root is chosen locally, never from a model-supplied path.
#[derive(Debug, Clone)]
pub struct UserCleanupAdapter {
    kind: UserCleanupKind,
    root: PathBuf,
}
impl UserCleanupAdapter {
    pub fn current_user(kind: UserCleanupKind) -> Result<Self, Status> {
        #[cfg(target_os = "linux")]
        if kind == UserCleanupKind::LinuxCache {
            let home = std::env::var_os("HOME")
                .map(PathBuf::from)
                .ok_or(Status::Unsupported)?;
            let root = std::env::var_os("XDG_CACHE_HOME")
                .map(PathBuf::from)
                .filter(|p| p.is_absolute())
                .unwrap_or_else(|| home.join(".cache"));
            if !is_native_absolute(&root)
                || home.starts_with(&root)
                || BaselineProtection.is_protected(&root)
            {
                return Err(Status::Unsupported);
            }
            let observed = crate::NativeGuard
                .observe_file(&root)?
                .ok_or(Status::Unsupported)?;
            if !observed.is_directory || observed.is_reparse_point {
                return Err(Status::Unsupported);
            }
            return Ok(Self { kind, root });
        }
        if !cfg!(windows) {
            return Err(Status::Unsupported);
        }
        let local = std::env::var_os("LOCALAPPDATA").map(PathBuf::from);
        let root = match kind {
            UserCleanupKind::UserTemp => std::env::var_os("TEMP")
                .or_else(|| std::env::var_os("TMP"))
                .map(PathBuf::from)
                .ok_or(Status::Unsupported)?,
            UserCleanupKind::NvidiaDxCache => {
                local.ok_or(Status::Unsupported)?.join(r"NVIDIA\DXCache")
            }
            UserCleanupKind::Direct3dShaderCache => {
                local.ok_or(Status::Unsupported)?.join("D3DSCache")
            }
            UserCleanupKind::LinuxCache => return Err(Status::Unsupported),
        };
        if !is_native_absolute(&root) || BaselineProtection.is_protected(&root) {
            return Err(Status::Unsupported);
        }
        // A misconfigured TEMP must never turn an entire profile or volume into a cleanup root.
        if root.parent().is_none()
            || ["USERPROFILE", "APPDATA", "LOCALAPPDATA"]
                .iter()
                .filter_map(std::env::var_os)
                .any(|known| {
                    root.as_os_str()
                        .to_string_lossy()
                        .eq_ignore_ascii_case(&known.to_string_lossy())
                        || within_root(&root, Path::new(&known))
                })
        {
            return Err(Status::Unsupported);
        }
        Ok(Self { kind, root })
    }
    pub fn kind(&self) -> UserCleanupKind {
        self.kind
    }
    pub fn root(&self) -> &Path {
        &self.root
    }
    /// Inventory an explicit local batch in a large cache. The paths come from the trusted local
    /// resolver/UI, never model JSON. Ancestors, age and each selected identity are checked.
    pub fn capture_selected(
        &self,
        selected: &[PathBuf],
        guard: &dyn Guard,
    ) -> Result<RecycleCleanupTarget, Status> {
        if selected.is_empty() || selected.len() > MAX_USER_CLEANUP_FILES {
            return Err(Status::Unsupported);
        }
        let root = observe_target(guard, self.root.clone())?;
        if !root.identity.is_directory {
            return Err(Status::Unsupported);
        }
        let mut directories = Vec::new();
        let mut seen_directories = HashSet::new();
        let mut files = Vec::new();
        for path in selected {
            if !within_root(&self.root, path) {
                return Err(Status::Unsupported);
            }
            let file = observe_target(guard, path.clone())?;
            if file.identity.is_directory {
                return Err(Status::Unsupported);
            }
            if self.kind == UserCleanupKind::UserTemp
                && SystemTime::now()
                    .duration_since(
                        modified_time(file.identity.modified_filetime).ok_or(Status::Partial)?,
                    )
                    .map_err(|_| Status::Partial)?
                    < MIN_TEMP_AGE
            {
                return Err(Status::Unsupported);
            }
            for (index, ancestor) in path.ancestors().skip(1).enumerate() {
                if ancestor == self.root {
                    break;
                }
                if index >= MAX_DEPTH || !within_root(&self.root, ancestor) {
                    return Err(Status::Unsupported);
                }
                if seen_directories.insert(ancestor.to_owned()) {
                    let directory = observe_target(guard, ancestor.to_owned())?;
                    if !directory.identity.is_directory {
                        return Err(Status::Unsupported);
                    }
                    directories.push(directory);
                }
            }
            files.push(file);
        }
        files.sort_by(|a, b| a.path.cmp(&b.path));
        directories.sort_by(|a, b| a.path.cmp(&b.path));
        let bytes = files
            .iter()
            .try_fold(0_u64, |sum, file| sum.checked_add(file.identity.size_bytes));
        let target = RecycleCleanupTarget {
            kind: self.kind,
            root,
            directories,
            files,
            bytes,
        };
        target
            .validate_structure()
            .map_err(|_| Status::Unsupported)?;
        Ok(target)
    }
    /// Bounded read-only inventory. TEMP files younger than seven days remain untouched. Overflow,
    /// unreadable metadata and links require a different plan; no truncated deletion manifest exists.
    pub fn capture(&self, guard: &dyn Guard) -> Result<RecycleCleanupTarget, Status> {
        let root = observe_target(guard, self.root.clone())?;
        if !root.identity.is_directory {
            return Err(Status::Unsupported);
        }
        let mut pending = vec![(self.root.clone(), 0_usize)];
        let mut directories = Vec::new();
        let mut files = Vec::new();
        let mut entries = 0_usize;
        let now = SystemTime::now();
        while let Some((directory, depth)) = pending.pop() {
            for entry in fs::read_dir(&directory).map_err(io_status)? {
                entries += 1;
                if entries > MAX_USER_CLEANUP_ENTRIES {
                    return Err(Status::Partial);
                }
                let entry = entry.map_err(io_status)?;
                let metadata = fs::symlink_metadata(entry.path()).map_err(io_status)?;
                if metadata.file_type().is_symlink() {
                    return Err(Status::Unsupported);
                }
                let file = observe_target(guard, entry.path())?;
                if file.identity.is_directory {
                    if depth >= MAX_DEPTH {
                        return Err(Status::Partial);
                    }
                    pending.push((file.path.clone(), depth + 1));
                    directories.push(file);
                } else if metadata.is_file() {
                    if self.kind == UserCleanupKind::UserTemp {
                        let age = now
                            .duration_since(
                                modified_time(file.identity.modified_filetime)
                                    .ok_or(Status::Partial)?,
                            )
                            .map_err(|_| Status::Partial)?;
                        if age < MIN_TEMP_AGE {
                            continue;
                        }
                    }
                    files.push(file);
                    if files.len() > MAX_USER_CLEANUP_FILES {
                        return Err(Status::Partial);
                    }
                } else {
                    return Err(Status::Unsupported);
                }
            }
        }
        files.sort_by(|a, b| a.path.cmp(&b.path));
        directories.sort_by(|a, b| a.path.cmp(&b.path));
        let bytes = files
            .iter()
            .try_fold(0_u64, |sum, file| sum.checked_add(file.identity.size_bytes));
        let target = RecycleCleanupTarget {
            kind: self.kind,
            root,
            directories,
            files,
            bytes,
        };
        target
            .validate_structure()
            .map_err(|_| Status::Unsupported)?;
        Ok(target)
    }
}

fn observe_target(guard: &dyn Guard, path: PathBuf) -> Result<FileTarget, Status> {
    let identity = guard.observe_file(&path)?.ok_or(Status::Unsupported)?;
    if identity.is_reparse_point || BaselineProtection.is_protected(&path) {
        return Err(Status::Unsupported);
    }
    Ok(FileTarget { path, identity })
}

fn modified_time(filetime: u64) -> Option<SystemTime> {
    let ticks = filetime.checked_sub(116_444_736_000_000_000)?;
    SystemTime::UNIX_EPOCH.checked_add(Duration::new(
        ticks / 10_000_000,
        (ticks % 10_000_000 * 100) as u32,
    ))
}

fn io_status(error: std::io::Error) -> Status {
    if error.kind() == std::io::ErrorKind::PermissionDenied {
        Status::PermissionDenied
    } else {
        Status::Error
    }
}

#[cfg(test)]
pub(crate) fn fixture_target(root: FileTarget, files: Vec<FileTarget>) -> RecycleCleanupTarget {
    let bytes = files
        .iter()
        .try_fold(0_u64, |sum, file| sum.checked_add(file.identity.size_bytes));
    RecycleCleanupTarget {
        kind: UserCleanupKind::UserTemp,
        root,
        directories: vec![],
        files,
        bytes,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::FileIdentity;

    fn file(path: &str, directory: bool, id: u128) -> FileTarget {
        FileTarget {
            path: path.into(),
            identity: FileIdentity {
                volume_serial: 1,
                file_id: id,
                size_bytes: 10,
                modified_filetime: 50,
                is_directory: directory,
                is_reparse_point: false,
            },
        }
    }

    #[test]
    fn cleanup_manifest_refuses_escape_links_duplicates_and_overflow() {
        let root = file(r"C:\Users\fixture\Temp", true, 1);
        let target = fixture_target(
            root.clone(),
            vec![file(r"C:\Users\fixture\Temp\cache.bin", false, 2)],
        );
        assert!(target.validate_structure().is_ok());
        for path in [
            r"C:\Users\fixture\Temp-other\a",
            r"C:\Users\fixture\Temp\..\secret",
            r"C:\Windows\a",
            r"C:\Users\fixture\Temp",
        ] {
            assert!(
                fixture_target(root.clone(), vec![file(path, false, 2)])
                    .validate_structure()
                    .is_err(),
                "{path}"
            );
        }
        let mut bad = target.clone();
        bad.files[0].identity.is_reparse_point = true;
        assert!(bad.validate_structure().is_err());
        let mut nested = fixture_target(
            root.clone(),
            vec![file(r"C:\Users\fixture\Temp\nested\cache.bin", false, 2)],
        );
        assert!(nested.validate_structure().is_err());
        nested
            .directories
            .push(file(r"C:\Users\fixture\Temp\nested", true, 3));
        assert!(nested.validate_structure().is_ok());
        let mut bad = target.clone();
        bad.files.push(bad.files[0].clone());
        bad.bytes = Some(20);
        assert!(bad.validate_structure().is_err());
        let files = (0..=MAX_USER_CLEANUP_FILES)
            .map(|i| file(&format!(r"C:\Users\fixture\Temp\{i}"), false, i as u128 + 2))
            .collect();
        assert!(fixture_target(root, files).validate_structure().is_err());
        let mut bad = target;
        bad.bytes = Some(999);
        assert!(bad.validate_structure().is_err());
    }

    #[cfg(windows)]
    #[test]
    fn native_inventory_only_includes_old_temp_files_and_bounds_shader_files() {
        use crate::NativeGuard;
        use std::fs::{File, FileTimes};
        let root =
            std::env::temp_dir().join(pcagent_shared::new_id("pcagent-user-cleanup-fixture"));
        fs::create_dir_all(&root).unwrap();
        let temp = root.join("temp");
        fs::create_dir(&temp).unwrap();
        fs::write(temp.join("fresh.tmp"), b"fixture").unwrap();
        let old = temp.join("old.tmp");
        fs::write(&old, b"old fixture").unwrap();
        File::options()
            .write(true)
            .open(&old)
            .unwrap()
            .set_times(
                FileTimes::new()
                    .set_modified(SystemTime::now() - MIN_TEMP_AGE - Duration::from_secs(5)),
            )
            .unwrap();
        let adapter = UserCleanupAdapter {
            kind: UserCleanupKind::UserTemp,
            root: temp.clone(),
        };
        let target = adapter.capture(&NativeGuard).unwrap();
        assert_eq!(target.files().len(), 1);
        assert_eq!(target.files()[0].path, old);
        assert!(adapter
            .capture_selected(&[temp.join("fresh.tmp")], &NativeGuard)
            .is_err());
        assert_eq!(
            adapter
                .capture_selected(std::slice::from_ref(&old), &NativeGuard)
                .unwrap()
                .files()
                .len(),
            1
        );
        assert!(temp.join("fresh.tmp").exists() && old.exists());

        let shaders = root.join("shaders");
        fs::create_dir(&shaders).unwrap();
        fs::write(shaders.join("first.bin"), b"fixture").unwrap();
        let adapter = UserCleanupAdapter {
            kind: UserCleanupKind::Direct3dShaderCache,
            root: shaders.clone(),
        };
        assert_eq!(adapter.capture(&NativeGuard).unwrap().files().len(), 1);
        for index in 0..MAX_USER_CLEANUP_FILES {
            fs::write(shaders.join(format!("{index}.bin")), b"fixture").unwrap();
        }
        assert_eq!(adapter.capture(&NativeGuard), Err(Status::Partial));
        assert_eq!(
            adapter
                .capture_selected(&[shaders.join("first.bin")], &NativeGuard)
                .unwrap()
                .files()
                .len(),
            1
        );
        assert_eq!(
            fs::read_dir(&shaders).unwrap().count(),
            MAX_USER_CLEANUP_FILES + 1
        );
        // Only this test's freshly created fixture tree is removed.
        fs::remove_dir_all(root).unwrap();
    }
}
