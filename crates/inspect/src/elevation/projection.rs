//! The only C4 projection of privileged DTOs. Paths, PIDs, creation times, raw app/user IDs,
//! journal IDs and trace identities never enter an observation. Names use the C5 dictionary.
use super::{ElevationResponse, ElevationStatus};
use crate::{CoverageStatus, Inspection};
use serde_json::{json, Value};

fn status(value: ElevationStatus) -> CoverageStatus {
    match value {
        ElevationStatus::Completed => CoverageStatus::Ok,
        ElevationStatus::Denied => CoverageStatus::Denied,
        ElevationStatus::Unsupported => CoverageStatus::Unsupported,
        ElevationStatus::Error => CoverageStatus::Error,
        _ => CoverageStatus::Partial,
    }
}

fn known_status(value: ElevationStatus) -> CoverageStatus {
    if matches!(
        value,
        ElevationStatus::Completed
            | ElevationStatus::Partial
            | ElevationStatus::Cancelled
            | ElevationStatus::TimedOut
    ) {
        CoverageStatus::Ok
    } else {
        status(value)
    }
}
fn measure(
    out: &mut Inspection,
    entity: &str,
    metric: &str,
    value: Value,
    unit: &str,
    source: &str,
    coverage: CoverageStatus,
) {
    let missing = value.is_null();
    out.value(entity, metric, value, unit, source);
    if let Some(last) = out.observations.last_mut() {
        last.status = if missing && coverage == CoverageStatus::Ok {
            CoverageStatus::Unsupported
        } else {
            coverage
        };
    }
}
pub(super) fn add(out: &mut Inspection, response: &ElevationResponse) {
    if let Some(summary) = &response.performance_trace {
        let source = "Fixed private ETW DPC/ISR/disk session";
        for sample in &summary.samples {
            let entity = out.entity("trace-class", None);
            measure(
                out,
                &entity,
                "trace_class",
                json!(sample.class),
                "category",
                source,
                CoverageStatus::Ok,
            );
            for (metric, value, unit) in [
                (
                    "trace_events_accepted",
                    Some(sample.accepted_events),
                    "count",
                ),
                ("trace_transfer_bytes_known", sample.transfer_bytes, "bytes"),
                (
                    "trace_duration_total",
                    sample.total_duration_us,
                    "microseconds",
                ),
                (
                    "trace_duration_maximum",
                    sample.maximum_duration_us,
                    "microseconds",
                ),
                ("trace_schema_errors", Some(sample.schema_errors), "count"),
                ("trace_timing_errors", Some(sample.timing_errors), "count"),
                (
                    "trace_boundary_events_excluded",
                    Some(sample.boundary_events),
                    "count",
                ),
            ] {
                measure(
                    out,
                    &entity,
                    metric,
                    json!(value),
                    unit,
                    source,
                    known_status(sample.status),
                );
            }
        }
        let entity = out.entity("trace-session", None);
        for (metric, value, unit) in [
            (
                "trace_observed_duration",
                summary.observed_duration_us,
                "microseconds",
            ),
            ("trace_events_lost", summary.events_lost, "count"),
            (
                "trace_realtime_buffers_lost",
                summary.realtime_buffers_lost,
                "count",
            ),
        ] {
            measure(
                out,
                &entity,
                metric,
                json!(value),
                unit,
                source,
                known_status(summary.status),
            );
        }
        out.coverage("performance_trace", status(summary.status), source,
            &format!("Native collection requires validated cleanup; no driver identity or causal attribution. Unknown loss counters remain unknown. Limits: {:?}.", summary.limitations));
    }
    if let Some(collection) = &response.mft {
        let entity = out.entity("ntfs-volume", None);
        let source = "FSCTL_GET_NTFS_VOLUME_DATA / FSCTL_GET_NTFS_FILE_RECORD";
        let summary = &collection.summary;
        for (metric, value, unit) in [
            (
                "ntfs_supported_logical_bytes_known",
                Some(summary.logical_bytes_known),
                "bytes",
            ),
            (
                "ntfs_supported_data_allocation_bytes_known",
                collection
                    .allocation_additive
                    .then_some(summary.data_cluster_bytes_known),
                "bytes",
            ),
            (
                "ntfs_files_examined",
                Some(summary.base_files as u64),
                "count",
            ),
            (
                "ntfs_hard_linked_files",
                Some(summary.hard_linked_files as u64),
                "count",
            ),
            (
                "ntfs_sparse_files",
                Some(summary.sparse_files as u64),
                "count",
            ),
            (
                "ntfs_compressed_files",
                Some(summary.compressed_files as u64),
                "count",
            ),
            (
                "ntfs_logical_size_unknown_files",
                Some(summary.unknown_logical_files as u64),
                "count",
            ),
            (
                "ntfs_allocation_unknown_files",
                Some(summary.unknown_data_allocation_files as u64),
                "count",
            ),
            (
                "ntfs_data_streams",
                Some(summary.data_streams as u64),
                "count",
            ),
            (
                "ntfs_named_data_streams",
                Some(summary.named_streams as u64),
                "count",
            ),
            (
                "ntfs_named_logical_bytes_known",
                Some(summary.named_logical_bytes_known),
                "bytes",
            ),
            (
                "ntfs_record_slots_walked",
                Some(collection.record_slots_walked),
                "count",
            ),
            (
                "ntfs_record_slots_not_walked",
                Some(collection.record_slots_not_walked),
                "count",
            ),
            (
                "ntfs_invalid_records",
                Some(collection.invalid_records),
                "count",
            ),
        ] {
            measure(
                out,
                &entity,
                metric,
                json!(value),
                unit,
                source,
                CoverageStatus::Ok,
            );
        }
        measure(
            out,
            &entity,
            "ntfs_volume_changed_during_scan",
            if collection.change_detected || collection.change_witness_available {
                json!(collection.change_detected)
            } else {
                Value::Null
            },
            "boolean",
            source,
            CoverageStatus::Ok,
        );
        measure(
            out,
            &entity,
            "ntfs_change_witness_available",
            json!(collection.change_witness_available),
            "boolean",
            source,
            CoverageStatus::Ok,
        );
        out.coverage("ntfs_inventory", CoverageStatus::Partial, source,
            &format!("Supported metadata streams only; live volume is not atomic. Allocation excludes metadata and is not reclaimable bytes. End: {:?}; additive: {}; limits: {:?}.",
                collection.end, collection.allocation_additive, summary.limitations));
        let tree = &collection.tree;
        for (metric, value, unit) in [
            (
                "ntfs_tree_attributed_files",
                Some(tree.attributed_files),
                "count",
            ),
            (
                "ntfs_tree_unattributed_files",
                Some(tree.unattributed_files),
                "count",
            ),
            (
                "ntfs_tree_attributed_logical_bytes_known",
                Some(tree.attributed_logical_bytes_known),
                "bytes",
            ),
            (
                "ntfs_tree_unattributed_logical_bytes_known",
                Some(tree.unattributed_logical_bytes_known),
                "bytes",
            ),
            (
                "ntfs_tree_attributed_allocation_bytes_known",
                tree.attributed_data_cluster_bytes_known,
                "bytes",
            ),
            (
                "ntfs_tree_unattributed_allocation_bytes_known",
                tree.unattributed_data_cluster_bytes_known,
                "bytes",
            ),
        ] {
            measure(
                out,
                &entity,
                metric,
                json!(value),
                unit,
                source,
                CoverageStatus::Ok,
            );
        }
        for (category, items) in [("directory", &tree.directories), ("file", &tree.files)] {
            for item in items {
                let entity = out.entity("ntfs-item", None);
                measure(
                    out,
                    &entity,
                    "ntfs_item_category",
                    json!(category),
                    "category",
                    source,
                    CoverageStatus::Ok,
                );
                for (metric, value, unit) in [
                    (
                        "ntfs_item_logical_bytes_known",
                        Some(item.logical_bytes_known),
                        "bytes",
                    ),
                    (
                        "ntfs_item_allocation_bytes_known",
                        item.data_cluster_bytes_known,
                        "bytes",
                    ),
                    ("ntfs_item_files", Some(item.files), "count"),
                    (
                        "ntfs_item_logical_unknown_files",
                        Some(item.unknown_logical_files),
                        "count",
                    ),
                    (
                        "ntfs_item_allocation_unknown_files",
                        Some(item.unknown_allocation_files),
                        "count",
                    ),
                    (
                        "ntfs_item_hard_linked_files",
                        Some(item.hard_linked_files),
                        "count",
                    ),
                ] {
                    measure(
                        out,
                        &entity,
                        metric,
                        json!(value),
                        unit,
                        source,
                        CoverageStatus::Ok,
                    );
                }
            }
        }
        out.coverage("ntfs_tree", CoverageStatus::Partial, source,
            &format!("Top items from indexed metadata only; paths remain local. Directory subtrees overlap and cannot be summed. Hard-linked files are attributed once; unattributed sizes remain separate. Limits: {:?}.", tree.limitations));
    }
    if let Some(summary) = &response.shadow_storage {
        let source = "WMI Win32_ShadowStorage";
        for sample in &summary.samples {
            let entity = out.entity("shadow-storage", None);
            let coverage = if sample.values_consistent == Some(false) {
                CoverageStatus::Partial
            } else {
                known_status(summary.status)
            };
            for (metric, value) in [
                ("shadow_storage_used_bytes", sample.used_bytes),
                ("shadow_storage_allocated_bytes", sample.allocated_bytes),
                ("shadow_storage_maximum_bytes", sample.maximum_bytes),
            ] {
                measure(
                    out,
                    &entity,
                    metric,
                    json!(value),
                    "bytes",
                    source,
                    coverage,
                );
            }
            measure(
                out,
                &entity,
                "shadow_storage_maximum_unlimited",
                json!(sample.maximum_unlimited),
                "boolean",
                source,
                coverage,
            );
        }
        out.coverage("shadow_storage", status(summary.status), source,
            &format!("Includes other shadow copies; unavailable per-restore-point attribution. Not reclaimable bytes. Limits: {:?}.", summary.limitations));
    }
    if let Some(summary) = &response.disk_reliability {
        let source = "WMI root/Microsoft/Windows/Storage MSFT_StorageReliabilityCounter";
        for sample in &summary.samples {
            let entity = out.entity("disk-reliability", None);
            let coverage = if sample.values_consistent() == Some(false) {
                CoverageStatus::Partial
            } else {
                known_status(summary.status)
            };
            for (metric, value, unit) in [
                (
                    "disk_reliability_temperature",
                    sample.temperature_celsius.map(u64::from),
                    "celsius",
                ),
                (
                    "disk_reliability_temperature_limit",
                    sample.temperature_limit_celsius.map(u64::from),
                    "celsius",
                ),
                (
                    "disk_reliability_wear",
                    sample.wear_percent.map(u64::from),
                    "percent",
                ),
                (
                    "disk_reliability_power_on_hours",
                    sample.power_on_hours.map(u64::from),
                    "hours",
                ),
                (
                    "disk_reliability_read_errors_total",
                    sample.read_errors_total,
                    "count",
                ),
                (
                    "disk_reliability_read_errors_uncorrected",
                    sample.read_errors_uncorrected,
                    "count",
                ),
                (
                    "disk_reliability_write_errors_total",
                    sample.write_errors_total,
                    "count",
                ),
                (
                    "disk_reliability_write_errors_uncorrected",
                    sample.write_errors_uncorrected,
                    "count",
                ),
                (
                    "disk_reliability_start_stop_cycles",
                    sample.start_stop_cycles.map(u64::from),
                    "count",
                ),
                (
                    "disk_reliability_maximum_read_latency",
                    sample.maximum_read_latency_ms,
                    "ms",
                ),
                (
                    "disk_reliability_maximum_write_latency",
                    sample.maximum_write_latency_ms,
                    "ms",
                ),
                (
                    "disk_reliability_maximum_flush_latency",
                    sample.maximum_flush_latency_ms,
                    "ms",
                ),
            ] {
                measure(out, &entity, metric, json!(value), unit, source, coverage);
            }
        }
        out.coverage(
            "disk_reliability",
            status(summary.status),
            source,
            &format!(
                "Device-reported historical counters; absent fields are unknown. Limits: {:?}.",
                summary.limitations
            ),
        );
    }
    if let Some(summary) = &response.process_images {
        let source = "OpenProcess / GetProcessTimes / QueryFullProcessImageNameW";
        for sample in &summary.samples {
            let entity = out.entity(
                "protected-process",
                sample.executable_basename().map(str::to_owned),
            );
            measure(
                out,
                &entity,
                "process_image_readable",
                json!(sample.image_path.as_ref().map(|_| true)),
                "boolean",
                source,
                status(sample.status),
            );
        }
        out.coverage("protected_process_images", status(summary.status), source,
            &format!("Creation time and process lifetime revalidated before/after path read. Full paths and identities stay local. Limits: {:?}.", summary.limitations));
    }
    if let Some(summary) = &response.battery_energy {
        let source = "Windows E3 historical estimates / powercfg srumutil CSV";
        let coverage = known_status(summary.status);
        let entity = out.entity("battery-energy-history", None);
        for (metric, value, unit) in [
            (
                "battery_energy_estimated_total",
                summary.estimated_energy_millijoules,
                "mJ",
            ),
            (
                "battery_energy_committed_rows",
                Some(u64::from(summary.committed_on_battery_rows)),
                "count",
            ),
            (
                "battery_energy_distinct_apps",
                Some(u64::from(summary.distinct_apps)),
                "count",
            ),
        ] {
            measure(out, &entity, metric, json!(value), unit, source, coverage);
        }
        for sample in &summary.samples {
            let entity = out.entity("battery-energy-app", sample.executable_name.clone());
            for (metric, value, unit) in [
                (
                    "battery_app_energy_estimated",
                    sample.estimated_energy_millijoules,
                    "mJ",
                ),
                (
                    "battery_app_screen_on_energy_estimated",
                    sample.screen_on_energy_millijoules,
                    "mJ",
                ),
                (
                    "battery_app_foreground_energy_estimated",
                    sample.foreground_energy_millijoules,
                    "mJ",
                ),
                (
                    "battery_app_committed_rows",
                    u64::from(sample.committed_rows),
                    "count",
                ),
                (
                    "battery_app_reported_row_durations",
                    sample.reported_row_duration_ms,
                    "ms",
                ),
            ] {
                measure(out, &entity, metric, json!(value), unit, source, coverage);
            }
            measure(
                out,
                &entity,
                "battery_app_historical_first_civil_stamp",
                json!(sample.first_civil_stamp.display()),
                "provider_civil_time",
                source,
                coverage,
            );
            measure(
                out,
                &entity,
                "battery_app_historical_last_civil_stamp",
                json!(sample.last_civil_stamp.display()),
                "provider_civil_time",
                source,
                coverage,
            );
        }
        out.coverage("battery_energy_history", status(summary.status), source,
            &format!("Historical E3 estimates for accepted committed on-battery rows of this user only. Civil timestamps have unknown timezone; summed row durations may overlap and are not a measurement window. Export time is the read time. Not physical power, exhaustive app attribution or battery discharge. Limits: {:?}.", summary.limitations));
    }
}

