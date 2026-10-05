# Architecture and safety — first vertical slice

Basis: PC Agent specification v0.1, 2026-09-25, sections 6.4, 7, 8, 13, 15, 17 and Lot A. The full specification is not implemented by this commit.

## Trust boundaries

The CLI owns consent. `diagnose` is offline unless `--allow-cloud` is present; preview and offline flags conflict with that permission. `doctor --check-cloud` grants only an explicit catalog lookup. Normal configuration checks never make network requests.

The provider never receives the serialized local `Snapshot` object. `cloud_observation` constructs a separate allowlisted envelope of compact measurements: usable values grouped by entity, each unit once, observation windows per metric and unmeasured metrics summarized once per reason. Candidate findings travel without local evidence IDs, under per-investigation aliases (`F1`, `F2`, …) that the controller resolves; local finding IDs are never accepted from the model. Sharing follows the user's choice (contract C5 of [the plan](plan.md)). In complete sharing, the default of the CLI and the desktop app, a payload carries `process_names`: the executable name of each process reference it mentions, reduced to its last path component, without control or bidirectional characters, and at most 64 characters long. Anonymized sharing (`--anonymize`) sends references only, and the prompt tells the model not to guess names. PIDs, creation times, paths and session identifiers never leave in either mode. The question itself is sent verbatim when cloud mode is explicitly enabled, and the help text and README disclose this.

The Workstation registry declares twelve tools for inspection and context, plus the optional action-proposal tool. Names, closed argument schemas, call type, ID length and uniqueness are validated before a batch runs; a batch with an invalid call runs nothing and the model is told which call was refused. Only a successful inspection with usable data unlocks an answer. Proposals carry locally issued observation references, never permission tokens. The model has no shell or action executor and cannot authorize a change.

The model writes the answer itself ([`answer.rs`](../crates/agent/src/answer.rs), contract C1 of [the plan](plan.md)). Every request requires a tool call, and `submit_answer` ends the investigation with restricted Markdown in the language of the question, a confidence, a category, recommended plans and follow-up questions. The model cites short references: `O1`, `O2`, … for usable observations and `F1`, `F2`, … for findings, including findings a fresher measurement has superseded. Local observation IDs never reach it. Before the answer is accepted, the controller checks:
- every citation resolves;
- every number with a unit or a percentage matches a quantity the model received, within its written precision, in either GB convention, as a share of a total, its complement or what remains;
- every executable name was sent to the model;
- the text contains no command, link or HTML.

Problems go back to the model with the fix and, for citations, the valid references; at most three repairs per investigation, shared with refused tool calls, text without a tool call and replies cut off at the output limit. What is still wrong after that is shown marked as unverified, never removed silently. Citations do not establish causal validity or relevance.

Execution completion, sensor coverage and diagnostic support are separate report dimensions. Coverage includes uncollected domains, inaccessible or omitted processes, missing inventory components, incomplete storage scans/pages and history gaps. The compatibility `status` is conservative; CLI exit status follows execution and action results, not a claim of PC health.

Within the agent crate, `lib.rs` coordinates the investigation, `budget.rs` owns limits and extension consent, and `report.rs` constructs coverage and factual observation citations. These modules keep their internal decisions behind small interfaces. The orchestration regression suite lives in `tests.rs`; scripted model and toolbox adapters exercise the same interfaces as the CLI.

## Telemetry

System memory uses `K32GetPerformanceInfo`. Page counts are multiplied by the reported page size with checked arithmetic. Physical memory and system commit are separate metrics.

