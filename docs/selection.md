# Selected evidence — backend contract

This document describes the phase-2 selected-evidence flow for **Explain** and **Investigate**,
displayed as **Summarize these measurements** and **Find the cause**.
The corresponding C3 addition is recorded separately in plan PR #34 before exposing these
commands. Existing `send_message`, `cloud_preview` and conversation events retain their contracts.

## Interface and conversation

**Summarize these measurements** keeps the current data view open and shows the answer in its
inspector. It reuses the open conversation when one exists, or creates one otherwise. The generated
question is displayed as a compact selection label rather than an ordinary typed message. The
answer still belongs to the saved conversation and uses the existing turn IDs, streaming events,
cancellation and verification.

The inspector's **Find the cause** action continues that summary's conversation in Chat. It
prepares a new `investigate` preview using the same selected reference and the summary's
`conversation_id`; the consumed `explain` ticket cannot be reused. The direct **Find the cause**
action opens a conversation focused on the selected measurements. Display metadata for selection
labels stays local; no command or event contract changes for this presentation.

An expired or invalidated reference stops either action with a visible explanation. History events
and finished recordings never fall back to a generic typed question when their capture is missing:
the person must refresh the view and select the measurements again. A new preview must still
match the conversation, sharing choices and model settings at send time.

## Local reference and source registry

A selection is a closed object, never a caller-supplied observation, name, PID, path or payload:

```json
{
  "source_id": "selection-source-<random UUID>",
  "target": {"kind": "resource", "resource": "memory"}
}
```

