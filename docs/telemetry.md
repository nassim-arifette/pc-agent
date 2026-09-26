# Telemetry crate (`pcagent-telemetry`) — Lot B

Basis: specification v0.1 sections 7.1–7.5, 13.3, 16.2, 17.1 and 19. Shared types live in `pcagent-shared`.

## Public API

```rust
Collector::new() / Collector::with_history(HistoryConfig)
Collector::snapshot(&mut self, include_processes: bool, limit: usize) -> Snapshot   // unchanged contract
Collector::sample_system(&mut self) -> Snapshot                                     // system only, reuses CPU ticks
Collector::process_tree(&mut self) -> ProcessTree                                   // get_process_tree
Collector::process_details(&mut self, entity_ref: &str) -> ProcessDetails           // get_process_details
Collector::hardware_inventory(&self) -> HardwareInventory                           // get_hardware_inventory
Collector::metric_history(&self, Metric, Duration) -> MetricSeries                  // get_metric_history
Collector::history(&self) -> &MetricHistory; boot_id(); session_id()
MetricHistory::{new, record, query}, SystemSample::from_snapshot                    // portable, fixture-friendly
cpu_busy_basis_points, process_cpu_basis_points, windows_family, boot_estimate_id, parse_multi_sz, tree::{link, relation}
```

## Design

- **Identity.** A session registry keys instances by (PID, creation FILETIME) and issues opaque `proc-<uuid>` refs. A complete enumeration retires instances whose PID vanished (`exited`) or now has a different readable creation time (`pid_reused`); unreadable PIDs keep their identity. Up to 256 retired instances are kept so lookups return a clear `stale` status. Enumerations flagged truncated never retire anything.
- **Process CPU** (`process_cpu_basis_points`): Δ(kernel+user) from two `GetProcessTimes` samples of the same instance divided by monotonic elapsed × `GetActiveProcessorCount(ALL_PROCESSOR_GROUPS)`, i.e. basis points of total capacity. First observation, regressions, empty windows and >5 % overshoot are `None` + `partial`; ≤5 % overshoot (timer granularity) is clamped to 10 000.
- **Process I/O**: `GetProcessIoCounters` transfer-count deltas (read/write/other bytes). Every item states that the counters include cached, network and device I/O and are not physical disk activity.
- **Process tree**: Toolhelp32 process snapshot (bounded to 16 384 entries) plus `GetProcessTimes` per node. `tree::relation` marks a parent created after its child as `pid_reused`, a missing parent as `parent_not_observed`, an unreadable creation time as `unknown`. `parent_ref` is only set for `observed` relations.
- **Details**: lookups accept only issued refs; bare PIDs return `not_found`. Live instances are re-opened and re-validated (creation time + `GetExitCodeProcess`) at lookup; evidence older than 30 s is `stale`.
- **Boot identity**: `boot-estimate-unix-s-<n>` = `GetSystemTimeAsFileTime − GetTickCount64`, rounded to one minute and computed once per collector. It is an estimate, labelled as such in snapshot and tree limitations; wall-clock changes can shift it between sessions. It is not a documented Windows boot identifier and must not be used alone to join persistent cross-boot history.
- **Inventory**: OS major/minor/build/UBR/DisplayVersion from `HKLM\SOFTWARE\Microsoft\Windows NT\CurrentVersion` (64-bit view), family inferred from the build (≥ 22000 → Windows 11), `GetNativeSystemInfo` architecture, logical processor count, `GetPhysicallyInstalledSystemMemory`, and volumes via `GetLogicalDriveStringsW`/`GetDriveTypeW`/`GetDiskFreeSpaceExW`. Only fixed and RAM-disk volumes are probed; network/removable/optical are listed with kind and `unsupported`. GPU is `unsupported` (DXGI not implemented). No serials or machine identifiers.
- **History**: `MetricHistory` is an in-memory ring buffer (default 1 800 samples / 1 h / 2 s expected interval) fed by every `snapshot`/`sample_system`. Queries are capped to retention and return explicit gaps (`not_recorded`, `evicted`, `missed_samples` > 2× interval, `boot_changed`), boot changes, median observed interval and unit. Values are never interpolated. Non-monotonic or zero-capacity records are rejected.
- **Cheap sampling**: `sample_system` reuses the previous CPU tick sample when it is 250 ms–60 s old instead of sleeping one second.

## Safety boundaries

The crate root is `#![deny(unsafe_code)]`; only the `windows` module allows it, with a `// SAFETY:` comment per block. Handles are RAII-owned and opened with `PROCESS_QUERY_LIMITED_INFORMATION` only. No process memory, command line or file content is read. Buffers are fixed-size; arithmetic on counters is checked. Non-Windows builds return `unsupported` for every native collector and never substitute host data.

## Validation status

- Portable logic (CPU share math, identity/PID reuse, tree validation, ring buffer bounds/gaps/boot changes, parsing) is unit-tested on Linux.
- Windows code is compiled and linted for `x86_64-pc-windows-msvc` from Linux. `#[cfg(windows)]` smoke tests run in the CI `windows-latest` job only; see the PR's CI run for their actual result.
- Not validated on a real Windows 11 PC: CPU share against Performance Monitor, I/O deltas, tree correctness under PID reuse, inventory values (DisplayVersion/UBR, installed memory, BitLocker-locked or offline volumes), boot estimate stability across clock changes and sleep, per-process cost of the collectors.
