# Architecture and safety

How PC Agent is built on `main` as of 26 October 2026: the crates and apps, the trust boundaries
between them, what happens during one turn, and where the detailed design notes live. The product
vision and the contracts C1 to C7 cited below are in [the plan](plan.md) (in French); this page
describes the code, not the plan.

## Design rules

- **The model is the analyst.** It chooses read-only tools, reasons and writes the answer in the
  language of the question. Local rules produce *findings* that it receives as hints, not as the
  answer.
- **The tools are the product.** Each one has a closed argument schema, reports its coverage
  (`ok`, `partial`, `denied`, `unsupported`, `needs_admin`) and never turns a missing value into
  zero.
- **The model has no execution authority.** It can propose an action; only a local click in the
  window (or a typed `yes` in the CLI) runs it, after the target has been checked again.
- **No administrator by default.** The window, the CLI and the model never run elevated.
  Administrator readings come from a separate helper, one closed task at a time, after a local
  click and Windows' own consent prompt.
- **Privacy by visible choice.** What leaves the PC is an explicit projection (contract C5) that
  the person can preview exactly before sending.
- **The verifier checks, it does not write.** Every cited figure is compared with the measurement
  it cites; what cannot be verified is repaired by the model or marked as unverified.

## Crates and apps

| Path | Package | Role | Unsafe code |
| --- | --- | --- | --- |
| `crates/shared` | `pcagent-shared` | Versioned evidence types: metrics, units, windows, statuses, process identities, snapshots | forbidden |
| `crates/telemetry` | `pcagent-telemetry` | Live collection: memory, CPU, per-process activity from one system process list, applications grouped by validated parent chains, PDH disk and GPU counters, NVML, services, process tree, in-memory history ([telemetry.md](telemetry.md)) | `windows` module only |
| `crates/inspect` | `pcagent-inspect` | Read-only system inspections (startup, services, events, system state, power and thermal, battery, installed apps, hardware and SMBIOS, drivers and Windows Update offers, cleanup categories, fixed WMI/registry/event-log queries) and the elevated helper's tasks and protocol ([system-inspection.md](system-inspection.md), [elevation.md](elevation.md)) | `windows` module and native `elevation` submodules only |
| `crates/diagnostics` | `pcagent-diagnostics` | Conservative local findings with a support level, and the before/after comparison of an action | forbidden |
| `crates/storage` | `pcagent-storage` | Bounded metadata scans of an authorized folder, optional local content comparison ([storage.md](storage.md)) | `windows` module only |
| `crates/policy` | `pcagent-policy` | Protected paths, preferences, cleanup classification and plans; pure, no filesystem access ([policy.md](policy.md)) | forbidden |
| `crates/actions` | `pcagent-actions` | Action plans, single-use local approval, revalidation, effects and journal ([actions.md](actions.md)) | `windows` module only |
| `crates/agent` | `pcagent-agent` | The model loop, tool registry, answer verification, privacy projection, Nebius and Tavily clients, credentials, selected evidence, routing and cost | forbidden |
| `crates/store` | `pcagent-store` | SQLite storage of conversations, the five-second history, recordings and detected events; the background sampler ([history-performance.md](history-performance.md)) | forbidden |
| `apps/cli` | `pcagent` | Command-line interface | forbidden |
| `apps/gui` | `pcagent-gui` | Tauri 2 desktop app: Rust commands in `src/`, plain JavaScript modules in `ui/` ([gui.md](gui.md)) | forbidden |
| `apps/elevated` | `pcagent-elevated` | The elevated helper executable: a thin `main` over `pcagent_inspect::elevation::worker_main` | forbidden |
| `apps/eval` | `pcagent-eval` | Capture, replay, MCP stdio server, grading and calibration tooling ([eval.md](eval.md)) | forbidden |

