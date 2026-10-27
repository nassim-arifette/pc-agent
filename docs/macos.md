# macOS native backend

The telemetry backend is selected at compile time through `telemetry::os::Backend`.
It uses read-only Darwin APIs, runs without elevation, and reuses the shared identity,
history and evidence types. It never makes a provider request.

This document currently describes the telemetry checkpoint, not a completed macOS
desktop release. Native inspections, actions, elevation and the WKWebView application
are separate integration work. Building the current `pcagent-gui` target on macOS
still builds its unsupported-platform entry point; it does **not** prove a native GUI.

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
GPU adapter inventory is not implemented in this checkpoint, so the hardware
inventory's aggregate status is `Partial`. A local mount is not evidence of a fixed
physical device: its kind remains `Unknown` until native storage discovery verifies it.
Mount capacity must not be summed as if every APFS volume had separate backing storage.

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
