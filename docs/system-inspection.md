# Native system inspection

`pcagent-inspect` implements the read-only Windows tools registered by
`crates/agent/src/system_tools.rs`. The public entry point is `inspect(Query)`. The tool registry
accepts only bounded integers and enumerated query names; it cannot accept a command, WQL, a
registry path, an event XPath, a remote host, or an arbitrary filesystem root.

Every result contains its collection time, origin, time window, typed observations, and coverage
per source. Observations include an opaque entity reference, metric, value, unit, native source,
and status. Missing values remain `null`. Coverage distinguishes `ok`, `partial`, `denied`,
`unsupported`, `needs_admin`, and `error`; the existing outer agent status remains compatible.
An inaccessible source does not prevent usable measurements from other sources being returned.

## Sources and bounds

| Tool | Sources and limits |
| --- | --- |
| `get_startup_programs` | Run/RunOnce names, HKCU once and HKLM 64/32-bit views, at most 500 names per key; StartupApproved; current-user/common Startup folder names; Task Scheduler COM, at most 128 visited/queued folders combined, 2,048 tasks, 100 triggers per task, and three seconds. Only logon tasks are reported. |
| `get_services` | Shared telemetry SCM enumeration, up to 500 services; read-only service start configuration; seven days of retained SCM events, associated using only the structured service-name field. Opaque host references group services sharing a process within the result. |
| `get_recent_events` | EvtQuery/EvtRender, at most 30 days, four seconds, 5,000 examined matching records, and 200 returned occurrence timestamps. Fixed providers/IDs cover power loss, crashes/hangs, WHEA, disk/storage, update failures, service failures, and resource exhaustion. |
| `get_system_state` | GetTickCount64, Windows version and reboot marker registry keys, fast-startup setting, WMI pagefile/hotfix fields; Windows Update Agent local history and installation-busy getter. At most 100 update-history records are examined. |
| `get_power_and_thermal` | GetSystemPowerStatus, PowerGetActiveScheme, two PDH samples one second apart for CPU frequency/performance limits and thermal zones, ACPI thermal WMI if permitted, existing NVML/GPU telemetry. At most 64 PDH thermal zones. |
| `get_installed_apps` | HKLM 64/32-bit and shared HKCU Uninstall registrations, at most 2,000 enumerated subkeys per view and 500 returned registrations. Display name/publisher, version, registry date, estimated size only. |
| `get_hardware_inventory` | Existing version/CPU/memory/volume inventory plus SMBIOS type 17 (16 MiB table cap and 128 memory devices), fixed WMI CPU/storage properties, at most 32 physical-drive IOCTL queries, and graphics driver registry version/date. Slot counts become partial lower bounds when additional memory devices are omitted. |
| `get_cleanup_opportunities` | Recycle-bin totals and metadata-only scans for every planned category. Each category gets 500 ms and 5,000 filesystem entries; pending traversal is also bounded. Observed reparse points, their ancestors, remote/removable roots, and file contents are not traversed/read. |
| `query_system` | Fixed names: operating_system, processor, physical_memory, pagefile, last_updates, thermal_zones, storage_devices, system_events, application_events, windows_version. WMI uses selected columns, three seconds of enumeration and at most 100 rows. |

All native inspections run on one dedicated worker at a time. The caller waits at most 15 seconds.
If a provider is still blocked, subsequent inspections report that the previous worker is busy;
they cannot accumulate additional workers. A timeout does not claim that the native API was
cancelled. COM initialization outlives all collected interfaces **and error objects**, including
IErrorInfo retained by failed WMI calls.

## Privacy and interpretation

Display names are held in a separate, non-serialized map. The agent exposes them only in complete
sharing as `shared_names`, after the common privacy sanitizer; anonymized sharing omits that map.
The cloud projection retains at most 500 observations in collection order, without changing the
local inspection. A truncated projection has partial status, an explicit coverage entry, and
returned/omitted/total observation counts. Only names referenced by retained observations, their
explicit opaque references (such as publishers), and associated service display names are shared.
No command values, scheduled-task actions, service binary paths/accounts, event message text,
machine identifiers, firmware serials, hardware IDs, or file contents are projected. Pending file
rename paths are discarded after checking whether the registry list is empty. Cleanup filesystem
paths stay local. Update history is a local database query; no update search/download is performed.

The following limits are part of the result, not hidden assumptions:

