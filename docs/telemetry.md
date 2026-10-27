# Telemetry crate (`pcagent-telemetry`)

The current scope and evidence/privacy contracts are in [the project plan](plan.md). Shared types live in `pcagent-shared`.

## Public API

```rust
Collector::new() / Collector::with_history(HistoryConfig)
Collector::snapshot(&mut self, include_processes: bool, limit: usize) -> Snapshot   // unchanged contract
Collector::snapshot_sorted(&mut self, include_processes: bool, limit: usize, ProcessSort) -> Snapshot
Collector::sample_system(&mut self) -> Snapshot                                     // system only, reuses CPU ticks
Collector::sample_processes(&mut self, limit: usize, ProcessSort) -> Snapshot         // reused windows, background history
Collector::sample_recording_processes(&mut self, limit: usize, ProcessSort) -> Snapshot // bounded GPU attribution, recording only
Collector::reset_recording_gpu(&mut self)                                            // clears attribution at recording start
Collector::process_activity(&mut self, window_seconds: u64, limit: usize) -> ProcessActivity
Collector::process_tree(&mut self) -> ProcessTree                                   // get_process_tree
Collector::process_details(&mut self, entity_ref: &str) -> ProcessDetails           // get_process_details
Collector::hardware_inventory(&self) -> HardwareInventory                           // get_hardware_inventory
Collector::metric_history(&self, Metric, Duration) -> MetricSeries                  // get_metric_history
Collector::instance(&self, entity_ref: &str) -> Option<&ProcessInstance>            // registry only, local display
Collector::history(&self) -> &MetricHistory; boot_id(); session_id()
ProgramNames::new(); ProgramNames::name(&self, pid: u32, created: u64) -> Option<String> // readable names, window only
MetricHistory::{new, record, query}, SystemSample::from_snapshot                    // portable, fixture-friendly
cpu_busy_basis_points, process_cpu_basis_points, windows_family, boot_estimate_id, parse_multi_sz, tree::{link, relation}
counters::{counter_evidence, scaled, clamp_share, busiest_core, gpu_engine_type, gpu_engines, COUNTER_METRICS}
service_processes() -> ServiceProcesses                                            // visible Win32 SCM services
current_resource_usage() -> ResourceUsage                                          // local sampler footprint
```

## Design

### Browser process activity

Windows activity classifies Firefox, Chrome, Edge, WebView2 and Chromium/Electron child
processes locally. `BrowserProcessType` is closed: `browser_main`, `renderer`, `extension`,
`gpu`, `network`, `storage`, `audio`, `media_decoding`, `utility`, `crash_handler`, `unknown`.
Firefox's `-contentproc` type and Chromium's fixed process/utility switches supply the
classification. For an otherwise unknown executable, a Chromium IPC signature is required;
`--type=renderer` alone does not establish a Chromium runtime. This heuristic does not
establish a product identity or authorize an action.

The Windows adapter reads `NtQueryInformationProcess(ProcessCommandLineInformation)` with
`PROCESS_QUERY_LIMITED_INFORMATION`, with a local WMI fallback. PID and creation time must
still match before and after the read. Arguments and temporary UTF-16 buffers are discarded
immediately after classification. **Command lines are never retained, logged, recorded,
selected or sent to a model.** Public DTOs contain only the closed family/type and status.
Missing access or an unrecognized switch becomes `unknown`, not a guessed renderer.

Application and per-type counts, classified counts and evidence sums cover all observed
members, before response ranking. Each type has its own opaque evidence reference; memory,
CPU and I/O retain their measured windows/statuses. Classification runs after the final
counter timestamp so its latency does not dilute CPU/I/O rates. Live PC requests the full
application inventory; callers may still choose a smaller response, with coverage explicit.
Summed working sets overlap shared pages: they are neither unique physical RAM nor exact
memory per tab, URL or site. A renderer may represent several tabs or sites.

### Ephemeral browser titles

`local_browser::read` is a separate desktop-only path, not a collector or model tool.
`EnumWindows`/`GetWindowTextW` reads visible window captions for revalidated browser instances.
On an explicit second request, UI Automation reads accessible TabItem names and selection
states. A browser/version that does not expose its tab controls reports `unsupported` or
`partial`; no tab count or title is invented, and captions are not mapped to renderer memory.

These response types are deliberately neither measurement nor selection-source types.
Titles do not enter SQLite, history, logs, eval captures or any model projection. The desktop
holds them in ephemeral view memory and clears it when leaving Live PC. The exact selection
preview contains the measured browser breakdown, never this local text. Inspection does not
send titles even with executable-name sharing enabled.

