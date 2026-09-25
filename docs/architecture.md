# Architecture and safety — first vertical slice

Basis: PC Agent specification v0.1, 2026-09-25, sections 6.4, 7, 8, 13, 15, 17 and Lot A. The full specification is not implemented by this commit.

## Trust boundaries

The CLI owns consent. `diagnose` is offline unless `--allow-cloud` is present; preview and offline flags conflict with that permission. `doctor --check-cloud` grants only an explicit catalog lookup. Normal configuration checks never make network requests.

The provider never receives the serialized local `Snapshot` object. `cloud_observation` constructs a separate allowlisted envelope. Local process names/identifiers are excluded. The question itself is sent verbatim when cloud mode is explicitly enabled, and the help text and README disclose this.

Only two named inspection tools are dispatched. Names, empty-object arguments, call type, ID length and uniqueness are validated before a whole batch is executed. No shell, arbitrary path, action executor or permission token exists in this initial crate. No model response can authorize a change.

The output schema selects a local finding by ID. It has no field for a new numeric assertion or arbitrary prose. Every rendered fact originates in collector evidence; final references are resolved to a locally constructed finding. This constraint does not establish causal validity or relevance; findings remain conservative observations and abstentions.

## Telemetry

System memory uses `K32GetPerformanceInfo`. Page counts are multiplied by the reported page size with checked arithmetic. Physical memory and system commit are separate metrics.

Process memory uses `K32GetProcessMemoryInfo` with `PROCESS_MEMORY_COUNTERS_EX`. Working set and private commit are separate values. Access is requested with `PROCESS_QUERY_LIMITED_INFORMATION`, not VM read/write or terminate rights. Handles are RAII-owned. `GetProcessTimes` provides the creation time so a reused PID does not inherit a prior instance's identity. An executable basename may be obtained locally; full paths are not retained in the model's data envelope.

The process enumeration buffer is capped at 16,384 entries; a full buffer is marked truncated. Read failures and denied permissions are counted, and inaccessible processes are not silently assigned zero memory. Top-N means the largest **readable** working sets, not a guarantee that inaccessible processes are smaller. No working-set total is advertised as exclusive physical memory. Aliases are only retained for currently readable processes within the collector session; boot identity and persistence are deferred.

CPU is calculated from two `GetSystemTimes` observations using monotonic elapsed time. Kernel time already includes idle time. Counter regressions and zero intervals remain unknown. Multi-group systems with more than 64 logical processors are explicitly unsupported for this aggregate.

## Failure behavior

Cloud errors never execute an action. Provider body text is not logged. TLS is mandatory, redirects are blocked, model IDs are explicit and account catalog membership is checked. There are no automatic retries or hidden provider/model switches. Missing usage is unknown, not zero. No monetary cost is estimated without verified pricing.

A 90-second budget is checked before/after model calls and between native tools; individual Win32 calls are synchronous, so a blocking native call is not yet forcibly interruptible. Ctrl+C exits the CLI rather than pretending to roll back a completed operation. There are no modifications to roll back in this slice.

No key is committed, embedded in a binary or sent to a model as prompt content. Environment variables are a development-only mechanism, not the planned Windows credential storage implementation.

## Official implementation references (consulted 2026-09-25)

- Microsoft, PERFORMANCE_INFORMATION: https://learn.microsoft.com/en-us/windows/win32/api/psapi/ns-psapi-performance_information
- Microsoft, GetProcessMemoryInfo and access rights: https://learn.microsoft.com/en-us/windows/win32/api/psapi/nf-psapi-getprocessmemoryinfo
- Microsoft, PROCESS_MEMORY_COUNTERS_EX: https://learn.microsoft.com/en-us/windows/win32/api/psapi/ns-psapi-process_memory_counters_ex
- Microsoft, GetSystemTimes and processor-group limitation: https://learn.microsoft.com/en-us/windows/win32/api/processthreadsapi/nf-processthreadsapi-getsystemtimes
- Nebius, client-side function calling: https://docs.tokenfactory.nebius.com/ai-models-inference/function-calling
- Nebius, account model catalog: https://docs.tokenfactory.nebius.com/api-reference/models/list-models

These references document interfaces, not measured results for this project.