- Unknown StartupApproved formats or absent approval values do not establish enabled/disabled
  state. RunOnce lacks an equivalent reliable approval flag. Protected tasks can be omitted.
  An incomplete trigger scan cannot establish that an enabled task has no active logon trigger;
  its startup state remains unknown unless a known active trigger or disabled task resolves it.
- SCM can omit services whose query-status access is denied. Service host aliases in different
  tool observations are not process identities; use process activity for cross-resource evidence.
- Event counts cover retained matching logs only. Cleared/rotated logs are not reconstructed;
  capped counts are lower bounds. Security logs and arbitrary event messages are never queried.
- Hotfix inventory excludes some update classes, and registry application dates can indicate
  servicing or repair. WUA reports the most recent successful installation in retained history;
  it does not prove that the update remains installed after a later uninstall.
- Uninstall sizes are installer estimates, and Store/portable applications may be absent.
- Thermal zones are not necessarily CPU package sensors. Performance limits do not by themselves
  establish overheating. Unsupported firmware/driver sensors remain unknown.
- Cleanup sizes are logical file lengths, may overlap categories, and are not guaranteed reclaimable
  disk space. Capped traversal returns lower bounds. Downloads are restricted to files at least
  100 MiB and 180 days old. Virtual disks require review; pagefile/hibernation files are `keep`.
  Browser cache discovery covers Chrome, Edge, and Firefox known layouts. Metadata can change
  concurrently; no cleanup or deletion is performed by these tools.

## Local autostart preference

`pcagent_inspect::autostart::{get, set}` is a separate local application-preference API, never a
model tool. After a local UI decision, `set` can change only the current user's `Run\PcAgent`
value. The enabled value contains the canonical current executable, quoted, plus the fixed
`--background` switch, with the documented 260 UTF-16-unit Run limit. Another executable,
relative/UNC paths, quotes, and control characters are rejected. This does not enable elevation.

## Validation

Synthetic tests cover SMBIOS lengths/capacities/device limits, unknown startup flags and incomplete
task-trigger scans, event string buffer bounds, bounded metadata scanning,
missing-provider values, thermal/counter conversion, UTC update dates, closed tool schemas,
consent-based name projection and cloud truncation, and autostart command validation. The non-Windows implementation
returns `unsupported`, never fixture observations.

On 2026-10-02 all nine native tool families completed locally without elevation or model calls.
Observed expected restrictions included unknown startup approval states, protected logon tasks,
ACPI thermal access requiring additional rights, Store/portable inventory gaps, and Delivery
Optimization size requiring additional rights. Standard PDH thermal/frequency/performance and
NVML readings, local update history, service events, and SMBIOS were available. Typical tool
durations were below one second to roughly seven seconds; large cleanup categories were bounded.
Autostart mutation was not executed during validation.

On 2026-10-03 the final release CLI repeated all nine families with anonymized projections in a
non-elevated process. They completed in 70–4053 ms, with partial coverage preserved where expected;
services and applications explicitly reported the 500-observation cloud limit. The same run
exercised process activity, a 30-second recording and persisted-history reopening. No startup
setting or other system setting was changed.

Reproduce a local names-free trace, including coverage (the output stays on the PC):

```powershell
$env:CARGO_TARGET_DIR = 'D:\pcagent-target'
$env:TEMP = 'D:\pcagent-tmp'
$env:TMP = 'D:\pcagent-tmp'
cargo test -p pcagent-inspect --locked
cargo clippy -p pcagent-inspect --all-targets --locked -- -D warnings
cargo run -p pcagent-inspect --example local_inspection --locked -- D:\pcagent-tmp\inspection.json
```

The example accepts an optional second argument (`startup`, `services`, `events`, `system`,
`power`, `apps`, `hardware`, `cleanup`, `query_system`) to inspect a single family.

Native semantics are documented by Microsoft: [event system fields](https://learn.microsoft.com/en-us/windows/win32/api/winevt/ne-winevt-evt_system_property_id),
[Run/RunOnce](https://learn.microsoft.com/en-us/windows/win32/setupapi/run-and-runonce-registry-keys),
[WOW registry views](https://learn.microsoft.com/en-us/windows/win32/winprog64/shared-registry-keys),
[Uninstall registration dates](https://learn.microsoft.com/en-us/windows/win32/msi/uninstall-registry-key),
[local update history](https://learn.microsoft.com/en-us/windows/win32/api/wuapi/nf-wuapi-iupdatesearcher-queryhistory),
and [update installation state](https://learn.microsoft.com/en-us/windows/win32/api/wuapi/nf-wuapi-iupdateinstaller-get_isbusy).
