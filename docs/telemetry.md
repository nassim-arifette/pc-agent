# Telemetry crate (`pcagent-telemetry`) — Lot B

Basis: specification v0.1 sections 7.1–7.5, 13.3, 16.2, 17.1 and 19. Shared types live in `pcagent-shared`.

## Public API

```rust
Collector::new() / Collector::with_history(HistoryConfig)
Collector::snapshot(&mut self, include_processes: bool, limit: usize) -> Snapshot   // unchanged contract
Collector::snapshot_sorted(&mut self, include_processes: bool, limit: usize, ProcessSort) -> Snapshot
Collector::sample_system(&mut self) -> Snapshot                                     // system only, reuses CPU ticks
Collector::process_tree(&mut self) -> ProcessTree                                   // get_process_tree
Collector::process_details(&mut self, entity_ref: &str) -> ProcessDetails           // get_process_details
Collector::hardware_inventory(&self) -> HardwareInventory                           // get_hardware_inventory
Collector::metric_history(&self, Metric, Duration) -> MetricSeries                  // get_metric_history
Collector::history(&self) -> &MetricHistory; boot_id(); session_id()
MetricHistory::{new, record, query}, SystemSample::from_snapshot                    // portable, fixture-friendly
cpu_busy_basis_points, process_cpu_basis_points, windows_family, boot_estimate_id, parse_multi_sz, tree::{link, relation}
counters::{counter_evidence, scaled, clamp_share, busiest_core, gpu_engine_type, gpu_engines, COUNTER_METRICS}
```

## Design

- **Identity.** A session registry keys instances by (PID, creation FILETIME) and issues opaque, random `proc-<8 hex>` refs, redrawn if they collide with a live or retired ref, so a model cannot guess the ref of a process it was not shown. A complete enumeration retires instances whose PID vanished (`exited`) or now has a different readable creation time (`pid_reused`); unreadable PIDs keep their identity. Up to 256 retired instances are kept so lookups return a clear `stale` status. Enumerations flagged truncated never retire anything.
- **Process CPU** (`process_cpu_basis_points`): Δ(kernel+user) from two `GetProcessTimes` samples of the same instance divided by monotonic elapsed × `GetActiveProcessorCount(ALL_PROCESSOR_GROUPS)`, i.e. basis points of total capacity. First observation, regressions, empty windows and >5 % overshoot are `None` + `partial`; ≤5 % overshoot (timer granularity) is clamped to 10 000.
- **Process I/O**: `GetProcessIoCounters` transfer-count deltas (read/write/other bytes). Every item states that the counters include cached, network and device I/O and are not physical disk activity.
- **Ranking**: `ProcessSort` selects working set (default), private commit, CPU, read I/O or write I/O. Ranking happens before truncation, across readable process identities; an unavailable memory measurement does not hide a readable CPU measurement. I/O ranks by bytes per second over the observed interval; missing values sort last. The first CPU/I/O observation is unknown, so the CLI takes two observations for those sorts.
- **Process tree**: Toolhelp32 process snapshot (bounded to 16 384 entries) plus `GetProcessTimes` per node. `tree::relation` marks a parent created after its child as `pid_reused`, a missing parent as `parent_not_observed`, an unreadable creation time as `unknown`. `parent_ref` is only set for `observed` relations.
- **Details**: lookups accept only issued refs; bare PIDs return `not_found`. Live instances are re-opened and re-validated (creation time + `GetExitCodeProcess`) at lookup; evidence older than 30 s is `stale`.
- **Boot identity**: native collection currently returns `None`. The pure `boot_estimate_id` helper remains tested but its wall-clock estimate is not used as an authorization identity. Persistent cross-boot joins and actions requiring a confirmed boot identity remain unavailable.
- **Inventory**: OS major/minor/build/UBR/DisplayVersion from `HKLM\SOFTWARE\Microsoft\Windows NT\CurrentVersion` (64-bit view), family inferred from the build (≥ 22000 → Windows 11), `GetNativeSystemInfo` architecture, logical processor count, `GetPhysicallyInstalledSystemMemory`, and volumes via `GetLogicalDriveStringsW`/`GetDriveTypeW`/`GetDiskFreeSpaceExW`. Only fixed and RAM-disk volumes are probed; network/removable/optical are listed with kind and `unsupported`. GPU is `unsupported` (DXGI not implemented). No serials or machine identifiers.
- **History**: `MetricHistory` is an in-memory ring buffer (default 1 800 samples / 1 h / 2 s expected interval) fed by every `snapshot`/`sample_system`. Queries are capped to retention and return explicit gaps (`not_recorded`, `evicted`, `missed_samples` > 2× interval, `boot_changed`), boot changes, median observed interval and unit. Values are never interpolated. Non-monotonic or zero-capacity records are rejected.
- **System performance counters (PDH)**: one query per collector (`PdhOpenQueryW`, `PdhAddEnglishCounterW` with language-neutral English paths, `PdhCollectQueryData`, `PdhGetFormattedCounterValue` / `PdhGetFormattedCounterArrayW` with `PDH_FMT_DOUBLE | PDH_FMT_NOCAP100`, `PdhCloseQuery` on drop). Every value is used only when both the call and its `CStatus` report valid data. Rate counters need two collections at least one second apart; the evidence window is the span between them. `snapshot` starts a fresh window before the CPU sleep; `sample_system` reuses a pending collection up to 60 s old and never sleeps (a window shorter than one second is `partial`, the pending collection is kept).

  | Metric | Counter | Unit |
  |---|---|---|
  | `disk_read_latency_microseconds` / `disk_write_latency_microseconds` | `\PhysicalDisk(_Total)\Avg. Disk sec/Read` / `sec/Write` | microseconds |
  | `disk_idle_basis_points` | `\PhysicalDisk(_Total)\% Idle Time` | basis points of the window (≤5 % overshoot clamped) |
  | `disk_queue_length` | `\PhysicalDisk(_Total)\Current Disk Queue Length` | outstanding requests, instantaneous (zero-length window at its end) |
  | `hard_fault_pages_per_second_x100` | `\Memory\Pages Input/sec` | pages/s × 100 |
  | `cpu_busiest_core_basis_points` | `\Processor Information(*)\% Processor Utility`, max over non-`_Total` instances | basis points of one logical processor; can exceed 10 000 with turbo |
  | `gpu_engine_basis_points` (entity `gpu-engine-<type>`) / `gpu_busiest_engine_basis_points` | `\GPU Engine(*)\Utilization Percentage`, per-process instances summed per physical engine, busiest engine per type | basis points of engine time |

  Absent objects, counters or instances (`PDH_CSTATUS_NO_OBJECT`, `NO_COUNTER`, `NO_INSTANCE`, …) are `unsupported`; an empty GPU Engine array (basic display adapter, most VMs/CI) is `unsupported`; invalid, negative, non-finite or out-of-range values are `partial` with no value. Nothing unknown becomes zero. GPU engine types are reduced to a fixed list of generic names (`3d`, `compute`, `copy`, `videodecode`, `videoencode`, …; anything else is `other`); PIDs, adapter LUIDs and engine indices are dropped before evidence is built. Processor Information uses `group,index` instances, so per-core CPU also works above 64 logical processors.