Processes come from one system list, `NtQuerySystemInformation(SystemProcessInformation)` ([`sysproc.rs`](../crates/telemetry/src/windows/sysproc.rs)). It gives the working set, commit charge (the same value as `PrivateUsage`), user and kernel time, I/O transfer counts, creation time and image name of every process without opening any. Services and other users' processes are therefore measured without administrator rights; on the development PC, 326 of 326 processes against 176 of 332 before. The structure layout is checked against the documented `windows-sys` definition. Every read of the returned buffer is bounds-checked, and an image name is read only when it lies inside the buffer. The buffer grows until the list fits, up to 64 MiB. The creation time keeps a reused PID from inheriting a prior instance's identity. Only the image basename is kept; full paths are not retained.

A first reading of every process is taken when the counters' one-second window starts, so the first snapshot already has each process's CPU and I/O rates. Later snapshots use the previous reading of the same instance. If the system list is unavailable, processes are opened one by one with `PROCESS_QUERY_LIMITED_INFORMATION` (never VM read/write or terminate rights; handles are RAII-owned), `K32GetProcessMemoryInfo`, `GetProcessTimes` and `GetProcessIoCounters`. That enumeration is capped at 16,384 entries, and a full buffer is marked truncated. Read failures and denied permissions are counted, and inaccessible processes are not assigned zero memory. Top-N ranks readable processes by the requested resource before limiting results; working set remains the default. No working-set total is advertised as exclusive physical memory. Session aliases include a bounded history of retired process identities; boot identity and persistence are deferred.

CPU is calculated from two `GetSystemTimes` observations using monotonic elapsed time. Kernel time already includes idle time. Counter regressions and zero intervals remain unknown. Multi-group systems with more than 64 logical processors are explicitly unsupported for this aggregate.

## Failure behavior

Cloud errors never execute an action. Provider body text is not logged. TLS is mandatory, redirects are blocked, model IDs are explicit and account catalog membership is checked. There are no automatic retries or hidden provider/model switches. Missing usage is unknown, not zero. No monetary cost is estimated without verified pricing.

A 600-second initial budget (10 requests, 20 tools, 300 seconds per request including reasoning, 8,192 output tokens per request) is checked before/after model calls and between tools; a synchronous native call is not forcibly interruptible. Round, tool and duration exhaustion produce a faithful partial report; an answer already written is shown instead of asking for more budget only to repair it. Only a local consent callback can grant an extension (+10 turns, +20 tools, +600 seconds, at most three times), preserving context, duplicate-call protection and pending jobs. Human decision time is excluded. JSON/noninteractive CLI paths decline without reading stdin; no model instruction grants consent. Scan jobs cancel and join on drop. During a locally approved action, Ctrl+C sets a cancellation flag checked between items and synthetic files. Already completed effects remain reported; there is no fictitious rollback. Durable start/finish records support inspection after interruption, and failure to persist a final record prevents an overall completed result. See [review status](archive/review-status.md) for guarded and intentional limits.

No key is committed, embedded in a binary or sent to a model as prompt content. Credentials come from the process environment or the per-user `%APPDATA%\pcagent\.env`, never from the current directory. That plaintext file is protected by profile permissions; a Windows credential vault remains future work.

## Official implementation references (consulted 2026-09-25)

- Microsoft, PERFORMANCE_INFORMATION: https://learn.microsoft.com/en-us/windows/win32/api/psapi/ns-psapi-performance_information
- Microsoft, GetProcessMemoryInfo and access rights: https://learn.microsoft.com/en-us/windows/win32/api/psapi/nf-psapi-getprocessmemoryinfo
- Microsoft, PROCESS_MEMORY_COUNTERS_EX: https://learn.microsoft.com/en-us/windows/win32/api/psapi/ns-psapi-process_memory_counters_ex
- Microsoft, GetSystemTimes and processor-group limitation: https://learn.microsoft.com/en-us/windows/win32/api/processthreadsapi/nf-processthreadsapi-getsystemtimes
- Nebius, client-side function calling: https://docs.tokenfactory.nebius.com/ai-models-inference/function-calling
- Nebius, account model catalog: https://docs.tokenfactory.nebius.com/api-reference/models/list-models

These references document interfaces, not measured results for this project.