Unsafe code is confined to those native modules, with a `// SAFETY:` comment per block as a
repository rule. Dependencies are pinned to exact versions in `[workspace.dependencies]`,
`Cargo.lock` is committed and builds use `--locked`. The workspace is meant to compile and test on
Linux too: native collectors there return `unsupported`, the window prints that it runs on
Windows, and the helper refuses to run.

The CLI and the window call the same crates for measurements, verification and actions; the
window adds only display rules, such as the words of its PC status pill.

## Trust boundaries

```text
+------------------------------------------- this PC --------------------------------------------+
|                                                                                                |
|  WebView page (untrusted) --invoke--> Rust host: pcagent-gui or pcagent CLI (standard user)    |
|                           <--events---   |   |      |      |                                   |
|                                          |   |      |      +--> collectors, scans, SQLite      |
|                                          |   |      +--local click--> actions broker, journal  |
|                                          |   +--local click + UAC--> pcagent-elevated (admin)  |
|                                          |                           no network, no model      |
+------------------------------------------+-----------------------------------------------------+
                                           | HTTPS, fixed endpoints, C5 projection, no redirects
              +----------------------------+------------------------------+
              v                            v                              v
     Nebius Token Factory           Tavily (optional, C6)          Windows Update (Microsoft)
   catalog, chat completions        typed lookups, local queries   only get_drivers online
```

1. **Page to host.** The page is treated as untrusted input. Tauri commands validate every
   argument; paths and item references are resolved on the Rust side, never taken from the page.
   The content security policy allows only the app's own files and Tauri's IPC. Besides PC
   Agent's own commands, the page holds only Tauri's default event permissions
   (`core:event:default` in `apps/gui/capabilities/main.json`, used to listen to the host's
   events): no file-system, shell or dialog permission. Text is always inserted as text, never as
   HTML. API keys are written by Rust and never sent back to the page, which
   sees only whether a key exists and its last four characters.
2. **Host to model.** The provider never receives a serialized local object. Every payload is
   built from a positive allowlist (`cloud_observation`, the tool projections) and passes through
   the frozen privacy filter of the turn (`privacy.rs`), including echoed model text and repair
   messages. Model output is untrusted: tool calls are validated against closed schemas, and an
   answer is checked before it is shown as final.
3. **Host to the web.** Web lookups exist only with a Tavily key and the person's separate,
   remembered consent; their queries are composed on the PC from measured references, never
   written by the model ([web-search.md](web-search.md)).
4. **Unelevated host to elevated helper.** One closed task, a private job directory with a
   protected ACL, a size-bounded file protocol, and a launch only after the local click and
   Windows' consent prompt (see [Elevated helper](#elevated-helper)).
