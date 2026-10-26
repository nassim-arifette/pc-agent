# Native system inspection

`pcagent-inspect` implements the read-only Windows tools registered by
`crates/agent/src/system_tools.rs`. The public entry point is `inspect(Query)`. The tool registry
accepts only bounded integers and enumerated query names; it cannot accept a command, WQL, a
registry path, an event XPath, a remote host, or an arbitrary filesystem root. The capability
registry declares these tools to the model only on Windows, and on a PC where Windows lists no
system battery `get_power_and_thermal` says so instead of promising battery readings
([tool-context.md](tool-context.md#declared-tools-the-capability-registry)).

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
| `get_recent_events` | EvtQuery/EvtRender, at most 30 days, four seconds, 5,000 examined matching records, and 200 returned occurrence timestamps (five per provider/event ID). Fixed providers/IDs cover power loss, crashes/hangs, WHEA, disk/storage, update failures, service failures, and resource exhaustion. Selected occurrences also expose named EventData crash/hang versions, exception/bugcheck codes and parameters, and update errors/KBs. |
| `get_system_state` | GetTickCount64, Windows version and reboot marker registry keys, fast-startup setting, WMI pagefile/hotfix fields; Windows Update Agent local history and installation-busy getter. At most 100 update-history records are examined. Safeguard indicators read the appraiser summary and up to 64 existing release-named target keys. |
| `get_power_and_thermal` | GetSystemPowerStatus, PowerGetActiveScheme, two PDH samples one second apart for CPU frequency/performance limits and thermal zones, ACPI thermal WMI if permitted, existing NVML/GPU telemetry. At most 64 PDH thermal zones. |
| `get_installed_apps` | HKLM 64/32-bit and shared HKCU Uninstall registrations, at most 2,000 enumerated subkeys per view and 500 returned registrations. Display name/publisher, version, registry date, estimated size only. |
| `get_hardware_inventory` | Existing version/CPU/memory/volume inventory plus SMBIOS types 1, 16 and 17 (16 MiB table, 65,536 structures, 128 arrays and 128 devices), fixed WMI CPU/storage properties, at most 32 physical-drive IOCTL queries, and graphics driver registry version/date. Manufacturer/model and RAM manufacturer/part reference are separate local names. Slot counts become partial lower bounds when devices are omitted. |
| `get_drivers` | SetupAPI enumerates present display, network, storage, audio, Bluetooth and system devices, at most 512 examined devices, four seconds and 100 returned drivers. Only INF description/provider and installed version/date are selected. Optional WUA driver-offer search defaults to the offline cache, examines at most 200 offers and returns at most 100. Its asynchronous result wait is seven seconds inside the existing 15-second inspection wait. |
| `get_cleanup_opportunities` | Recycle-bin totals and metadata-only scans for every planned category. Each category gets 500 ms and 5,000 filesystem entries; pending traversal is also bounded. Observed reparse points, their ancestors, remote/removable roots, and file contents are not traversed/read. |
| `query_system` | Fixed names: operating_system, processor, physical_memory, pagefile, last_updates, thermal_zones, storage_devices, system_events, application_events, windows_version. WMI uses selected columns, three seconds of enumeration and at most 100 rows. |

All native inspections through `inspect(Query)` run on one shared, dedicated worker at a time.
The caller waits at most 15 seconds. If a provider is still blocked, subsequent inspections report
that the previous worker is busy; they cannot accumulate additional workers. A timeout does not
claim that the native API was cancelled. The only other worker is battery-only:
`live_battery()` runs the same battery read as `Query::Battery` for the Live PC window, one read
at a time, with a six-second wait, so a live refresh never makes an agent tool or a recording tick
report a busy worker. While its previous read is still blocked, a new call returns at once with
partial `collector` coverage and no values ([battery.md](battery.md)). A driver-offer timeout preserves the installed readings and returns partial coverage;
WUA abort/cleanup remains on the same worker, which stays busy until cleanup finishes. COM
initialization outlives all collected interfaces **and error objects**, including
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
paths stay local. Update history is a local database query. Driver offers are searched separately
only by `get_drivers`: `windows_update: none` skips that source, `offline` explicitly sets
`Online = false`, and `online` requires an explicit request and may contact the configured update
service (Microsoft or an enterprise service). No driver download, installation, device mutation or
automatic WUA service upgrade is performed. A fixed nonpersonal client application ID is used.

## Local search keys

SMBIOS system-memory arrays are associated with their type 17 devices using structure handles
that remain local; video and cache memory arrays are excluded when the array metadata exists.
Type 1 selects only manufacturer and product model. Type 16 provides the firmware-advertised
maximum capacity (including the extended byte field) and memory-device count. Type 17 provides
type, form factor, capacity, maximum transfer rate, configured transfer rate, manufacturer and
part reference. The SMBIOS speed fields are **MT/s**, not a measured clock frequency; the native
metric is now `memory_speed_megatransfers_per_second`. Unknown and extended encodings are checked,
and missing configured speed never falls back to the device's maximum speed.

Firmware capacity is an uncertain purchasing limit. Memory devices include soldered memory as
well as sockets, and virtual machines can expose synthetic layouts. Chip/row-of-chips/die/TSOP
forms produce only a partial `likely_soldered` mounting hint; `memory_soldered` remains unknown.
DIMM/SO-DIMM/CAMM and other module forms are identified as modules, without promising that the PC
can accept a particular upgrade. Mixed soldered and socketed configurations are possible.

Event details select these named fields, without rendering a message or reading unrelated data:

| Provider / ID | Selected data |
| --- | --- |
| Application Error / 1000 | AppName, AppVersion, ModuleName, ModuleVersion, ExceptionCode; application/module are reduced to file names and shared only after C5 consent. |
| Application Hang / 1002 | AppName and AppVersion, including the native ANSI version field. |
| WER-SystemErrorReporting / 1001 | Numeric bugcheck code and four parameters parsed strictly from `param1`; dump `param2` and report `param3` are never selected. |
| WindowsUpdateClient / 20, 25, 31 | errorCode and KB numbers extracted locally from updateTitle; the title and update/service GUIDs are discarded. ID 25 and older ID 31 templates can omit the title, so the KB stays unknown. |

Missing or unknown fields produce null observations. Counts still describe retained matching
logs; a capped count is a lower bound, and an unreadable empty scan does not assert zero failures.
The field lists were checked against the registered Windows provider templates on the development
PC, independently of event contents.

Safeguard reads use only GStatus, GatedBlockId and GatedBlockReason. GStatus 0 reports a hold and
2 reports none; another/absent value is unknown. Existing target names ending in a release such
as 24H2/25H2 are discovered instead of guessing a prefix for the next release. Only eight-digit
hold IDs and finite reason categories are exposed. Appraiser values can be stale, so they remain
partial evidence and cannot establish current upgrade eligibility or authorize bypassing a hold.

Installed and offered driver versions are compared as four numeric components. WUA has no driver
version property, so an offer needs an exact numeric suffix in its title; an unfamiliar suffix is
unknown. Offers are associated only with a unique exact class/provider/model match. Missing,
different or ambiguous descriptions stay unassociated; instance IDs and hardware IDs are never
used. `older`, `same`, `newer` and `unknown` describe the comparison, without recommending an
installation. Offline cache freshness is unknown. NVIDIA's Windows version is converted locally
(for example 32.0.15.6094 to 560.94), and compared with `nvmlSystemGetDriverVersion` from the
System32 library; a missing result or mismatch remains visible.

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

Synthetic tests cover SMBIOS types 1/16/17, selected-string privacy, old/extended capacities and
transfer rates, system/video arrays and device limits, structured crash/hang/bugcheck/update fields,
hostile fields, missing KBs, safeguard states and safe IDs, numeric driver comparisons, WUA title
formats and ambiguous model matching, unknown startup flags and incomplete
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
`power`, `apps`, `hardware`, `drivers`, `cleanup`, `query_system`) to inspect a single family.
The driver trace uses offline search only. The new lot's compilation/native results must be
recorded separately from the earlier nine-family validation; the earlier dates do not validate it.

Native semantics are documented by Microsoft: [event system fields](https://learn.microsoft.com/en-us/windows/win32/api/winevt/ne-winevt-evt_system_property_id),
[Run/RunOnce](https://learn.microsoft.com/en-us/windows/win32/setupapi/run-and-runonce-registry-keys),
[WOW registry views](https://learn.microsoft.com/en-us/windows/win32/winprog64/shared-registry-keys),
[Uninstall registration dates](https://learn.microsoft.com/en-us/windows/win32/msi/uninstall-registry-key),
[local update history](https://learn.microsoft.com/en-us/windows/win32/api/wuapi/nf-wuapi-iupdatesearcher-queryhistory),
and [update installation state](https://learn.microsoft.com/en-us/windows/win32/api/wuapi/nf-wuapi-iupdateinstaller-get_isbusy).
The local search keys additionally follow [DMTF SMBIOS 3.8](https://www.dmtf.org/sites/default/files/standards/documents/DSP0134_3.8.0.pdf),
[SetupAPI driver provider properties](https://learn.microsoft.com/en-us/windows-hardware/drivers/install/devpkey-device-driverprovider),
[Microsoft safeguard indicators](https://learn.microsoft.com/en-us/windows/deployment/update/safeguard-holds),
[WUA Online](https://learn.microsoft.com/en-us/windows/win32/api/wuapi/nf-wuapi-iupdatesearcher-get_online),
[asynchronous WUA operations](https://learn.microsoft.com/en-us/windows/win32/wua_sdk/guidelines-for-asynchronous-wua-operations),
[driver-offer properties](https://learn.microsoft.com/en-us/windows/win32/wua_sdk/iwindowsdriverupdate-properties),
and [NVML system driver version](https://docs.nvidia.com/deploy/nvml-api/latest/api/group__nvmlSystemQueries.html).
