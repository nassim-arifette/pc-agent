# Optional hardware sensor readers

`get_power_and_thermal` can append sensors from **already-running local publishers** to its
Windows power policy, performance counters, ACPI thermal zones and NVML readings. Optional
supplier absence is reported separately and does not suppress another supplier's measurements.
PC Agent does not start or install a supplier, enable sharing, elevate, load a sensor DLL or
driver, bypass a supplier limit, or contact its web server.

The adapter is `crates/inspect/src/windows/sensors.rs`; the portable parsers and synthetic tests
are `crates/inspect/src/parse/sensors.rs`. No new public query or shared metric enum is needed:
the existing C4 `Inspection` contains string metrics, typed values, units, observation references,
timestamps, origin and per-source coverage. Linux keeps the existing unsupported native stub;
parser tests run on both platforms.

## Integration hooks

The readers are integrated through these existing module boundaries:

1. `parse.rs`: declare `pub(crate) mod sensors;`.
2. `windows/mod.rs`: declare `mod sensors;`.
3. The existing power collector: call `super::sensors::collect(out)` after the native sources.
4. `windows/wmi.rs`: make its bounded `query` helper `pub(super)` and copy finite `VT_R4`/`VT_R8`
   scalars to JSON, alongside its existing integer/string conversions. No by-reference or array
   VARIANT is accepted.

The root already enables the `windows-sys` features `Win32_Foundation`, `Win32_System_Memory`
and `Win32_System_Threading`. The adapter needs no new dependency, Cargo feature, shell command,
query argument or GUI command.

## LibreHardwareMonitor WMI

The allowlisted query is `SELECT SensorType,Parent,Value FROM Sensor` in
`root\LibreHardwareMonitor`, with at most 256 rows and the existing bounded WMI enumeration.
Neither `Name`, `Identifier`, `Min` nor `Max` is requested. The local-only `Parent` prefix can
classify a component as CPU, GPU, motherboard, memory, storage, battery, power supply or unknown;
the parent string is never retained in the parsed output. Components and individual readings
receive fresh opaque references, with no PID, machine identifier or device serial number.

