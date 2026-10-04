# Phase 1 backend contracts

This implementation covers the conversation engine, privacy boundary, read-only inspection tools,
process activity, and persistent performance history. Frontend JavaScript, styles, markup and
mock data are deliberately unchanged. The conversation and settings screens still need the
integration described below before all backend capabilities are visible to a person.

## Model boundary and verification

`converse` preserves the existing scripted-model interface. Hosts that show a consent preview use
`conversation_message_with_privacy` and `converse_with_privacy` with the same frozen
`PrivacyContext`. The model still finishes through the closed `submit_answer` schema, after at
least one usable inspection. It cannot execute an action, expand a storage permission or authorize
more inference budget.

The verifier checks source-local quantities, executable names, citations, markup and action
references. C4 inspection records carry their own metric, value, unit, status, entity, source and
measurement window. Missing or denied values are not numeric evidence. Timestamp values cannot
prove a duration. Application aggregates and individual processes have separate evidence: an
explicit process reference wins, while an executable name can select one unambiguous application
group. IO byte rates, event/page rates, current memory and memory deltas remain distinct.

The repair budget remains three per investigation, shared with malformed or missing tool calls
and provider output truncation. Remaining unsupported claims are marked; the model's prose is not
replaced by local diagnostic templates. Details and limits are in
[answer-verification.md](answer-verification.md).

## Privacy and storage consent

Complete sharing permits sanitized program, service and application names. Anonymized sharing
uses opaque references and reapplies that choice to previous conversation prose and retained
evidence. Local analysis does not instantiate a provider or make a model request.

`PrivacyContext` removes known user/profile/machine identities, known credentials and detectable
private text. Collectors still use positive field allowlists. The trusted host can add a credential
loaded from its local settings with `protect_secret`; the context cannot be serialized and its
debug output withholds its data. The GUI adds its locally configured Nebius credential before
publishing the preview. Free-text classification has limits described in the verification guide;
the exact sanitized first-message preview remains visible before consent.

The GUI `cloud_preview` command adds optional `file_names` (default `false`). A requested file-name
preview requires complete sharing and an existing authorized scan. The response adds
`file_name_sharing`, the finite names and relative paths a later storage tool can disclose. The
first data message remains `message`; `bytes` describes exactly that message. Paths use a known
folder placeholder or `%SCOPE%` for the locally authorized root. Absolute profile paths, UNC
servers, file contents and hashes are excluded.

The matching start request carries `fileNames` in its camel-case request object. It requires
cloud mode, complete sharing and the approved folder. Changing this choice, the question, the
conversation, the sharing mode or the selected folder invalidates the preview. Tickets expire
after five minutes and are single-use. Name authorization is frozen to a scan: a rescan does not
inherit it. Older frontend clients omit the new field and retain sizes-only storage sharing.

## Conversation commands and events

| Command | Backend contract |
| --- | --- |
| `conversations_list()` | Up to 200 local summaries, dates, turn count and `last_turn_status`. |
| `conversation_get(id)` | Local conversation with C2 turn fields and the legacy `report` retained on each turn. |
| `conversation_delete(id)`, `conversations_clear()` | Remove local conversation records. An in-flight turn cannot recreate a deleted conversation. |
| `privacy_get()` | `{mode: "complete" / "anonymized" / "local", confirmed: boolean}`. Missing or invalid settings propose complete mode with `confirmed: false`. |
| `privacy_set(settings)` | Persist the closed settings object in `%APPDATA%\\pcagent\\privacy.json`. Changing mode or revoking confirmation invalidates previews. Confirming the same mode retains its ticket. |
| `send_message(conversation_id?, text, scope_ids, preview_id?)` | Use the stored mode. Cloud modes require confirmation and the matching preview. Attach at most one current completed scan. Returns `{id, conversation_id, turn_id}`; `id` remains for the old client. |
| `stop_turn(turn_id)` | Stop that active turn, cancel a pending extension and allow a recording to stop. A stale identifier cannot stop a newer turn. |
| `answer_extension(turn_id?, id?, accept)` | Answer a pending extension. `id` is the legacy identifier. At least one identifier is required; if both are given they must identify the same turn. |