#[cfg(test)]
mod tests {
    use super::super::fixtures::{empty_response, request, validate_response};
    use super::super::{
        energy::{parse_export, EnergySummary},
        mft::{Collection, CollectionEnd, Limit, TreeItem, TreeLimit, TreeSummary},
        system::{DiskReliabilitySample, ShadowStorageSample, SystemReadLimit, SystemReadSummary},
        to_inspection, ElevatedTask, ElevationLimit, ElevationResponse, ElevationStatus,
    };
    use crate::CoverageStatus;
    use pcagent_shared::Status;
    use serde_json::json;
    use std::time::Duration;

    fn energy() -> EnergySummary {
        let csv = concat!(
            "AppId,UserId,TimeStamp,OnBattery,ScreenOn,BatterySaverActive,LowPowerEpochActive,Foreground,InteractivityState,Container,Committed,TimeInMSec,MeasuredBitmap,TotalEnergyConsumption,CPUEnergyConsumption\r\n",
            "\"C:\\Users\\private-user\\Application\\app.exe\",S-1-5-21-111-222-333-1001,2020-09-08:20:13:00.0000,TRUE,TRUE,FALSE,FALSE,TRUE,NotUnique,FALSE,TRUE,1000,b00000000,7,900000\r\n"
        );
        parse_export(csv.as_bytes(), "S-1-5-21-111-222-333-1001")
    }