The official [v0.9.3 Sensor provider](https://github.com/LibreHardwareMonitor/LibreHardwareMonitor/blob/v0.9.3/LibreHardwareMonitor/WMI/Sensor.cs)
assigns zero when the library's optional value is absent. Therefore **every WMI zero is unknown**,
including a possibly stopped fan or idle load. A nonzero finite reading in a recognized sensor
type is retained, but has `partial` status: this WMI schema has no measurement timestamp, so its
age is unknown. The observation's timestamp is the PC Agent read time, not an invented supplier
sample time. The parser rejects strings masquerading as numeric readings.

The provider existed in the official v0.9.3 tree. It is absent from the official v0.9.4 and
master trees checked during implementation; the current repository retains only its
[historical WMI example](https://github.com/LibreHardwareMonitor/LibreHardwareMonitor/blob/master/LibreHardwareMonitor.Windows.Forms/TestScripts/basicwmi.py).
An installed application does not imply that WMI is available. Missing namespace/class or no
published rows yields `unsupported`; denied and failed reads retain their own coverage status.
The [v0.9.3 WmiProvider](https://github.com/LibreHardwareMonitor/LibreHardwareMonitor/blob/v0.9.3/LibreHardwareMonitor/WMI/WmiProvider.cs)
is the versioned authority for the legacy interface; there is no HTTP fallback in this adapter.

## HWiNFO shared memory

The native reader opens the fixed local `Global\HWiNFO_SENS_SM2` mapping with `FILE_MAP_READ`.
It opens the existing `Global\HWiNFO_SM2_MUTEX` with synchronization access and waits at most
50 ms. No object is created. A missing, inaccessible, busy or abandoned mutex yields no sensor
values. Handles, view and acquired ownership are released on every exit path.

[MapViewOfFile](https://learn.microsoft.com/en-us/windows/win32/api/memoryapi/nf-memoryapi-mapviewoffile)
creates a read-only view of only 44/48 header bytes first; `VirtualQuery` verifies its committed
mapped readable region before the first header byte is copied. The parser validates signature,
known layout, section strides, counts, checked offsets, bounds and non-overlap before mapping
the declared extent (at most 4 MiB) and allocating the snapshot. The complete bounded view also
validates the extent against the actual mapping before any table byte is copied. Under the mutex,
the header must agree before the copy, inside the copy and after the copy. A changed header
discards the complete snapshot. Parsing occurs in the owned copy after releasing the mutex.

The author's public `hwisenssm2.h` describes packed versions 1 and 2 and header revisions 0 and
1. This adapter accepts those layouts with their exact documented strides; future layouts are
`unsupported`. Version 2 adds UTF-8 labels and units. Only units are decoded; UTF-8 and ANSI user
labels are ignored. Original English device/reading label bytes may match a finite local
category/role/boolean allowlist but are never converted into an outgoing name.

The original header is [redistributed by myrsm](https://github.com/qdel/myrsm/blob/main/hwisenssm2.h).
The HWiNFO author explicitly approved that redistribution in the
[official forum on 16 October 2025](https://www.hwinfo.com/forum/threads/introducing-remote-sensor-monitor-a-restful-web-server.1025/page-4).
The publisher's [shared memory documentation section](https://www.hwinfo.com/forum/forums/shared-memory-interface.13/)
requires forum access and could not be read directly in this session; the public original header
was read through its authorized mirror. No proprietary SDK/library code is embedded or executed.

`poll_time` is a signed `__time64_t`, whose documented representation is
[UTC seconds since 1 January 1970](https://learn.microsoft.com/en-us/cpp/c-runtime-library/reference/time-time32-time64?view=msvc-170).
Checked conversion preserves the actual poll timestamp on each numeric/boolean sensor
observation. A missing/zero/negative/overflowing date is unknown; a date in the future is flagged
as `future`. A poll more than 15 seconds old is `stale`. These cases return unknown sensor values,
with the known timestamp/age and freshness category retained in provider metadata. The 15-second
cutoff is PC Agent's freshness bound, not a promise about every supplier's polling interval.
The header's optional polling period is not used to invent a freshness or extrema window.

The `DEAD` signature means inactive sharing. The reader respects absence and expiration, including
the publisher's [documented 12-hour non-Pro sharing limit](https://www.hwinfo.com/forum/threads/settings-tab-not-opening.11030/).
It never enables or renews sharing and never reads remote mapping names.

The existing mutex is opened with `SYNCHRONIZE` only. Microsoft's
[OpenMutexW documentation](https://learn.microsoft.com/en-us/windows/win32/api/synchapi/nf-synchapi-openmutexw)
specifies that this is the only required access right; its
[synchronization access-rights table](https://learn.microsoft.com/en-us/windows/win32/sync/synchronization-object-security-and-access-rights)
marks `MUTEX_MODIFY_STATE` as reserved for future use. A thread that acquired ownership can
[ReleaseMutex](https://learn.microsoft.com/en-us/windows/win32/api/synchapi/nf-synchapi-releasemutex)
through this handle. The adapter does not request additional rights.

## Bounds and projections

| Bound | Behavior |
| --- | --- |
| 4 MiB copied shared-memory extent | Reject larger declared extents before allocation/copy |
| 256 component records | Reject larger tables |
| 4096 reading records examined | Reject larger tables |
| 256 sensor readings returned per provider | Return bounded readings and report truncation |
| 50 ms mutex wait | Report busy; never read without ownership |
| 15 s HWiNFO maximum sample age | Stale readings are unknown |
| 256 LHM WMI rows | Apply the existing WMI time bound and report a partial inventory |

| Selected reading | Metric | Canonical unit |
| --- | --- | --- |
| Temperature | `sensor_temperature_celsius` | `degrees_celsius` |
| Fan speed | `sensor_fan_speed_rpm` | `revolutions_per_minute` |
| Power | `sensor_power_watts` | `watts` |
| Voltage | `sensor_voltage_volts` | `volts` |
| Current | `sensor_current_amperes` | `amperes` |
| Clock | `sensor_clock_megahertz` | `megahertz` |
| Load/usage | `sensor_load_percent` | `percent` |
| Explicit named HWiNFO thermal-throttling boolean | `sensor_thermal_throttling` | `boolean` |

HWiNFO units must match a finite list. Fahrenheit is converted to Celsius; milli/kilo unit
variants are converted explicitly. Nonfinite values, unsupported/custom units, unterminated unit
fields and broad encoding sanity-bound violations become unknown. These bounds are not hardware
health thresholds. A HWiNFO fan speed of zero remains a measured zero, unlike ambiguous LHM WMI.
Only an exact original English thermal-throttling label, `Yes/No` unit and value 0/1 can yield that
boolean. Other flags are not inferred from clocks, load, temperatures or power limits.

Provider metadata includes freshness, known measurement time, age, truncation and counts of
unsupported readings/unknown values. Component category, component/provider references and
optional fixed reading role accompany individual values. A generic temperature is not relabeled
as CPU package temperature; a Tctl/Tdie reading retains `die_or_control`, and an unknown hardware
model remains an unknown component category. Min/max/average readings are omitted because their
accumulation window is not known.

All modes, including Complete sharing, receive **only opaque references and finite categories**
for these suppliers. No supplier name dictionary is populated, and neither device names, serials,
raw identifiers, sensor IDs, custom labels nor custom units are projected. Source and coverage
text consists only of fixed adapter-owned strings.

## Validation

Synthetic parser tests cover legacy WMI null-to-zero ambiguity, rejected string numbers, negative
voltage, missing fields, private parent identifiers, strict sample limits, both packed layouts and
both revisions, ANSI/UTF-8 units, exact timestamp freshness boundaries, future/invalid/overflowing
dates, signature/version rejection, truncated headers/tables, bad strides, overlap, count/extent
bounds, invalid component links, header changes, nonfinite values, empty tables and unterminated
unit fields. Native projection tests cover typed unknowns, actual poll timestamp, grouped opaque
references, fixed categories, no implicit names and partial undated WMI values.
A Windows fixture creates a fresh random local mutex, acquires it through a separate
`SYNCHRONIZE`-only handle, releases it with the adapter's ownership guard, and requires another
thread to acquire it within the same bound. It never opens the supplier's mutex or mapping.

The isolated files were formatted and checked for whitespace errors. Cargo verification and a
real `get_power_and_thermal` probe are coordinated by the integrating task; supplier absence on
that PC validates absence handling, not actual temperature/fan/power reading on a supplied PC.
No LHM or HWiNFO application has been installed or started for this implementation.
