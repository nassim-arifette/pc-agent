// The core's sentences in the interface language. The Rust side keeps its English text as the text
// of record (model payloads, JSON output, the answer checks) and sends this window, beside it, the same
// statement as a closed key with typed values (`Phrase` in crates/shared, built in view.rs and
// crates/diagnostics), closed codes for missing values, GPU slowdowns and elevated coverage, and a
// flag for names it could not read. They are worded here and each value is formatted by its unit;
// anything this window cannot word (a report saved before, an unknown key or value) keeps the
// stored English text.
import { language, t } from "./i18n.js";
import { basisPoints, bytes, number } from "./format.js";
import { cliEn, cliFr } from "./i18n/cli.js";

const en = {
  ...cliEn,
  "label.metric": "{metric}",
  "support.short_sample": "A single short sample cannot establish sustained contention, a leak or the cause of a slowdown.",
  "support.memory_pressure": "Observed together with low available memory in the same sample; one sample cannot attribute a slowdown.",
  "support.contributor_only": "No memory contention is observed in this sample; a large allocation is not a problem by itself.",
  "support.abstention": "Explicit abstention: causes remain unmeasured or unestablished.",
  "hypothesis.memory_pressure": "Memory contention may contribute to the reported symptom.",
  "hypothesis.page_reads": "Memory pressure may be forcing pages to be read back from disk.",
  "hypothesis.commit_pressure": "Allocation failures or page-file growth may occur if commit keeps rising.",
  "hypothesis.cpu_pressure": "CPU contention may contribute to the reported symptom.",
  "hypothesis.storage_latency": "Slow storage requests may contribute to the reported symptom.",
  "hypothesis.single_thread": "A single-threaded workload may be limited by one processor.",
  "hypothesis.gpu_cooling": "If it persists under the same workload, cooling (dust, blocked vents, a soft surface under a laptop) or the power supply may be limiting the GPU.",
  "counter.single_thread": "A moderate aggregate CPU value does not exclude a single saturated thread.",
  "counter.commit_share": "System commit is {share} of its limit, so commit pressure is not also observed.",
  "counter.page_reads": "Hard-fault page reads were {rate}, so heavy paging is not also observed.",
  "counter.disk_latency": "Average disk read latency was {latency}, so disk reads were not observed to be slow.",
  "counter.disk_idle": "Disks were idle {share} of the window; averages over few requests are noisy.",
  "missing.representative_window": "representative observation window",
  "action.volume_comparison": "Free space: {before} → {after}.",
  // Findings (crates/diagnostics): summaries, next steps and limitations.
  "summary.low_available_memory": "Available physical memory is low in this sample. Memory contention is plausible but unproven.",
  "summary.hard_fault_pressure": "Many pages were read from disk to resolve hard faults while available memory was low. Paging pressure is plausible but unproven.",
  "summary.high_commit_ratio": "System commit is close to its current limit. This does not measure physical RAM occupancy or page-file activity.",
  "summary.high_cpu_sample": "Aggregate CPU activity is high during this short observation. Its connection to the reported symptom needs more evidence.",
  "summary.high_disk_latency": "Average disk request latency is high in this sample. Storage contention is plausible but unproven.",
  "summary.saturated_logical_processor": "One logical processor was nearly saturated while aggregate CPU was not. A single busy thread is plausible but unproven.",
  "summary.high_gpu_engine_sample": "A GPU engine was nearly fully busy during this short observation. High GPU use is expected for games and rendering and is not a fault by itself.",
  "summary.largest_memory_contributor": "This readable process instance has the largest private commit observed. Size alone does not indicate a leak, malware or uselessness.",
  "summary.no_contention_observed": "No memory or aggregate CPU contention was observed in this sample. No software intervention is identified from these measurements.",
  "summary.insufficient_evidence": "The available measurements do not establish the cause of the symptom. No intervention or hardware purchase is justified by this report alone.",
  "summary.gpu_clock_slowdown": "An NVIDIA GPU was reducing its clocks because of heat or a hardware limit during this sample. Laptops do this near their limits; a sustained slowdown under load makes games and rendering slower.",
  "summary.gpu_memory_pressure": "An NVIDIA GPU's memory was nearly full during this sample. Games and applications that need more video memory can stutter when it is full.",
  "next.observe_longer": "Observe a representative workload over a longer period and check resources not yet measured.",
  "next.memory_contributors": "Inspect process memory contributors and observe whether low availability persists.",
  "next.hard_faults": "Observe whether hard faults and low availability persist together under the same workload.",
  "next.disk_latency": "Observe disk latency over a longer representative workload and identify which activity issues the requests.",
  "next.instance_growth": "Observe whether this instance grows under the same workload before drawing conclusions.",
  "next.gpu_slowdown": "Check the vents and airflow, then observe the GPU temperature and slowdown reasons again under the same workload.",
  "next.gpu_memory": "Close other GPU-heavy applications or lower texture and resolution settings, then observe GPU memory again under the same workload.",
  "limit.thresholds": "Initial observation thresholds are prototype triggers, not validated diagnostic rules.",
  "limit.no_purchase": "This observation is not a hardware replacement or purchase recommendation.",
  "limit.disk_aggregate": "Disk counters aggregate every physical disk (_Total); the affected device is not identified and one sample cannot separate a slow device from a burst of work.",
  "limit.hard_faults": "Pages Input/sec includes executables and mapped files being loaded; hard faults are not hardware faults.",
  "limit.commit_limit": "Commit limit includes page files; it is not a RAM purchase recommendation.",
  "limit.processor_utility": "Processor utility is normalized to nominal frequency and threads migrate between processors; the busy thread is not identified.",
  "limit.gpu_engines": "GPU engines are aggregated by generic type; dedicated memory, adapter and the using process are not reported.",
  "limit.inaccessible_processes": "Inaccessible processes may be larger; working sets and commit are not exclusive RAM.",
  "limit.nvml_instant": "NVML reports the driver's state at one instant; a short spike can set a reason.",
  "limit.gpu_memory_reserve": "Memory in use includes the driver's reserve; the process holding it is not reported on Windows.",
  "limit.not_measured": "Not measured in this snapshot: {metrics}.",
  // Facts of findings, with their measured values.
  "fact.available_of_total": "Available physical memory was {available} of {total} ({share}).",
  "fact.available_share": "Available physical memory was {share}.",
  "fact.hard_faults_over": "Hard-fault page reads averaged {rate} over {window}.",
  "fact.hard_faults": "Hard-fault page reads were {rate}.",
  "fact.disk_read_latency": "Average disk read latency was {latency}.",
  "fact.disk_write_latency": "Average disk write latency was {latency}.",
  "fact.disk_read_latency_over": "Average disk read latency was {latency} over {window}.",
  "fact.disk_write_latency_over": "Average disk write latency was {latency} over {window}.",
  "fact.disk_idle": "Disk idle time was {share}.",
  "fact.disk_queue": "Disk requests outstanding at the end of the window: {count}.",
  "fact.commit_of_limit": "System commit was {used} of a {limit} limit ({share}).",
  "fact.commit_share": "System commit was {share} of its limit.",
  "fact.cpu_busy_over": "Aggregate CPU busy was {share} of total capacity over {window}.",
  "fact.cpu_busy_of_total": "Aggregate CPU busy was {share} of total capacity.",
  "fact.cpu_busy": "Aggregate CPU busy was {share}.",
  "fact.busiest_core": "The busiest logical processor had {share} processor utility.",
  "fact.gpu_engine_over": "The busiest GPU engine was {share} utilized over {window}.",
  "fact.gpu_engine_type": "The {engine} reached that value.",
  "fact.process_private_commit": "Process {process} had {size} private commit.",
  "fact.metric_value": "{metric}: {value}.",
  "fact.gpu_slowdown": "{gpu} reported slowdown reasons: {reasons}.",
  "fact.gpu_temperature": "{gpu} was at {temperature}.",
  "fact.gpu_memory": "{gpu} had {used} of {total} memory in use ({share}).",
  // Facts of tool observations (crates/agent/src/report.rs).
  "observation.metric_value": "{metric}: {value} ({entity})",
  "observation.logical_processors": "Logical processors: {count}.",
  "observation.installed_memory": "Installed memory: {size}.",
  "observation.architecture": "Architecture: {architecture}.",
  "observation.hardware_capacity": "Hardware inventory describes capacity, not the cause of a symptom or an upgrade recommendation.",
  "observation.volume_free": "Volume free space: {free} of {total}.",
  "observation.scanned_size": "Scanned logical size: {size} (not reclaimable space).",
  "observation.scan_files": "Files indexed by the scan: {count}; see its coverage counters for skipped items.",
  "observation.ranked_items": "Ranked items returned: {count}; this page is not a complete inventory.",
  "observation.item_size": "{item}: {size} logical size; no deletion recommendation implied.",
  "observation.history_range": "{metric}: {count} usable samples, observed range {min} to {max}.",
  "observation.history_interval": "Observed interval: {interval}.",
  "observation.history_gaps": "Recorded gaps: {count}; no interpolation or causal trend inferred.",
  // Limitations most turns carry (view.rs NOTES).
  "note.processes_system_list": "Every process comes from one system list (NtQuerySystemInformation) without opening any, so services and other users' processes are measured without administrator rights.",
  "note.processes_unknown": "Process enumeration failed; coverage is unknown.",
  "note.processes_one_by_one": "The system process list was unavailable: processes were opened one by one, and those of services or other users may be missing.",
  "note.working_sets": "Working sets include shared pages and must not be summed as exclusive physical RAM.",
  "note.process_identities": "Process identities are session-scoped (PID plus creation time).",
  "note.process_rates": "Process CPU and I/O are rates over the measurement window (about one second), or since this collector's previous reading of the same instance.",
  "note.boot_id": "boot_id is an estimate (GetSystemTimeAsFileTime minus GetTickCount64, rounded to one minute), fixed per collector; clock changes can shift it between sessions.",
  "note.short_sample": "Short sample: no sustained contention, memory leak or causal diagnosis can be established.",
  "note.recording_gpu": "Recorded process GPU uses the existing PDH window for the retained process list. First windows, missing engines and identities that cannot be revalidated remain unknown; occupancy is not per-application energy.",
  "note.activity_window": "CPU and I/O are window averages; memory is measured at the end. Rates and memory changes use PID plus creation time, never a PID alone.",
  "note.activity_groups": "Application groups are an image-and-parent heuristic within one Windows session, not verified product identities. Executable basenames can coincide. Generic shells and service hosts form separate boundaries; all members are aggregated before the returned lists are capped.",
  "note.activity_references": "Application references are scoped to this observation; process references remain stable for PID plus creation time in the collector session.",
  "note.activity_working_sets": "Working-set sums include shared pages; they are not exclusive physical RAM. Memory increase/decrease is observed net change, not evidence of a leak.",
  "note.activity_gpu": "GPU is the busiest observed physical engine. Application GPU sums members on each physical engine before taking the maximum; missing or newly created counter instances are counted in coverage. No GPU instance means unknown, never zero.",
  "note.activity_exited": "Exited processes are missing from end-of-window attribution, and newly started or reused instances have no full-window rate. Short windows cannot establish sustained contention.",
  "note.activity_ranking": "Rows are ranked by CPU, then I/O throughput, then working set; service and raw process metadata remain local until privacy projection.",
  "note.activity_no_baseline": "The initial process list was unavailable; memory is measured but rates and changes are unknown.",
  "note.activity_no_final_list": "The final system process list could not be read.",
  "note.services_visible": "SCM enumeration covers Win32 services visible to this account; services with a denied SERVICE_QUERY_STATUS right may be omitted by Windows. Kernel drivers are excluded.",
  "note.services_pids": "Service PIDs are observations, not execution authority; starting and stopping services have no reliable PID attribution.",
  "note.services_stalled": "SCM pagination made no progress; remaining services are unavailable.",
  "note.services_bound": "Service enumeration reached the page bound.",
  "note.history_samples": "Persisted local samples; values are not interpolated. After 24 hours, values are duration-weighted minute averages with measured minima and maxima. Instantaneous gauges use the sampling interval as averaging weight, not as a measured window.",
  "note.history_top_processes": "Only the top 20 processes at each sample are retained; absence of a process is unknown, not zero.",
  "note.history_timestamps": "Observation timestamps and accumulated measured duration are preserved separately from the nominal sampling cadence. A recording's first rate may cover the preceding background interval. Minute buckets overlapping the requested range are returned in full.",
  "note.history_events": "Event boundaries are the first and last anomalous samples, not a measured continuous duration. Peak and contributor observations retain their own measured windows; derived values identify their original operands. A contributor is observed resource use, not a proven cause.",
  "note.local_evidence": "Local evidence only: no model explanation was requested.",
  "note.ranked_working_set": "Process list is ranked by working set; unknown values sort last.",
  "note.ranked_private_commit": "Process list is ranked by private commit; unknown values sort last.",
  "note.ranked_cpu": "Process list is ranked by CPU; unknown values sort last.",
  "note.ranked_io_read": "Process list is ranked by read I/O rate; unknown values sort last.",
  "note.ranked_io_write": "Process list is ranked by write I/O rate; unknown values sort last.",
  // Values: metrics (Metric::label), entities, GPU slowdown reasons, names not read.
  "metric.physical_total_bytes": "Physical memory",
  "metric.physical_available_bytes": "Available memory",
  "metric.system_commit_bytes": "Commit charge",
  "metric.system_commit_limit_bytes": "Commit limit",
  "metric.cpu_busy_basis_points": "CPU (all cores)",
  "metric.process_working_set_bytes": "Working set",
  "metric.process_private_commit_bytes": "Private commit",
  "metric.process_cpu_basis_points": "CPU",
  "metric.process_io_read_bytes": "I/O read",
  "metric.process_io_write_bytes": "I/O write",
  "metric.process_io_other_bytes": "I/O other",
  "metric.process_working_set_increase_bytes": "Working-set increase",
  "metric.process_working_set_decrease_bytes": "Working-set decrease",
  "metric.process_private_commit_increase_bytes": "Private-commit increase",
  "metric.process_private_commit_decrease_bytes": "Private-commit decrease",
  "metric.process_io_read_bytes_per_second": "I/O read rate",
  "metric.process_io_write_bytes_per_second": "I/O write rate",
  "metric.process_io_other_bytes_per_second": "I/O other rate",
  "metric.process_gpu_basis_points": "Busiest observed process GPU engine",
  "metric.cpu_busiest_core_basis_points": "Busiest core",
  "metric.disk_read_latency_microseconds": "Disk read latency",
  "metric.disk_write_latency_microseconds": "Disk write latency",
  "metric.disk_idle_basis_points": "Disk idle time",
  "metric.disk_queue_length": "Disk queue length",
  "metric.hard_fault_pages_per_second_x100": "Hard-fault page reads",
  "metric.gpu_engine_basis_points": "GPU engine",
  "metric.gpu_busiest_engine_basis_points": "Busiest GPU engine",
  "metric.gpu_utilization_basis_points": "GPU busy",
  "metric.gpu_memory_used_bytes": "GPU memory used",
  "metric.gpu_memory_total_bytes": "GPU memory",
  "metric.gpu_temperature_celsius": "GPU temperature",
  "metric.gpu_sm_clock_megahertz": "GPU clock",
  "metric.gpu_power_milliwatts": "GPU power",
  "metric.gpu_slowdown_reasons": "GPU slowdown",
  "entity.system": "system",
  "entity.gpu": "NVIDIA GPU {n}",
  "engine.3d": "3D engine",
  "engine.compute": "compute engine",
  "engine.copy": "copy engine",
  "engine.videodecode": "video decode engine",
  "engine.videoencode": "video encode engine",
  "engine.videoprocessing": "video processing engine",
  "engine.overlay": "overlay engine",
  "engine.legacyoverlay": "legacy overlay engine",
  "engine.security": "security engine",
  "engine.crypto": "crypto engine",
  "engine.vr": "VR engine",
  "engine.graphics": "graphics engine",
  "engine.other": "other GPU engine",
  "slowdown.power_cap": "power cap",
  "slowdown.hardware_slowdown": "hardware slowdown",
  "slowdown.thermal_driver": "thermal (driver)",
  "slowdown.thermal_hardware": "thermal (hardware)",
  "slowdown.power_brake": "power brake",
  "slowdown.none": "none",
  "unit.pages": "{value} pages/s",
  "process.unnamed": "Unnamed process",
  "process.unnamedPid": "Unnamed process (PID {pid})",
  "application.unnamed": "Unnamed application",
  // Why a live value is missing (view.rs MISSING_REASONS), else by its status.
  "missing.first_reading": "Not measured yet: a rate needs two readings of the same process.",
  "missing.query_failed": "Windows did not answer the measurement query.",
  "missing.inconsistent": "The counter returned an inconsistent value.",
  "missing.gpu_unattributed": "No graphics engine could be attributed to it: unknown, not zero.",
  "missing.no_gpu_counters": "Windows reports no GPU counters here (basic display adapter or virtual machine).",
  "missing.gpu_counters_unusable": "GPU counters were reported, but none was usable.",
  "missing.nvml_unsupported": "Not available from this NVIDIA driver.",
  "missing.nvml_error": "The NVIDIA driver returned an error.",
  "missing.status.partial": "Measurement incomplete.",
  "missing.status.unsupported": "Not available on this PC.",
  "missing.status.permission_denied": "Access denied.",
  "missing.status.stale": "Measurement too old.",
  "missing.status.cancelled": "Measurement stopped.",
  "missing.status.error": "Measurement failed.",
  // Elevated results (crates/inspect/src/elevation/notes.rs): what each scope leaves out, then
  // why it is incomplete. The performance trace is not offered in this window.
  "coverage.note.system_cache": "Metadata snapshot only; observed allocation is not reclaimable space. Missing values remain unknown.",
  "coverage.note.performance_trace": "Native collection requires validated cleanup; no driver identity or causal attribution. Unknown loss counters remain unknown.",
  "coverage.note.ntfs_inventory": "Supported metadata streams only; the drive was in use, so this is not a frozen snapshot. Allocation excludes metadata and is not reclaimable space.",
  "coverage.note.ntfs_tree": "Top items from indexed metadata only; paths stay on this PC. Folder subtrees overlap and cannot be added up. Hard-linked files are counted once; unattributed sizes stay separate.",
  "coverage.note.shadow_storage": "Includes other shadow copies; space per restore point is not available. Not reclaimable space.",
  "coverage.note.disk_reliability": "Historical counters as each disk reports them; absent fields are unknown.",
  "coverage.note.protected_process_images": "Creation time and process lifetime are checked again before and after reading the path. Full paths and identities stay on this PC.",
  "coverage.note.battery_energy_history": "Windows' historical estimates, for this account's use on battery only. Times have no known time zone, and summed durations may overlap: they are not a measurement window. Not physical power, a complete attribution to apps or battery discharge.",
  "coverage.note.elevated_task": "No usable aggregate measurement was returned; no cleanup recommendation is available.",
  "coverage.reason.other": "Another limit of this reading.",
  "coverage.reason.elevation.metadata_snapshot": "Sizes come from file system metadata at one moment.",
  "coverage.reason.elevation.allocation_lower_bound": "Allocated sizes are a lower bound.",
  "coverage.reason.elevation.enumeration_incomplete": "Not every item could be listed.",
  "coverage.reason.elevation.entry_limit": "The walk reached its item limit.",
  "coverage.reason.elevation.memory_limit": "The walk reached its memory limit.",
  "coverage.reason.elevation.duration_limit": "The check reached its time limit.",
  "coverage.reason.elevation.cancelled": "The check was stopped before the end.",
  "coverage.reason.elevation.restore_points_unimplemented": "Restore points are not measured by this check.",
  "coverage.reason.elevation.default_cache_location_only": "Only the cache's default location was read.",
  "coverage.reason.elevation.policy_cache_location_unconfirmed": "A cache location set by policy could not be confirmed.",
  "coverage.reason.elevation.mft_accuracy_unvalidated": "The accuracy of the file index reading is not validated on real drives yet.",
  "coverage.reason.elevation.source_unavailable": "The source could not be read.",
  "coverage.reason.elevation.platform_unsupported": "Not supported on this version of Windows.",
  "coverage.reason.cache.denied_directories": "Folders that could not be opened (access denied): {count}",
  "coverage.reason.cache.denied_files": "Files that could not be read (access denied): {count}",
  "coverage.reason.cache.reparse_skipped": "Links and junctions skipped: {count}",
  "coverage.reason.cache.placeholders_skipped": "Cloud files not downloaded, skipped: {count}",
  "coverage.reason.cache.other_volume_skipped": "Items on another drive, skipped: {count}",
  "coverage.reason.cache.special_skipped": "Special files skipped: {count}",
  "coverage.reason.cache.vanished": "Items that disappeared during the walk: {count}",
  "coverage.reason.cache.hard_link_aliases": "Hard links counted once: {count}",
  "coverage.reason.cache.changed_during_scan": "Items that changed during the walk: {count}",
  "coverage.reason.cache.allocation_unknown_files": "Files with an unknown allocated size: {count}",
  "coverage.reason.cache.errors": "Read errors: {count}",
  "coverage.reason.cache.truncated": "The walk stopped at its limit.",
  "coverage.reason.cache.cancelled": "The walk was stopped before the end.",
  "coverage.reason.end.record_limit": "The reading stopped at its record limit.",
  "coverage.reason.end.memory_limit": "The reading stopped at its memory limit.",
  "coverage.reason.end.run_limit": "The reading stopped at its fragment limit.",
  "coverage.reason.end.cancelled": "The reading was stopped before the end.",
  "coverage.reason.end.timed_out": "The reading reached its time limit.",
  "coverage.reason.end.source_unavailable": "The file index could not be read.",
  "coverage.reason.end.invalid_source": "The file index returned invalid data.",
  "coverage.reason.mft.allocation_not_additive": "Allocated sizes overlap and cannot be added up.",
  "coverage.reason.mft.offline_subset": "Only the records read in this pass are counted.",
  "coverage.reason.mft.native_accuracy_unvalidated": "The accuracy of this reading is not validated on real drives yet.",
  "coverage.reason.mft.metadata_allocation_excluded": "Space used by the file system's own metadata is not counted.",
  "coverage.reason.mft.system_records_excluded": "The file system's own records are not counted.",
  "coverage.reason.mft.directory_allocation_excluded": "Space used by folder indexes is not counted.",
  "coverage.reason.mft.named_data_excluded": "Named data streams are not counted.",
  "coverage.reason.mft.reparse_data_excluded": "Reparse point data (links, cloud files) is not counted.",
  "coverage.reason.mft.encrypted_data_excluded": "Encrypted files are not counted.",
  "coverage.reason.mft.unsupported_data_flags": "Files stored in an unsupported way are not counted.",
  "coverage.reason.mft.attribute_list_unresolved": "Some large or fragmented files could not be fully resolved.",
  "coverage.reason.mft.missing_base": "Some file records were missing their main record.",
  "coverage.reason.mft.missing_data": "Some files had no readable data size.",
  "coverage.reason.mft.incomplete_extents": "Not every fragment of some files could be read.",
  "coverage.reason.mft.compression_layout_unvalidated": "Sizes of compressed files are not validated.",
  "coverage.reason.mft.live_volume_not_atomic": "The drive was in use during the reading: it is not a frozen snapshot.",
  "coverage.reason.mft.volume_changed": "The drive changed during the reading.",
  "coverage.reason.mft.change_witness_unavailable": "Changes during the reading could not be detected.",
  "coverage.reason.mft.record_limit": "The reading stopped at its record limit.",
  "coverage.reason.mft.memory_limit": "The reading stopped at its memory limit.",
  "coverage.reason.mft.duration_limit": "The reading reached its time limit.",
  "coverage.reason.mft.cancelled": "The reading was stopped before the end.",
  "coverage.reason.mft.source_unavailable": "The file index could not be read.",
  "coverage.reason.mft.invalid_record": "Invalid records were skipped.",
  "coverage.reason.mft.run_limit": "The reading stopped at its fragment limit.",
  "coverage.reason.tree.index_memory_limit": "The folder tree reached its memory limit.",
  "coverage.reason.tree.index_node_limit": "The folder tree reached its size limit.",
  "coverage.reason.tree.name_links_truncated": "Some files have more names than were kept.",
  "coverage.reason.tree.invalid_file_name": "Some file names were invalid.",
  "coverage.reason.tree.missing_file_name": "Some files had no readable name.",
  "coverage.reason.tree.parent_missing": "The parent folder of some items was not found.",
  "coverage.reason.tree.parent_sequence_changed": "Some parent folders changed during the reading.",
  "coverage.reason.tree.parent_metadata_incomplete": "The metadata of some parent folders was incomplete.",
  "coverage.reason.tree.parent_cycle": "Some folders formed a loop and were left out.",
  "coverage.reason.tree.path_conflict": "Some paths were ambiguous.",
  "coverage.reason.tree.path_limit": "Some paths were too long to keep.",
  "coverage.reason.tree.depth_limit": "Some folders were too deep to keep.",
  "coverage.reason.tree.filename_membership_unresolved": "Some files could not be placed in a folder.",
  "coverage.reason.tree.collection_incomplete": "The file index was not read completely.",
  "coverage.reason.tree.interrupted": "Building the folder tree was interrupted.",
  "coverage.reason.tree.allocation_not_additive": "Allocated sizes overlap and cannot be added up.",
  "coverage.reason.tree.top_directories_truncated": "Only the largest folders are listed.",
  "coverage.reason.tree.top_files_truncated": "Only the largest files are listed.",
  "coverage.reason.system.provider_unavailable": "Windows did not provide this information.",
  "coverage.reason.system.provider_value_missing": "Some values were not reported by the device or Windows.",
  "coverage.reason.system.provider_values_inconsistent": "Some reported values were inconsistent and were left out.",
  "coverage.reason.system.enumeration_incomplete": "Not every item could be listed.",
  "coverage.reason.system.row_limit": "The reading reached its item limit.",
  "coverage.reason.system.duration_limit": "The check reached its time limit.",
  "coverage.reason.system.cancelled": "The check was stopped before the end.",
  "coverage.reason.system.platform_unsupported": "Not supported on this version of Windows.",
  "coverage.reason.system.shadow_storage_includes_other_snapshots": "Shadow copy space also includes copies that are not restore points.",
  "coverage.reason.system.restore_point_attribution_unavailable": "Space per restore point is not available.",
  "coverage.reason.system.storage_counters_are_device_reported": "Disk counters are as each disk reports them.",
  "coverage.reason.system.raw_smart_attributes_unavailable": "Raw SMART attributes are not read.",
  "coverage.reason.system.process_identity_changed": "A process changed during the reading.",
  "coverage.reason.system.process_exited": "A process ended during the reading.",
  "coverage.reason.system.process_identity_unavailable": "The identity of a process could not be checked.",
  "coverage.reason.system.image_path_too_long": "A program path was too long.",
  "coverage.reason.system.protected_process_denied": "A protected process refused access.",
  "coverage.reason.energy.historical_estimate": "These are Windows' historical estimates, not measurements.",
  "coverage.reason.energy.provider_coverage_incomplete": "Windows' history does not cover every period or app.",
  "coverage.reason.energy.civil_time_zone_unknown": "Times have no known time zone.",
  "coverage.reason.energy.row_durations_may_overlap": "Summed durations may overlap.",
  "coverage.reason.energy.no_committed_on_battery_rows": "Windows recorded no use on battery.",
  "coverage.reason.energy.export_unavailable": "Windows' energy export is not available.",
  "coverage.reason.energy.export_failed": "Windows' energy export failed.",
  "coverage.reason.energy.export_encoding_unsupported": "The text encoding of the export is not supported.",
  "coverage.reason.energy.export_schema_unsupported": "The format of the export is not supported.",
  "coverage.reason.energy.export_malformed": "Malformed rows of the export were left out.",
  "coverage.reason.energy.duplicate_row": "Duplicate rows were counted once.",
  "coverage.reason.energy.numeric_overflow": "Some values were too large and were left out.",
  "coverage.reason.energy.export_size_limit": "The export reached its size limit.",
  "coverage.reason.energy.row_limit": "The reading reached its row limit.",
  "coverage.reason.energy.app_limit": "Only the apps with the most energy are listed.",
  "coverage.reason.energy.duration_limit": "The check reached its time limit.",
  "coverage.reason.energy.cancelled": "The check was stopped before the end.",
  "coverage.reason.energy.platform_unsupported": "Not supported on this version of Windows.",
  "coverage.reason.energy.native_accuracy_unvalidated": "The accuracy of this reading is not validated on real PCs yet.",
};