    fn mft_collection() -> Collection {
        let mut collection = Collection::cancelled_before_open();
        collection.end = CollectionEnd::Enumerated;
        collection.record_slots_at_start = 40;
        collection.record_slots_walked = 40;
        collection.summary.limitations.remove(&Limit::Cancelled);
        collection
    }

    fn mft_response(collection: Collection) -> ElevationResponse {
        let request = request(
            ElevatedTask::MftInventory {
                volume: super::super::DriveLetter::parse("C").unwrap(),
            },
            Duration::from_secs(10),
        );
        let mut response = empty_response(
            &request,
            ElevationStatus::Partial,
            ElevationLimit::MetadataSnapshot,
        );
        response.mft = Some(collection);
        validate_response(&response, &request).unwrap();
        response
    }

    #[test]
    fn mft_unknown_change_witness_is_unknown_and_detected_changes_remain_usable() {
        for (available, changed, expected, expected_status) in [
            (
                false,
                false,
                serde_json::Value::Null,
                CoverageStatus::Unsupported,
            ),
            (true, false, json!(false), CoverageStatus::Ok),
            (false, true, json!(true), CoverageStatus::Ok),
            (true, true, json!(true), CoverageStatus::Ok),
        ] {
            let mut collection = mft_collection();
            collection.change_witness_available = available;
            collection.change_detected = changed;
            if available {
                collection
                    .summary
                    .limitations
                    .remove(&Limit::ChangeWitnessUnavailable);
            }
            if changed {
                collection.summary.limitations.insert(Limit::VolumeChanged);
            }
            let inspection = to_inspection(&mft_response(collection));
            let change = inspection
                .observations
                .iter()
                .find(|item| item.metric == "ntfs_volume_changed_during_scan")
                .unwrap();
            assert_eq!(change.value, expected);
            assert_eq!(change.status, expected_status);
            let witness = inspection
                .observations
                .iter()
                .find(|item| item.metric == "ntfs_change_witness_available")
                .unwrap();
            assert_eq!(witness.value, json!(available));
            assert_eq!(witness.status, CoverageStatus::Ok);
            assert_eq!(inspection.status, Status::Partial);
        }
    }