5. **Proposal to effect.** A plan becomes an effect only through a single-use approval token
   issued by the local process, after the broker revalidates each target (see
   [Actions broker and journal](#actions-broker-and-journal)).
6. **Data at rest.** Everything stays in the person's Windows profile:
   - `%APPDATA%\pcagent\.env`: Nebius key and model, optional reasoning setting and Tavily key;
     plain text protected by the profile's permissions (no credential vault yet);
   - `%APPDATA%\pcagent\privacy.json` and `model-routing.json`: sharing and routing choices;
   - `%APPDATA%\pcagent\pcagent.sqlite`: conversations (90 days) and measurement history (7 days);
   - `%LOCALAPPDATA%\pcagent\journal`: the action journal;
   - `%LOCALAPPDATA%\dev.pcagent.desktop`: the WebView's own data (display preferences, selection
     labels);
   - `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`, value `PcAgent`: only while *Start
     with Windows* is on.

Credentials come from the process environment first, then from `%APPDATA%\pcagent\.env`, and only
the four names `NEBIUS_API_KEY`, `NEBIUS_MODEL_ID`, `NEBIUS_REASONING` and `TAVILY_API_KEY` are
read from that file. A `.env` file in the current directory is never read: running PC Agent
inside someone else's folder must not send your measurements to their account.

## Data flow of a turn

The desktop app path; the CLI follows the same steps without the page.

1. **Composer.** The person types a question. The toggles under the composer show what will leave
   (program names, web lookups, an attached scanned folder and its file names).
2. **Preview ticket.** Just before sending, the page calls `cloud_preview` (or reuses the preview
   *See what leaves* just showed). The host takes a fresh snapshot, reads the earlier turns of the
   conversation and freezes the snapshot, the context, the sharing choice and the
   `PrivacyContext` (local user, profile and machine names to filter) in a single-use ticket that
   expires after five minutes. No dialog waits for a confirmation.
3. **Send.** `send_message` starts the turn only if the ticket still matches the question, the
   conversation, the sharing choice and the model policy. Without a usable key and model, the
   turn runs locally (local rules only, nothing sent).
4. **Worker.** One worker thread runs `converse_with_privacy` (or `converse_with_selection` for
   Explain and Investigate) with a `Workstation` toolbox: the read-only tools, the history tools
   over the local sampler, approved helper results, web lookups when consented, and
   `prepare_action_plan`.
5. **Events.** The window follows `turn:step` (each model request and each tool, with its
   duration), `turn:delta` (the answer draft while it streams), `turn:extension` (a budget
   request), then `turn:done` or `turn:failed`. The page keeps the exact message sent with the
   turn's steps for the session. A reloaded window reattaches a running turn with
   `current_turn`; Stop calls `stop_turn`.
6. **Result.** `view.rs` turns the report into display data: the verified answer, its citations,
   unverified passages, steps, usage and cost per request. The turn is saved in SQLite, action
   proposals wait on the Rust side for a click, and an answer that lacked administrator readings
   gets the matching *Deeper checks* card (a local rule over `needs_admin` coverage).

**Explain and Investigate.** From Live PC, History or Storage, the page passes a host-issued
selection reference, never measurements or names. `selection_preview` freezes that capture and
returns the exact first message and a ticket; `selection_send` consumes it. Explain (shown as
*Summarize these measurements*) exposes only the frozen evidence, cited `O0`, and starts no
collector; Investigate (*Find the cause*) adds the normal toolbox. Details:
[selection.md](selection.md).

## The agent loop

The loop lives in `crates/agent/src/lib.rs` (`converse_inner`).

- **Messages.** A system prompt built from the declared tools (it mentions proposals and web
  lookups only when those tools exist); a first message with the question, the initial
  measurements and candidate findings under per-turn aliases (`F1`, `F2`…), plus the most recent
  earlier turns of the conversation within 12,000 serialized bytes (`MAX_PAST_CHARS`), which are
  context, never measured evidence. Each tool reply carries its citation alias (`O1`, `O2`…),
  `current_finding_ids` and `citable_observation_ids`. Local observation IDs never reach the model.
- **Tool choice.** Every request uses `tool_choice: "required"`: the model always calls a tool,
  and ends the turn by calling `submit_answer`. After a rejected `submit_answer`, once usable
  measurements exist, a repair request may name that single tool (the limited C1 exception of 19
  October); the stream is then closed as soon as that call's arguments are complete.
- **Batches.** A batch of tool calls is validated as a whole (names, closed schemas, call IDs,
  duplicates) before anything runs; an invalid batch runs nothing and the model is told which
  call was refused. Only an inspection with usable data unlocks an answer.
- **Budgets** (`budget.rs`). Per turn: 10 model requests, 20 tool calls and 600 seconds, time
  spent waiting for the person excluded. Per request: at most 300 seconds and 8,192 output tokens,
  reasoning included. At a limit, the person can extend the same turn by the same amounts, at
  most three times; JSON and non-interactive CLI runs never extend. Stop refuses the next request
  and discards the reply of the one in flight, keeping its billed usage; a request already sent
  cannot be recalled.
- **Verification** (`answer.rs`, `answer/grounding.rs`, contract C1). Before an answer is
  accepted:
  - every citation (`[O2]`, `[F1]`, `[W1]`) exists in this conversation;
  - every number with a unit or a percentage matches a quantity of the source cited in the same
    passage, within its written precision, in either GB/GiB convention, as a bound, a share of a
    total, its complement or what remains; the question and earlier answers are never evidence;
  - different programs, metric families and batteries cannot lend each other values, and an
    older measurement written in the present tense is flagged `historical_wording`;
  - every executable name was actually sent to the model;
  - the text contains no command, link or HTML;
  - prices and version numbers cited with `[W…]` appear in that web snippet.

  Limits and covered cases: [answer-verification.md](answer-verification.md).
- **Repairs.** Problems go back to the model with the fix and the valid references, at most three
  times per turn, shared with refused tool calls, text without a tool call and replies cut off at
  the output limit. A cut-off reply is asked again once, shorter. The same problems twice in a row
  end the repairs. What is still wrong is shown with the passages marked unverified: the best
  attempt written since the last measurements, never a local template.
- **Context compaction** (`tool_context.rs`). An exactly repeated result is replaced by a
  reference to the earlier one (`same_result_as`); past 32 KiB of older tool context, repeated
  metadata is folded. Raw values keep a readable form next to them. The verifier always reads the
  complete projection ([tool-context.md](tool-context.md)).
- **Routing** (`routing.rs`). The configured model by default, or always Super, or always Ultra.
  Automatic mode escalates at most once from Super to Ultra and is available only with a
  validated local calibration from reviewed real runs; without one it stays off. An escalation
  keeps the measurements, citations and budgets of the turn and grants no new authority.
- **Cost** (`cost.rs`). Measured input and output tokens (reasoning counts as output) times the
  catalog prices of the model, read from the account's `/v1/models?verbose=true` with the time
  they were read: an estimate, not an invoice. Without a verified price only the usage is shown;
  when a request reported no usage (a stream closed early), the cost is a lower bound. Tavily
  credits are counted separately, not added to the displayed cost.

### Reports and failures

- A report keeps three separate dimensions: `execution_status` (`completed`, `budget_exhausted`,
  `failed`), `coverage` (what each domain and target could read, including uncollected domains,
  inaccessible processes, partial scans and history gaps) and `diagnostic_support` (`supported`,
  `plausible`, `insufficient`). A completed turn is not a claim that the PC is healthy, and a
  citation does not establish a cause.
- A failed or timed-out request is not retried automatically; the turn ends with a partial report
  that keeps the measurements and the best answer written so far, if any. The one exception is a
  request cut by the turn's own time limit: if the person extends the turn, that request is sent
  again. Errors never execute an action, and missing usage stays unknown, never zero.

## Nebius Token Factory client

`crates/agent/src/nebius.rs` and `models.rs`:

- Fixed endpoint `https://api.tokenfactory.nebius.com/v1` (OpenAI-compatible
  `chat/completions`, and `models?verbose=true` for the account catalog); HTTPS only, redirects
  disabled, 5-second connection timeout, bounded response sizes. Provider error bodies are never
  logged or shown.
- **NVIDIA-only guard.** A model ID must start with `nvidia/` and contain `nemotron`, must be
  listed in the account catalog (checked before a client's first request; the window builds one
  per turn), and must not be declared there without tool support. There is no fallback model or
  provider.
- Requests stream (`"stream": true` with usage in the last event), with NVIDIA's sampling for
  Nemotron 3 (top_p 0.95; temperature 1.0 for Super and Ultra, 0.6 for Nano) and the reasoning
  switches of Nemotron's chat template (`enable_thinking`, plus `low_effort` for Super or
  `medium_effort` for Ultra in the *Light* setting).