const fr = {
  ...cliFr,
  "label.metric": "{metric}",
  "support.short_sample": "Une seule mesure courte ne permet pas d'établir une saturation durable, une fuite de mémoire ou la cause d'un ralentissement.",
  "support.memory_pressure": "La même mesure indique aussi une faible mémoire disponible ; une seule mesure ne permet pas d'attribuer un ralentissement.",
  "support.contributor_only": "Aucune saturation de la mémoire n'est observée dans cette mesure ; une allocation importante n'est pas un problème en soi.",
  "support.abstention": "Abstention explicite : les causes restent non mesurées ou non établies.",
  "hypothesis.memory_pressure": "Une saturation de la mémoire pourrait contribuer au symptôme signalé.",
  "hypothesis.page_reads": "La pression sur la mémoire pourrait imposer de relire des pages depuis le disque.",
  "hypothesis.commit_pressure": "Des allocations pourraient échouer ou le fichier d'échange pourrait grandir si la mémoire engagée continue d'augmenter.",
  "hypothesis.cpu_pressure": "Une saturation du processeur pourrait contribuer au symptôme signalé.",
  "hypothesis.storage_latency": "Des requêtes de stockage lentes pourraient contribuer au symptôme signalé.",
  "hypothesis.single_thread": "Une charge sur un seul fil d'exécution pourrait être limitée par un processeur.",
  "hypothesis.gpu_cooling": "Si cela persiste avec la même charge, le refroidissement (poussière, aérations bouchées, surface souple sous un portable) ou l'alimentation pourrait limiter la carte graphique.",
  "counter.single_thread": "Une activité globale modérée du processeur n'exclut pas un seul fil d'exécution saturé.",
  "counter.commit_share": "La mémoire engagée représente {share} de sa limite ; aucune pression sur cette limite n'est donc observée.",
  "counter.page_reads": "Les relectures de pages étaient de {rate} ; aucune pagination intense n'est donc observée.",
  "counter.disk_latency": "Le temps moyen de lecture du disque était de {latency} ; les lectures n'ont donc pas été observées comme lentes.",
  "counter.disk_idle": "Les disques étaient inactifs pendant {share} de la période ; une moyenne sur peu de requêtes est peu représentative.",
  "missing.representative_window": "période d'observation représentative",
  "action.volume_comparison": "Espace libre : {before} → {after}.",
  "summary.low_available_memory": "La mémoire physique disponible est faible dans cette mesure. Une saturation de la mémoire est plausible, mais pas prouvée.",
  "summary.hard_fault_pressure": "De nombreuses pages ont été relues depuis le disque (défauts de page matériels) alors que la mémoire disponible était faible. Une pression de pagination est plausible, mais pas prouvée.",
  "summary.high_commit_ratio": "La mémoire engagée du système est proche de sa limite actuelle. Cela ne mesure ni l'occupation de la RAM physique ni l'activité du fichier d'échange.",
  "summary.high_cpu_sample": "L'activité globale du processeur est élevée pendant cette courte observation. Son lien avec le symptôme signalé demande d'autres éléments.",
  "summary.high_disk_latency": "Le temps de réponse moyen du disque est élevé dans cette mesure. Une saturation du stockage est plausible, mais pas prouvée.",
  "summary.saturated_logical_processor": "Un processeur logique était presque saturé alors que le processeur dans son ensemble ne l'était pas. Un seul fil d'exécution très actif est plausible, mais pas prouvé.",
  "summary.high_gpu_engine_sample": "Un moteur du GPU était presque entièrement occupé pendant cette courte observation. Une forte utilisation du GPU est normale pour les jeux et le rendu, et n'est pas une panne en soi.",
  "summary.largest_memory_contributor": "Ce processus lisible a la plus grande mémoire privée engagée observée. Sa taille seule n'indique ni une fuite, ni un logiciel malveillant, ni qu'il est inutile.",
  "summary.no_contention_observed": "Aucune saturation de la mémoire ou du processeur n'a été observée dans cette mesure. Ces mesures ne désignent aucune intervention logicielle.",
  "summary.insufficient_evidence": "Les mesures disponibles n'établissent pas la cause du symptôme. Ce rapport seul ne justifie ni intervention ni achat de matériel.",
  "summary.gpu_clock_slowdown": "Un GPU NVIDIA réduisait ses fréquences à cause de la chaleur ou d'une limite matérielle pendant cette mesure. Les portables le font près de leurs limites ; un ralentissement durable en charge ralentit les jeux et le rendu.",
  "summary.gpu_memory_pressure": "La mémoire d'un GPU NVIDIA était presque pleine pendant cette mesure. Les jeux et les applications qui ont besoin de plus de mémoire vidéo peuvent saccader quand elle est pleine.",
  "next.observe_longer": "Observez une utilisation représentative sur une plus longue durée et vérifiez les ressources pas encore mesurées.",
  "next.memory_contributors": "Regardez quels processus utilisent la mémoire et observez si la mémoire disponible reste faible.",
  "next.hard_faults": "Observez si les défauts de page matériels et la faible mémoire disponible persistent ensemble avec la même utilisation.",
  "next.disk_latency": "Observez le temps de réponse du disque sur une utilisation représentative plus longue et repérez l'activité qui envoie les requêtes.",
  "next.instance_growth": "Observez si ce processus grossit avec la même utilisation avant de conclure.",
  "next.gpu_slowdown": "Vérifiez les aérations et la circulation de l'air, puis observez à nouveau la température du GPU et les raisons du ralentissement avec la même utilisation.",
  "next.gpu_memory": "Fermez les autres applications gourmandes en GPU ou baissez la qualité des textures et la résolution, puis observez à nouveau la mémoire du GPU avec la même utilisation.",
  "limit.thresholds": "Les seuils de cette première observation sont des déclencheurs de prototype, pas des règles de diagnostic validées.",
  "limit.no_purchase": "Cette observation n'est pas une recommandation de remplacer ou d'acheter du matériel.",
  "limit.disk_aggregate": "Les compteurs de disque regroupent tous les disques physiques (_Total) : le disque concerné n'est pas identifié, et une seule mesure ne distingue pas un disque lent d'un pic de travail.",
  "limit.hard_faults": "Le compteur Pages Input/sec inclut le chargement des exécutables et des fichiers mappés ; un défaut de page matériel n'est pas une panne matérielle.",
  "limit.commit_limit": "La limite de mémoire engagée inclut les fichiers d'échange ; ce n'est pas une recommandation d'acheter de la RAM.",
  "limit.processor_utility": "L'utilité du processeur est ramenée à sa fréquence nominale, et les fils d'exécution passent d'un processeur à l'autre ; le fil très actif n'est pas identifié.",
  "limit.gpu_engines": "Les moteurs du GPU sont regroupés par type générique ; la mémoire dédiée, la carte et le processus qui les utilise ne sont pas indiqués.",
  "limit.inaccessible_processes": "Des processus inaccessibles peuvent être plus gros ; les plages de travail et la mémoire engagée ne sont pas de la RAM exclusive.",
  "limit.nvml_instant": "NVML indique l'état du pilote à un instant donné ; un pic bref peut suffire à déclencher une raison.",
  "limit.gpu_memory_reserve": "La mémoire utilisée inclut la réserve du pilote ; Windows n'indique pas le processus qui l'occupe.",
  "limit.not_measured": "Non mesuré dans cette capture : {metrics}.",
  "fact.available_of_total": "La mémoire physique disponible était de {available} sur {total} ({share}).",
  "fact.available_share": "La mémoire physique disponible était de {share}.",
  "fact.hard_faults_over": "Les lectures de pages sur défaut matériel étaient en moyenne de {rate} sur {window}.",
  "fact.hard_faults": "Les lectures de pages sur défaut matériel étaient de {rate}.",
  "fact.disk_read_latency": "Le temps moyen de lecture du disque était de {latency}.",
  "fact.disk_write_latency": "Le temps moyen d'écriture du disque était de {latency}.",
  "fact.disk_read_latency_over": "Le temps moyen de lecture du disque était de {latency} sur {window}.",
  "fact.disk_write_latency_over": "Le temps moyen d'écriture du disque était de {latency} sur {window}.",
  "fact.disk_idle": "Le disque était inactif {share} du temps.",
  "fact.disk_queue": "Requêtes disque en attente à la fin de la mesure : {count}.",
  "fact.commit_of_limit": "La mémoire engagée du système était de {used} pour une limite de {limit} ({share}).",
  "fact.commit_share": "La mémoire engagée du système était à {share} de sa limite.",
  "fact.cpu_busy_over": "Le processeur était occupé à {share} de sa capacité totale sur {window}.",
  "fact.cpu_busy_of_total": "Le processeur était occupé à {share} de sa capacité totale.",
  "fact.cpu_busy": "Le processeur était occupé à {share}.",
  "fact.busiest_core": "Le processeur logique le plus chargé avait une utilité de {share}.",
  "fact.gpu_engine_over": "Le moteur du GPU le plus chargé était utilisé à {share} sur {window}.",
  "fact.gpu_engine_type": "Le {engine} a atteint cette valeur.",
  "fact.process_private_commit": "{process} avait {size} de mémoire privée engagée.",
  "fact.metric_value": "{metric} : {value}.",
  "fact.gpu_slowdown": "Le {gpu} a signalé des raisons de ralentissement : {reasons}.",
  "fact.gpu_temperature": "Le {gpu} était à {temperature}.",
  "fact.gpu_memory": "Le {gpu} utilisait {used} de mémoire sur {total} ({share}).",
  "observation.metric_value": "{metric} : {value} ({entity})",
  "observation.logical_processors": "Processeurs logiques : {count}.",
  "observation.installed_memory": "Mémoire installée : {size}.",
  "observation.architecture": "Architecture : {architecture}.",
  "observation.hardware_capacity": "L'inventaire matériel décrit les capacités, pas la cause d'un symptôme ni une recommandation de mise à niveau.",
  "observation.volume_free": "Espace libre du volume : {free} sur {total}.",
  "observation.scanned_size": "Taille logique analysée : {size} (ce n'est pas de l'espace récupérable).",
  "observation.scan_files": "Fichiers indexés par l'analyse : {count} ; ses compteurs de couverture indiquent les éléments ignorés.",
  "observation.ranked_items": "Éléments classés renvoyés : {count} ; cette page n'est pas un inventaire complet.",
  "observation.item_size": "{item} : {size} de taille logique ; ce n'est pas une recommandation de suppression.",
  "observation.history_range": "{metric} : {count} mesures exploitables, entre {min} et {max}.",
  "observation.history_interval": "Intervalle observé : {interval}.",
  "observation.history_gaps": "Interruptions enregistrées : {count} ; aucune interpolation ni tendance causale n'en est déduite.",
  "note.processes_system_list": "Tous les processus viennent d'une seule liste du système (NtQuerySystemInformation), sans en ouvrir aucun : les services et les processus des autres utilisateurs sont mesurés sans droits administrateur.",
  "note.processes_unknown": "La liste des processus n'a pas pu être lue ; la couverture est inconnue.",
  "note.processes_one_by_one": "La liste des processus du système était indisponible : les processus ont été ouverts un par un, et ceux des services ou des autres utilisateurs peuvent manquer.",
  "note.working_sets": "Les plages de travail incluent des pages partagées : leur somme n'est pas de la RAM physique exclusive.",
  "note.process_identities": "L'identité d'un processus ne vaut que pour cette session (PID et heure de création).",
  "note.process_rates": "Le processeur et les E/S des processus sont des débits sur la fenêtre de mesure (environ une seconde), ou depuis la lecture précédente du même processus.",
  "note.boot_id": "boot_id est une estimation (GetSystemTimeAsFileTime moins GetTickCount64, arrondie à la minute), fixe pour ce collecteur ; un changement d'horloge peut la décaler d'une session à l'autre.",
  "note.short_sample": "Mesure courte : elle ne permet d'établir ni saturation durable, ni fuite de mémoire, ni diagnostic de cause.",
  "note.recording_gpu": "Le GPU des processus enregistré utilise la fenêtre PDH existante pour la liste de processus retenue. Les premières fenêtres, les moteurs manquants et les identités impossibles à revérifier restent inconnus ; l'occupation n'est pas l'énergie consommée par application.",
  "note.activity_window": "Le processeur et les E/S sont des moyennes sur la fenêtre ; la mémoire est mesurée à la fin. Les débits et les variations de mémoire utilisent le PID et l'heure de création, jamais le PID seul.",
  "note.activity_groups": "Les groupes d'applications sont une estimation fondée sur l'image et le processus parent dans une session Windows, pas des identités de produit vérifiées. Des noms d'exécutables peuvent coïncider. Les shells génériques et les hôtes de services forment des groupes à part ; tous les membres sont additionnés avant que les listes renvoyées soient limitées.",
  "note.activity_references": "Les références d'applications ne valent que pour cette observation ; celles des processus restent stables pour un même PID et une même heure de création pendant la session du collecteur.",
  "note.activity_working_sets": "Les sommes de plages de travail incluent des pages partagées ; ce n'est pas de la RAM physique exclusive. Une hausse ou une baisse de mémoire est une variation nette observée, pas la preuve d'une fuite.",
  "note.activity_gpu": "Le GPU est le moteur physique le plus chargé observé. Le GPU d'une application additionne ses membres sur chaque moteur physique avant de prendre le maximum ; les instances de compteur manquantes ou nouvelles sont comptées dans la couverture. Sans instance GPU, la valeur est inconnue, jamais nulle.",
  "note.activity_exited": "Les processus terminés manquent à l'attribution de fin de fenêtre, et les processus nouveaux ou réutilisés n'ont pas de débit sur toute la fenêtre. Une fenêtre courte ne permet pas d'établir une saturation durable.",
  "note.activity_ranking": "Les lignes sont classées par processeur, puis par débit d'E/S, puis par plage de travail ; les services et les métadonnées brutes des processus restent sur ce PC jusqu'à la projection de confidentialité.",
  "note.activity_no_baseline": "La liste initiale des processus était indisponible ; la mémoire est mesurée, mais les débits et les variations sont inconnus.",
  "note.activity_no_final_list": "La liste finale des processus du système n'a pas pu être lue.",
  "note.services_visible": "L'énumération du gestionnaire de services couvre les services Win32 visibles par ce compte ; Windows peut omettre ceux dont le droit SERVICE_QUERY_STATUS est refusé. Les pilotes du noyau sont exclus.",
  "note.services_pids": "Les PID des services sont des observations, pas une autorisation d'agir ; un service qui démarre ou s'arrête n'a pas d'attribution de PID fiable.",
  "note.services_stalled": "La pagination du gestionnaire de services n'avançait plus ; les services restants sont indisponibles.",
  "note.services_bound": "L'énumération des services a atteint sa limite de pages.",
  "note.history_samples": "Mesures locales enregistrées ; les valeurs ne sont pas interpolées. Au-delà de 24 heures, ce sont des moyennes par minute pondérées par la durée, avec les minima et maxima mesurés. Les jauges instantanées utilisent l'intervalle d'échantillonnage comme poids, pas comme fenêtre mesurée.",
  "note.history_top_processes": "Seuls les 20 premiers processus de chaque mesure sont conservés ; l'absence d'un processus est inconnue, pas nulle.",
  "note.history_timestamps": "Les heures d'observation et la durée mesurée cumulée sont conservées à part de la cadence nominale. Le premier débit d'un enregistrement peut couvrir l'intervalle d'arrière-plan précédent. Les tranches d'une minute qui chevauchent la période demandée sont renvoyées en entier.",
  "note.history_events": "Les bornes d'un événement sont la première et la dernière mesure anormales, pas une durée continue mesurée. Les observations du pic et du contributeur gardent leurs propres fenêtres ; les valeurs dérivées indiquent leurs opérandes d'origine. Un contributeur est une utilisation observée des ressources, pas une cause prouvée.",
  "note.local_evidence": "Mesures locales uniquement : aucune explication n'a été demandée au modèle.",
  "note.ranked_working_set": "La liste des processus est classée par plage de travail ; les valeurs inconnues sont en dernier.",
  "note.ranked_private_commit": "La liste des processus est classée par mémoire privée engagée ; les valeurs inconnues sont en dernier.",
  "note.ranked_cpu": "La liste des processus est classée par processeur ; les valeurs inconnues sont en dernier.",
  "note.ranked_io_read": "La liste des processus est classée par débit de lecture ; les valeurs inconnues sont en dernier.",
  "note.ranked_io_write": "La liste des processus est classée par débit d'écriture ; les valeurs inconnues sont en dernier.",
  "metric.physical_total_bytes": "Mémoire physique",
  "metric.physical_available_bytes": "Mémoire disponible",
  "metric.system_commit_bytes": "Mémoire engagée",
  "metric.system_commit_limit_bytes": "Limite de mémoire engagée",
  "metric.cpu_busy_basis_points": "Processeur (tous les cœurs)",
  "metric.process_working_set_bytes": "Plage de travail",
  "metric.process_private_commit_bytes": "Mémoire privée engagée",
  "metric.process_cpu_basis_points": "Processeur",
  "metric.process_io_read_bytes": "Lectures E/S",
  "metric.process_io_write_bytes": "Écritures E/S",
  "metric.process_io_other_bytes": "Autres E/S",
  "metric.process_working_set_increase_bytes": "Hausse de la plage de travail",
  "metric.process_working_set_decrease_bytes": "Baisse de la plage de travail",
  "metric.process_private_commit_increase_bytes": "Hausse de la mémoire privée engagée",
  "metric.process_private_commit_decrease_bytes": "Baisse de la mémoire privée engagée",
  "metric.process_io_read_bytes_per_second": "Débit de lecture",
  "metric.process_io_write_bytes_per_second": "Débit d'écriture",
  "metric.process_io_other_bytes_per_second": "Débit des autres E/S",
  "metric.process_gpu_basis_points": "Moteur GPU le plus chargé du processus",
  "metric.cpu_busiest_core_basis_points": "Cœur le plus chargé",
  "metric.disk_read_latency_microseconds": "Temps de lecture du disque",
  "metric.disk_write_latency_microseconds": "Temps d'écriture du disque",
  "metric.disk_idle_basis_points": "Inactivité du disque",
  "metric.disk_queue_length": "File d'attente du disque",
  "metric.hard_fault_pages_per_second_x100": "Lectures de pages sur défaut matériel",
  "metric.gpu_engine_basis_points": "Moteur du GPU",
  "metric.gpu_busiest_engine_basis_points": "Moteur du GPU le plus chargé",
  "metric.gpu_utilization_basis_points": "Occupation du GPU",
  "metric.gpu_memory_used_bytes": "Mémoire du GPU utilisée",
  "metric.gpu_memory_total_bytes": "Mémoire du GPU",
  "metric.gpu_temperature_celsius": "Température du GPU",
  "metric.gpu_sm_clock_megahertz": "Fréquence du GPU",
  "metric.gpu_power_milliwatts": "Puissance du GPU",
  "metric.gpu_slowdown_reasons": "Ralentissement du GPU",
  "entity.system": "système",
  "entity.gpu": "GPU NVIDIA {n}",
  "engine.3d": "moteur 3D",
  "engine.compute": "moteur de calcul",
  "engine.copy": "moteur de copie",
  "engine.videodecode": "moteur de décodage vidéo",
  "engine.videoencode": "moteur d'encodage vidéo",
  "engine.videoprocessing": "moteur de traitement vidéo",
  "engine.overlay": "moteur de superposition",
  "engine.legacyoverlay": "moteur de superposition ancien",
  "engine.security": "moteur de sécurité",
  "engine.crypto": "moteur de chiffrement",
  "engine.vr": "moteur de réalité virtuelle",
  "engine.graphics": "moteur graphique",
  "engine.other": "autre moteur du GPU",
  "slowdown.power_cap": "plafond de puissance",
  "slowdown.hardware_slowdown": "ralentissement matériel",
  "slowdown.thermal_driver": "chaleur (pilote)",
  "slowdown.thermal_hardware": "chaleur (matériel)",
  "slowdown.power_brake": "frein d'alimentation",
  "slowdown.none": "aucune",
  "unit.pages": "{value} pages/s",
  "process.unnamed": "Processus sans nom",
  "process.unnamedPid": "Processus sans nom (PID {pid})",
  "application.unnamed": "Application sans nom",
  "missing.first_reading": "Pas encore mesuré : un débit demande deux lectures du même processus.",
  "missing.query_failed": "Windows n'a pas répondu à la requête de mesure.",
  "missing.inconsistent": "Le compteur a renvoyé une valeur incohérente.",
  "missing.gpu_unattributed": "Aucun moteur graphique n'a pu lui être attribué : la valeur est inconnue, pas nulle.",
  "missing.no_gpu_counters": "Windows n'expose ici aucun compteur GPU (carte d'affichage de base ou machine virtuelle).",
  "missing.gpu_counters_unusable": "Des compteurs GPU ont été signalés, mais aucun n'était exploitable.",
  "missing.nvml_unsupported": "Non disponible avec ce pilote NVIDIA.",
  "missing.nvml_error": "Le pilote NVIDIA a renvoyé une erreur.",
  "missing.status.partial": "Mesure incomplète.",
  "missing.status.unsupported": "Non disponible sur ce PC.",
  "missing.status.permission_denied": "Accès refusé.",
  "missing.status.stale": "Mesure trop ancienne.",
  "missing.status.cancelled": "Mesure interrompue.",
  "missing.status.error": "Échec de la mesure.",
  "coverage.note.system_cache": "Instantané des métadonnées seulement ; l'espace alloué observé n'est pas de l'espace récupérable. Les valeurs manquantes restent inconnues.",
  "coverage.note.performance_trace": "La collecte native exige un nettoyage validé ; ni identité de pilote ni attribution de cause. Les compteurs de pertes inconnus restent inconnus.",
  "coverage.note.ntfs_inventory": "Flux de métadonnées pris en charge seulement ; le lecteur était utilisé, ce n'est donc pas une image figée. L'espace alloué exclut les métadonnées et n'est pas de l'espace récupérable.",
  "coverage.note.ntfs_tree": "Plus gros éléments des métadonnées indexées seulement ; les chemins restent sur ce PC. Les sous-dossiers se chevauchent et ne s'additionnent pas. Les fichiers à liens physiques sont comptés une fois ; les tailles non attribuées restent à part.",
  "coverage.note.shadow_storage": "Inclut les autres clichés instantanés ; l'espace par point de restauration n'est pas disponible. Ce n'est pas de l'espace récupérable.",
  "coverage.note.disk_reliability": "Compteurs historiques tels que chaque disque les indique ; les champs absents sont inconnus.",
  "coverage.note.protected_process_images": "L'heure de création et la durée de vie du processus sont revérifiées avant et après la lecture du chemin. Les chemins complets et les identités restent sur ce PC.",
  "coverage.note.battery_energy_history": "Estimations historiques de Windows, pour l'utilisation sur batterie de ce compte seulement. Les heures n'ont pas de fuseau horaire connu, et les durées additionnées peuvent se chevaucher : ce n'est pas une fenêtre de mesure. Ce n'est ni une puissance physique, ni une attribution complète aux applications, ni la décharge de la batterie.",
  "coverage.note.elevated_task": "Aucune mesure globale exploitable n'a été renvoyée ; aucune recommandation de nettoyage n'est possible.",
  "coverage.reason.other": "Une autre limite de cette lecture.",
  "coverage.reason.elevation.metadata_snapshot": "Les tailles viennent des métadonnées du système de fichiers, à un instant donné.",
  "coverage.reason.elevation.allocation_lower_bound": "Les tailles allouées sont un minimum.",
  "coverage.reason.elevation.enumeration_incomplete": "Tous les éléments n'ont pas pu être listés.",
  "coverage.reason.elevation.entry_limit": "Le parcours a atteint sa limite d'éléments.",
  "coverage.reason.elevation.memory_limit": "Le parcours a atteint sa limite de mémoire.",
  "coverage.reason.elevation.duration_limit": "La vérification a atteint sa limite de durée.",
  "coverage.reason.elevation.cancelled": "La vérification a été arrêtée avant la fin.",
  "coverage.reason.elevation.restore_points_unimplemented": "Les points de restauration ne sont pas mesurés par cette vérification.",
  "coverage.reason.elevation.default_cache_location_only": "Seul l'emplacement par défaut du cache a été lu.",
  "coverage.reason.elevation.policy_cache_location_unconfirmed": "Un emplacement de cache défini par une stratégie n'a pas pu être confirmé.",
  "coverage.reason.elevation.mft_accuracy_unvalidated": "La précision de la lecture de l'index des fichiers n'est pas encore validée sur de vrais disques.",
  "coverage.reason.elevation.source_unavailable": "La source n'a pas pu être lue.",
  "coverage.reason.elevation.platform_unsupported": "Non pris en charge par cette version de Windows.",
  "coverage.reason.cache.denied_directories": "Dossiers impossibles à ouvrir (accès refusé) : {count}",
  "coverage.reason.cache.denied_files": "Fichiers impossibles à lire (accès refusé) : {count}",
  "coverage.reason.cache.reparse_skipped": "Liens et jonctions ignorés : {count}",
  "coverage.reason.cache.placeholders_skipped": "Fichiers cloud non téléchargés, ignorés : {count}",
  "coverage.reason.cache.other_volume_skipped": "Éléments sur un autre lecteur, ignorés : {count}",
  "coverage.reason.cache.special_skipped": "Fichiers spéciaux ignorés : {count}",
  "coverage.reason.cache.vanished": "Éléments disparus pendant le parcours : {count}",
  "coverage.reason.cache.hard_link_aliases": "Liens physiques comptés une seule fois : {count}",
  "coverage.reason.cache.changed_during_scan": "Éléments modifiés pendant le parcours : {count}",
  "coverage.reason.cache.allocation_unknown_files": "Fichiers dont la taille allouée est inconnue : {count}",
  "coverage.reason.cache.errors": "Erreurs de lecture : {count}",
  "coverage.reason.cache.truncated": "Le parcours s'est arrêté à sa limite.",
  "coverage.reason.cache.cancelled": "Le parcours a été arrêté avant la fin.",
  "coverage.reason.end.record_limit": "La lecture s'est arrêtée à sa limite d'enregistrements.",
  "coverage.reason.end.memory_limit": "La lecture s'est arrêtée à sa limite de mémoire.",
  "coverage.reason.end.run_limit": "La lecture s'est arrêtée à sa limite de fragments.",
  "coverage.reason.end.cancelled": "La lecture a été arrêtée avant la fin.",
  "coverage.reason.end.timed_out": "La lecture a atteint sa limite de durée.",
  "coverage.reason.end.source_unavailable": "L'index des fichiers n'a pas pu être lu.",
  "coverage.reason.end.invalid_source": "L'index des fichiers a renvoyé des données invalides.",
  "coverage.reason.mft.allocation_not_additive": "Les tailles allouées se chevauchent et ne s'additionnent pas.",
  "coverage.reason.mft.offline_subset": "Seuls les enregistrements lus pendant ce passage sont comptés.",
  "coverage.reason.mft.native_accuracy_unvalidated": "La précision de cette lecture n'est pas encore validée sur de vrais disques.",
  "coverage.reason.mft.metadata_allocation_excluded": "L'espace occupé par les métadonnées du système de fichiers n'est pas compté.",
  "coverage.reason.mft.system_records_excluded": "Les enregistrements propres au système de fichiers ne sont pas comptés.",
  "coverage.reason.mft.directory_allocation_excluded": "L'espace occupé par les index de dossiers n'est pas compté.",
  "coverage.reason.mft.named_data_excluded": "Les flux de données nommés ne sont pas comptés.",
  "coverage.reason.mft.reparse_data_excluded": "Les données des points d'analyse (liens, fichiers cloud) ne sont pas comptées.",
  "coverage.reason.mft.encrypted_data_excluded": "Les fichiers chiffrés ne sont pas comptés.",
  "coverage.reason.mft.unsupported_data_flags": "Les fichiers stockés d'une façon non prise en charge ne sont pas comptés.",
  "coverage.reason.mft.attribute_list_unresolved": "Certains fichiers volumineux ou fragmentés n'ont pas pu être entièrement résolus.",
  "coverage.reason.mft.missing_base": "Il manquait l'enregistrement principal de certains fichiers.",
  "coverage.reason.mft.missing_data": "Certains fichiers n'avaient pas de taille de données lisible.",
  "coverage.reason.mft.incomplete_extents": "Les fragments de certains fichiers n'ont pas tous pu être lus.",
  "coverage.reason.mft.compression_layout_unvalidated": "Les tailles des fichiers compressés ne sont pas validées.",
  "coverage.reason.mft.live_volume_not_atomic": "Le lecteur était utilisé pendant la lecture : ce n'est pas une image figée.",
  "coverage.reason.mft.volume_changed": "Le lecteur a changé pendant la lecture.",
  "coverage.reason.mft.change_witness_unavailable": "Les changements pendant la lecture n'ont pas pu être détectés.",
  "coverage.reason.mft.record_limit": "La lecture s'est arrêtée à sa limite d'enregistrements.",
  "coverage.reason.mft.memory_limit": "La lecture s'est arrêtée à sa limite de mémoire.",
  "coverage.reason.mft.duration_limit": "La lecture a atteint sa limite de durée.",
  "coverage.reason.mft.cancelled": "La lecture a été arrêtée avant la fin.",
  "coverage.reason.mft.source_unavailable": "L'index des fichiers n'a pas pu être lu.",
  "coverage.reason.mft.invalid_record": "Des enregistrements invalides ont été ignorés.",
  "coverage.reason.mft.run_limit": "La lecture s'est arrêtée à sa limite de fragments.",
  "coverage.reason.tree.index_memory_limit": "L'arborescence a atteint sa limite de mémoire.",
  "coverage.reason.tree.index_node_limit": "L'arborescence a atteint sa limite de taille.",
  "coverage.reason.tree.name_links_truncated": "Certains fichiers ont plus de noms que ceux conservés.",
  "coverage.reason.tree.invalid_file_name": "Certains noms de fichiers étaient invalides.",
  "coverage.reason.tree.missing_file_name": "Certains fichiers n'avaient pas de nom lisible.",
  "coverage.reason.tree.parent_missing": "Le dossier parent de certains éléments n'a pas été trouvé.",
  "coverage.reason.tree.parent_sequence_changed": "Certains dossiers parents ont changé pendant la lecture.",
  "coverage.reason.tree.parent_metadata_incomplete": "Les métadonnées de certains dossiers parents étaient incomplètes.",
  "coverage.reason.tree.parent_cycle": "Certains dossiers formaient une boucle et ont été écartés.",
  "coverage.reason.tree.path_conflict": "Certains chemins étaient ambigus.",
  "coverage.reason.tree.path_limit": "Certains chemins étaient trop longs pour être conservés.",
  "coverage.reason.tree.depth_limit": "Certains dossiers étaient trop profonds pour être conservés.",
  "coverage.reason.tree.filename_membership_unresolved": "Certains fichiers n'ont pas pu être placés dans un dossier.",
  "coverage.reason.tree.collection_incomplete": "L'index des fichiers n'a pas été lu en entier.",
  "coverage.reason.tree.interrupted": "La construction de l'arborescence a été interrompue.",
  "coverage.reason.tree.allocation_not_additive": "Les tailles allouées se chevauchent et ne s'additionnent pas.",
  "coverage.reason.tree.top_directories_truncated": "Seuls les plus gros dossiers sont listés.",
  "coverage.reason.tree.top_files_truncated": "Seuls les plus gros fichiers sont listés.",
  "coverage.reason.system.provider_unavailable": "Windows n'a pas fourni cette information.",
  "coverage.reason.system.provider_value_missing": "Certaines valeurs n'ont pas été indiquées par l'appareil ou par Windows.",
  "coverage.reason.system.provider_values_inconsistent": "Certaines valeurs indiquées étaient incohérentes et ont été écartées.",
  "coverage.reason.system.enumeration_incomplete": "Tous les éléments n'ont pas pu être listés.",
  "coverage.reason.system.row_limit": "La lecture a atteint sa limite d'éléments.",
  "coverage.reason.system.duration_limit": "La vérification a atteint sa limite de durée.",
  "coverage.reason.system.cancelled": "La vérification a été arrêtée avant la fin.",
  "coverage.reason.system.platform_unsupported": "Non pris en charge par cette version de Windows.",
  "coverage.reason.system.shadow_storage_includes_other_snapshots": "L'espace des clichés instantanés inclut aussi des copies qui ne sont pas des points de restauration.",
  "coverage.reason.system.restore_point_attribution_unavailable": "L'espace par point de restauration n'est pas disponible.",
  "coverage.reason.system.storage_counters_are_device_reported": "Les compteurs sont ceux que chaque disque indique lui-même.",
  "coverage.reason.system.raw_smart_attributes_unavailable": "Les attributs SMART bruts ne sont pas lus.",
  "coverage.reason.system.process_identity_changed": "Un processus a changé pendant la lecture.",
  "coverage.reason.system.process_exited": "Un processus s'est terminé pendant la lecture.",
  "coverage.reason.system.process_identity_unavailable": "L'identité d'un processus n'a pas pu être vérifiée.",
  "coverage.reason.system.image_path_too_long": "Le chemin d'un programme était trop long.",
  "coverage.reason.system.protected_process_denied": "Un processus protégé a refusé l'accès.",
  "coverage.reason.energy.historical_estimate": "Ce sont des estimations historiques de Windows, pas des mesures.",
  "coverage.reason.energy.provider_coverage_incomplete": "L'historique de Windows ne couvre pas toutes les périodes ni toutes les applications.",
  "coverage.reason.energy.civil_time_zone_unknown": "Les heures n'ont pas de fuseau horaire connu.",
  "coverage.reason.energy.row_durations_may_overlap": "Les durées additionnées peuvent se chevaucher.",
  "coverage.reason.energy.no_committed_on_battery_rows": "Windows n'a enregistré aucune utilisation sur batterie.",
  "coverage.reason.energy.export_unavailable": "L'export d'énergie de Windows n'est pas disponible.",
  "coverage.reason.energy.export_failed": "L'export d'énergie de Windows a échoué.",
  "coverage.reason.energy.export_encoding_unsupported": "L'encodage du texte de l'export n'est pas pris en charge.",
  "coverage.reason.energy.export_schema_unsupported": "Le format de l'export n'est pas pris en charge.",
  "coverage.reason.energy.export_malformed": "Des lignes mal formées de l'export ont été écartées.",
  "coverage.reason.energy.duplicate_row": "Les lignes en double ont été comptées une fois.",
  "coverage.reason.energy.numeric_overflow": "Certaines valeurs étaient trop grandes et ont été écartées.",
  "coverage.reason.energy.export_size_limit": "L'export a atteint sa limite de taille.",
  "coverage.reason.energy.row_limit": "La lecture a atteint sa limite de lignes.",
  "coverage.reason.energy.app_limit": "Seules les applications qui consomment le plus sont listées.",
  "coverage.reason.energy.duration_limit": "La vérification a atteint sa limite de durée.",
  "coverage.reason.energy.cancelled": "La vérification a été arrêtée avant la fin.",
  "coverage.reason.energy.platform_unsupported": "Non pris en charge par cette version de Windows.",
  "coverage.reason.energy.native_accuracy_unvalidated": "La précision de cette lecture n'est pas encore validée sur de vrais PC.",
};

