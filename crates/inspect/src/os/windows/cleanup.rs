use crate::{CoverageStatus as C, Inspection};
use serde_json::Value;
use std::os::windows::fs::MetadataExt;
use std::{
    fs,
    path::{Path, PathBuf},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use windows_sys::Win32::UI::Shell::{SHQueryRecycleBinW, SHQUERYRBINFO};

#[derive(Debug, Default)]
struct Totals {
    bytes: u64,
    files: u64,
    newest: Option<u64>,
    partial: bool,
    denied: bool,
    exists: bool,
}
/// Metadata only. Never follow a reparse point or read a file. Each category gets its own
/// deadline and entry limit so large caches cannot starve later categories.
fn scan(paths: &[PathBuf], old_large: bool, start: Instant, budget: &mut usize) -> Totals {
    let mut total = Totals::default();
    let mut pending = Vec::new();
    for path in paths {
        match safe_root(path) {
            Ok(true) => pending.push(path.clone()),
            Ok(false) => total.partial = true,
            Err(error) => {
                total.partial = true;
                total.denied |= error.kind() == std::io::ErrorKind::PermissionDenied;
            }
        }
    }
    while let Some(path) = pending.pop() {
        if *budget == 0 || start.elapsed() > Duration::from_millis(500) {
            total.partial = true;
            break;
        }
        *budget -= 1;
        let meta = match fs::symlink_metadata(&path) {
            Ok(m) => m,
            Err(e) => {
                if e.kind() != std::io::ErrorKind::NotFound {
                    total.partial = true;
                    total.denied |= e.kind() == std::io::ErrorKind::PermissionDenied;
                }
                continue;
            }
        };
        total.exists = true;
        if meta.file_attributes() & 0x400 != 0 {
            total.partial = true;
            continue;
        }
        if meta.is_dir() {
            match fs::read_dir(&path) {
                Ok(entries) => {
                    for item in entries {
                        if pending.len() >= 5_000 || start.elapsed() > Duration::from_millis(500) {
                            total.partial = true;
                            break;
                        }
                        match item {
                            Ok(item) => pending.push(item.path()),
                            Err(_) => total.partial = true,
                        }
                    }
                }
                Err(e) => {
                    total.partial = true;
                    total.denied |= e.kind() == std::io::ErrorKind::PermissionDenied;
                }
            }
        } else if meta.is_file() {
            let modified = meta.modified().ok();
            total.partial |= old_large && modified.is_none();
            if old_large
                && !(meta.len() >= 100 * 1024 * 1024
                    && modified
                        .and_then(|t| SystemTime::now().duration_since(t).ok())
                        .is_some_and(|d| d >= Duration::from_secs(180 * 86400)))
            {
                continue;
            }
            total.bytes = total.bytes.saturating_add(meta.len());
            total.files += 1;
            if let Some(time) = modified
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .map(|d| d.as_millis() as u64)
            {
                total.newest = Some(total.newest.unwrap_or(0).max(time));
            }
        }
    }
    total
}
/// Initial roots can contain a reparse point in a parent (for example a redirected cache).
/// Reject those too: testing only the final path component would still traverse that link.
pub(super) fn safe_root(path: &Path) -> std::io::Result<bool> {
    let text = path.to_string_lossy();
    let text = text.strip_prefix(r"\\?\").unwrap_or(&text);
    let bytes = text.as_bytes();
    if bytes.len() < 3
        || !bytes[0].is_ascii_alphabetic()
        || bytes[1] != b':'
        || !matches!(bytes[2], b'\\' | b'/')
    {
        return Ok(false);
    }
    let root = super::wide(&text[..3]);
    // SAFETY: fixed drive-root prefix extracted from an absolute local-format path.
    let kind = unsafe { windows_sys::Win32::Storage::FileSystem::GetDriveTypeW(root.as_ptr()) };
    if !matches!(
        kind,
        windows_sys::Win32::System::WindowsProgramming::DRIVE_FIXED
            | windows_sys::Win32::System::WindowsProgramming::DRIVE_RAMDISK
    ) {
        return Ok(false);
    }
    for ancestor in path.ancestors() {
        match fs::symlink_metadata(ancestor) {
            Ok(meta) => {
                if meta.file_attributes() & 0x400 != 0 {
                    return Ok(false);
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }
    Ok(true)
}
fn env_path(name: &str) -> Option<PathBuf> {
    std::env::var_os(name).map(PathBuf::from)
}
fn joined(base: &Option<PathBuf>, suffix: &str) -> Vec<PathBuf> {
    base.as_ref()
        .map(|p| vec![p.join(suffix)])
        .unwrap_or_default()
}
pub(super) fn collect(out: &mut Inspection) {
    let local = env_path("LOCALAPPDATA");
    let profile = env_path("USERPROFILE");
    let system = env_path("SystemRoot");
    let mut categories: Vec<(&str, &str, &str, Vec<PathBuf>, bool)> = vec![
        (
            "temporary_files",
            "regenerable",
            "Windows Storage > Temporary files",
            {
                let mut paths = joined(&local, "Temp");
                let redirected = std::env::temp_dir();
                if !paths.contains(&redirected) {
                    paths.push(redirected);
                }
                paths
            },
            false,
        ),
        (
            "nvidia_shader_cache",
            "regenerable",
            "Windows Storage > Temporary files",
            joined(&local, r"NVIDIA\DXCache"),
            false,
        ),
        (
            "directx_shader_cache",
            "regenerable",
            "Windows Storage > Temporary files",
            joined(&local, "D3DSCache"),
            false,
        ),
        (
            "npm_cache",
            "regenerable",
            "npm cache management after review",
            joined(&local, "npm-cache"),
            false,
        ),
        (
            "pip_cache",
            "regenerable",
            "pip cache management after review",
            joined(&local, r"pip\Cache"),
            false,
        ),
        (
            "cargo_cache",
            "regenerable",
            "Cargo cache management after review",
            joined(
                &env_path("CARGO_HOME").or_else(|| profile.as_ref().map(|p| p.join(".cargo"))),
                r"registry\cache",
            ),
            false,
        ),
        (
            "nuget_cache",
            "regenerable",
            "NuGet cache management after review",
            joined(&profile, r".nuget\packages"),
            false,
        ),
        (
            "gradle_cache",
            "regenerable",
            "Gradle cache management after review",
            joined(
                &env_path("GRADLE_USER_HOME")
                    .or_else(|| profile.as_ref().map(|p| p.join(".gradle"))),
                "caches",
            ),
            false,
        ),
        (
            "old_large_downloads",
            "review",
            "Review selected downloads individually",
            joined(&profile, "Downloads"),
            true,
        ),
        (
            "previous_windows",
            "review",
            "Windows Storage > Temporary files; removes rollback ability",
            system
                .as_ref()
                .and_then(|p| p.parent())
                .map(|p| vec![p.join("Windows.old")])
                .unwrap_or_default(),
            false,
        ),
        (
            "windows_update_cache",
            "review",
            "Windows Storage > Temporary files",
            joined(&system, r"SoftwareDistribution\Download"),
            false,
        ),
        (
            "delivery_optimization",
            "review",
            "Windows Storage > Temporary files",
            joined(
                &system,
                r"ServiceProfiles\NetworkService\AppData\Local\Microsoft\Windows\DeliveryOptimization\Cache",
            ),
            false,
        ),
        (
            "hibernation_file",
            "keep",
            "System-managed; no deletion recommendation",
            system
                .as_ref()
                .and_then(|p| p.parent())
                .map(|p| vec![p.join("hiberfil.sys")])
                .unwrap_or_default(),
            false,
        ),
        (
            "pagefile",
            "keep",
            "System-managed; no deletion recommendation",
            system
                .as_ref()
                .and_then(|p| p.parent())
                .map(|p| vec![p.join("pagefile.sys")])
                .unwrap_or_default(),
            false,
        ),
        (
            "docker_virtual_disks",
            "review",
            "Docker's own storage management; never delete active virtual disks",
            joined(&local, r"Docker\wsl"),
            false,
        ),
    ];
    let mut browser_paths = Vec::new();
    let mut browser_partial = false;
    for base in [
        joined(&local, r"Google\Chrome\User Data"),
        joined(&local, r"Microsoft\Edge\User Data"),
    ]
    .into_iter()
    .flatten()
    {
        if !safe_root(&base).unwrap_or(false) {
            browser_partial = true;
            continue;
        }
        let entries = match fs::read_dir(base) {
            Ok(entries) => entries,
            Err(e) => {
                browser_partial |= e.kind() != std::io::ErrorKind::NotFound;
                continue;
            }
        };
        {
            for (index, entry) in entries.enumerate() {
                if index >= 100 {
                    browser_partial = true;
                    break;
                }
                let entry = match entry {
                    Ok(e) => e,
                    Err(_) => {
                        browser_partial = true;
                        continue;
                    }
                };
                if !safe_root(&entry.path()).unwrap_or(false) {
                    browser_partial = true;
                    continue;
                }
                let name = entry.file_name();
                let name = name.to_string_lossy();
                if name == "Default" || name.starts_with("Profile ") {
                    browser_paths.push(entry.path().join(r"Cache\Cache_Data"));
                    browser_paths.push(entry.path().join("Code Cache"));
                    browser_paths.push(entry.path().join("GPUCache"));
                }
            }
        }
    }
    for base in joined(&local, r"Mozilla\Firefox\Profiles") {
        if !safe_root(&base).unwrap_or(false) {
            browser_partial = true;
            continue;
        }
        let entries = match fs::read_dir(base) {
            Ok(entries) => entries,
            Err(e) => {
                browser_partial |= e.kind() != std::io::ErrorKind::NotFound;
                continue;
            }
        };
        {
            for (index, entry) in entries.enumerate() {
                if index >= 100 {
                    browser_partial = true;
                    break;
                }
                let entry = match entry {
                    Ok(e) => e,
                    Err(_) => {
                        browser_partial = true;
                        continue;
                    }
                };
                if !safe_root(&entry.path()).unwrap_or(false) {
                    browser_partial = true;
                    continue;
                }
                browser_paths.push(entry.path().join("cache2"));
            }
        }
    }
    categories.push((
        "browser_caches",
        "regenerable",
        "Browser Settings > Clear cached files",
        browser_paths,
        false,
    ));
    out.coverage("browser_profile_discovery",if browser_partial{C::Partial}else{C::Ok},"Known browser profile directories","At most 100 entries per known browser profile root; reparse points and unreadable entries are skipped. Browser cache layout/version coverage is limited to Chrome, Edge and Firefox.");
    // WSL disk paths come from the fixed Lxss registry; paths stay local and only ext4.vhdx
    // metadata is read. Distribution names, commands and guest file contents are not read.
    let mut wsl = Vec::new();
    let mut wsl_partial = false;
    match super::registry::Key::open(
        windows_sys::Win32::System::Registry::HKEY_CURRENT_USER,
        r"SOFTWARE\Microsoft\Windows\CurrentVersion\Lxss",
        windows_sys::Win32::System::Registry::KEY_WOW64_64KEY,
    ) {
        Ok(Some(key)) => {
            if let Ok((names, capped)) = key.names(true, 100) {
                wsl_partial |= capped;
                for name in names {
                    if let Ok(Some(distro)) = key.child(&name) {
                        if let Some(path) = distro.string("BasePath") {
                            wsl.push(PathBuf::from(path).join("ext4.vhdx"));
                        } else {
                            wsl_partial = true;
                        }
                    } else {
                        wsl_partial = true;
                    }
                }
            } else {
                wsl_partial = true;
            }
        }
        Ok(None) => {}
        Err(_) => wsl_partial = true,
    }
    out.coverage("wsl_distribution_discovery",if wsl_partial{C::Partial}else{C::Ok},"Registry.Lxss BasePath","At most 100 current-user WSL registrations; distribution paths remain local and only virtual disk metadata is inspected.");
    categories.push((
        "wsl_virtual_disks",
        "review",
        "WSL distribution storage management; never delete an active disk",
        wsl,
        false,
    ));
    for (category, level, method, paths, old_large) in categories {
        let mut budget = 5_000;
        let total = scan(&paths, old_large, Instant::now(), &mut budget);
        let entity = out.entity("cleanup", None);
        out.value(
            &entity,
            "cleanup_category",
            category,
            "category",
            "Filesystem metadata",
        );
        out.value(
            &entity,
            "cleanup_level",
            level,
            "category",
            "Local cleanup classification",
        );
        out.value(
            &entity,
            "cleanup_method",
            method,
            "description",
            "Local cleanup classification",
        );
        // An unreadable root is unknown, never an empty category. A partial traversal is a lower bound.
        let readable = !paths.is_empty() && (total.exists || !total.denied && !total.partial);
        out.value(
            &entity,
            "cleanup_size_bytes",
            if readable {
                Value::from(total.bytes)
            } else {
                Value::Null
            },
            "bytes",
            "Filesystem metadata (logical length)",
        );
        out.value(
            &entity,
            "cleanup_file_count",
            if readable {
                Value::from(total.files)
            } else {
                Value::Null
            },
            "count",
            "Filesystem metadata",
        );
        out.value(
            &entity,
            "cleanup_newest_modified_unix_ms",
            total.newest.map(Value::from).unwrap_or(Value::Null),
            "unix_ms",
            "Filesystem metadata",
        );
        out.value(
            &entity,
            "cleanup_is_lower_bound",
            total.partial,
            "boolean",
            "Filesystem metadata",
        );
        out.coverage(category,if total.denied{C::NeedsAdmin}else if total.partial{C::Partial}else if paths.is_empty(){C::Unsupported}else{C::Ok},"Filesystem metadata",if total.denied{"Some sizes require additional rights; values are unknown or lower bounds. No elevation was attempted."}else if total.partial{"Traversal was capped or a reparse point/error was skipped; measured totals are lower bounds."}else{"Metadata only; sizes are logical file lengths, not guaranteed recoverable disk space. Categories can overlap. Cleanup requires separate local approval."});
    }
    let mut recycle = SHQUERYRBINFO {
        cbSize: std::mem::size_of::<SHQUERYRBINFO>() as u32,
        ..Default::default()
    };
    // SAFETY: null root requests the current user's recycle bins; valid initialized output.
    let result = unsafe { SHQueryRecycleBinW(std::ptr::null(), &mut recycle) };
    if result >= 0 {
        let bytes = recycle.i64Size;
        let files = recycle.i64NumItems;
        out.value(
            "recycle-bin",
            "cleanup_category",
            "recycle_bin",
            "category",
            "Win32.SHQueryRecycleBinW",
        );
        out.value(
            "recycle-bin",
            "cleanup_level",
            "review",
            "category",
            "Local cleanup classification",
        );
        out.value(
            "recycle-bin",
            "cleanup_size_bytes",
            u64::try_from(bytes)
                .ok()
                .map(Value::from)
                .unwrap_or(Value::Null),
            "bytes",
            "Win32.SHQueryRecycleBinW",
        );
        out.value(
            "recycle-bin",
            "cleanup_file_count",
            u64::try_from(files)
                .ok()
                .map(Value::from)
                .unwrap_or(Value::Null),
            "count",
            "Win32.SHQueryRecycleBinW",
        );
        out.coverage("recycle_bin",C::Ok,"Win32.SHQueryRecycleBinW","Review before emptying; recycle-bin contents can still be restored. Modification dates are not exposed by this API.");
    } else {
        out.coverage(
            "recycle_bin",
            C::Partial,
            "Win32.SHQueryRecycleBinW",
            "Recycle-bin totals unavailable.",
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn nonexistent_categories_are_empty_but_bounded_scans_are_partial() {
        let path =
            std::env::temp_dir().join(pcagent_shared::new_id("nonexistent-inspection-fixture"));
        let mut budget = 5;
        let t = scan(
            std::slice::from_ref(&path),
            false,
            Instant::now(),
            &mut budget,
        );
        assert_eq!(t.bytes, 0);
        assert!(!t.partial);
        let mut budget = 0;
        let t = scan(&[path], false, Instant::now(), &mut budget);
        assert!(t.partial);
    }
    #[test]
    fn metadata_scan_counts_real_lengths_without_reading_contents() {
        let root = std::env::temp_dir().join(pcagent_shared::new_id("inspection-metadata-fixture"));
        fs::create_dir(&root).unwrap();
        let file = root.join("fixture.bin");
        fs::File::create(&file).unwrap().set_len(8192).unwrap();
        let mut budget = 20;
        let total = scan(
            std::slice::from_ref(&root),
            false,
            Instant::now(),
            &mut budget,
        );
        assert_eq!(total.bytes, 8192);
        assert_eq!(total.files, 1);
        assert!(!total.partial);
        fs::remove_file(file).unwrap();
        fs::remove_dir(root).unwrap();
    }
    #[test]
    fn network_and_relative_roots_are_never_walked() {
        assert!(!safe_root(Path::new(r"\\host\share\cache")).unwrap());
        assert!(!safe_root(Path::new("cache")).unwrap());
    }
}