For a first cloud message, choose the mode with `privacy_set`, request `cloud_preview` with the
matching `names` flag (`true` for complete), display its result, and confirm that same mode before
calling `send_message` with the returned preview ID. Confirming the existing mode keeps the
preview valid. A local message needs no preview or provider configuration. `scope_ids` contains
the current scan's opaque ID, not a filesystem path.

The stable `turn_id` is assigned before execution, included in events and preserved as the SQLite
turn ID. Failed attempts are stored with an error and an ordinary report shape as well. The GUI
report JSON is versioned. `turn:done` carries `{turn_id, conversation_id, turn}`; the turn contains
the answer, unverified excerpts, cited observations, tool events, usage, cost, status, error and
the legacy report. `turn:failed` adds `code` and `message` to that completion payload.

`turn:step` carries `{turn_id, at_unix_ms, step}`. `turn:extension` carries the request and usage.
`turn:delta` carries `{turn_id, request, text, replace: true, unverified: true}`: `text` is the
entire draft so far, not an append-only chunk. The verified completion replaces it. Existing
`inv:*`, `start_investigation` and `stop_investigation` remain available for the unchanged frontend.

Stopping prevents further provider requests and suppresses subsequent draft events. A synchronous
provider request already in flight may still finish; its tokens remain accounted for, while its
late answer/tool calls are discarded and the turn reports cancellation. The transport is not
forcibly interrupted. Native inspections remain bounded; recording additionally observes the stop
flag while waiting for samples.

Cloud storage tools receive only the scope IDs attached to the approved turn. Previously scanned
folders remain available locally but do not become visible to that model run automatically.

## Read-only inspection tools

| Tool | Observations |
| --- | --- |
| `get_process_activity` | Per-process and per-application CPU, current memory, memory deltas, IO rates, GPU activity and associated services over a bounded window. |
| `get_startup_programs` | Run/RunOnce registrations, startup folders, approval state and scheduled logon tasks; no commands or task actions. |
| `get_services` | State, startup type, opaque shared-process groups and available recent failure counts. |
| `get_recent_events` | Selected crash, hang, resource, hardware, disk and update events; counts and timestamps, without messages or user/computer fields. |
| `get_system_state` | Uptime, Windows build, pending reboot markers, fast-startup setting, available update records and pagefile observations. |
| `get_power_and_thermal` | Power source, charge/saver, active plan, processor frequency/performance limits, supported firmware and GPU temperatures; separate battery capacities, estimated wear, optional cycles, charge state and signed power flow (see [battery.md](battery.md)). |
| `get_installed_apps` | Uninstall registrations, publisher/version/date and size estimates, with incomplete Store/portable-app coverage stated. |
| `get_hardware_inventory` | CPU, memory modules/slots, volume space, available storage characteristics and display-driver version/date, without serial numbers. |
| `get_cleanup_opportunities` | Bounded metadata inventory of cache, temporary, rollback, large-download and virtual-disk categories, with recovery policy and coverage. |
| `query_system` | One fixed allowlisted WMI, registry or event query; no arbitrary WQL, paths, hosts or commands. |

Every schema rejects undeclared fields and validates bounds. Native inspection lives in
`crates/inspect`; non-Windows builds report unsupported coverage. Estimates, partial lower bounds,
permission failures and missing sensors remain distinct. A cleanup observation is not permission
to delete anything.

## Persistent measurement service

`pcagent_store::Sampler` owns a background collector and a SQLite connection. Normal sampling
runs every five seconds; a requested recording uses a one-second interval for 30–120 seconds.
Samples retain the system and up to twenty processes with local names. The agent applies sharing
rules when projecting a history result. Seven days are retained; data older than twenty-four
hours is aggregated into minute windows with extrema and coverage retained.

