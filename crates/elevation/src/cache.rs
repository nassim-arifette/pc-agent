//! Fixed system-cache metadata collection. No deletion or reclaim estimate is produced.
use super::protocol::{
    CacheCategory, CacheCoverage, CacheSummary, ElevatedTask, ElevationError, ElevationLimit,
    ElevationRequest, ElevationResponse, ElevationStatus,
};
use pcagent_shared::{unix_ms, Status, Window};
use pcagent_storage::{
    CancelToken, ClassicBackend, ScanBackend, ScanOptions, ScanScope, Truncation,
};
use std::{io::ErrorKind, path::PathBuf, time::Instant};

pub(crate) struct CacheRoot {
    pub category: CacheCategory,
    pub path: PathBuf,
}

pub(crate) fn collect(
    request: &ElevationRequest,
    roots: &[CacheRoot],
    check_cancel: &mut dyn FnMut() -> Result<bool, ElevationError>,
) -> Result<ElevationResponse, ElevationError> {
    let started = unix_ms();
    let clock = Instant::now();
    let mut response = ElevationResponse::empty(
        request,
        ElevationStatus::Partial,
        ElevationLimit::MetadataSnapshot,
    );
    if matches!(request.task, ElevatedTask::MftInventory { .. }) {
        // No MFT result is claimed until allocation and link attribution pass volume fixtures.
        response.status = ElevationStatus::Unsupported;
        response.limitations = vec![ElevationLimit::MftAccuracyUnvalidated];
        return Ok(response);
    }
    for root in roots {
        if check_cancel()? {
            response.status = ElevationStatus::Cancelled;
            response.limitations.push(ElevationLimit::Cancelled);
            break;
        }
        let Some(remaining) = request.budget().checked_sub(clock.elapsed()) else {
            response.status = ElevationStatus::TimedOut;
            response.limitations.push(ElevationLimit::DurationLimit);
            break;
        };
        let metadata = match std::fs::symlink_metadata(&root.path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == ErrorKind::NotFound => {
                if root.category == CacheCategory::DeliveryOptimization {
                    response.caches.push(CacheSummary::unavailable(
                        root.category,
                        ElevationStatus::Partial,
                        ElevationLimit::DefaultCacheLocationOnly,
                    ));
                    continue;
                }
                response.caches.push(CacheSummary {
                    category: root.category,
                    status: ElevationStatus::Completed,
                    exists: Some(false),
                    observed_logical_bytes: Some(0),
                    observed_allocated_bytes: Some(0),
                    files: Some(0),
                    coverage: None,
                    limitations: vec![ElevationLimit::MetadataSnapshot],
                });
                continue;
            }
            Err(error) => {
                response.caches.push(CacheSummary::unavailable(
                    root.category,
                    if error.kind() == ErrorKind::PermissionDenied {
                        ElevationStatus::Denied
                    } else {
                        ElevationStatus::Error
                    },
                    ElevationLimit::SourceUnavailable,
                ));
                continue;
            }
        };
        if !metadata.is_dir() || metadata.file_type().is_symlink() {
            response.caches.push(CacheSummary::unavailable(
                root.category,
                ElevationStatus::Partial,
                ElevationLimit::EnumerationIncomplete,
            ));
            continue;
        }
        let scope = match ScanScope::authorize(&root.path) {
            Ok(scope) => scope,
            Err(_) => {
                response.caches.push(CacheSummary::unavailable(
                    root.category,
                    ElevationStatus::Partial,
                    ElevationLimit::SourceUnavailable,
                ));
                continue;
            }
        };
        let options = ScanOptions {
            max_entries: 100_000,
            max_duration: remaining,
            index_budget_bytes: 32 * 1024 * 1024,
            max_denied_listed: 0,
        };
        let cancel = CancelToken::default();
        let mut cancel_error = None;
        let index = ClassicBackend.scan(&scope, &options, &cancel, &mut |_| {
            match check_cancel() {
                Ok(true) => cancel.cancel(),
                Err(error) => {
                    cancel_error = Some(error);
                    cancel.cancel();
                }
                Ok(false) => {}
            }
            if clock.elapsed() >= request.budget() {
                cancel.cancel();
            }
        });
        if let Some(error) = cancel_error {
            return Err(error);
        }
        let totals = index.totals();
        let c = index.coverage();
        let mut limits = vec![ElevationLimit::MetadataSnapshot];
        if totals.allocation_unknown_files > 0 {
            limits.push(ElevationLimit::AllocationLowerBound);
        }
        let status = if clock.elapsed() >= request.budget() {
            limits.push(ElevationLimit::DurationLimit);
            ElevationStatus::TimedOut
        } else if c.cancelled {
            limits.push(ElevationLimit::Cancelled);
            ElevationStatus::Cancelled
        } else if index.status() == Status::Ok {
            ElevationStatus::Completed
        } else {
            limits.push(ElevationLimit::EnumerationIncomplete);
            ElevationStatus::Partial
        };
        if let Some(truncation) = c.truncated {
            limits.push(match truncation {
                Truncation::EntryLimit => ElevationLimit::EntryLimit,
                Truncation::MemoryBudget => ElevationLimit::MemoryLimit,
                Truncation::Duration => ElevationLimit::DurationLimit,
            });
        }
        let status = if root.category == CacheCategory::DeliveryOptimization {
            // Policy can relocate this cache; only the documented default directory was inspected.
            limits.push(ElevationLimit::DefaultCacheLocationOnly);
            if status == ElevationStatus::Completed {
                ElevationStatus::Partial
            } else {
                status
            }
        } else {
            status
        };
        response.caches.push(CacheSummary {
            category: root.category,
            status,
            exists: Some(true),
            observed_logical_bytes: Some(totals.logical_bytes),
            observed_allocated_bytes: Some(totals.allocated_bytes),
            files: Some(totals.files),
            coverage: Some(CacheCoverage {
                directories: c.dirs,
                denied_directories: c.denied_dirs,
                denied_files: c.denied_files,
                reparse_skipped: c.reparse_skipped,
                placeholders_skipped: c.placeholders,
                other_volume_skipped: c.other_volume_skipped,
                special_skipped: c.special_skipped,
                vanished: c.vanished,
                hard_link_aliases: c.hard_link_aliases,
                changed_during_scan: c.changed_during_scan,
                allocation_unknown_files: c.allocation_unknown_files,
                errors: c.errors,
                truncated: c.truncated.is_some(),
                cancelled: c.cancelled,
            }),
            limitations: limits,
        });
        if matches!(
            status,
            ElevationStatus::Cancelled | ElevationStatus::TimedOut
        ) {
            response.status = status;
            break;
        }
    }
    if !matches!(
        response.status,
        ElevationStatus::Cancelled | ElevationStatus::TimedOut
    ) {
        // A directory total is not a restore-point total; this category stays unknown.
        response.caches.push(CacheSummary::unavailable(
            CacheCategory::RestorePoints,
            ElevationStatus::Unsupported,
            ElevationLimit::RestorePointsUnimplemented,
        ));
        response.status = ElevationStatus::Partial;
    }
    response.window = Window::finish(started, clock);
    Ok(response)
}