export const DICTIONARIES = { en, fr };

/** A sentence of these dictionaries with its {placeholders} filled, or null when unknown. */
function say(key, vars = {}) {
  const template = (DICTIONARIES[language()] ?? en)[key] ?? en[key];
  if (template == null) return null;
  let missing = false;
  const text = template.replace(/\{(\w+)\}/g, (_, name) => {
    const value = vars[name];
    if (value == null || value === "") missing = true;
    return value ?? "";
  });
  return missing ? null : text;
}

/** The label of a metric (`cpu_busy_basis_points`), or null for a name this window does not know. */
export function metricLabel(name) {
  return say(`metric.${name}`);
}

/** "Processeur" → "processeur" inside a list; acronyms ("GPU memory") keep their capitals. */
function inSentence(label) {
  return label.length > 1 && label[1] === label[1].toLowerCase() ? label[0].toLowerCase() + label.slice(1) : label;
}

/** Metric labels listed inside a sentence, or null when one of them is unknown. */
function metricList(names) {
  if (!Array.isArray(names) || !names.length) return null;
  const labels = names.map(metricLabel);
  return labels.every(Boolean) ? labels.map(inSentence).join(", ") : null;
}

/** The GPU slowdown reasons of closed codes (`thermal_driver`), or "none" for an empty list. */
export function slowdownText(codes) {
  if (!Array.isArray(codes)) return null;
  if (!codes.length) return say("slowdown.none");
  const names = codes.map((code) => say(`slowdown.${code}`));
  return names.every(Boolean) ? names.join(", ") : null;
}

