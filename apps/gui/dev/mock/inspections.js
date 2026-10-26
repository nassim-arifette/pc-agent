import { GiB, MiB } from "./common.js";

export function createInspectionFixtures() {
  function mftResult(volume) {
    const now = Date.now();
    const o = (metric, value, unit = "count") => ({ observation_id: `obs-${metric}`, measured_at_unix_ms: now, window: { started_at_unix_ms: now - 9000, finished_at_unix_ms: now, duration_ms: 9000 }, entity_ref: "ntfs-volume-1", metric, value, unit, status: "partial", source: "synthetic_fixture", origin: "synthetic_fixture" });
    const item = (path, files, size, unknown = 0) => ({ path, files, logical_bytes_known: Math.round(size * 0.97), data_cluster_bytes_known: size, unknown_logical_files: unknown, unknown_allocation_files: unknown, hard_linked_files: 0 });
    return {
      status: "partial", measured_at_unix_ms: now, window: { started_at_unix_ms: now - 9000, finished_at_unix_ms: now, duration_ms: 9000 }, origin: "synthetic_fixture",
      observations: [
        o("ntfs_files_examined", 1284311), o("ntfs_supported_data_allocation_bytes_known", 187.4 * GiB, "bytes"), o("ntfs_supported_logical_bytes_known", 179.2 * GiB, "bytes"),
        o("ntfs_record_slots_walked", 1000000), o("ntfs_record_slots_not_walked", 312480),
      ],
      coverage: [
        { scope: "ntfs_inventory", status: "partial", source: "FSCTL_GET_NTFS_VOLUME_DATA, $MFT", detail: "Record limit reached: 312,480 record slots were not walked. Live volume, not an atomic snapshot." },
        { scope: "ntfs_tree", status: "partial", source: "FILE_NAME metadata", detail: "Largest 16 attributed folders and files; parents and children overlap." },
      ],
      local_mft_tree: {
        directories: [
          item("Users", 412004, 96.3 * GiB, 12), item("Users\\Alex", 398110, 91.8 * GiB, 12), item("Users\\Alex\\AppData", 301442, 54.1 * GiB, 9),
          item("Windows", 512881, 31.6 * GiB, 40), item("Program Files", 98102, 22.4 * GiB), item("Users\\Alex\\Downloads", 1204, 18.7 * GiB),
        ],
        files: [
          item("Users\\Alex\\Downloads\\windows-11-24h2.iso", 1, 6.1 * GiB), item("hiberfil.sys", 1, 4.7 * GiB), item("pagefile.sys", 1, 4.0 * GiB),
          item("Users\\Alex\\Videos\\vacances.mp4", 1, 2.2 * GiB),
        ],
        indexed_nodes: 250000, indexed_name_links: 262114, retained_bytes: 31 * MiB, attributed_files: 1190220, unattributed_files: 94091,
        attributed_logical_bytes_known: 170.1 * GiB, unattributed_logical_bytes_known: 9.1 * GiB, attributed_data_cluster_bytes_known: 177.9 * GiB, unattributed_data_cluster_bytes_known: 9.5 * GiB,
        limitations: ["index_node_limit", "top_directories_truncated", "top_files_truncated"],
      },
    };
  }

  let entitySequence = 0;

  function entityRef(prefix) {
    entitySequence += 1;
    return `${prefix}-00000000-0000-4000-8000-${String(entitySequence).padStart(12, "0")}`;
  }

  function inspection(rows, coverage, durationMs) {
    const now = Date.now();
    const window = { started_at_unix_ms: now - durationMs, finished_at_unix_ms: now, duration_ms: durationMs };
    const observations = rows.map(([entity_ref, metric, value, unit, status = "ok"], i) => ({
      observation_id: `measurement-${now}-${i}`, measured_at_unix_ms: now, window, entity_ref, metric, value, unit,
      status: value == null && status === "ok" ? "unsupported" : status, source: "synthetic_fixture", origin: "synthetic_fixture",
    }));
    const status = coverage.every((c) => c.status === "ok") && observations.every((o) => o.status === "ok") ? "ok" : "partial";
    return { status, measured_at_unix_ms: now, window, origin: "synthetic_fixture", observations, coverage };
  }

  function systemCachesResult() {
    const source = "pcagent-elevated / pcagent-storage ClassicBackend metadata";
    const cache = (category, exists, logical, allocated, files, unknown, status = "ok") => {
      const e = entityRef("system-cache");
      return [[e, "system_cache_category", category, "category", status], [e, "system_cache_exists", exists, "boolean", status], [e, "system_cache_logical_bytes", logical, "bytes", status], [e, "system_cache_allocated_known_bytes", allocated, "bytes", status], [e, "system_cache_file_count", files, "count", status], [e, "system_cache_allocation_unknown_files", unknown, "count", status]];
    };
    return inspection(
      [...cache("previous_windows", false, 0, 0, 0, 0), ...cache("windows_update_download", true, 2.31 * GiB, 2.33 * GiB, 1204, 0), ...cache("delivery_optimization", true, 4.12 * GiB, 4.15 * GiB, 389, 2, "partial")],
      [
        { scope: "previous_windows", status: "ok", source, detail: "Metadata snapshot only; observed allocation is not reclaimable space. Missing values remain unknown. Limits: [MetadataSnapshot]; coverage: None." },
        { scope: "windows_update_download", status: "ok", source, detail: "Metadata snapshot only; observed allocation is not reclaimable space. Missing values remain unknown. Limits: [MetadataSnapshot]; coverage: Some(CacheCoverage { directories: 212, denied_directories: 0 })." },
        { scope: "delivery_optimization", status: "partial", source, detail: "Metadata snapshot only; observed allocation is not reclaimable space. Missing values remain unknown. Limits: [MetadataSnapshot, AllocationLowerBound, DefaultCacheLocationOnly]; coverage: Some(CacheCoverage { directories: 41, denied_directories: 0 })." },
      ],
      4200,
    );
  }

  function shadowStorageResult() {
    const e = entityRef("shadow-storage");
    return inspection(
      [[e, "shadow_storage_used_bytes", 6.2 * GiB, "bytes"], [e, "shadow_storage_allocated_bytes", 6.5 * GiB, "bytes"], [e, "shadow_storage_maximum_bytes", 12.75 * GiB, "bytes"], [e, "shadow_storage_maximum_unlimited", false, "boolean"]],
      [{ scope: "shadow_storage", status: "partial", source: "WMI Win32_ShadowStorage", detail: "Includes other shadow copies; unavailable per-restore-point attribution. Not reclaimable bytes. Limits: [RestorePointAttributionUnavailable]." }],
      1800,
    );
  }

  function diskReliabilityResult() {
    const disk = (temperature, limit, wear, hours, errors, cycles, latencies, status = "ok") => {
      const e = entityRef("disk-reliability");
      return [
        [e, "disk_reliability_temperature", temperature, "celsius", status], [e, "disk_reliability_temperature_limit", limit, "celsius", status], [e, "disk_reliability_wear", wear, "percent", status],
        [e, "disk_reliability_power_on_hours", hours, "hours", status], [e, "disk_reliability_read_errors_total", errors[0], "count", status], [e, "disk_reliability_read_errors_uncorrected", errors[1], "count", status],
        [e, "disk_reliability_write_errors_total", errors[2], "count", status], [e, "disk_reliability_write_errors_uncorrected", errors[3], "count", status], [e, "disk_reliability_start_stop_cycles", cycles, "count", status],
        [e, "disk_reliability_maximum_read_latency", latencies[0], "ms", status], [e, "disk_reliability_maximum_write_latency", latencies[1], "ms", status], [e, "disk_reliability_maximum_flush_latency", latencies[2], "ms", status],
      ];
    };
    return inspection(
      [...disk(41, 84, 3, 4211, [0, 0, 0, 0], 1532, [48, 112, 9]), ...disk(35, null, null, 12890, [null, null, null, null], 4410, [null, null, null])],
      [{ scope: "disk_reliability", status: "partial", source: "WMI root/Microsoft/Windows/Storage MSFT_StorageReliabilityCounter", detail: "Device-reported historical counters; absent fields are unknown. Limits: [ProviderValueMissing]." }],
      2600,
    );
  }

  function batteryEnergyResult() {
    const history = entityRef("battery-energy-history");
    const rows = [[history, "battery_energy_estimated_total", 48.2e6, "mJ"], [history, "battery_energy_committed_rows", 1284, "count"], [history, "battery_energy_distinct_apps", 7, "count"]];
    const apps = [[16.4e6, 14.1e6, 12.2e6, 412, 9.8], [9.7e6, 8.9e6, 7.4e6, 233, 6.1], [6.1e6, 5.2e6, 0.4e6, 198, 7.9], [5.3e6, 4.8e6, 4.1e6, 140, 3.2], [4.2e6, 3.1e6, 0, 151, 7.2], [3.6e6, 3.3e6, 2.9e6, 88, 2.4], [2.9e6, 1.1e6, 0, 62, 5.5]];
    apps.forEach(([energy, screen, foreground, records, hours], i) => {
      const e = entityRef("battery-energy-app");
      rows.push(
        [e, "battery_app_energy_estimated", energy, "mJ"], [e, "battery_app_screen_on_energy_estimated", screen, "mJ"], [e, "battery_app_foreground_energy_estimated", foreground, "mJ"],
        [e, "battery_app_committed_rows", records, "count"], [e, "battery_app_reported_row_durations", Math.round(hours * 3600000), "ms"],
        [e, "battery_app_historical_first_civil_stamp", `2026-09-${21 + (i % 3)} 08:${10 + i}:00.000000000 (provider civil time; zone unknown)`, "provider_civil_time"],
        [e, "battery_app_historical_last_civil_stamp", `2026-10-0${4 + (i % 2)} 2${i % 4}:4${i}:00.000000000 (provider civil time; zone unknown)`, "provider_civil_time"],
      );
    });
    return inspection(
      rows,
      [{ scope: "battery_energy_history", status: "partial", source: "Windows E3 historical estimates / powercfg srumutil CSV", detail: "Historical E3 estimates for accepted committed on-battery rows of this user only. Civil timestamps have unknown timezone; summed row durations may overlap and are not a measurement window. Export time is the read time. Not physical power, exhaustive app attribution or battery discharge. Limits: [ExportPartial]." }],
      7400,
    );
  }

  const CHECK_RESULTS = { system_caches: systemCachesResult, shadow_storage: shadowStorageResult, disk_reliability: diskReliabilityResult, battery_energy: batteryEnergyResult };
  return { mftResult, CHECK_RESULTS };
}
