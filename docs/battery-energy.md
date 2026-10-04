# Historical per-application energy estimates

The existing battery health/recording collectors describe battery capacity, charge and
discharge. The privileged E3 collector adds a different source: Windows' historical
per-application **estimates**. CPU activity, GPU activity and I/O traffic remain activity
measurements; they are not converted into application watts.

## Fixed provider and approval

Only a locally approved `pcagent-elevated` task can invoke the inbox `powercfg.exe` from
`GetSystemDirectoryW`, with the fixed `/srumutil /output <private energy.csv> /csv` options.
It never invokes a shell, searches `PATH`, changes a power plan, stops a service, resets
SRUM or reads application file contents. The export contains sensitive raw identities
and therefore stays inside the same private job directory as the existing helper
protocol. It is never logged or sent to the model.

The executable and its ancestors remain pinned against replacement. The export is
created privately before launch and pinned without delete sharing; an in-place writer
may write it. The child is created suspended, assigned to a private unnamed job with
`KILL_ON_JOB_CLOSE` and a one-process limit, then resumed. It gets a minimal explicit
environment and no inherited handles or stdout/stderr. Cancellation, the helper
watchdog, the request deadline and an 8 MiB size limit bound the export. Raw CSV is read
only after the owned process signals exit successfully.

## Interpretation and confidentiality

The parser accepts only a reviewed set of named columns and UTF-8 (optional BOM) or
UTF-16LE with BOM. It rejects malformed CSV, unknown/duplicate columns, unrecognized
timestamp forms, conflicting duplicate buckets and numeric overflow. It accepts the
documented `2020-09-08:20:13:00.0000` civil timestamp form; ISO-like space/`T` separators
also preserve civil fields without assigning a time zone. `Container` is boolean,
`InteractivityState` is a bounded provider state word, and `MeasuredBitmap` accepts the
documented `b00000000` binary form.

Only rows for the helper's actual job-owner SID, marked `OnBattery` and `Committed`,
contribute. `TotalEnergyConsumption` is the sole summed energy field in milli-joules.
Component columns are never added again. Filtering prevents another account's history
and AC-only rows from becoming battery evidence. Raw `AppId` and `UserId` remain local;
application references are opaque and only valid executable basenames may enter the
existing C5 names map after the user's choice.

Every successful estimate keeps `Partial` coverage and explicit limitations:

- It describes accepted historical rows, not current power or measured battery drain.
- E3 may omit components and applications; totals do not describe all machine energy.
- Provider civil timestamps have unknown time zone and cannot become a UTC window.
- Reported row durations may overlap and may be zero. Their sum is not elapsed wall
  time and must not be a denominator for watts or autonomy.
- At most 100,000 rows and 1,024 application groups are processed. The largest 32
  groups are returned. The aggregate total always refers to rows actually accepted;
  any collection/ranking limit remains explicit.
- No qualifying rows means unknown energy (`null`), never a fabricated zero. An
  accepted zero-valued provider row remains a known zero estimate.

The outer helper `Window` describes when the export was collected. First/last civil
stamps describe the retained historical data and remain distinct. The parser does not
assume the export always covers seven complete days.

## Evidence and remaining native acceptance

Synthetic fixtures cover the complete field spelling/state format visible in
Microsoft's E3 screenshot, account/AC/commit filtering, units, duplicate normalization,
quoting, encoding, overflow, date validation, bounded ranking, closed DTOs and privacy.
A controlled file fixture checks that the private pinned output permits in-place
writes and denies replacement. These tests do not run `powercfg`.

A successful native export on a battery-capable Windows 11 machine, cancellation and
helper termination while the writer runs, denied elevation, current build/schema
compatibility and the utility's exact file-sharing behavior still need explicit native
acceptance. `NativeAccuracyUnvalidated` is retained until that evidence exists. A
provider error or an unknown schema yields no numeric energy claim.

Primary sources:

- [Microsoft: E3 application power measurement](https://devblogs.microsoft.com/sustainable-software/measuring-your-application-power-and-carbon-impact-part-1/).
  The enlarged “Benchmark data” image provides the named CSV fields and state formats.
- [Microsoft: fixed `/srumutil` export options](https://learn.microsoft.com/en-us/windows-hardware/design/device-experiences/powercfg-command-line-options#srumutil).