- The desktop app shows the `submit_answer` arguments as a draft while they stream; the verified
  answer replaces it when the turn ends.

## Tool registry

Tools are declared in a local registry (`ToolSpec` in `tools.rs`) and sent as JSON schemas with
`additionalProperties: false`. Arguments are bounded integers, closed enumerations or short
references with a fixed shape (`proc-…`, `scope-…`, `device-…`, an update number `KB…` or an
error code `0x…`). References to local items resolve only against what the PC itself issued; no
tool accepts a path, a command or free text. Unknown keys, wrong types and out-of-range values
are rejected before any dispatch.

| Family | Tools | Source |
| --- | --- | --- |
| System and processes | `get_system_snapshot`, `get_processes`, `get_process_details`, `get_process_tree`, `get_process_activity` | `workstation.rs`, `activity_tools.rs` over `pcagent-telemetry` |
| Windows inspections | `get_startup_programs`, `get_services`, `get_recent_events`, `get_system_state`, `get_power_and_thermal`, `get_installed_apps`, `get_hardware_inventory`, `get_drivers`, `get_cleanup_opportunities`, `query_system` | `system_tools.rs` over `pcagent-inspect` |
| History | `get_metric_history`, `get_activity_events`, `record_activity` | `history_tools.rs` over the local sampler |
| Storage | `list_storage_scopes`, `get_storage_overview`, `start_storage_scan`, `get_largest_items`, `get_cleanup_candidates`, `get_user_preferences` | `workstation.rs` over `pcagent-storage` and `pcagent-policy` |
| Administrator results | `get_approved_elevated_inspection` (reads a result the person already approved; never launches anything) | `elevated_tools.rs` |
| Proposals | `prepare_action_plan` (at most three plans per turn) | `proposals.rs`, declared when actions are enabled |
| Web, optional | `lookup_driver_notes`, `lookup_known_issue`, `lookup_upgrade_price`, `lookup_pc_specs` (at most three searches per turn) | `external_tools.rs`, declared with a key and consent |
| Selected evidence | `get_selected_evidence` | `selection.rs`, Explain and Investigate only |
| Answer | `submit_answer` | `answer.rs` |