/** A reference the window names itself: the system, an NVIDIA GPU, a generic GPU engine type. */
function entityText(reference) {
  if (reference === "system") return say("entity.system");
  const gpu = /^gpu-nvidia-(\d+)$/.exec(reference);
  if (gpu) return say("entity.gpu", { n: gpu[1] });
  const engine = /^gpu-engine-([a-z0-9]+)$/.exec(reference);
  if (engine) return say(`engine.${engine[1]}`) ?? say("engine.other");
  // Other references (an application's opaque reference) read the same in every language.
  return reference;
}

/** A process of this PC by the name the window shows ("Google Chrome"), or the window's own word
 * for a process without a name, which only its PID tells apart (#70: the PID otherwise stays in
 * the inspector). */
function processText({ name, pid }) {
  return name ? name : say("process.unnamedPid", { pid });
}

/** One typed value (`{unit, value}`) as text in the interface language, or null if unknown. */
export function quantityText(quantity) {
  if (!quantity || typeof quantity !== "object") return null;
  const v = quantity.value;
  const n = typeof v === "number" && Number.isFinite(v) ? v : null;
  switch (quantity.unit) {
    case "bytes":
      return n == null ? null : bytes(n);
    case "bytes_per_second":
      return n == null ? null : `${bytes(n)}/s`;
    case "basis_points":
      return n == null ? null : basisPoints(n);
    case "microseconds":
      return n == null ? null : `${number(n / 1000, 1)} ms`;
    case "milliseconds":
      return n == null ? null : `${number(n)} ms`;
    case "pages_per_second_x100":
      return n == null ? null : say("unit.pages", { value: number(n / 100, 1) });
    case "count":
      return n == null ? null : number(n);
    case "celsius":
      return n == null ? null : `${number(n)} °C`;
    case "megahertz":
      return n == null ? null : `${number(n)} MHz`;
    case "milliwatts":
      return n == null ? null : `${number(n / 1000, 1)} W`;
    case "gpu_slowdown":
      return slowdownText(v);
    case "metric":
      return metricLabel(v);
    case "metrics":
      return metricList(v);
    case "entity":
      return typeof v === "string" ? entityText(v) : null;
    case "process":
      return v && Number.isInteger(v.pid) ? processText(v) : null;
    case "text":
      return typeof v === "string" ? v : null;
    default:
      return null;
  }
}

