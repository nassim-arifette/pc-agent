# History storage and chart performance

The October 10 performance work reduces repeated reads and per-sample storage without changing
the public `metric_history` command, its evidence fields, or the SQLite table schema. It is a
first implementation for issues #58 and #59. The proposed `history_chart` command, dedicated
system table, period-wide event counts and collection disk budget still require the separate
plan/contract work described in those issues.

## Storage and compatibility

New sample payloads use a private versioned JSON envelope. Labels are interned within each row;
system and process measurements use tuples with shared dictionaries. Values, unknowns, sources,
statuses, observation windows, sample counts and extrema are preserved. Weighted numerators are
decimal strings so that values larger than `u64::MAX` never pass through floating point.

The reader accepts both legacy JSON and compact rows, including a database containing both.
Retention, process-peak queries and system history use the same codec. System-chart reads validate
process tuples but do not allocate their full `SampleValue` objects. Legacy rows still need a full
decode. An older executable cannot read newly written compact rows; reverting only the executable
after collecting new data is therefore not supported.

There is no bulk rewrite on opening a database. Existing raw rows become compact when normal
retention aggregates them after 24 hours; existing minute rows expire with the seven-day retention.
SQLite reuses freed pages, so the existing database file does not immediately shrink. This work
does not run `VACUUM` or modify a user's database during validation.

## Reading and drawing

The sampler keeps one separate history reader. The first request for any of the five system-chart
metrics computes all five under one SQLite read transaction: CPU, available memory, total memory,
disk idle time and GPU load. Indexed metadata chooses the aggregation width before sample payloads
are streamed. Each series retains at most 2,000 points, using the existing reduction rules and
preserving origin/session boundaries, weighted means, extrema, coverage and gaps.

Only the finished series for one exact time window are cached. Cache admission is limited to an
8 MiB serialized result. SQLite `data_version` detects commits by other connections;
`total_changes` detects writes on the reader itself. Both are checked on the same connection,
before and after the read. Clearing history discards the cache. A cache does not hide appended,
deleted or aggregated measurements or changed events.
An intervening write can require another read, including between the five frontend requests;
the cache does not promise that a concurrently changing database is read only once per loading
operation. A single reduced-response command remains the next contract-level optimization.

The frontend keeps its five existing requests. It ignores old responses and errors after changing
period or leaving the view, and shows which period is loading. Display density follows the chart's
width, approximately one point per 2.5 pixels. Shaded bands retain measured minima and maxima;
segments do not cross gaps or unknown readings. Memory operands are joined by timestamp, with
ambiguous coincident operands left unknown. When a gauge exposes no averaging weight, a displayed
point keeps an original mean from its pixel bin instead of inventing a mean across cadences.
Resizing redraws cached data without another database request.

## Reproduce the offline benchmark

Use a new path on a drive with enough space. The benchmark refuses to overwrite an existing file;
`--reuse` accepts only its marked synthetic databases. It makes no collector, credential or network
call. The fixture has 12 system measurements and 20 processes with six measurements each, a gap,
a collector restart, unavailable GPU periods and a short peak. Older days are generated as minute
aggregates rather than first creating a much larger intermediate raw database.

```powershell
$env:CARGO_TARGET_DIR = 'D:\pcagent-target'
$env:TEMP = 'D:\pcagent-tmp'
$env:TMP = 'D:\pcagent-tmp'
cargo run --release --locked -p pcagent-store --example history_benchmark -- `
  --db D:\pcagent-tmp\history-new.db --duration 1h `
  --output D:\pcagent-tmp\history-new.json --max-bytes-per-row 16384
```

Supported durations are `10m`, `1h`, `24h` and `7d`. A saved result can be passed with
`--baseline <old-result.json>` to compare deterministic public-response checksums and storage.
`--min-speedup` and `--max-median-ms` are optional local performance gates, not portable CI timing
requirements. An independent probe checks exact numerators above `u64::MAX` before and after
minute aggregation.