The model sees opaque process references; in complete sharing a payload also carries
`process_names`, the executable name of each reference it mentions (a basename, cleaned, at most
64 characters). Storage items travel as aliases (`file-3`, `dir-7`); real names leave only from
the finite list frozen in the preview when file-name sharing is on.

## History and storage

- **Sampler** (`crates/store/src/sampler.rs`). While the app runs, one background thread records
  the system and 20 processes (ranked alternately by processor and by memory) every 5 seconds,
  and every second during a recording (30 to 120 seconds), in `pcagent.sqlite`. Raw rows are
  kept 24 hours, then one-minute summaries until seven days. Gaps (app closed, PC asleep) stay
  gaps. Detected events are anomalies only (processor peak, saturated memory, disk activity or
  latency); processor and memory events name the program measured at the peak, which is not
  presented as a cause, and disk events are attributed to no program. The sampler reports its
  own measured processor cost against a target of 0.5 %.
- **Conversations** (`crates/store/src/lib.rs`, contract C2). Tables `conversations` and `turns`
  (versioned JSON), next to `samples`, `sample_entities`, `activity_events`, `recordings` and
  `recording_battery_samples`; schema version 5. Conversations expire after 90 days; the person
  can delete one or all, and clear the history separately.
- **Folder scans** (`crates/storage`). A folder is authorized locally, then walked for metadata
  only, without following links or opening cloud placeholders. When the scanned folder is
  attached to a question, the turn uses the Storage view's own `StorageService` and reuses that
  scan without rescanning; without an attached folder, the model gets an empty service and sees
  no folder at all.

## Desktop app

```text
ui/ (HTML, CSS, JavaScript modules, no build step) --invoke--> src/app.rs (Tauri commands)
        ^                                                           |
        +------------ events: turn:*, scan:* <----------------------+
                                                                    v
             pcagent-telemetry, -inspect, -agent, -storage, -store, -policy, -actions
```

