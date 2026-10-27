# macOS native backend

The telemetry backend is selected at compile time through `telemetry::os::Backend`.
It uses read-only Darwin APIs, runs without elevation, and reuses the shared identity,
history and evidence types. It never makes a provider request.

This document describes implementation checkpoints, not a completed macOS desktop
release. The actual Tauri desktop target now compiles on macOS, including the command
handlers; this is no longer an unsupported-platform entry point. A native WKWebView
launch and visual validation are still required. Native actions, elevation
and OS-aware capability integration are still in progress.

## Measurements and meaning

| Measurement | Native source | Meaning / limitation |
| --- | --- | --- |
| Installed RAM | `sysctl(hw.memsize)` | Bytes of physical memory |
| Available RAM | `host_statistics64(HOST_VM_INFO64)` and `host_page_size` | Free plus inactive pages; speculative pages are already included in free pages. Reclaimable-memory estimate, not Activity Monitor memory pressure |
| System CPU | `host_statistics(HOST_CPU_LOAD_INFO)` | Busy fraction of aggregate user, system, nice and idle ticks; regressions/wraps are unknown |
| Per-process RSS and CPU | `proc_pidinfo(PROC_PIDTASKINFO)` | Resident bytes and CPU time normalized to all logical CPUs over a monotonic window |
| Process identity, parent, open files | `proc_pidinfo(PROC_PIDTBSDINFO)` | PID plus creation time; open file count is not a Windows handle count |
| Per-process I/O | `proc_pid_rusage(RUSAGE_INFO_V2)` | Physical disk read/write deltas; excludes network and cache-only I/O |
| Process session | `getsid` | POSIX session ID, not a Windows interactive session |
| OS, architecture, CPUs | `sysctl(kern.osproductversion, hw.machine, hw.logicalcpu)` | No host name, hardware serial or machine UUID |
| Mounted volume capacity | `getfsstat(MNT_NOWAIT)` | Cached total/free/available bytes; APFS volumes may share a container |
| Sampler overhead | `CLOCK_THREAD_CPUTIME_ID` and own `PROC_PIDTASKINFO` | Calling thread CPU and process RSS |

Windows system/private commit and per-process GPU metrics remain `Unsupported`;
swap usage, wired memory and virtual address-space size are not substituted.
GPU adapter names now come from `MTLCopyAllDevices`; dedicated VRAM remains unknown
on unified-memory hardware. A local mount is not evidence of a fixed
physical device: its kind remains `Unknown` until native storage discovery verifies it.
Mount capacity must not be summed as if every APFS volume had separate backing storage.

## Native inspection checkpoint

The macOS inspector now reads battery sources through IOKit, the caller's launchd
services through `launchctl list`, system-domain services through `launchctl print system`,
bounded error/fault counts through `log show`,
and available updates through the read-only `softwareupdate --list` scan.
The latter may contact Apple's update service; it never downloads or installs updates.
Commands use absolute allowlisted executables, no shell, fixed locale, bounded
stdout/stderr, a deadline and a dedicated child process group that is killed and
reaped on failure. Raw logs, update titles, paths and command lines never enter evidence.

Metal exposes GPU names (local display only), unified memory, removable/low-power
flags and the recommended working-set budget. That budget is **not** VRAM usage
or VRAM capacity. Foundation exposes thermal pressure, not a temperature; Apple
returns nominal on unsupported devices, so nominal is explicitly ambiguous.
Low-power-mode `false` can also mean unsupported. Stable public temperature
sensors are unavailable and root-only `powermetrics` is not run by the inspector.

Battery capacity remains in relative native units; energy, wear and cycle count
are unknown when the public power-source API cannot establish them. Missing or
contradictory values never become measured zeros.

Startup inspection reads user, local and system LaunchAgent plists through Core
Foundation and merges their Disabled key with current-GUI-domain launchctl overrides.
Enabled does not mean running. It reads legacy Open at Login items through a fixed
System Events script; Automation/TCC denial is explicit, not an empty list. Modern
third-party SMAppService background permissions have no public enumeration API here
and remain explicit partial coverage. Names stay in the local name map; plist
commands, paths and content never enter observations.

System state includes public OS version, uptime, active CPU count, physical memory,
load averages and update offers. Load is not CPU percent; an update requiring a
restart is not a system-wide pending-restart flag.

Cleanup, actions, elevation and the native desktop remain separate integration work.
Targeted macOS inspection tests: 77 passed, including fixture privacy, unknown/error states, time/output bounds,
child cleanup and a live Metal/Foundation hardware projection.

The shared desktop conversion also rejects root and mismatched real/effective Unix
credentials before starting the host. Existing GUI/actions/elevation unit tests run
on macOS: 151 passed at this checkpoint, with targeted Clippy clean. This does not
claim an interactive GUI run or a separately provisioned standard-account validation.

## Identity, coverage and privacy

- Creation times are converted from Unix microseconds to the shared 100-nanosecond
  FILETIME epoch. The identity field keeps the same unit and epoch across platforms.
- Each process read checks creation time before and after collecting its counters.
  The shared registry never transfers history or action authority to a reused PID.
- Process-tree links use the shared creation-time validator. Missing, denied and
  unverifiable nodes are explicit; parent observations never authorize actions.
- Every enumerated PID contributes to readable, permission-denied or unavailable
  coverage. Output limits apply after enumeration and ranking. The native macOS
  backend accepts the requested row count; callers may still impose their own limits.
- Activity rows currently form one-process application groups. They do not claim
  to reconstruct macOS application bundles. Newly readable is not proof of launch;
  disappearance behind a permission failure is not reported as exit.
- Argument buffers are inspected locally only for closed application-role markers
  and are cleared and dropped immediately. No arguments, environment values,
  executable paths or UIDs enter an observation or log.
- Executable basenames and volume mount points are local metadata; model disclosure
  continues to go through the existing C5 preview/projection, never a raw inventory.

## Sampling and local validation

`pcagent snapshot --json` exercises the native collector without cloud access.
Explicit snapshots obtain a one-second CPU window. Background sampling does not
sleep to manufacture a first measurement: CPU is unknown until a valid later
baseline exists. Per-process rates require the same identity and a nonzero window;
missing counters are never emitted as measured zero.

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
cargo build --release --locked \
  -p pcagent -p pcagent-gui -p pcagent-eval -p pcagent-elevated
node --experimental-default-type=module --test apps/gui/dev/*.test.mjs
python3 scripts/test-eval-mcp-client.py
```

Use Rust 1.90 and Node 22. Keep Cargo's target directory outside the checkout.
Native tests cover live memory/CPU/process identity, process details/tree,
background warm-up and activity evidence uniqueness. Synthetic tests cover
page accounting/overflow and argument/environment separation.

A non-root run on an administrator-capable macOS account is **not** evidence of
testing under a separate standard account. Native desktop EN/FR, light/dark,
standard-account testing, unsigned `.app` packaging and Gatekeeper instructions
must be verified with the completed desktop integration before a macOS release.
