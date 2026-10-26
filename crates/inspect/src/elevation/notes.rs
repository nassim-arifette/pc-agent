//! Why an elevated result is not complete, as closed codes the desktop window words in its own
//! language. The inspection's English coverage `detail` stays the C4 text the model can read;
//! these notes reach only the local window, like `local_mft_tree`, and never carry Rust Debug
//! output, paths or identities.
use super::{mft::CollectionEnd, ElevationResponse};
use serde::Serialize;

/// The window's explanation of one coverage entry of [`super::to_inspection`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CoverageNote {
    /// The coverage `scope` this note explains.
    pub scope: String,
    /// What the scope measures and leaves out, as a closed key.
    pub note: &'static str,
    /// Closed reasons (`family.code`), in the helper's order.
    pub reasons: Vec<CoverageReason>,
}

/// One closed reason, with its count when it counts skipped items.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CoverageReason {
    pub code: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub count: Option<u64>,
}

/// `family.code` from a closed enum's own serialized (snake_case) name.
fn reason(family: &str, value: &impl Serialize) -> CoverageReason {
    let name = serde_json::to_value(value)
        .ok()
        .and_then(|value| value.as_str().map(str::to_owned))
        .unwrap_or_else(|| "unknown".into());
    CoverageReason {
        code: format!("{family}.{name}"),
        count: None,
    }
}

fn counted(code: &str, count: u64) -> Option<CoverageReason> {
    (count > 0).then(|| CoverageReason {
        code: code.into(),
        count: Some(count),
    })
}

fn flag(code: &str, set: bool) -> Option<CoverageReason> {
    set.then(|| CoverageReason {
        code: code.into(),
        count: None,
    })
}

fn note(
    scope: &str,
    note: &'static str,
    reasons: impl IntoIterator<Item = CoverageReason>,
) -> CoverageNote {
    CoverageNote {
        scope: scope.into(),
        note,
        reasons: reasons.into_iter().collect(),
    }
}

/// One note per coverage entry that [`super::to_inspection`] writes for this response, with the
/// same scopes, so the window can explain an incomplete entry without its English detail.
pub fn coverage_notes(response: &ElevationResponse) -> Vec<CoverageNote> {
    let mut notes = Vec::new();
    for cache in &response.caches {
        let mut reasons: Vec<CoverageReason> = cache
            .limitations
            .iter()
            .map(|limit| reason("elevation", limit))
            .collect();
        if let Some(c) = &cache.coverage {
            reasons.extend(
                [
                    counted("cache.denied_directories", c.denied_directories),
                    counted("cache.denied_files", c.denied_files),
                    counted("cache.reparse_skipped", c.reparse_skipped),
                    counted("cache.placeholders_skipped", c.placeholders_skipped),
                    counted("cache.other_volume_skipped", c.other_volume_skipped),
                    counted("cache.special_skipped", c.special_skipped),
                    counted("cache.vanished", c.vanished),
                    counted("cache.hard_link_aliases", c.hard_link_aliases),
                    counted("cache.changed_during_scan", c.changed_during_scan),
                    counted("cache.allocation_unknown_files", c.allocation_unknown_files),
                    counted("cache.errors", c.errors),
                    flag("cache.truncated", c.truncated),
                    flag("cache.cancelled", c.cancelled),
                ]
                .into_iter()
                .flatten(),
            );
        }
        notes.push(note(cache.category.as_str(), "system_cache", reasons));
    }
    if let Some(trace) = &response.performance_trace {
        notes.push(note(
            "performance_trace",
            "performance_trace",
            trace.limitations.iter().map(|limit| reason("trace", limit)),
        ));
    }
    if let Some(collection) = &response.mft {
        let mut reasons: Vec<CoverageReason> = [
            (collection.end != CollectionEnd::Enumerated).then(|| reason("end", &collection.end)),
            flag(
                "mft.allocation_not_additive",
                !collection.allocation_additive,
            ),
        ]
        .into_iter()
        .flatten()
        .collect();
        reasons.extend(
            collection
                .summary
                .limitations
                .iter()
                .map(|limit| reason("mft", limit)),
        );
        notes.push(note("ntfs_inventory", "ntfs_inventory", reasons));
        notes.push(note(
            "ntfs_tree",
            "ntfs_tree",
            collection
                .tree
                .limitations
                .iter()
                .map(|limit| reason("tree", limit)),
        ));
    }
    let system = |limits: &[super::system::SystemReadLimit]| {
        limits
            .iter()
            .map(|limit| reason("system", limit))
            .collect::<Vec<_>>()
    };
    if let Some(summary) = &response.shadow_storage {
        notes.push(note(
            "shadow_storage",
            "shadow_storage",
            system(&summary.limitations),
        ));
    }
    if let Some(summary) = &response.disk_reliability {
        notes.push(note(
            "disk_reliability",
            "disk_reliability",
            system(&summary.limitations),
        ));
    }
    if let Some(summary) = &response.process_images {
        notes.push(note(
            "protected_process_images",
            "protected_process_images",
            system(&summary.limitations),
        ));
    }
    if let Some(summary) = &response.battery_energy {
        notes.push(note(
            "battery_energy_history",
            "battery_energy_history",
            summary
                .limitations
                .iter()
                .map(|limit| reason("energy", limit)),
        ));
    }
    if notes.is_empty() {
        notes.push(note(
            "elevated_task",
            "elevated_task",
            response
                .limitations
                .iter()
                .map(|limit| reason("elevation", limit)),
        ));
    }
    notes
}