- **NVIDIA GPUs (NVML)**: `nvml.dll` is loaded at run time with `LoadLibraryExW(..., LOAD_LIBRARY_SEARCH_SYSTEM32)`, so only the copy the NVIDIA driver installs in System32 is used, never one from the current directory or `PATH`. It is initialized once per collector (`nvmlInit_v2`) and shut down with it. Per device (`nvmlDeviceGetCount_v2`, `nvmlDeviceGetHandleByIndex_v2`), read-only:

  | Metric (entity `gpu-nvidia-<index>`) | NVML function | Unit |
  | --- | --- | --- |
  | `gpu_utilization_basis_points` | `nvmlDeviceGetUtilizationRates` | basis points of the driver's last sample period (1/6 s to 1 s) |
  | `gpu_memory_used_bytes` / `gpu_memory_total_bytes` | `nvmlDeviceGetMemoryInfo` (v1: used = total − free, driver reserve included) | bytes |
  | `gpu_temperature_celsius` | `nvmlDeviceGetTemperatureV`, else the deprecated `nvmlDeviceGetTemperature` | degrees Celsius |
  | `gpu_sm_clock_megahertz` | `nvmlDeviceGetClockInfo(NVML_CLOCK_SM)` | MHz |
  | `gpu_power_milliwatts` | `nvmlDeviceGetPowerUsage` | mW |
  | `gpu_slowdown_reasons` | `nvmlDeviceGetCurrentClocksEventReasons`, else the deprecated `…ThrottleReasons` | NVML bits, masked to power cap, hardware, thermal and power-brake slowdowns |

  Prototypes, structures and constants follow NVIDIA's `nvml.h` (API 13). A function missing from the library, `NVML_ERROR_NOT_SUPPORTED` or a denied permission gives `unsupported`/`permission_denied` evidence with the NVML code; out-of-range values are errors. Nothing becomes zero. Without `nvml.dll` (no NVIDIA driver) nothing is added; a library that fails to initialize is stated as a limitation. Device names stay in `Snapshot.gpus` for local display and are never sent to a provider. Reading NVML can wake a sleeping discrete GPU on hybrid laptops, so it happens only when a snapshot is requested.
- **Cheap sampling**: `sample_system` reuses the previous CPU tick sample when it is 250 ms–60 s old instead of sleeping one second.

## Safety boundaries

The crate root is `#![deny(unsafe_code)]`; only the `windows` module allows it, with a `// SAFETY:` comment per block. Handles are RAII-owned and opened with `PROCESS_QUERY_LIMITED_INFORMATION` only. No process memory, command line or file content is read. Buffers are fixed-size; arithmetic on counters is checked. Non-Windows builds return `unsupported` for every native collector and never substitute host data.

## Validation status

- Portable logic (counter conversion, GPU engine aggregation and naming, CPU share math, identity/PID reuse, tree validation, ring buffer bounds/gaps/boot changes, parsing) is unit-tested on Linux.
- Native Windows smoke tests run locally and in CI. They verify availability and invariants on the test machine, not measurement accuracy across hardware configurations.
- NVML was read on a GeForce MX350 (WDDM, hybrid laptop) on 2026-09-27: 61–62 °C, 83.5 MiB of 2 GiB, SM 1354 MHz, no slowdown reason; power draw is `unsupported` there, as `nvidia-smi` shows `N/A`. CI runners have no NVIDIA driver and check that nothing is invented. Values were not compared under load or on other NVIDIA products.
- Not validated on a real Windows 11 PC: CPU share against Performance Monitor, I/O deltas, tree correctness under PID reuse, inventory values (DisplayVersion/UBR, installed memory, BitLocker-locked or offline volumes), boot estimate stability across clock changes and sleep, per-process cost of the collectors, PDH values against Performance Monitor (disk latency, Pages Input/sec, Processor Utility, GPU Engine on real WDDM drivers). The `GPU Engine` counter set is provided by the graphics kernel and its instance-name format is not a documented contract; unrecognized names are rejected or reported as `other`.
