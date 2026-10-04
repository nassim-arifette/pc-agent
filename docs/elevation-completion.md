# Privileged system inspection additions

The GUI/model host stays unelevated. A local helper approval authorizes one fixed,
bounded read; it grants no model execution authority and introduces no network or
general command entry point. Closed helper DTOs retain local identities only where
needed, and C4 projections use opaque references.

## Shadow storage

The helper reads `UsedSpace`, `AllocatedSpace` and `MaxSpace` from fixed local
`root\cimv2:Win32_ShadowStorage` WQL. It never queries a volume GUID, device identifier,
path or restore-point description. UINT64 maximum means unlimited and is represented
separately; missing values remain absent. A derived consistency flag distinguishes
missing and contradictory provider values without replacing a measurement.

These are **shadow-storage associations**, not a restore-point inventory, per-restore-
point sizes or reclaimable bytes. Other VSS snapshots can contribute. The old restore-
point category must retain unknown coverage until an attributable source is added.

## Disk reliability

The fixed `root\Microsoft\Windows\Storage:MSFT_StorageReliabilityCounter` query selects
documented numeric fields only: temperature and operating limit, wear, power-on hours,
read/write error totals and corrected/uncorrected counts, cycle counts and maximum
read/write/flush latency. Wear is a provider UInt8 percentage; 100 indicates its
estimated wear limit, and values above 100 are preserved. Counter contradictions and
missing properties degrade coverage explicitly.

These device-reported counters are not raw SMART attributes, a physical surface test,
a “health score” or proof of reliability. Device IDs, serials, model names and dates
are never requested. Empty or absent providers remain unsupported; fields missing from
one row remain unknown rather than zero.

## Protected process image paths

The task accepts 1–16 local `{pid, creation_time_filetime}` targets. PID zero, missing
creation identity and duplicate PIDs are refused. `OpenProcess` requests only
`PROCESS_QUERY_LIMITED_INFORMATION`; no debug privilege, process memory or command
line is requested. The same retained handle must still be alive and match creation
time before and after `QueryFullProcessImageNameW`. PID reuse, exit, denied access and
overlong/malformed paths produce explicit unknown samples.

The full image path is local helper data, redacted from `Debug`. Only the executable
basename may enter the C5 names map. Elevation does not make every protected process
readable; a refusal is retained as a refusal.

## Delivery Optimization location

The helper reads only `HKLM\SOFTWARE\Policies\Microsoft\Windows\DeliveryOptimization` /
`DOModifyCacheDrive` in the 64-bit registry view. A validated absolute folder is a cache
**candidate**; all existing ancestors are pinned without delete sharing and reparse
points/network/removable roots are excluded. The resolution must remain alive while
the directory scan runs.

Only `%SYSTEMDRIVE%`, derived from `GetWindowsDirectoryW`, can be expanded. Other
environment values, UNC/device paths, ADS, reserved names and traversal are rejected.
A drive-only policy does not document the exact cache folder; its location remains
unknown rather than guessed. The documented default also remains a candidate when no
policy is configured. Policy/default inspection is not confirmation of DoSvc's
effective state; both preserve partial coverage.

## Bounds and validation

WMI returns at most 32 rows with 100 ms enumeration waits. The common helper watchdog
also bounds blocked COM setup/provider calls. Cancellation and request time budget are
checked between reads. Only copied scalars escape the initialized COM apartment. Raw
provider errors, identities and property contents are not logged.

Portable/native synthetic fixtures cover closed DTOs, provider integer parsing,
unlimited/missing/contradictory VSS values, process identity bounds/privacy and safe
policy paths. No actual VSS, disk, protected-process or relocation acceptance is
claimed by those fixtures.

Primary sources:

- [Microsoft: Win32_ShadowStorage](https://learn.microsoft.com/en-us/previous-versions/windows/desktop/legacy/aa394433(v=vs.85)).
- [Microsoft: MSFT_StorageReliabilityCounter](https://learn.microsoft.com/en-us/windows-hardware/drivers/storage/msft-storagereliabilitycounter).
- [Microsoft: QueryFullProcessImageNameW](https://learn.microsoft.com/en-us/windows/win32/api/winbase/nf-winbase-queryfullprocessimagenamew).
- [Microsoft: Delivery Optimization reference](https://learn.microsoft.com/en-us/windows/deployment/do/delivery-optimization-reference).

Historical E3 estimates have a separate provider and acceptance record in
[battery-energy.md](battery-energy.md).