#[cfg(test)]
mod tests {
    use super::*;
    use pcagent_shared::new_id;
    use std::time::Duration;

    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let path = std::env::temp_dir().join(new_id("elev-cache-fixture"));
            std::fs::create_dir(&path).unwrap();
            Self(path)
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            // Only this newly created fixture subtree is removed.
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn controlled_cache_is_read_only_and_no_reclaim_is_claimed() {
        let fixture = Fixture::new();
        let cache = fixture.0.join("cache");
        std::fs::create_dir(&cache).unwrap();
        std::fs::write(cache.join("data.bin"), [0_u8; 17]).unwrap();
        std::fs::hard_link(cache.join("data.bin"), cache.join("alias.bin")).unwrap();
        let request =
            ElevationRequest::new(ElevatedTask::SystemCaches, Duration::from_secs(10)).unwrap();
        let roots = [CacheRoot {
            category: CacheCategory::WindowsUpdateDownload,
            path: cache.clone(),
        }];
        let response = collect(&request, &roots, &mut || Ok(false)).unwrap();
        assert_eq!(response.status, ElevationStatus::Partial);
        assert_eq!(response.caches[0].observed_logical_bytes, Some(17));
        assert_eq!(response.caches[0].files, Some(1));
        assert_eq!(
            response.caches[0]
                .coverage
                .as_ref()
                .unwrap()
                .hard_link_aliases,
            1
        );
        assert_eq!(response.caches[1].observed_allocated_bytes, None);
        assert_eq!(std::fs::read(cache.join("data.bin")).unwrap().len(), 17);
        assert!(cache.join("alias.bin").exists());
        let encoded = serde_json::to_string(&response).unwrap();
        assert!(!encoded.contains("reclaim"));
        assert!(!encoded.contains(&fixture.0.to_string_lossy().to_string()));
        assert!(response.validate(&request).is_ok());
    }

    #[test]
    fn mft_remains_unsupported_and_cancel_does_not_scan() {
        let request = ElevationRequest::new(
            ElevatedTask::MftInventory {
                volume: super::super::DriveLetter::parse("C").unwrap(),
            },
            Duration::from_secs(10),
        )
        .unwrap();
        let response = collect(&request, &[], &mut || {
            panic!("MFT must not enumerate paths")
        })
        .unwrap();
        assert_eq!(response.status, ElevationStatus::Unsupported);
        assert!(response.caches.is_empty());
        let request =
            ElevationRequest::new(ElevatedTask::SystemCaches, Duration::from_secs(10)).unwrap();
        let roots = [CacheRoot {
            category: CacheCategory::PreviousWindows,
            path: PathBuf::from("unreadable-fixture"),
        }];
        let response = collect(&request, &roots, &mut || Ok(true)).unwrap();
        assert_eq!(response.status, ElevationStatus::Cancelled);
        assert!(response.caches.is_empty());
    }
}