    fn tree_item(path: &str, logical: u64, allocated: u64) -> TreeItem {
        TreeItem {
            path: path.into(),
            files: 1,
            logical_bytes_known: logical,
            data_cluster_bytes_known: Some(allocated),
            unknown_logical_files: 0,
            unknown_allocation_files: 0,
            hard_linked_files: 0,
        }
    }

    #[test]
    fn local_mft_items_zip_with_opaque_categories_and_keep_nonadditive_bytes_hidden() {
        for additive in [true, false] {
            let mut collection = mft_collection();
            collection.records_examined = 5;
            collection.summary.input_records = 5;
            collection.summary.base_files = 2;
            collection.summary.data_streams = 2;
            collection.summary.logical_bytes_known = 300;
            collection.summary.data_cluster_bytes_known = 12_288;
            collection.retained_bytes_high_water = 4096;
            collection.tree = TreeSummary {
                directories: vec![
                    tree_item(r"\Users\private-owner\large", 200, 8192),
                    tree_item(r"\Users\private-owner\small", 100, 4096),
                ],
                files: vec![
                    tree_item(r"\Users\private-owner\large\large.bin", 200, 8192),
                    tree_item(r"\Users\private-owner\small\small.bin", 100, 4096),
                ],
                indexed_nodes: 4,
                indexed_name_links: 4,
                retained_bytes: 4096,
                attributed_files: 2,
                attributed_logical_bytes_known: 300,
                attributed_data_cluster_bytes_known: Some(12_288),
                ..TreeSummary::default()
            };
            if !additive {
                collection.records_examined += 1;
                collection.summary.input_records += 1;
                collection.invalid_records = 1;
                collection.allocation_additive = false;
                collection.summary.limitations.insert(Limit::InvalidRecord);
                collection
                    .tree
                    .limitations
                    .insert(TreeLimit::AllocationNotAdditive);
                collection.tree.attributed_data_cluster_bytes_known = None;
                collection.tree.unattributed_data_cluster_bytes_known = None;
                for item in collection
                    .tree
                    .directories
                    .iter_mut()
                    .chain(&mut collection.tree.files)
                {
                    item.data_cluster_bytes_known = None;
                }
            }
            let response = mft_response(collection);
            let tree = &response.mft.as_ref().unwrap().tree;
            let inspection = to_inspection(&response);
            let categories: Vec<_> = inspection
                .observations
                .iter()
                .filter(|item| item.metric == "ntfs_item_category")
                .collect();
            let local_items: Vec<_> = tree
                .directories
                .iter()
                .map(|item| ("directory", item))
                .chain(tree.files.iter().map(|item| ("file", item)))
                .collect();
            assert_eq!(categories.len(), local_items.len());
            let mut references = std::collections::BTreeSet::new();
            for (category, (expected_category, local)) in categories.iter().zip(local_items) {
                assert_eq!(category.value, json!(expected_category));
                assert!(category.entity_ref.starts_with("ntfs-item-"));
                assert!(references.insert(&category.entity_ref));
                assert!(!inspection.names.contains_key(&category.entity_ref));
                let logical = inspection
                    .observations
                    .iter()
                    .find(|item| {
                        item.entity_ref == category.entity_ref
                            && item.metric == "ntfs_item_logical_bytes_known"
                    })
                    .unwrap();
                assert_eq!(logical.value, json!(local.logical_bytes_known));
                let allocation = inspection
                    .observations
                    .iter()
                    .find(|item| {
                        item.entity_ref == category.entity_ref
                            && item.metric == "ntfs_item_allocation_bytes_known"
                    })
                    .unwrap();
                assert_eq!(allocation.value, json!(local.data_cluster_bytes_known));
                assert_eq!(
                    allocation.status,
                    if additive {
                        CoverageStatus::Ok
                    } else {
                        CoverageStatus::Unsupported
                    }
                );
            }
            if !additive {
                for metric in [
                    "ntfs_supported_data_allocation_bytes_known",
                    "ntfs_tree_attributed_allocation_bytes_known",
                    "ntfs_tree_unattributed_allocation_bytes_known",
                ] {
                    let measurement = inspection
                        .observations
                        .iter()
                        .find(|item| item.metric == metric)
                        .unwrap();
                    assert!(measurement.value.is_null());
                    assert_eq!(measurement.status, CoverageStatus::Unsupported);
                }
            }
            let serialized = serde_json::to_string(&inspection).unwrap();
            assert!(!serialized.contains("private-owner"));
            assert!(!serialized.contains("large.bin"));
            assert!(!serialized.contains("small.bin"));
            assert!(!serialized.contains("local_mft_tree"));
            assert_eq!(inspection.status, Status::Partial);
        }
    }