- **Identity.** A session registry keys instances by (PID, creation FILETIME) and issues opaque, random `proc-<8 hex>` refs, redrawn if they collide with a live or retired ref, so a model cannot guess the ref of a process it was not shown. A complete enumeration retires instances whose PID vanished (`exited`) or now has a different readable creation time (`pid_reused`); unreadable PIDs keep their identity. Up to 256 retired instances are kept so lookups return a clear `stale` status. Enumerations flagged truncated never retire anything.
- **Process CPU** (`process_cpu_basis_points`): Δ(kernel+user) from two `NtQuerySystemInformation.SystemProcessInformation` readings of the same PID and creation time, divided by monotonic elapsed × `GetActiveProcessorCount(ALL_PROCESSOR_GROUPS)`, i.e. basis points of total capacity. The per-handle fallback uses `GetProcessTimes`. Explicit snapshots take a fresh baseline, which overrides older registry counters; reused samples reject windows older than 60 seconds. Missing baselines, regressions, empty windows and >5 % overshoot are `None` + `partial`; ≤5 % overshoot (timer granularity) is clamped to 10 000.
- **Process I/O**: system-list transfer-count deltas, or `GetProcessIoCounters` in the fallback (read/write/other bytes). Activity and persistent history convert them to bytes per second using the actual observation window. Every item states that the counters include cached, network and device I/O and are not physical disk activity.
- **Ranking**: `ProcessSort` selects working set (default), private commit, CPU, read I/O or write I/O. Ranking happens before truncation, across readable process identities; an unavailable memory measurement does not hide a readable CPU measurement. I/O ranks by bytes per second over the observed interval; missing values sort last. Explicit snapshots bracket a first process window; reused samples need an earlier observation.
- **Process tree**: the system process list supplies parent PIDs and creation times even when protected processes cannot be opened. Toolhelp32 plus `GetProcessTimes` is the bounded fallback. `tree::relation` marks a parent created after its child as `pid_reused`, a missing parent as `parent_not_observed`, an unreadable creation time as `unknown`. `parent_ref` is only set for `observed` relations. Nodes and links use registry-generated opaque references, never PIDs embedded in a reference.
- **Details**: lookups accept only issued refs; bare PIDs return `not_found`. Live instances are re-opened and re-validated (creation time + `GetExitCodeProcess`) at lookup; evidence older than 30 s is `stale`.
- **Boot identity**: native collection estimates boot time from wall clock minus uptime, rounded to a minute and fixed for the collector lifetime. This local estimate is not a confirmed identity; clock changes can shift it between sessions. It must not authorize actions or justify cross-session identity joins.
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
- **Process activity**: a configurable 1–30-second window (3 seconds by default) matches process instances across two system lists. Working-set/private-commit increases and decreases are distinct unsigned metrics; a decrease is never reported as growth. CPU, byte rates and memory observations retain the real monotonic window. PID 0 is idle accounting, not an application; processes without a usable creation time remain uncovered. Applications combine same-image processes within a Windows session and validated descendants, avoiding grouping unrelated children of shell/service hosts. Microsoft Edge WebView2 runtime processes (`msedgewebview2.exe`), which many applications embed (Teams, Outlook, Widgets, Search, PC Agent itself), are never joined by image: each WebView2 tree (a browser process and its renderer, GPU, utility and crash-handler children) joins the application that started it through the same validated parent chain (observed parent, created no later than its child, same session, not a shell/service host). A tree whose starter is a shell or service host, has exited, runs in another session or has a reused PID forms its own `msedgewebview2.exe` group per tree; a browser process shared by several hosts (same user data folder) counts toward the host that started it. A group is named after its earliest member, preferring an application over a hosted runtime. Only parent PIDs, creation times, sessions and image names are used; no command line is read. Aggregates are computed before truncation and list the retained member references and member truncation. Missing members keep aggregates partial. Service names remain local until the explicit sharing projection.
- **Per-process GPU**: the PDH GPU Engine query is bracketed by both identity-bearing process lists. Only stable PID/creation pairs receive engine measurements. Application aggregation adds members on the same physical engine, then takes the busiest engine; it does not sum independent per-process maxima. Driver counter instance identifiers, PIDs and adapter LUIDs remain private. Invalid, duplicated, missing and unattributable instances are counted in coverage; absence is never zero GPU use.
- **Recorded process GPU**: only requested recordings use `sample_recording_processes`. It reads the already bounded process list before the existing PDH read, retaining at most 20 process identities under that read's next monotonic baseline. On the next matching PDH window, query-only handle checks after the read confirm unchanged, still-running instances: `GetExitCodeProcess` precedes `GetProcessTimes`, whose nonzero exit FILETIME also excludes a terminated process with exit code 259. No second system-list enumeration or measurement sleep is added. Reset or unavailable baselines, first windows, missing engines, exited processes, reused PIDs and denied checks yield unknown values. Invalid or duplicate engine instances make the affected process's GPU unknown; invalid PDH entries without a usable identity make every per-process value in that window unknown. Rejection counts remain explicit in evidence limitations. Each GPU value retains the PDH source, actual window and engine-time unit, and a UTC span inconsistent with the monotonic duration by more than 50 ms is rejected. Normal background samples keep their existing collection path. The retained list is ranked by CPU during recordings, so recorded CPU/GPU/I/O peaks are candidates among observed processes, not an exhaustive application ranking or per-application energy measurement.
- **Services**: `EnumServicesStatusExW` enumerates Win32 services using only the SCM enumeration right. It returns name, display name, state and a usable running-service PID; state transitions whose PID is not meaningful return PID 0. Enumeration is bounded and read-only. Windows can omit services for which query-status access is denied; drivers are outside this enumeration. Consequently attribution describes visible services, not proof that no other service shares a host.
- **Readable program names** (`ProgramNames`, for the desktop window only): the name Task Manager shows, such as *Google Chrome* for `chrome.exe`. For one process instance (PID plus creation time), `OpenProcess` with `PROCESS_QUERY_LIMITED_INFORMATION` (RAII handle) first checks the creation time, so a reused PID never lends another program its name, then `QueryFullProcessImageNameW` gives the executable's path. Only absolute paths on a fixed, removable or RAM drive letter (`GetDriveTypeW`) are read: a bare name would be searched along the DLL search path, and a UNC, device or network-drive path could block on the network. `GetFileVersionInfoSizeW` and `GetFileVersionInfoW` (at most 256 KiB, the file mapped as data, never run) then `VerQueryValueW` read `FileDescription`, else `ProductName`, from the string table of the first `\VarFileInfo\Translation` entry, then from the U.S. English (Unicode, then Windows-1252) and language-neutral tables. The value is cleaned like an executable name: control, bidirectional-formatting and byte-order characters removed, whitespace collapsed, trimmed, at most 64 characters; an empty value counts as none. Results, misses included, are cached by lower-case full path (512 executables) and by instance (2048), the oldest forgotten first, so a refresh opens no process twice; the paths stay in that cache and are never returned. No administrator rights are needed: a process that refuses a query-only handle (some protected processes and other users' processes), the idle and System processes, pseudo-processes without a file and executables without a usable version resource keep their executable name. The name is local: it is never part of a snapshot, a tool output, a preview or a model payload (C5, note of 26 October). Other systems have no readable names. Icons are not read.
- **Cheap sampling**: `sample_system` reuses the previous CPU tick sample when it is 250 ms–60 s old instead of sleeping one second. `sample_processes` also reuses process counters; after initial warm-up it introduces no fixed sleep at the background 5-second or recording 1-second cadence. The store owns durable history, cadence, retention, gaps and global historical aliases; collector references are scoped to a collector session.
- **Footprint**: `current_resource_usage` reads this calling thread's accumulated kernel+user CPU time (`GetThreadTimes`), the whole host process working set and the active logical-processor count across all groups. The sampler derives CPU from counter differences and elapsed monotonic time, retaining both one-core and whole-machine denominators. Working set is not the sampler's incremental RAM. Non-Windows values are `None`.