- `src/app.rs` holds the commands and the threads: one owns the live collector (dashboard,
  process activity, preview snapshots), one reads the battery, the sampler records history, and
  each turn and each scan runs on its own thread. It also owns the notification-area icon and the
  startup guard that refuses an elevated session.
- `src/view.rs` turns core types into display data (raw value, text and status for every
  measurement); `src/session.rs` reports each real step and turns Stop into a refused next
  request; `src/preview.rs` and `src/selection.rs` hold the single-use tickets; `src/settings.rs`
  and `src/model_routing.rs` keep the local choices; `src/protocol.rs` projects stored turns.
  These modules are portable and unit-tested on every platform. `src/app.rs`,
  `src/elevation.rs` (the helper slot, tested with fake tasks) and `src/actions.rs` (the waiting
  proposals) are Windows-only.
- `ui/js/main.js` mounts one feature view at a time (`chat/view.js`, `history/view.js`,
  `live/view.js`, `storage/view.js`, `settings/view.js`, `settings/setup.js`). Related UI and
  presentation helpers live beside their feature; `selection/` owns selected-item interactions,
  and `elevation/` owns the read-only helper cards. Shared shell, state, localization and DOM
  utilities stay at the root. `api.js` is the only bridge to Tauri; `markdown.js`
  renders the model's restricted Markdown as DOM nodes; `dom.js` never uses `innerHTML`.
- The design mock (`dev/mock.js`) answers every command with synthetic data so the interface can
  be reviewed in a browser; it lives outside `ui/` and is never embedded. Layout, views and the
  visual system: [gui.md](gui.md).

## Elevated helper

`pcagent-elevated` is a separate executable next to the app; only that fixed sibling is launched.
It has no model client, no HTTP client, no shell and no arbitrary query; the battery check runs
Windows' own `powercfg.exe` from the system directory with fixed arguments.

- **Closed tasks** (`crates/inspect/src/elevation/protocol.rs`, protocol version 2):
  `system_caches`, `mft_inventory` (one drive letter), `shadow_storage`, `disk_reliability`,
  `process_images`, `battery_energy` and `performance_trace`. The window offers the quick drive
  analysis (`mft_inventory`) and four deeper checks (`system_caches`, `shadow_storage`,
  `disk_reliability`, `battery_energy`); `process_images` is offered by the CLI only
  (`pcagent elevated-inspect process-image`). `performance_trace` (ETW) returns `unsupported`
  until its cleanup after a forced stop is proven ([etw.md](etw.md)).
- **Approval.** `elevation_prepare` creates the request and returns the task and its budget (at
  most 120 seconds) for review; only the local *Allow and start* click calls `elevation_run`,
  which launches the helper with `ShellExecuteExW` and the `runas` verb, so Windows asks for
  consent. The approval object cannot be cloned or serialized and is consumed once.
- **File protocol.** A private `pcagent-elevjob-<uuid>` directory under `%TEMP%`, with a protected
  ACL, holds `request.json` (at most 8 KiB, bound by a SHA-256 digest and a random nonce),
  `response.json` (at most 64 KiB), the `running` and `cancel` markers, and for the battery check
  a private `powercfg` export. Unknown fields are rejected at every level; files are created once
  and never overwritten. The helper accepts exactly three options (`--job-dir`, `--nonce`,
  `--request-digest`), never a path to scan or a command.
- **Lifetime.** Watchdogs bound setup and collection; cancellation, timeout and a late consent
  terminate only the helper's own process handle. The window waits for the helper to exit and
  for its job files to be removed before it quits.
- **Results.** `to_inspection` turns a response into C4 observations with coverage; job IDs,
  nonces, user SIDs and paths stay local. A completed result is cached for five minutes, where
  `get_approved_elevated_inspection` can read it with the sharing choice applied.

Design and bounds: [elevation.md](elevation.md), [elevation-completion.md](elevation-completion.md),
[mft.md](mft.md), [battery-energy.md](battery-energy.md).

## Actions broker and journal