`HistoryQuery` specifies the metric, entity and time range. Results preserve gaps and unknown
values, include anomaly events where observed, and survive process restarts. Recording records
include begin/end markers and a completion/cancellation/interruption state. Clearing samples is
separate from deleting a conversation. A recording abandoned by a crashed process expires after
a 180-second grace period, checked before a new recording and during minute maintenance. The
grace avoids interrupting a recording owned by another local client. `SamplerStatus` reports observed collection cost: thread
CPU divided by elapsed time and logical processor count, and the whole host process's working set. That working set is not an
estimate of the sampler's incremental allocation.

| Local command | Backend contract |
| --- | --- |
| `history_status()` | Optional `SamplerStatus`, including the active recording, its progressing sample count and measured overhead. |
| `metric_history(metric, from_unix_ms, to_unix_ms, entity_ref?)` | A typed stored metric over a bounded interval; the default entity is the system. |
| `record_activity_start(duration_seconds)` | Start a 30–120 second recording and return its record immediately. |
| `record_activity_cancel(recording_id)` | Cancel that recording while preserving its measured samples and markers. |
| `battery_recording(recording_id)` | `Option<BatteryRecording>` containing dated battery points, gaps, coverage and continuous discharge segments for an existing recording. Unknown or deleted IDs return `null`; older recordings without battery samples report unavailable coverage. |
| `history_clear()` | Clear stored performance data independently of conversation deletion. |
| `startup_get()`, `startup_set(enabled)` | Inspect/change the explicit per-user Windows startup choice. |

The model tools `get_metric_history`, `get_activity_events` and `record_activity` use the same
persistent service. Historical results carry measurement times separately from query time, so a
fresh archive query cannot certify a claim about current usage. System anomaly peaks and an
observed process contributor are separate values; a correlated contributor is not a proven cause.
Events retain each peak's native source and actual observation window, and the contributor's own
measurement at that peak. Derived memory/disk percentages retain their operands; mismatched
windows remain explicit. Schema-v3 event rows without these fields are migrated as unknown
metadata and cannot certify numerical claims. Recordings also return the ten highest observed
process CPU peaks, including those below the system anomaly threshold. The additive
`resource_activity` result retains up to ten separate GPU, read-I/O and write-I/O peaks per metric,
with their own units, source and actual windows. Unavailable measurements stay absent with explicit
coverage; different resource quantities are not ranked against each other or converted into energy.
Sampling twenty processes
does not establish a complete ranking or causality.
Cloud history is bounded to 120 points (30 per system metric in a recording), thirty events and
thirty gap details, preserving extrema, measurement counts and explicit omissions.
Persisted process references remain unique across collector sessions and can be queried through
`get_metric_history`; they do not identify live processes for `get_process_details`.

## CLI entry points

| Command | Backend behavior |
| --- | --- |
| `pcagent diagnose QUESTION` | Local rules by default; `--allow-cloud` explicitly authorizes a provider request and displays the model's cited answer. `--anonymize` changes sharing. |
| `pcagent diagnose QUESTION --preview-cloud` | Print the sanitized first-message projection without a provider request. This is a separate dry run, not a reusable GUI consent ticket; a later invocation collects a new snapshot. |
| `pcagent chat --allow-cloud` | Keep bounded question/answer/evidence context in memory for terminal follow-ups. Conversation text is not added to the desktop conversation database. Performance samples still use persistent local history. |
| `pcagent inspect TOOL --arguments JSON` | Validate and execute one declared read-only tool locally; unknown tools and undeclared/out-of-range arguments fail. `--anonymize` selects the opaque projection. |
| `pcagent history` | Read a typed metric from the persisted database, with `--metric`, `--entity-ref`, `--window-seconds` and optional `--end-unix-ms`. |
| `pcagent record --seconds 30` | Record for 30–120 seconds, then print the recording, system CPU history, separate observed process CPU/GPU/I/O peaks, dated battery evolution and sampler status. |
| `pcagent sample --seconds 60` | Run normal background collection for the requested duration, then print measured sampler overhead. |

The inspection/history/record/sample commands accept `--database` for a separate local database.
Without it they use the desktop database. Diagnostic storage access requires explicit
`--scan-root`; filenames additionally require `--share-file-names`, which conflicts with
`--anonymize`. The allowlist is printed before an authorized cloud run. Content comparison is a
separate local `--compare-content` choice; file bytes and hashes are not model evidence.