#[cfg(test)]
mod tests {
    use super::super::{
        mft::{Collection, CollectionEnd, Limit, TreeLimit},
        protocol::{
            CacheCategory, CacheCoverage, CacheSummary, ElevatedTask, ElevationLimit,
            ElevationRequest, ElevationResponse, ElevationStatus,
        },
        system::{SystemReadLimit, SystemReadSummary},
        to_inspection, DriveLetter,
    };
    use super::*;
    use std::time::Duration;

    fn empty_response(task: ElevatedTask, limit: ElevationLimit) -> ElevationResponse {
        let request = ElevationRequest::new(task, Duration::from_secs(10)).unwrap();
        ElevationResponse::empty(&request, ElevationStatus::Partial, limit)
    }

    fn codes(note: &CoverageNote) -> Vec<(&str, Option<u64>)> {
        note.reasons
            .iter()
            .map(|r| (r.code.as_str(), r.count))
            .collect()
    }

    /// Every note explains a coverage entry of the same inspection, in the same order.
    fn same_scopes(response: &ElevationResponse, notes: &[CoverageNote]) {
        let inspection = to_inspection(response);
        let scopes: Vec<&str> = inspection
            .coverage
            .iter()
            .map(|c| c.scope.as_str())
            .collect();
        let noted: Vec<&str> = notes.iter().map(|n| n.scope.as_str()).collect();
        assert_eq!(scopes, noted);
    }

    #[test]
    fn mft_notes_name_the_end_the_limits_and_the_tree_limits_without_debug_output() {
        let mut collection = Collection::cancelled_before_open();
        collection.end = CollectionEnd::RecordLimit;
        collection.allocation_additive = false;
        collection.summary.limitations.insert(Limit::RecordLimit);
        collection
            .tree
            .limitations
            .insert(TreeLimit::TopDirectoriesTruncated);
        let mut response = empty_response(
            ElevatedTask::MftInventory {
                volume: DriveLetter::parse("C").unwrap(),
            },
            ElevationLimit::MetadataSnapshot,
        );
        response.mft = Some(collection);
        let notes = coverage_notes(&response);
        same_scopes(&response, &notes);
        assert_eq!(notes[0].note, "ntfs_inventory");
        let inventory = codes(&notes[0]);
        assert_eq!(inventory[0], ("end.record_limit", None));
        assert_eq!(inventory[1], ("mft.allocation_not_additive", None));
        assert!(inventory.contains(&("mft.record_limit", None)));
        assert!(inventory.contains(&("mft.cancelled", None)));
        assert_eq!(codes(&notes[1]), [("tree.top_directories_truncated", None)]);
        let json = serde_json::to_string(&notes).unwrap();
        for debug in ["RecordLimit", "TopDirectoriesTruncated", "Some(", "Limits:"] {
            assert!(!json.contains(debug), "{json}");
        }
        // An enumerated volume does not report its end as a limit.
        let mut whole = Collection::cancelled_before_open();
        whole.end = CollectionEnd::Enumerated;
        response.mft = Some(whole);
        assert!(!codes(&coverage_notes(&response)[0])
            .iter()
            .any(|(code, _)| code.starts_with("end.")));
    }

    #[test]
    fn cache_notes_count_what_the_walk_skipped() {
        let mut response =
            empty_response(ElevatedTask::SystemCaches, ElevationLimit::MetadataSnapshot);
        response.caches.push(CacheSummary {
            category: CacheCategory::DeliveryOptimization,
            status: ElevationStatus::Partial,
            exists: Some(true),
            observed_logical_bytes: Some(10),
            observed_allocated_bytes: Some(12),
            files: Some(3),
            coverage: Some(CacheCoverage {
                directories: 41,
                denied_directories: 2,
                denied_files: 0,
                reparse_skipped: 0,
                placeholders_skipped: 0,
                other_volume_skipped: 0,
                special_skipped: 0,
                vanished: 0,
                hard_link_aliases: 0,
                changed_during_scan: 0,
                allocation_unknown_files: 5,
                errors: 0,
                truncated: true,
                cancelled: false,
            }),
            limitations: vec![
                ElevationLimit::MetadataSnapshot,
                ElevationLimit::DefaultCacheLocationOnly,
            ],
        });
        let notes = coverage_notes(&response);
        same_scopes(&response, &notes);
        assert_eq!(notes[0].scope, "delivery_optimization");
        assert_eq!(notes[0].note, "system_cache");
        assert_eq!(
            codes(&notes[0]),
            [
                ("elevation.metadata_snapshot", None),
                ("elevation.default_cache_location_only", None),
                ("cache.denied_directories", Some(2)),
                ("cache.allocation_unknown_files", Some(5)),
                ("cache.truncated", None),
            ]
        );
    }

    #[test]
    fn system_reads_and_empty_results_have_their_own_notes() {
        let mut response = empty_response(
            ElevatedTask::DiskReliability,
            ElevationLimit::MetadataSnapshot,
        );
        response.disk_reliability = Some(SystemReadSummary {
            status: ElevationStatus::Partial,
            samples: Vec::new(),
            truncated: false,
            read_errors: 0,
            limitations: vec![SystemReadLimit::ProviderValueMissing],
        });
        let notes = coverage_notes(&response);
        same_scopes(&response, &notes);
        assert_eq!(notes[0].note, "disk_reliability");
        assert_eq!(codes(&notes[0]), [("system.provider_value_missing", None)]);
        let empty = empty_response(
            ElevatedTask::ShadowStorage,
            ElevationLimit::SourceUnavailable,
        );
        let notes = coverage_notes(&empty);
        same_scopes(&empty, &notes);
        assert_eq!(notes[0].note, "elevated_task");
        assert_eq!(codes(&notes[0]), [("elevation.source_unavailable", None)]);
    }
}