    #[test]
    fn accepted_historical_estimates_are_usable_but_never_complete_coverage() {
        let request = request(ElevatedTask::BatteryEnergy, Duration::from_secs(10));
        let mut response = empty_response(
            &request,
            ElevationStatus::Partial,
            ElevationLimit::MetadataSnapshot,
        );
        response.battery_energy = Some(energy());
        validate_response(&response, &request).unwrap();
        let inspection = to_inspection(&response);
        assert_eq!(inspection.status, Status::Partial);
        let coverage = inspection
            .coverage
            .iter()
            .find(|item| item.scope == "battery_energy_history")
            .unwrap();
        assert_eq!(coverage.status, CoverageStatus::Partial);
        assert!(coverage.detail.contains("Historical E3 estimates"));
        assert!(coverage
            .detail
            .contains("accepted committed on-battery rows"));
        assert!(coverage.detail.contains("unknown timezone"));
        let measured = inspection
            .observations
            .iter()
            .find(|item| item.metric == "battery_energy_estimated_total")
            .unwrap();
        assert_eq!(measured.value, json!(7));
        assert_eq!(measured.unit, "mJ");
        assert_eq!(measured.status, CoverageStatus::Ok);
        assert!(measured.source.contains("historical estimates"));
        let civil = inspection
            .observations
            .iter()
            .find(|item| item.metric == "battery_app_historical_first_civil_stamp")
            .unwrap();
        assert_eq!(civil.unit, "provider_civil_time");
        assert!(civil.value.as_str().unwrap().contains("zone unknown"));
        assert_eq!(
            civil.window.started_at_unix_ms,
            response.window.started_at_unix_ms
        );
        assert!(!civil.value.as_str().unwrap().contains("UTC"));
    }

