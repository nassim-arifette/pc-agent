# Measured idle footprint

This is a short native measurement of the published **v0.1.1 Windows GUI**, not a
benchmark of the features currently being implemented.

## Environment and workload

- Date: 27 October 2026, 08:06–08:12 UTC.
- Windows Server 2022 VM, 8 logical processors, 32 GiB physical memory.
- Published `pcagent-gui.exe`, SHA-256
  `129c5eb2680ebd57f5e14d74a12b0251b5a836050bb752ee1e6f2d7340298f57`
  (the binary from the [v0.1.1 release](https://github.com/nassim-arifette/pc-agent/releases/tag/v0.1.1)).
- A temporary standard account, verified before launch: medium integrity
  (`S-1-16-8192`), not elevated and not a member of Administrators.
- A fresh application profile, without a provider key. No question, scan, recording,
  elevated check or action was started. The ordinary sampler ran throughout.
- The native GUI was launched under that account. After a 25-second startup wait,
  one approximately three-minute window-open phase was followed by a hidden-window
  phase of the same duration. Hiding was confirmed through the native window handle;
  the GUI process continued running.

This does not establish the footprint of a configured conversation, a large scan,
speech recognition, watches, tracing or another new feature. An isolated fresh-profile
run also does not reproduce a typical long-lived desktop session.

## Memory and CPU results

Every approximately five seconds, the measurement enumerated the GUI's current
descendant tree by PID/parent PID and summed `WorkingSet64`, `PrivateMemorySize64`
and `TotalProcessorTime`. Both phases contained seven processes: the GUI and six
`msedgewebview2` descendants.

| Measure | Window open | Window hidden |
| --- | ---: | ---: |
| Measured phase duration | 181.6 s | 181.4 s |
| Samples | 36 | 36 |
| Mean summed working sets | 316.0 MiB | 203.7 MiB |
| Maximum summed working sets | 351.4 MiB | 232.8 MiB |
| Mean private bytes | 130.6 MiB | 131.8 MiB |
| Maximum private bytes | 137.3 MiB | 133.9 MiB |
| Mean CPU, one logical processor = 100% | 0.41% | 0.17% |
| Mean CPU, whole 8-processor VM = 100% | 0.05% | 0.02% |

CPU is the change in summed processor time divided by elapsed wall time; the
whole-machine percentage additionally divides by eight. This is not the sampling
script's CPU use, a Task Manager instantaneous peak, or a load-test result.

**Memory interpretation matters.** Working sets include resident shared pages and
can count the same page in several WebView2 processes. Private bytes are private
committed memory, not private resident memory. The declining working set in this
single ordered run is not proof that hiding the window saves that amount: warm-up,
cache trimming and the order of the phases are confounders. The hidden phase
measures a live hidden window, not a tray-start or suspended-process mode.

## SQLite size after a known duration

After roughly 6 minutes 34 seconds between the first and last stored samples:

| Item | Bytes | MiB |
| --- | ---: | ---: |
| `pcagent.sqlite` main file | 4,096 | 0.004 |
| Write-ahead log (`-wal`) | 3,407,272 | 3.249 |
| Shared-memory index (`-shm`) | 32,768 | 0.031 |
| **Total live on-disk files** | **3,444,136** | **3.285** |
| Logical database pages (`page_count × page_size`) | 1,142,784 | 1.090 |

There were **80 samples** at the ordinary 5,000 ms resolution, no activity events
and no recordings. Stored sample payloads totaled 991,138 bytes. A read-only
`PRAGMA quick_check` returned `ok` after the run.

The main file alone would misleadingly suggest that the database stayed at 4 KiB:
the sampler was writing in WAL mode. Count all three live files; logical database
pages and physical WAL allocation answer different questions and must not be
added together.

These are final sizes from a fresh profile, not a measured hourly growth rate.
No daily or seven-day extrapolation is claimed: minute aggregation, checkpoints,
retention, process churn, active recordings and the History storage changes affect
that result. A long-duration retention/compaction test remains a separate History
acceptance criterion.

## Reproduction protocol

1. Record the exact binary hash, Windows version, available memory and logical
   processor count. Use a fresh standard-user profile and verify its token before
   launching the GUI; never disable the anti-elevation guard.
2. Leave provider credentials absent and avoid scans, recordings and cloud calls.
   Record whether the profile is fresh or already configured.
3. Wait for startup, then sample the GUI and its descendant processes for at least
   three minutes. Count WebView2 processes too. Keep timestamped raw samples
   locally, and fail the run if the GUI exits.
4. Hide the native window without stopping the process, wait ten seconds and
   repeat for the same duration. Reverse the phase order and repeat if comparing
   window visibility rather than merely reporting an idle baseline.
5. Record the main SQLite, WAL and shared-memory file sizes at the same instant.
   Query sample counts, resolutions, first/last timestamps and logical page size
   read-only. Do not copy process names, user/profile paths or other collected
   evidence into a published report.
6. Remove temporary test accounts and permissions even if measurement fails.
   Preserve the evidence locally until the documented figures are reviewed.

The initial attempt did not produce valid measurements and was discarded.
The figures above come only from the completed, token-verified run.