The other target variants are `process {entity_ref}`, `event {event_id}`,
`recording {recording_id}`, `storage {scope_id, item_ref}`, `watch_alert {alert_id}` and
`elevated_inspection {inspection_id}`. Resource names are the finite
`memory`, `cpu`, `disk`, `gpu`, `thermal_power` and `battery` set. References are resolved
exclusively against host-owned, already measured snapshots, the latest live battery read, process
activity, persisted history and the current completed authorized storage index. A battery
selection freezes one live battery read: the C4 observations of the metrics the window
displays (a partial value it marks as not reported included), only for the system batteries it
shows, under their opaque `battery-<uuid>` references,
with the read's window and fixed coverage text and no names (the metric list is in
[battery.md](battery.md#live-display)). Explain never reads the battery again, and
`thermal_power` stays limited to GPU measurements. Unknown fields and invalid references are
rejected.

The source registry issues random source references when a local view publishes measurements.
`selection_sources()` returns the current locally displayable targets, their source reference and
measurement dates. It does not collect, access the network or authorize an action. A source must
still be present when a preview is prepared and when that preview is consumed. New live captures
receive new references and never rewrite a frozen capture while the user reads consent. A process
collector/session restart, deleting history, clearing recordings or replacing/revoking an
authorized storage scan invalidates the corresponding sources and consent tickets. Sources and
tickets have a five-minute monotonic lifetime; wall-clock rollback cannot extend consent. A saved
administrator inspection is also refused once 24 hours have passed since its measurement, and a
watch alert once it leaves the seven-day history; `elevation_select` and `watch_alert_detail`
publish them again while they are still available, and deleting a watch revokes its alerts'
sources and tickets. The
registry retains at most 96 sources (the dashboard, process activity and battery publish about 24
a minute), 16 MiB of serialized source data, 2 MiB per source and 16
pending tickets. Eviction can expire a capture sooner than five minutes. Cloud projections are
limited to 128 KiB; oversized selections are rejected before any provider request.

The 6 October complements of C3 ([plan section 3](plan.md)) add two publishers. Any folder or
file of the latest completed scan (map, tree or list) is published by
`scan_item_facts(scope_id, item_ref)`, which returns the `selection_source_id` of a source freezing
the item at its rank among the same-kind items of its parent folder, with its item facts. The page
uses exactly that source for Explain and Investigate, never another source with the same `storage`
target (a `scan_page` source carries no facts), and calls `scan_item_facts` again when the source
expired or a new scan revoked it. A finished recording is published by `battery_recording` or
`recording_detail(recording_id)`; the page never asks for the detail of a running recording.

## Preview and execution

The additive commands are:

| Command | Result |
| --- | --- |
| `selection_sources()` | Host-issued local selection references and measurement dates. |
| `selection_preview(reference, intent, question?, conversation_id?, names)` | Single-use preview ID, exact first model data message, frozen selected tool payload, measurement dates/age and mode. |
| `selection_send(reference, intent, question?, conversation_id?, preview_id)` | Existing `{conversation_id, turn_id}` start result and existing `turn:*` events. |

`intent` is exactly `explain` or `investigate`. The optional question is bounded by the existing
question byte limit. When absent, the backend supplies a question about the selected target; it
supplies no explanatory answer. The approved question, target, source, intent, prior conversation,
sharing mode and frozen privacy projection must all match the preview. A failed check consumes
no network call. Successful consumption is single-use. Local mode remains available as explicitly
labelled local evidence; it must not claim to be an LLM explanation. `selection_preview` also
accepts the optional existing `file_names` consent choice for its finite name allowlist.

**Explain** runs the existing analyst and C1 verifier with only the read-only
`get_selected_evidence()` tool. The frozen measurements, source dates, windows, coverage and
limitations already arrive in the first message as `initial_observation` (`O0`, with display
values and `citable_observation_ids`). They count as the turn's inspection, so the analyst can
answer in its first request. `get_selected_evidence()` returns the same evidence again, as a short
`same_result_as: "O0"` reply. It starts no collector, scan, recording or action proposal.
Reading a retained capture with unavailable sensors still completes this inspection: the analyst
may explain insufficient evidence, while all missing values and original sensor statuses remain
unchanged. Such a read cannot certify a numerical claim.

**Investigate** opens or follows a normal conversation, initially scoped to the selected evidence,
with `get_selected_evidence()` and the normal read-only investigation tools. Fresh measurement is
permitted only after matching consent. Actions remain proposals requiring the existing separate
local approval. A selected storage item authorizes only its already authorized scope; an event or
recording authorizes no storage scope.

## Evidence, freshness and privacy

The first data message contains an explicit selected-evidence envelope. The same builder is used
for preview and actual execution. Local source IDs, event IDs, recording IDs, snapshot IDs,
session/boot IDs, PIDs, creation times and absolute paths never enter it. Opaque process references
and storage aliases retain their measured identity; their names follow C5. Storage names require
the existing finite file-name consent and are otherwise aliases.

Every numeric observation retains its metric, unit, status, original source and measurement window.
Age is computed once for the frozen preview. Recent cached data are still labelled already
measured. Every selected capture precedes the selected turn and cannot verify current-state claims;
historical data never become a fresh collection. Unknown timestamps and clock rollback
are explicit. Re-reading stored data changes neither its observation date nor its measurement
window. Missing values remain unknown.

An activity event's measured peak and separately measured contributor are evidence; its episode
boundaries are not a measurement window and its contributor is not a proven cause. Legacy events
without peak metadata remain coverage/context only. Recordings reuse their stored points, extrema,
gaps and battery evolution; Explain never starts or extends a recording. Bounded omission counts
and response limits remain explicit. Storage selection is bound to the scan identity, not merely
an item alias that another scan could reuse.

A storage item source may carry `item_facts`: the cutoff (scan end minus 182 days), the logical
and allocated bytes and the files not modified since then, the undated files, the closed
extension-based file types with their files and bytes, the largest file under its alias, status
and limitations, with `volume_total_bytes` in the same object so a share of the drive can be
checked. Byte fields end in `_bytes` and counts in `_count`, which the C1 verifier reads. The
frozen page states `rank_scope: "siblings_of_parent"` with `parent_ref` and the parent's alias, so
its offset and total are a rank among siblings, not in the whole scan. File types leave as
nameless totals, and the largest file leaves under its alias in every sharing mode, even with
file-name consent; its name and path stay in the local inspector. The facts describe the end of
the scan: a figure written in the present tense is reported as historical wording.

Selected evidence enters the normal observation/citation path. The analyst writes the answer;
the verifier checks numbers, names and citations against the actual frozen and subsequent tool
results. The controller does not substitute locally generated explanatory prose.

## Host integration

The portable modules are `crates/agent/src/selection.rs` and `apps/gui/src/selection.rs`.
`FrozenSelection::freeze` resolves a typed target against a host-owned source. Its first-message
builder is shared by the preview and `converse_with_selection`, including O0 and historical ages.
`SelectedTools` delegates nothing in Explain, including an attempted unregistered collector call.
The GUI registry binds the immutable source, exact question/history/sharing and model context
revision in one single-use ticket. Its settings revision is independent of ordinary live captures
and cloud-preview refreshes; those refreshes do not revoke an earlier valid selection. Historical
reads in progress cannot republish evidence after local history deletion. Selected storage cloud references use aliases; real scope and
scan references remain local. The preview also exposes the finite file-name allowlist used by
subsequent Investigate tools.
Conversation deletion revokes both ticket stores while holding the history lock. Preview publication
and ticket consumption verify the current conversation under that same lock; the legacy asynchronous
preview rechecks it after its collector wait. A deletion after consumption never creates a replacement
conversation containing the removed context. Model policy remains locked through ticket consumption,
with ticket storage acquired before routing, so a concurrent setting change cannot approve stale policy.
The app registers sources in its existing measurement/history/scan read paths, invalidates them
when the corresponding state is replaced or deleted, and uses the existing conversation worker
and event protocol for approved selected turns. The interface chooses where the answer appears;
the backend persists the turn independently of the currently displayed view.

Validation uses synthetic snapshots, persisted-history fixtures and scripted models. It covers
invalid/stale references, conversation-context revocation, process-session identity changes, scan replacement, unknown historical metadata,
immutable C5 projection, single-use/mismatched consent, forbidden collectors in Explain and
citation verification. Live paid provider calls and real user actions are unnecessary.
The scripted-model test compares the actual first message with the reviewed preview, verifies
the selected CPU measurement against O1 and forbids the native collector. Store fixtures preserve
recording/event dates and reject legacy peaks or saved values presented as current measurements.

## Local routing configuration

`model_routing_get` returns model policy and calibration availability; `model_routing_set` stores
only the closed `RoutingSettings` object in `model-routing.json` beside the existing app settings.
`automatic_available` is true when automatic mode is in use with its valid calibration or, in any
other mode, when the calibration file is valid for the Super and Ultra models the window lists (the
exact settings its Auto choice saves), so the window offers Auto exactly when `model_routing_set`
would accept it. `model_routing_get` reads that file again for this, without changing the policy
in use, its revision or any frozen preview; `calibration_reviewed_at_unix_ms` and
`calibrated_rules` describe the same calibration.
The default is `configured`, preserving the explicitly configured Nemotron model. The other modes
are `always_super`, `always_ultra` and `automatic`; no key or endpoint is accepted by these commands.
Automatic mode requires a validated, human-reviewed campaign in the fixed local
`nemotron-routing-calibration.json` file. Missing, synthetic, malformed or unsupported calibration
cannot enable escalation. Calibration schema v2 binds the original tool registry, analyst/verifier
and replay harness identity. Missing or outdated identities are rejected by the shared routing
validator; the loader never stamps old evidence with the current identity. On startup a rejected
automatic policy falls back to `configured` with a visible limitation. No quality labels are
invented by this implementation.

Both legacy and selected previews freeze routing settings, the configured model ID, calibration
and a local revision. Sending rechecks the policy and configured model; the worker checks the model
again before constructing its transport. A changed setting requires a new preview. Model changes
produce an additive `model_changed` step; usage includes per-request model identity and cost, with
the aggregate cost unknown when any request cost is unknown. Existing event fields are preserved.
The window's `usage` also groups the requests by model (requests, tokens, an exact cost only when
all of that model's requests are priced, and the dated price) and gives the total, or without one
what the priced requests cost at least; the legacy single `cost` stays absent after a change. The
CLI text report prints the change and each model's cost the same way ([gui.md](gui.md#model-routing-super-then-ultra)).

Explain reads frozen evidence without constructing a native collector, requesting helper work or
creating action proposals. Investigate shares the existing worker, cancellation, extensions,
conversation persistence and action/job cleanup. It can read results of previously approved
helper jobs from the bounded local cache; reading that cache never opens a UAC prompt.