    #[test]
    fn raw_energy_identities_and_paths_are_absent_from_serialized_inspection() {
        let request = request(ElevatedTask::BatteryEnergy, Duration::from_secs(10));
        let mut response = empty_response(
            &request,
            ElevationStatus::Partial,
            ElevationLimit::MetadataSnapshot,
        );
        response.battery_energy = Some(energy());
        let inspection = to_inspection(&response);
        assert_eq!(
            inspection
                .names
                .values()
                .map(String::as_str)
                .collect::<Vec<_>>(),
            ["app.exe"]
        );
        let serialized = serde_json::to_string(&inspection).unwrap();
        let request = serde_json::to_value(&request).unwrap();
        for private in [
            "private-user",
            "S-1-5-21-111-222-333-1001",
            "Application",
            "AppId",
            "UserId",
            "app.exe",
            request["nonce"].as_str().unwrap(),
            request["job_id"].as_str().unwrap(),
        ] {
            assert!(
                !serialized.contains(private),
                "unexpected local identity in C4"
            );
        }
    }

    #[test]
    fn consistent_shadow_storage_numbers_are_usable_with_partial_enumeration() {
        let request = request(ElevatedTask::ShadowStorage, Duration::from_secs(10));
        let mut response = empty_response(
            &request,
            ElevationStatus::Partial,
            ElevationLimit::MetadataSnapshot,
        );
        response.shadow_storage = Some(SystemReadSummary {
            status: ElevationStatus::Partial,
            samples: vec![ShadowStorageSample {
                used_bytes: Some(20),
                allocated_bytes: Some(30),
                maximum_bytes: Some(100),
                maximum_unlimited: Some(false),
                values_consistent: Some(true),
            }],
            truncated: false,
            read_errors: 1,
            limitations: vec![
                SystemReadLimit::EnumerationIncomplete,
                SystemReadLimit::RestorePointAttributionUnavailable,
            ],
        });
        validate_response(&response, &request).unwrap();
        let inspection = to_inspection(&response);
        assert_eq!(inspection.status, Status::Partial);
        assert_eq!(inspection.coverage[0].status, CoverageStatus::Partial);
        let used = inspection
            .observations
            .iter()
            .find(|item| item.metric == "shadow_storage_used_bytes")
            .unwrap();
        assert_eq!(used.value, json!(20));
        assert_eq!(used.status, CoverageStatus::Ok);
        assert!(inspection.coverage[0]
            .detail
            .contains("Not reclaimable bytes"));
    }