## Safety boundaries

The crate root is `#![deny(unsafe_code)]`; only the `windows` module allows it, with a `// SAFETY:` comment per block. Processes are read from one system list (`NtQuerySystemInformation`) without opening them; the fallback opens handles with `PROCESS_QUERY_LIMITED_INFORMATION` only, RAII-owned, as do readable program names. No process memory or command line is read; the only file data read is a running executable's version resource (`GetFileVersionInfoW`, mapped as data), for its readable name. Buffers are fixed-size; arithmetic on counters is checked. Non-Windows builds return `unsupported` for every native collector and never substitute host data.

## Validation status

- Portable logic (counter conversion, GPU engine aggregation and naming, CPU share math, identity/PID reuse, tree validation, application grouping, ring buffer bounds/gaps/boot changes, parsing) is unit-tested on Linux.
- WebView2 attribution is tested on synthetic process rows (Teams, Outlook and PC Agent hosts; exited, reused-PID, other-session and shell/service-host starters). It has not yet been checked against real Teams, Outlook, Widgets or Search trees on a Windows 11 PC.
- Native Windows smoke tests run locally and in CI. They verify availability and invariants on the test machine, not measurement accuracy across hardware configurations.
- A local 3-second activity capture on 2026-10-02 covered 385/385 readable process identities, 141 attributed services and 25 processes with usable GPU-engine observations (GPU coverage remained partial). The observed window was 3008 ms. The first debug activity call took 3887 ms wall time, 625 ms of the calling thread's CPU and a whole-process working set of 22,974,464 bytes. Timing starts after constructing the collector and is not a background-sampler cost measurement. The raw capture was kept outside the repository. No process names or local identifiers from it are reproduced here.
- NVML was read on a GeForce MX350 (WDDM, hybrid laptop) on 2026-09-27: 61–62 °C, 83.5 MiB of 2 GiB, SM 1354 MHz, no slowdown reason; power draw is `unsupported` there, as `nvidia-smi` shows `N/A`. CI runners have no NVIDIA driver and check that nothing is invented. Values were not compared under load or on other NVIDIA products.
- Still not independently compared against Performance Monitor: CPU share, I/O rates, disk latency, Pages Input/sec, Processor Utility and per-process GPU engine use. Tree correctness under real PID reuse, inventory edge cases, and boot-estimate stability across clock changes and sleep also remain unvalidated. The `GPU Engine` counter set is provided by the graphics kernel and its instance-name format is not a documented contract; unrecognized names are rejected or reported as `other`.