Each reported cold bundle opens a fresh `Store` and reads the five metrics, then repeats them on
that reader to measure the warm application cache. The report uses the median of five repetitions.
Cold here means no application cache; it does not mean the operating system's disk cache was
flushed. Fresh-database bytes include SQLite overhead. The fixture and timings are measurements
of this workload, not promises about an arbitrary user's database.

## Initial measurements on the development PC

| Synthetic fixture | Legacy database | Compact database | Reduction | Identical response checksum |
| --- | ---: | ---: | ---: | --- |
| 10 minutes, 114 samples | 5,287,936 bytes | 1,552,384 bytes | 70.6% | yes |
| 1 hour, 684 samples | 31,309,824 bytes | 8,896,512 bytes | 71.6% | yes |

The initial 12 KiB-per-row exploratory target was missed: the ten-minute compact fixture uses
13,617 bytes per row including SQLite overhead. The documented 16 KiB gate checks this first
implementation without concealing that earlier result. It is not a disk-retention budget or a
bound on minute rows with high process turnover.

The first ten-minute cold bundle measured 369.42 ms before and 44.70 ms after; its warm compact
bundle measured 1.03 ms. The first one-hour run measured 1,933.08 ms before and 1,460.73 ms after,
with a 3.32 ms warm compact bundle. The one-hour run overlapped another release build under severe
memory pressure, so its latency comparison is inconclusive; the exploratory 2x speed gate failed.
Storage sizes, exact-response checksums and the separate u128 precision probe remain valid.

## Final replays after build and native validation

The benchmark was rebuilt from the final source. The following sequential runs reused the marked
fixtures after compilation and the isolated native app had stopped. All five runs exited
successfully. Times are medians of five bundles, in milliseconds; each bundle reads five metrics.
The first two rows compare equivalent fixtures stored in the legacy and compact formats.

| Fixture and query window | Legacy executable, cold | New executable, cold | New executable, warm |
| --- | ---: | ---: | ---: |
| 10-minute fixture, 10-minute window | 369.42 (initial baseline) | 16.66 | 0.32 |
| 1-hour fixture, 1-hour window | 896.86 (final replay) | 83.09 | 0.99 |
| Legacy 10-minute database, 10-minute window | 369.42 (initial baseline) | 45.03 | 0.18 |
| 7-day compact fixture, 1-hour window | not measured | 85.67 | 0.97 |
| 7-day compact fixture, 24-hour window | not measured | 1,819.46 | 2.69 |
| 7-day compact fixture, 7-day window | not measured | 3,203.64 | 2.18 |

The final one-hour comparison is 10.79x faster for this workload. The ten-minute legacy timing
was not repeated, so its before/after timing comparison is less controlled. The seven-day fixture
contains 25,917 rows and occupies 341,585,920 bytes (13,180 bytes per row). Its longest response
retains at most 1,995 points and preserves both expected gaps. No legacy seven-day timing was
measured. Warm timings assume no intervening write; live collection can invalidate the cache.

The 16,384-byte-per-row gate passed for all three compact fixtures. No speed threshold was enabled
for these final replays. Public-response checksums match the legacy baseline for both short
fixtures (`973e07c3496b7169`, `4ecc870ddbd6cfea`), including the old database read by the new
executable. The seven-day checksum (`7b7d957d87d5eecc`) is stable across repetitions. Coverage,
unknowns, extrema, gaps, point bounds and exact u128 precision checks all passed.

Local reports are under `D:\pcagent-tmp\history-perf-20261010`: `baseline-1h-quiet.json`,
`optimized-10m-final.json`, `optimized-1h-final.json`, `legacy-10m-final.json` and
`optimized-7d-final.json`. The preparation run overlapped compilation and contributes no timings
to this final table. These synthetic results do not establish a production disk budget or a
latency guarantee for databases with more processes, legacy rows or concurrent writes.