    #[test]
    fn inconsistent_wmi_counters_do_not_become_ok_when_other_fields_are_missing() {
        let request = request(ElevatedTask::DiskReliability, Duration::from_secs(10));
        let sample: DiskReliabilitySample = serde_json::from_value(json!({
            "temperature_celsius":35,"temperature_limit_celsius":null,"wear_percent":101,
            "power_on_hours":null,"read_errors_total":1,"read_errors_corrected":1,
            "read_errors_uncorrected":1,"write_errors_total":null,"write_errors_corrected":null,
            "write_errors_uncorrected":null,"start_stop_cycles":null,"load_unload_cycles":null,
            "maximum_read_latency_ms":null,"maximum_write_latency_ms":null,"maximum_flush_latency_ms":null
        })).unwrap();
        assert_eq!(sample.values_consistent(), Some(false));
        let mut response = empty_response(
            &request,
            ElevationStatus::Partial,
            ElevationLimit::MetadataSnapshot,
        );
        response.disk_reliability = Some(SystemReadSummary {
            status: ElevationStatus::Partial,
            samples: vec![sample],
            truncated: false,
            read_errors: 0,
            limitations: vec![
                SystemReadLimit::ProviderValueMissing,
                SystemReadLimit::ProviderValuesInconsistent,
            ],
        });
        validate_response(&response, &request).unwrap();
        let inspection = to_inspection(&response);
        assert_eq!(inspection.status, Status::Partial);
        assert_eq!(inspection.coverage[0].status, CoverageStatus::Partial);
        let observed = inspection
            .observations
            .iter()
            .find(|item| item.metric == "disk_reliability_read_errors_total")
            .unwrap();
        assert_eq!(observed.value, json!(1));
        assert_eq!(observed.status, CoverageStatus::Partial);
        let missing = inspection
            .observations
            .iter()
            .find(|item| item.metric == "disk_reliability_temperature_limit")
            .unwrap();
        assert_eq!(missing.value, serde_json::Value::Null);
    }
}