/**
 * A phrase (`{key, values}`) worded in the interface language, or `fallback` (the stored English
 * text) when there is none, its key is unknown or one of its values cannot be formatted.
 */
export function phraseText(phrase, fallback = null) {
  if (!phrase || typeof phrase.key !== "string") return fallback;
  const values = {};
  for (const [name, quantity] of Object.entries(phrase.values || {})) values[name] = quantityText(quantity);
  return say(phrase.key, values) ?? fallback;
}

// ---------- Findings and sources ----------

/** Both measurements stay visible, including real zeros and unknown sides. */
export function volumeComparison(volume) {
  if (!volume) return "";
  const show = (value) => Number.isFinite(value) && value >= 0 ? bytes(value) : t("value.unknown");
  return say("action.volume_comparison", {
    before: show(volume.free_before_bytes),
    after: show(volume.free_after_bytes),
  });
}

/** A finding's summary in the interface language. */
export function findingSummary(finding) {
  return phraseText(finding?.summary_phrase, finding?.summary ?? "");
}

/** A finding's facts, in order. */
export function findingFacts(finding) {
  return (finding?.facts || []).map((text, i) => phraseText(finding.fact_phrases?.[i], text));
}

/** A finding's suggested next step, or "" when it has none. */
export function findingNextStep(finding) {
  return finding?.next_step ? phraseText(finding.next_step_phrase, finding.next_step) : "";
}