## Frontend work still required

- Add explicit storage-name consent and display `file_name_sharing` before sending `fileNames`.
  The current program-name checkbox alone does not authorize filenames.
- Display the sanitized question/context preview and sharing mode clearly, including when text
  was omitted. Use `privacy_get`/`privacy_set` for the persisted complete/anonymized/local choice,
  its first-launch confirmation and the composer indicator.
- Adopt `send_message` and the stable `turn_id` events, replacing accumulated streaming drafts
  when `replace` is true, then replacing them with the verified final turn. Existing `inv:*`
  listeners remain supported during migration; do not render both event families twice.
- Connect persistent history periods, missing-data coverage, anomaly events and the sampler's
  measured resource footprint to the history view.
- Add recording start/stop/status controls and show its begin/end markers before requesting an
  explanation of the recording.
- Add the opt-in Windows startup control beside the measured collection overhead.
- Expose the process/application grouping and service association in the live view; retain
  process-specific drill-down so an aggregate is never confused with one process.
- Connect action proposals to a separate local approval flow. Read-only observations and cleanup
  policy classifications must not become automatic execution.

These are frontend integration tasks; this backend change does not claim visual QA or new
frontend behavior. Native smoke traces and deterministic tests validate tool shape and controller
behavior, not comparative answer quality against another model.

## Completion and validation boundary

The backend implementation has separate synthetic regression coverage for the C1 repair and
grounding rules, C2 retained evidence and stable persisted turn IDs, C3 payload/control identities,
and C5 first-message equality, sharing downgrade, protected credentials and consent expiry.
Cancellation during the final streamed reply has a scripted regression which preserves billed
usage.

Local validation on 2026-10-03 used Rust 1.90.0, locked dependencies and build/temporary directories
on D:. Formatting and workspace Clippy with `--all-targets -- -D warnings` passed on Windows.
The final Windows workspace suite passed 387 tests; the one live-provider test stayed ignored.
Both `pcagent` and `pcagent-gui` built successfully in release mode.

The Linux workspace suite passed 360 tests, with the same live-provider test ignored; workspace
Clippy also passed with all targets and warnings denied. Linux validation used Ubuntu under WSL,
with its Rust toolchain, dependency cache and build artifacts on D:. Test fixtures used a small
ext4 image stored on D:, since the Windows filesystem exposed through WSL does not preserve the
sparse-allocation metadata required by the scanner's Unix regression. The image was unmounted
after validation; the regression itself was neither changed nor skipped.

The release CLI then ran all ten process/inspection tool families in a non-elevated process,
without provider calls. Native process activity read 363/363 enumerated instances, with 144
attributed services; GPU coverage remained partial. A requested 30-second recording retained 28
actual samples and ten observed process peaks; reopening its SQLite file in another invocation
returned the same 28 system-history points. Out-of-range arguments were rejected. These counts
describe the measured run, not a guarantee of uninterrupted one-second collection on every PC.
Local traces remain outside the repository in `D:\pcagent-tmp\phase1-final-smoke-20261003`.

The release CLI's five-second sampler benchmark retained 13 samples over a measured 60,236 ms
resource window, with no sampler error: 1.42% of one logical CPU, or 0.17% of the machine's eight
logical processors, and a whole-process working set of 39,870,464 bytes (about 38.0 MiB). This run
met the 0.5% / 50 MiB sampler targets. It measures the CLI host, not incremental allocation or the
desktop window and its WebView processes; it is not a hardware-independent guarantee. The local
result and database are `D:\pcagent-tmp\phase1-final-footprint-20261003.json` and `.sqlite`.

The native runs supply process-coverage evidence for WP2, tool traces for WP4, and recorded-history,
restart and overhead measurements for WP5 on this PC. Missing sensors or denied native access
remain explicit. WP1's comparative answer quality still needs the WP7 model evaluation; scripted
tests do not establish that comparison. First-launch screens, recording controls and the other
frontend tasks above remain outside this backend delivery.
