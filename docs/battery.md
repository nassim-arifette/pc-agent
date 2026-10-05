# Battery inspection and recording

`pcagent_inspect::inspect(Query::Battery)` reads battery information independently of
thermal providers, WMI and GPU sensors. `get_power_and_thermal` also includes these
observations. Reads require no administrator session and never set battery state.
Each battery has separate observations; multiple batteries are never implicitly summed.

## Native sources and bounds

SetupAPI enumerates present `GUID_DEVICE_BATTERY` interfaces. The adapter opens them with
`GENERIC_READ`, then uses only `IOCTL_BATTERY_QUERY_TAG`, `IOCTL_BATTERY_QUERY_INFORMATION`
and `IOCTL_BATTERY_QUERY_STATUS`. All three control codes require read access only. The
tag wait and status timeout are zero. Information levels are exclusively
`BatteryInformation` and `BatteryEstimatedTime` with `AtRate = 0`. Device names,
manufacturer names, serial numbers and `BatteryUniqueID` are never requested.
[Microsoft's enumeration example](https://learn.microsoft.com/en-us/windows/win32/power/enumerating-battery-devices)
documents the SetupAPI sequence;
[query information](https://learn.microsoft.com/en-us/windows/win32/power/ioctl-battery-query-information)
documents optional unsupported results.

There are at most eight device interfaces, an 8 KiB interface buffer and a four-second
scan budget. Returned numeric records must have exactly the expected length (36-byte
information, 16-byte status or four-byte scalar). A tag is rechecked after all reads;
changed or unreadable tags omit the whole device snapshot. The existing single native
worker returns to its caller after 15 seconds. Synchronous driver I/O can exceed the scan
budget; the caller timeout does not cancel it or create another worker.
[Immediate status reads](https://learn.microsoft.com/en-us/windows/win32/power/battery-wait-status-str)
and [tag lifetime](https://learn.microsoft.com/en-us/windows/win32/power/ioctl-battery-query-tag)
define these semantics.

The process keeps at most 64 local interface/tag associations. References have the form
`battery-<random UUID>` and remain stable across samples of that interface and tag.
A tag change, observed absence or cache reset creates a new reference, breaking continuity.
Process restart starts a new reference session. Paths and tags remain local, unlogged and
unserialized; no physical identity is inferred across restarts. All display-name maps stay
empty. Inspection failures use fixed coverage text without device identifiers.

## Observation contract

All measurements use C4 `Observation`, including their own native query window and finish
timestamp. Information fields are dated by the information read; dynamic status fields by
the status read; the driver's runtime estimate by its estimate read. Derived fields retain
the complete device-read window. Measurement origin is live. Null values have unsupported
coverage; contradicting raw measurements are partial and carry explicit consistency flags.
Missing drivers, unsupported optional fields, relative units and unknown sentinels never
produce invented zero measurements.

| Metric | Unit | Meaning |
| --- | --- | --- |
| `battery_kind` | `category` | `system`, `short_term` or `non_system` |
| `battery_capacity_units` | `category` | `milliwatt_hours` or `relative` |
| `battery_rechargeable` | `boolean` | Reported rechargeable technology, otherwise null |
| `battery_design_capacity_mwh` | `milliwatt_hours` | Reported capacity when new |
| `battery_full_charge_capacity_mwh` | `milliwatt_hours` | Current reported fully charged capacity |
| `battery_critical_bias_mwh` | `milliwatt_hours` | Driver-reported capacity reserve bias |
| `battery_cycle_count` | `count` | Optional positive cycle counter |
| `battery_capacities_consistent` | `boolean` | Known comparable capacities satisfy bounds; null when not comparable |
| `battery_capacity_retention_percent` | `percent` | Full/design × 100 when admissible |
| `battery_estimated_wear_percent` | `percent` | 100 − retention when admissible |
| `battery_remaining_capacity_mwh` | `milliwatt_hours` | Reported remaining capacity; zero is valid |
| `battery_charge_percent` | `percent` | Remaining/full × 100 when comparable |
| `battery_power_rate_mw` | `milliwatts` | Native signed rate: positive charging, negative discharging |
| `battery_discharge_power_mw` | `milliwatts` | Positive magnitude only for consistent discharge and negative rate |
| `battery_charge_power_mw` | `milliwatts` | Positive magnitude only for consistent charge and positive rate |
| `battery_voltage_mv` | `millivolts` | Optional terminal voltage |
| `battery_charge_state` | `category` | `charging`, `discharging`, `idle` or `unknown` |
| `battery_on_ac` | `boolean` | Native AC flag |
| `battery_critical` | `boolean` | Native hardware critical flag |
| `battery_state_consistent` | `boolean` | Known state bits agree with one another and the available rate sign |
| `battery_estimated_runtime_windows_seconds` | `seconds` | Optional battery driver's estimate at present drain |
| `battery_estimated_runtime_at_current_rate_seconds` | `seconds` | Remaining/discharge power × 3600, assuming constant drain |

Sources are fixed strings, not user-controlled text:

- `DeviceIoControl/IOCTL_BATTERY_QUERY_INFORMATION`
- `DeviceIoControl/IOCTL_BATTERY_QUERY_STATUS`
- `DeviceIoControl/BatteryEstimatedTime`
- `derived/IOCTL_BATTERY_QUERY_INFORMATION+IOCTL_BATTERY_QUERY_STATUS`

Relative capacities have undefined physical units; their energy and power fields are null.
Their comparable remaining/full ratio can still provide charge percentage. Short-term
fail-safe devices, including UPS providers, remain separate from normal system batteries.
No conversion from mAh to mWh is attempted. Design/full zero and unknown capacity sentinels
are unavailable; zero remaining capacity is meaningful. A zero cycle counter means the
counter is unsupported. Critical bias is already applied by the driver and is not subtracted
again. These interpretations follow
[BATTERY_INFORMATION](https://learn.microsoft.com/en-us/windows/win32/power/battery-information-str).

Wear is an estimate of reported capacity loss, not a health score or a diagnosis. It requires
absolute units, rechargeable technology and known valid design/full capacities. A full
capacity above design is preserved as a raw reading while retention and wear stay null;
negative wear is never clamped. Unknown remaining capacity does not hide valid static retention
and wear estimates. A known remaining capacity above full blocks those estimates, charge
percentage and calculated runtime. Calibration and manufacturing variation can affect the
reported capacities.

Unknown state bits, simultaneous charge/discharge, AC with discharge, or a rate sign that
contradicts flags set `battery_state_consistent = false`. The signed raw rate is retained with
partial status. Derived charge/discharge magnitudes and runtime stay null. A missing rate
does not imply zero drain. Voltage is in mV; power uses mW only for absolute reporting.
[BATTERY_STATUS](https://learn.microsoft.com/en-us/windows/win32/power/battery-status-str)
defines the native flags, sentinels and signed-rate convention.

The Windows runtime metric is a driver estimate rather than a guaranteed countdown. It is
accepted only during consistent discharge. The separately named calculated runtime additionally
requires comparable absolute remaining capacity and strictly positive discharge magnitude;
neither estimate is extrapolated while charging. A zero or missing native rate prevents the
calculated estimate; an available driver estimate is retained separately. Driver estimates
can vary with workload, and remaining capacity can be coarse.
[BatteryEstimatedTime](https://learn.microsoft.com/en-us/windows/win32/power/battery-query-information-str)
documents the current-drain request and optional unknown result.

## Recording and attribution limits

The sampler uses this lightweight query only during a requested 30–120 second recording, once
per sampling tick. Normal five-second background sampling adds no battery query. SQLite schema 5
stores battery ticks under their recording, with foreign-key cascade on deletion/retention and
the existing seven-day limit. Each point preserves its native timestamp, window, unit, source,
status, signed reading and opaque battery reference. Capture order remains explicit even if the
wall clock repeats or moves backwards. Recording offsets come from `Instant`; UTC is used only
for bounded, consistency-checked alignment within a native read, not as the recording's elapsed
clock. Older rows without an offset have no
derived drain estimate. The recording's UTC epoch is paired with its start `Instant` before
database work. Each tick must agree with that epoch plus its actual monotonic offset within
50 ms; both the complete inspection window and each native query window must also agree with
their monotonic durations. The inspection window is persisted and rechecked when reading.
Clock discontinuities retain the native values and actual tick offsets with
`clock_consistent = false` and explicit `clock_inconsistent` gaps, without derived power or
runtime. Legacy rows without the inspection window cannot certify this alignment.

`record_activity` adds a `battery` result alongside the existing system histories and process
peaks. `Sampler::battery_recording(id)` and the GUI Rust command
`battery_recording({recordingId})` return the same `Option<BatteryRecording>` from storage without
collecting again. Unknown/deleted IDs return `null`; an existing recording without battery data
returns explicit unavailable coverage. The output contains at most 128 points across 16 battery
references, with collection/response omissions and gaps reported. No local database, interface
path, tag, machine ID or process name is included in this battery DTO.

Each continuous, coherent, absolute-unit system-battery discharge segment can expose
`battery_measured_capacity_decrease_mwh`, signed `battery_average_power_rate_mw` and
`battery_estimated_runtime_from_recording_seconds`. The estimate uses reported remaining
capacity divided by observed capacity loss per elapsed second; each derived observation includes
its actual capacity operands and window. The driver already applies `CriticalBias`; it is never
subtracted again. Charging, gaps, replacement batteries, clock inconsistencies, missing offsets,
relative units and capacity increases break continuity. Zero measured loss cannot establish an
autonomy estimate. Sampling more often does not improve a coarse hardware fuel gauge's resolution.

Battery power describes whole-battery flow. This collector cannot attribute watts or joules
to individual applications. Dated CPU, GPU and I/O peaks during a recording can show
correlations with drain among the 20 processes selected at each tick; they are not an
exhaustive ranking of applications. GPU occupancy reuses the sampler's existing PDH window
and requires an unchanged process identity observed before its monotonic baseline and
verified still running after its finish, including a zero native exit time. First windows,
exited processes, absent GPU engines and denied identity checks remain unknown. Rejected
or contradictory engine instances make that process's GPU unknown; invalid PDH entries
without a usable identity make all per-process GPU values in that window unknown.
CPU percentage or GPU occupancy alone is not an energy measurement. I/O
counters include cached, network and device traffic, not just physical disk activity.
Application energy attribution
would require an additional trustworthy energy provider and explicit coverage of its limits.

The fixed `open_settings_page` target `batterysaver-usagedetails` offers Windows' battery usage
page after local approval. The backend does not scrape that page or read SRUM. A future frontend
can display health fields from `get_power_and_thermal`, fetch `battery_recording` while/after a
problem recording, and align its dates with existing CPU/GPU/I/O evidence. No frontend files
were changed for this backend lot. Wear is capacity-based estimation, and runtime assumes that
the recorded workload remains comparable.

## Synthetic validation

Portable parser tests cover exact binary lengths, missing sources, unknown sentinels,
zero remaining capacity, optional cycle counts, charge/idle/discharge, sign and flag conflicts,
full above design, remaining above full, relative/UPS units and independent batteries.
Windows-only tests verify SDK layout sizes, read-access IOCTL masks, zero waits, reference
continuity and bounded cache resets. Projection fixtures verify native C4 dates, units,
signed values, nulls and the absence of local paths/tags/names in serialized observations.
These tests do not open any real battery device.

## Native validation — 2026-10-04

Two Windows release probes ran without elevation, with anonymized local inspection outputs
and separate recording databases. Both reported 47,310 mWh design
capacity, 39,900 mWh full-charge and remaining capacity, 100% charge and coherent idle/on-AC
state. Capacity retention was 84.3373% and estimated wear 15.6627%. Cycle count, signed power
rate and both runtime estimates were explicitly unsupported. These are dated driver readings,
not a guarantee of future capacity or runtime.

The first requested 30-second recording completed with 28 persisted battery points, no rejected
observations and one reported sampling gap. The 140 unavailable battery observations remained
null; no discharge evolution or runtime was derived on AC. CPU and read/write activity peaks
were available. No usable retained per-process GPU measurement was available in this run; GPU
activity stayed unknown. The recording happened during the release build, so its workload is
not an idle baseline or an application-energy benchmark. Passive supplier absence, recording
coverage and unsupported values are explicit; discharge/rate and per-application energy
acceptance remain separate.

The second probe used the final release binary after compilation stopped, from 05:18:50 to
05:19:36 UTC. Its 30-second recording completed with 29 persisted battery points, no rejected
observations, no response omissions and two `not_recorded` gaps. All 29 stored points had
consistent clocks. The 145 unavailable battery observations remained null; CPU and read/write
peaks were available, while retained per-process GPU activity remained unknown. The battery
was still idle/on AC, so this run also cannot validate discharge evolution or runtime.