/**
 * A measurement an answer cited during an earlier turn keeps only its English text: its phrase is
 * found among the earlier answers' sources of the same conversation, by the same text.
 */
function earlierPhrase(text, turns) {
  for (const turn of turns || []) {
    for (const source of turn?.report?.answer?.citations || []) {
      const i = (source.facts || []).indexOf(text);
      if (i >= 0 && source.fact_phrases?.[i]) return source.fact_phrases[i];
    }
  }
  return null;
}

/** The facts of a cited source, in order. `turns` are the conversation's, for earlier sources. */
export function citationFacts(citation, turns = []) {
  const phrases = citation?.fact_phrases || [];
  return (citation?.facts || []).map((text, i) => phraseText(phrases[i] ?? (citation.earlier ? earlierPhrase(text, turns) : null), text));
}

/**
 * What limited an answer: the turn's limitations, then its selected finding's, without repeats.
 * `worded` says whether the window put it in its own words; the others keep the core's English.
 */
export function limitationLines(report) {
  const seen = new Set();
  const lines = [];
  const add = (texts, phrases) =>
    (texts || []).forEach((text, i) => {
      if (seen.has(text)) return;
      seen.add(text);
      const worded = phraseText(phrases?.[i]);
      lines.push({ text: worded ?? text, worded: worded != null });
    });
  add(report?.limitations, report?.limitation_phrases);
  add(report?.selected?.limitations, report?.selected?.limitation_phrases);
  return lines;
}

