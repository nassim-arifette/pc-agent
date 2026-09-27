# Architecture and safety — first vertical slice

Basis: PC Agent specification v0.1, 2026-09-25, sections 6.4, 7, 8, 13, 15, 17 and Lot A. The full specification is not implemented by this commit.

## Trust boundaries

The CLI owns consent. `diagnose` is offline unless `--allow-cloud` is present; preview and offline flags conflict with that permission. `doctor --check-cloud` grants only an explicit catalog lookup. Normal configuration checks never make network requests.

The provider never receives the serialized local `Snapshot` object. `cloud_observation` constructs a separate allowlisted envelope of compact measurements: usable values grouped by entity, each unit once, observation windows per metric and unmeasured metrics summarized once per reason. Candidate findings travel without local evidence IDs, under per-investigation aliases (`F1`, `F2`, …) that the controller resolves; local finding IDs are never accepted from the model. Local process names/identifiers are excluded. The question itself is sent verbatim when cloud mode is explicitly enabled, and the help text and README disclose this.

The Workstation registry declares twelve tools for inspection and context, plus the optional action-proposal tool. Names, closed argument schemas, call type, ID length and uniqueness are validated before a batch runs. Only a successful inspection with usable data unlocks a conclusion. Proposals carry locally issued observation references, never permission tokens. The model has no shell or action executor and cannot authorize a change.

The output schema selects current local findings and usable inspection observations by ID. It has no field for a new numeric assertion or arbitrary prose. Fresh comparable measurements move superseded findings into local history. Hardware/storage/history citations resolve to locally generated facts and their retained payloads. Missing, failed, context and proposal outputs cannot satisfy an observation citation. Observation-only conclusions abstain on diagnosis; citations do not establish causal validity or relevance.

Execution completion, sensor coverage and diagnostic support are separate report dimensions. Coverage includes uncollected domains, inaccessible or omitted processes, missing inventory components, incomplete storage scans/pages and history gaps. The compatibility `status` is conservative; CLI exit status follows execution and action results, not a claim of PC health.

Within the agent crate, `lib.rs` coordinates the investigation, `budget.rs` owns limits and extension consent, and `report.rs` constructs coverage and factual observation citations. These modules keep their internal decisions behind small interfaces. The orchestration regression suite lives in `tests.rs`; scripted model and toolbox adapters exercise the same interfaces as the CLI.

## Telemetry

System memory uses `K32GetPerformanceInfo`. Page counts are multiplied by the reported page size with checked arithmetic. Physical memory and system commit are separate metrics.

Process memory uses `K32GetProcessMemoryInfo` with `PROCESS_MEMORY_COUNTERS_EX`. Working set and private commit are separate values. Access is requested with `PROCESS_QUERY_LIMITED_INFORMATION`, not VM read/write or terminate rights. Handles are RAII-owned. `GetProcessTimes` provides the creation time so a reused PID does not inherit a prior instance's identity. An executable basename may be obtained locally; full paths are not retained in the model's data envelope.

The process enumeration buffer is capped at 16,384 entries; a full buffer is marked truncated. Read failures and denied permissions are counted, and inaccessible processes are not assigned zero memory. Top-N ranks readable processes by the requested resource before limiting results; working set remains the default. No working-set total is advertised as exclusive physical memory. Session aliases include a bounded history of retired process identities; boot identity and persistence are deferred.

CPU is calculated from two `GetSystemTimes` observations using monotonic elapsed time. Kernel time already includes idle time. Counter regressions and zero intervals remain unknown. Multi-group systems with more than 64 logical processors are explicitly unsupported for this aggregate.

## Failure behavior

Cloud errors never execute an action. Provider body text is not logged. TLS is mandatory, redirects are blocked, model IDs are explicit and account catalog membership is checked. There are no automatic retries or hidden provider/model switches. Missing usage is unknown, not zero. No monetary cost is estimated without verified pricing.

A 90-second initial budget is checked before/after model calls and between tools; a synchronous native call is not forcibly interruptible. Round, tool and duration exhaustion produce a faithful partial report. Only a local consent callback can grant an extension (+6 turns, +12 tools, +90 seconds, at most three times), preserving context, duplicate-call protection and pending jobs. Human decision time is excluded. JSON/noninteractive CLI paths decline without reading stdin; no model instruction grants consent. Scan jobs cancel and join on drop. During a locally approved action, Ctrl+C sets a cancellation flag checked between items and synthetic files. Already completed effects remain reported; there is no fictitious rollback. Durable start/finish records support inspection after interruption, and failure to persist a final record prevents an overall completed result. See [review status](review-status.md) for guarded and intentional limits.

No key is committed, embedded in a binary or sent to a model as prompt content. Credentials come from the process environment or the per-user `%APPDATA%\pcagent\.env`, never from the current directory. That plaintext file is protected by profile permissions; a Windows credential vault remains future work.

## Official implementation references (consulted 2026-09-25)

- Microsoft, PERFORMANCE_INFORMATION: https://learn.microsoft.com/en-us/windows/win32/api/psapi/ns-psapi-performance_information
- Microsoft, GetProcessMemoryInfo and access rights: https://learn.microsoft.com/en-us/windows/win32/api/psapi/nf-psapi-getprocessmemoryinfo
- Microsoft, PROCESS_MEMORY_COUNTERS_EX: https://learn.microsoft.com/en-us/windows/win32/api/psapi/ns-psapi-process_memory_counters_ex
- Microsoft, GetSystemTimes and processor-group limitation: https://learn.microsoft.com/en-us/windows/win32/api/processthreadsapi/nf-processthreadsapi-getsystemtimes
- Nebius, client-side function calling: https://docs.tokenfactory.nebius.com/ai-models-inference/function-calling
- Nebius, account model catalog: https://docs.tokenfactory.nebius.com/api-reference/models/list-models

These references document interfaces, not measured results for this project.
