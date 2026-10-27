use crate::{CoverageStatus as C, Inspection};
use serde_json::json;
use std::{
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

pub(super) fn cache() -> Option<PathBuf> {
    std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| {
            std::env::var_os("HOME")
                .map(|p| PathBuf::from(p).join(".cache"))
                .filter(|p| p.is_absolute())
        })
}
pub(super) fn collect(out: &mut Inspection) {
    let Some(cache) = cache() else {
        out.coverage(
            "cleanup",
            C::Unsupported,
            "XDG cache",
            "No absolute user cache root.",
        );
        return;
    };
    let trash = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")));
    scan(out, &cache, "linux.cache");
    if let Some(trash) = trash {
        scan(out, &trash.join("Trash/files"), "linux.trash");
    }
}
fn scan(out: &mut Inspection, root: &Path, category: &str) {
    let entity = out.entity("cleanup", None);
    let root_status = match std::fs::symlink_metadata(root) {
        Ok(metadata) if !metadata.is_dir() || metadata.file_type().is_symlink() => {
            Some(C::Unsupported)
        }
        Ok(_) => std::fs::read_dir(root).err().map(|e| {
            if e.kind() == std::io::ErrorKind::PermissionDenied {
                C::Denied
            } else {
                C::Error
            }
        }),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => Some(if e.kind() == std::io::ErrorKind::PermissionDenied {
            C::Denied
        } else {
            C::Error
        }),
    };
    if let Some(status) = root_status {
        out.value(
            &entity,
            "cleanup_category",
            category,
            "category",
            "XDG metadata",
        );
        for (metric, unit) in [
            ("cleanup_size_bytes", "bytes"),
            ("cleanup_file_count", "count"),
        ] {
            out.value(
                &entity,
                metric,
                serde_json::Value::Null,
                unit,
                "XDG metadata",
            );
            out.observations.last_mut().unwrap().status = status;
        }
        out.coverage(
            category,
            status,
            "XDG metadata",
            "Root is unreadable, linked or not a directory; no size was measured.",
        );
        return;
    }
    let mut bytes = 0u64;
    let mut count = 0u64;
    let mut partial = false;
    let mut pending = vec![root.to_path_buf()];
    let clock = Instant::now();
    while let Some(path) = pending.pop() {
        if clock.elapsed() > Duration::from_secs(2) {
            partial = true;
            break;
        }
        let metadata = match std::fs::symlink_metadata(&path) {
            Ok(m) => m,
            Err(e) if path == root && e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(_) => {
                partial = true;
                continue;
            }
        };
        if metadata.file_type().is_symlink() {
            partial = true;
            continue;
        }
        if metadata.is_file() {
            count += 1;
            if let Some(total) = bytes.checked_add(metadata.len()) {
                bytes = total
            } else {
                partial = true;
                break;
            }
        } else if metadata.is_dir() {
            match std::fs::read_dir(&path) {
                Ok(entries) => {
                    for entry in entries {
                        match entry {
                            Ok(e) => pending.push(e.path()),
                            Err(_) => partial = true,
                        }
                    }
                }
                Err(_) => partial = true,
            }
        }
    }
    out.value(
        &entity,
        "cleanup_category",
        category,
        "category",
        "XDG cache/trash metadata",
    );
    out.value(
        &entity,
        "cleanup_size_bytes",
        json!(bytes),
        "bytes",
        "Filesystem metadata lower bound",
    );
    out.value(
        &entity,
        "cleanup_file_count",
        json!(count),
        "count",
        "Filesystem metadata",
    );
    out.value(
        &entity,
        "cleanup_is_lower_bound",
        partial,
        "boolean",
        "Filesystem metadata",
    );
    out.value(
        &entity,
        "cleanup_level",
        if category == "linux.cache" {
            "regenerable"
        } else {
            "manual"
        },
        "category",
        "XDG cache/trash metadata",
    );
    out.coverage(category,if partial{C::Partial}else{C::Ok},"XDG metadata only","No file contents read and no deletion. Cache cleanup proposes exact files for review; trash is retained until the user empties it outside this action.");
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn links_are_not_reported_as_empty_and_missing_cache_is_observed_empty() {
        let root = std::env::temp_dir().join(pcagent_shared::new_id("cleanup-fixture"));
        std::fs::create_dir(&root).unwrap();
        let link = root.join("link");
        std::os::unix::fs::symlink(&root, &link).unwrap();
        let mut out = Inspection::new();
        scan(&mut out, &link, "linux.cache");
        assert!(out
            .observations
            .iter()
            .find(|o| o.metric == "cleanup_size_bytes")
            .unwrap()
            .value
            .is_null());
        let mut out = Inspection::new();
        scan(&mut out, &root.join("missing"), "linux.cache");
        assert_eq!(
            out.observations
                .iter()
                .find(|o| o.metric == "cleanup_size_bytes")
                .unwrap()
                .value,
            json!(0)
        );
        std::fs::remove_dir_all(root).unwrap();
    }
}