/** A coverage target: a metric's label when it is one, else the opaque reference itself. */
export function targetLabel(target) {
  return metricLabel(target) ?? target;
}

// ---------- Live values and names ----------

/** Why a live value is missing, for its tooltip: its closed reason, else its status. */
export function missingReason(measure) {
  if (!measure || measure.value != null) return "";
  return (measure.note_code && say(`missing.${measure.note_code}`)) || (measure.status && say(`missing.status.${measure.status}`)) || "";
}

/** An application group's name, or the window's own word when none was read. */
export function applicationName(app) {
  return app?.unnamed ? say("application.unnamed") : app?.name ?? "";
}

/** A process's executable name, or the window's own word when none was read. */
export function processName(process) {
  return process?.unnamed ? say("process.unnamed") : process?.name ?? "";
}

/** An action target as the review lists it: a process without a name gets the window's words. */
export function targetText(target) {
  return target?.kind === "process" && target.unnamed && Number.isInteger(target.pid) ? say("process.unnamedPid", { pid: target.pid }) : target?.label ?? "";
}

// ---------- Elevated results ----------

function reasonText(reason) {
  const counted = reason?.count != null ? { count: number(reason.count) } : {};
  return say(`coverage.reason.${reason?.code}`, counted) ?? say("coverage.reason.other");
}

/**
 * The incomplete coverage of an elevated result (quick drive analysis, deeper checks), worded from
 * its closed notes (`coverage_notes`): what the scope leaves out and why it is incomplete. The
 * English `detail` is the model's text and never shown here; a scope without a note keeps its
 * status only.
 */
export function coverageLines(result) {
  const notes = result?.coverage_notes || [];
  return (result?.coverage || [])
    .filter((c) => c.status !== "ok")
    .map((c) => {
      const note = notes.find((n) => n.scope === c.scope);
      const reasons = [...new Set((note?.reasons || []).map(reasonText))];
      return { scope: c.scope, status: c.status, note: note ? say(`coverage.note.${note.note}`) : null, reasons };
    });
}