`crates/actions` with the model-facing wiring in `crates/agent/src/proposals.rs`:

```text
model --prepare_action_plan (opaque refs only)--> Broker::prepare_plan --> ActionPlan (awaiting approval)
local click --approve_locally--> ApprovalToken (single use) --> Broker::execute --> result + before/after
```

- The model receives only the plan's model view (references, estimate, recovery, preconditions);
  resolved paths, PIDs and identities stay local, and `execute` is never a tool.
- Catalog: show an item in Explorer; open an allowlisted Settings page (Storage Sense, Apps,
  Startup apps, Windows Update, optional updates, Power and sleep, battery usage); ask an
  application to close normally (`WM_CLOSE`, never `TerminateProcess`); send scanned files to the
  Recycle Bin; run a cleanup adapter (current-user TEMP files older than seven days, NVIDIA
  DXCache, Direct3D shader cache, at most 64 files, through the Recycle Bin); turn off a
  current-user startup entry, and restore it from the journal. The desktop app keeps the default
  `reversible_only` preference, so it never deletes a file permanently; only the CLI's marked
  synthetic demo cache can be deleted, and only with an explicit preference.
- Before each item, the broker checks the target again (file identity, process identity, registry
  state, protection policy) and refuses anything changed or protected. Each item has fsync'd
  start and finish records; an interrupted action is reported as `needs_inspection`, never
  replayed.
- Verification follows the action: actual bytes and free space before and after for cleanup,
  three system samples on each side for a normal close, and no performance claim for opening
  Explorer or Settings. A file in the Recycle Bin still occupies its drive.

## Web lookups

`crates/agent/src/external_tools.rs` and `tavily.rs`, contract C6. Four typed lookups compose
their query on the PC from measured references (driver provider and version, a KB number or stop
code, the memory type and size, the PC maker and model), restricted to fixed domains, at most five
results, never `include_answer` or page content. Results are cited `[W1]`, `[W2]`, are not
measurements of the PC, and their prices and versions are checked against their snippets.
Details: [web-search.md](web-search.md).

## Related documents

- [gui.md](gui.md): the desktop app, its views and its validation history.
- [selection.md](selection.md): Explain and Investigate from a selected item.
- [answer-verification.md](answer-verification.md): what the verifier covers and does not.
- [telemetry.md](telemetry.md), [system-inspection.md](system-inspection.md),
  [storage.md](storage.md), [policy.md](policy.md), [actions.md](actions.md): collectors and
  effects.
- [validation.md](validation.md): how the project is validated, and what is not yet.
- Superseded design records, kept for their history: [backend-phase1.md](backend-phase1.md),
  [backend-delivery.md](backend-delivery.md), [backend-completion.md](backend-completion.md) and
  the [archive](archive/README.md).

## External references

Consulted on 25 September 2026 for the first collectors and the Nebius client; they document
interfaces, not measured results for this project.

- Microsoft, [PERFORMANCE_INFORMATION](https://learn.microsoft.com/en-us/windows/win32/api/psapi/ns-psapi-performance_information)
  (system memory and commit, `K32GetPerformanceInfo`).
- Microsoft, [GetProcessMemoryInfo and access rights](https://learn.microsoft.com/en-us/windows/win32/api/psapi/nf-psapi-getprocessmemoryinfo)
  and [PROCESS_MEMORY_COUNTERS_EX](https://learn.microsoft.com/en-us/windows/win32/api/psapi/ns-psapi-process_memory_counters_ex)
  (the per-process fallback).
- Microsoft, [GetSystemTimes and the processor-group limitation](https://learn.microsoft.com/en-us/windows/win32/api/processthreadsapi/nf-processthreadsapi-getsystemtimes).
- Nebius, [client-side function calling](https://docs.tokenfactory.nebius.com/ai-models-inference/function-calling)
  and the [account model catalog](https://docs.tokenfactory.nebius.com/api-reference/models/list-models).
