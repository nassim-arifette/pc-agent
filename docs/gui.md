# Desktop app (`apps/gui`, package `pcagent-gui`)

A Tauri 2 window over the same Rust core as the `pcagent` CLI (spec 6). Every measurement,
finding, scan and Nebius request goes through the crates the CLI uses; the window adds no
diagnostic logic of its own.

## Layout

One window after the hybrid direction chosen on 2026-10-01 (chat first, context on demand), with
the rail of the phase 2 desktop mockups:

- **Rail**: *Chat*, *History*, *Live PC*, *Storage* and, at the bottom, *Settings* with a dot for
  the AI status.
- **Sidebar**: on the chat, the conversation list: *New conversation*, a search box, saved
  conversations grouped by day, then the model with the current sharing choice. A view that
  declares a [view bar](#views-and-the-view-bar) shows it in the same place, above the same model
  status; the other views use the full width. It folds away (button or narrow window, where it
  floats over the view).
- **Conversation**: a welcome screen with the composer and four suggested questions (one names a
  drive that is almost full), then the thread. While a run is going, its real steps appear with
  their durations and Stop replaces Send. Nebius replies are streamed: the answer appears as the
  model writes it, marked as a draft whose figures are checked when it finishes, then the reviewed
  answer replaces it. An answer shows the model's text with numbered citation
  chips, the model, duration and cost, the steps and cited measurements, follow-up questions, Copy,
  Ask again, and whether every figure was checked against the measurements. A local analysis shows
  the rules' best finding and offers to ask the AI.
- **Context panel**, closed by default. It opens on the right tab from what the person is looking
  at: a citation chip (*Sources*, with that measurement highlighted), the *PC status* pill (*Live
  PC*), *Analyze a folder* or *Storage* (*Storage*). *Sources* also keeps each answer's limits and
  its requests, tokens and price. *Live PC* shows memory, processor, drives, graphics cards and
  each system battery, refreshes every 5 s while the window is visible (pause button) and draws
  short sparklines. Below them, programs are grouped by application
  (`process_activity`, a 2-second window measured every 10 s only while the tab is visible), with
  their own sums, sortable by memory or processor; opening a group shows its services and its
  processes with their own values, so a group is never mistaken for one process. Below 1100 px the panel floats over the conversation.

The *PC status* pill in the top bar names the most important thing in a few words: a system
battery Windows flags as critical while unplugged (Windows' own flag, no percentage threshold), a
drive under 5 % or 5 GiB free, memory at 90 % or more, a processor at 90 % or more, or a slowed
graphics card. These thresholds are hints for the status line, not findings. A battery alert does
not hide the welcome screen's suggestion about an almost-full drive, and a battery read older than
about 30 s never raises it.

The full *Storage* and *Settings* screens open in the conversation area. Storage picks or drops a
folder, follows the scan and lists the largest folders and files, each of which can be selected
to explain it or investigate (see the action bar below); nothing is deleted there.

## Live PC, selection, Explain and Investigate

The *Live PC* view (`#/live`) shows the resources as selectable cards (memory, processor, each
fixed drive, each graphics card, each system battery) and the applications as selectable rows. Selecting one shows the
**action bar** right under it and fills the inspector on the right with its details:

- **Explain** uses only the measurements already taken for that item: nothing new is collected and
  no action is proposed (`get_selected_evidence` only).
- **Investigate** starts from them with the usual read-only tools; actions stay proposals.
- **Monitor** is shown disabled with *Coming soon*: watching a resource against a threshold does
  not exist yet, and the bar does not pretend it does.

The action bar (`ui/js/actionbar.js`) is the same in Live PC, History and Storage: a floating
toolbar (`role="toolbar"`) under the selected item, one tab stop with the arrow keys between its
enabled buttons, and the *E · Enter* hint. Only one item is selected at a time; selecting another
moves the bar, and Escape or a second click on the item clears it. Near the right edge it aligns to
the item's end, near the bottom of the window it opens above the item; History puts it under the
right end of its full-width rows. While an item is selected and the focus is not in a text field,
`E` runs Explain and `Enter` runs Investigate (Enter keeps its own meaning on any other button).
Without a host-issued reference yet, Explain and Investigate are disabled and their tooltip says
why. The inspector keeps what the actions use and an *Investigate to go further* link; a process
chosen among an application's members keeps the bar under that application's row, and the bar then
acts on the process the inspector names.

Each system battery has its own card (*Battery*, or *Battery 1* and *Battery 2*; batteries are
never summed): its charge, its state with the power (*Discharging at 8.4 W*), and while
discharging the time left, labelled *per Windows* for the driver's estimate or *at this rate* for
the one calculated from the energy left and the power. The sparkline is the charge over the reads
this window made. The inspector shows the signed power (positive while charging), both estimates
under their own labels, the energy left, the full-charge capacity against the design capacity
with the estimated wear (an estimate, not a diagnosis), the charge cycles and the power over the
span the window's reads actually cover. A battery in relative units shows its percentage, state,
Windows' estimate and cycles only. Every value Windows did not report reads *Not reported*, never
zero. Values older than about 30 s are never shown as current (the card would show their age). A
desktop PC shows no battery card; a battery Windows lists but cannot read, or whose reads stopped
finishing, shows one *Not reported* card that says so and cannot be selected. Explain and
Investigate use the battery read's own source (`battery_source_id`, resource `battery`), which
freezes that read without reading the battery again. It holds only the batteries shown and the
metrics the window displays for them (a partial value shown as not reported included), under their opaque `battery-<uuid>` references, with the
read's window and coverage ([battery.md](battery.md#live-display)). One target covers every
battery shown, so with two batteries the readings of every battery shown are sent. Applications' battery use is never shown here.

The page never sends measurements or names itself. It passes a host-issued reference
(`selection_source_id` of the dashboard or of `process_activity`, or `battery_source_id`, plus a
closed target) to
`selection_preview`, which freezes that capture and returns the exact first message and a
single-use ticket; `selection_send` must match it exactly ([selection.md](selection.md)). No
dialog stands before the send (C5, 9 October): the turn starts at once and its steps keep the exact
first message (*Message sent to Nebius*). The question is written in
the interface language and follows C5 like the data: never a PID, a program name only in complete
sharing. An application is explained through its largest measured process, which the inspector
names. History events and finished recordings use the same flow through `selection_sources()`;
an expired source refuses the action and asks for a refresh, without starting a generic inquiry.

*Summarize these measurements* keeps the data view open and writes its draft and final answer in
the right-hand inspector, with citations, verification, Stop, errors and the exact sent message.
It appends to the current conversation when one exists. *Find the cause* opens the Chat with a
fresh Investigate ticket in that same conversation. From the summary it uses the summary's own
conversation and frozen reference, even after another conversation was opened. An expired
reference or deleted conversation refuses the continuation. The eventual cost is shown when
reported; no numerical estimate is invented before an investigation.

Selection requests appear as compact labels, separate from typed user questions. Their local
display metadata (intent, label and conversation/turn IDs) stays in WebView storage, bounded to
500 entries and 90 days, and is removed when the corresponding conversation or history is deleted.
It never enters model context. Old reports without this metadata retain their original display.
A running turn reattaches in the Chat after a reload, with its Stop control; an inspector result
remains readable as an ordinary saved turn there.

## Storage selection

Each page of scan results the host returns is published as a selection source
(`retain_storage_page`): the page carries its `selection_source_id` and the target of an item is
`{ kind: "storage", scope_id, item_ref }`. Selecting a folder or a file of the results shows the
action bar. The question and the preview name it generically (*the folder chosen in the scan*): it
leaves under an alias, its name and path staying on this PC; file names are not offered from here. Sources expire after five minutes (or earlier when newer captures fill
the registry): before a preview, the page looks the item up in `selection_sources()` and, when its
source is gone, reads the same page of the same scan again with `scan_page`, which publishes it
anew. A new scan revokes the previous one's sources; a page the host could not publish leaves the
bar disabled with *scan the folder again*.

## Quick drive analysis

The Storage view also offers a *Quick drive analysis* of a fixed NTFS drive. Its drive menu lists
the fixed volumes of the live measurements whose file system is NTFS (`volumes[].kind` and
`filesystem` of `dashboard`; a volume whose file system could not be read stays listed, the helper
checks again), C: before the first measurement; on a PC without one the card says so and offers
nothing to start. It runs the closed
`mft_inventory` task of the separate elevated helper (`pcagent-elevated`, no network, no model): the
page calls `elevation_prepare` with the drive letter and the 120 s budget, shows exactly what will
happen, and only the local *Allow and start* click calls `elevation_run`, after which Windows asks
for administrator consent. The helper reads the `$MFT` in bulk (see [mft.md](mft.md)): names, folders
and sizes, never file contents, and changes nothing. The result stays in the window: files examined,
supported data measured, the share of the index walked (partial when a bound is reached) and the
largest attributed folders and files (`local_mft_tree`), whose parent/child sizes overlap. *Stop*
calls `elevation_cancel`; after an error or a stop the page waits for `elevation_status` to return
`null` before offering another analysis, and a reload recovers a prepared or running task.

## Deeper checks

Below it, a *Deeper checks* card offers four other closed tasks of the same helper, all without
arguments: *Windows caches* (`system_caches`: the size of `Windows.old`, downloaded updates and the
Delivery Optimization cache), *Restore points and shadow copies* (`shadow_storage`, from
`Win32_ShadowStorage`), *Disk health* (`disk_reliability`: temperature, wear, hours powered on,
errors and slowest responses as each disk reports them) and *Battery use by app*
(`battery_energy`: Windows' historical energy estimates on battery, for the current account). The
flow is the quick analysis's: *Prepare* calls `elevation_prepare` with the 120 s budget (which also
counts the time the Windows consent prompt stays open), the review says exactly what the check
reads, that it changes nothing and that Windows will ask for consent, and only *Allow and start*
calls `elevation_run`. The result is drawn generically from the inspection (`adminchecks.js`):
observations grouped by entity, a label per metric, values formatted by unit, a value the source
did not give shown as not reported, and incomplete coverage under *Limits of this check*. For five
minutes after it completes, the chat's AI answers can read the result through
`get_approved_elevated_inspection`, with the sharing choice applied; the card says so and offers to
ask about it. The two cards share the helper's single slot: while one holds it (prepared, running
or cleaning up), the other's buttons are disabled with a hint. Process images and the performance
trace are not offered here. On a PC where Windows lists no system battery, *Battery use by app*
stays listed with its *Prepare* button disabled and the reason in place of its description.

## What this PC offers

`app_info` carries `capabilities`, the part of the agent's capability registry
([tool-context.md](tool-context.md#declared-tools-the-capability-registry)) that describes this
PC and installation: `windows`, `history_sampler`, `elevated_helper` (`pcagent-elevated.exe` is
next to the app, checked once at start), `nvidia_gpu`, `battery` (a system battery, from the
live battery read) and `ntfs_volumes` (a fixed NTFS volume, from the live volumes). Each is
`true`, `false` or `null` while its probe has not answered yet; the hardware probes answer with
the first live measurements, so `ui/js/capabilities.js` reads `app_info` again after a live
measurement while one is `null` (at most twelve times) and notifies `info` when they change. Like
the model's catalog, the page counts an unknown capability as present. The capabilities of a turn
(an approved folder, web lookups, action proposals) are decided when it starts and are not listed.
The same probes restrict the tools of every turn. Without the helper, the Storage view shows a
note instead of the quick drive analysis and the deeper checks, and the chat offers no deeper-check
card under an answer.

## History, recording and startup

The *History* view reads the local sampler (`history_status`, `metric_history`): processor,
memory, disk activity and graphics card over the last hour, 24 hours or 7 days, as four charts on
one time axis. Unmeasured periods (PC Agent closed, PC asleep) stay breaks in the lines and are
counted, never drawn through. Detected anomalies are listed with their peak, duration and the
program measured at the peak, which is not presented as the proven cause (a selected event says
so). Selecting an event shows the action bar, whose *Explain* and *Investigate* send that event
through the selected-evidence preview.

*Record a problem* takes one measurement per second for 30, 60 or 120 seconds
(`record_activity_start`), with a countdown and *Stop* (`record_activity_cancel`, which keeps the
measurements); the finished recording is then selected like an event, and the action bar's
*Explain* or *Investigate* sends that exact recording. When the PC reported a
battery during it, the finished recording also shows the battery's charge at the start and end, the
last measured rate, the loss measured between capacity readings (with its average power and the time
left at that rate) and Windows' own estimate, read once from the local store
(`battery_recording`) without collecting again. The *Collection* card
shows the sampler's measured processor cost against its target and the whole window's memory,
the optional *Start with Windows* setting (`startup_get`, `startup_set`, off by default) and
*Clear the history* (`history_clear`), which keeps conversations.

## Action proposals

In a cloud turn, the model may propose actions with `prepare_action_plan`; it cannot run them.
The proposals of the last finished turn wait on the Rust side (`action_plans`) and appear as cards
under the answer, each saying what it would do, how many files and bytes, and how to recover
(Recycle Bin, reopen the program, regenerated cache). *Review and confirm* lists every exact
target; *Run now* calls `action_run` for that one plan, which leaves the waiting list first so it
cannot run twice. The same broker, guard, protection policy and journal as the CLI revalidate each
target just before running and refuse anything changed or protected; the result reports what was
done and the free space or system measurements before and after. *Ignore* calls
`action_decline`. A newer turn replaces older proposals, which can no longer run.

A start-up entry turned off from here offers *Restore*: `startup_restore_prepare` reads that
action's local journal and prepares a fresh restoration plan, which opens in the same review and
runs only through *Run now* (`action_run`). Preparing it replaces the proposals still waiting, so the
button stays disabled until they are decided or ignored. The plan identifiers are known only in the
session that ran the action; afterwards the CLI's `restore-startup` remains the way back.

## Visual system

Warm neutrals, a single accent (*Encre* `#3B5BDB`, `#8FA2F5` in dark), Segoe UI Variable and 1 px
strokes rather than shadows, close to Windows 11. Colours are tokens in `ui/styles/tokens.css`,
light and dark. Iris, the former mascot, is reduced to a mark (`ui/js/brand.js`): an accent tile
with two eyes that signs answers and never animates on its own. The design exploration and the
clickable prototype live on a private design canvas; the decisions are recorded in
[the plan](plan.md).

## Sharing and privacy

Decided on 9 October (plan, section 7 quater; C5): no dialog stands before a send, not even the
first one. What leaves is shown and changed under the composer, and remembered by the Rust side
(`privacy_get`, `privacy_set`).

- **Toggles under the composer**, pressed while their part leaves:
  - *Names*: program names (complete sharing) or opaque references (anonymized);
  - *Web*: lookups through Tavily, on by default; with no Tavily key it is dashed and opens Settings;
  - the scanned folder (its name), shown after a scan: it goes with the questions until detached;
  - *File names*, shown while a folder is attached in complete sharing: the finite list of names
    and folder-relative paths (`%SCOPE%\...`), never file contents or paths outside the folder.
- **See what leaves** (the eye) opens the exact first message for the text typed, or a sample
  question when the box is empty: the question as sent after identities, paths and secrets were
  removed, what is sent and never sent, the file names, and the JSON. Nothing is sent from there.
- Each cloud question needs a frozen single-use preview ticket. When *See what leaves* was opened
  for the typed question, the next send reuses that exact preview if the question and settings
  still match; otherwise it prepares a fresh one. An expired or invalidated ticket refuses the
  send. The turn's steps keep that message (*Message sent to Nebius*, this session only), including
  failed model requests. `confirmed` is still stored after a first send but no longer required.
- Sharing changes are saved in order, and a send waits for them. Changes while its preview is
  being prepared cancel the send; the question stays in the composer. Only one send can prepare
  at a time, including selection actions. Changing file-name sharing invalidates host tickets too.
- **Local analysis** (on this PC only) is chosen in Settings, or used when no key is set up; the
  composer then shows a *On this PC* pill, which goes back to the AI in one click when it is ready.
- Settings keeps the three modes as cards and the file-names toggle; web search keeps its own card.
- When the budget runs out, the window asks before extending it and shows the cost so far.
- The key is written to `%APPDATA%\pcagent\.env` (the CLI's file) by the Rust side and never sent
  back to the page, which only sees whether a key exists and its last four characters.

## First launch and Nebius setup

An unconfigured profile with no saved setup decision opens `#/setup` on an ordinary launch.
An explicit hash route, a turn restored after a reload, and a profile whose key/model are already
configured keep their current destination. Settings always offers **Open setup guide**, including
after skipping, finishing or choosing local mode. The guide does not reset existing credentials,
reasoning, routing or privacy choices.

The guide reuses the Settings AI and privacy cards, in English and French and the current theme:

- **Open Nebius Token Factory** calls `open_nebius_console`, a fixed-address browser launch of
  `https://tokenfactory.nebius.com/`. The command accepts no URL, reads no key and makes no account.
- The key is typed in a password field and sent once to the existing `save_cloud_settings`
  command. A saved key is never read back, only its configured flag and existing four-character
  hint. The draft is cleared after a successful save or closing the view; it is not stored in
  browser preferences, logs, diagnostics or previews.
- **Load account models** saves pending form changes, then calls `cloud_models` explicitly.
  Until then, suggestions are not an assertion of account availability. The account response
  replaces the suggestions, excluding models that explicitly lack tool support. The current ID
  remains visible as a custom choice if absent; no fallback model is silently selected or saved.
- **Test connection** also saves pending changes, then calls the existing `test_cloud`. Both
  operations read catalog metadata with a 10-second backend timeout. Neither starts an inference,
  sends a question or PC data, or grants diagnostic/UAC consent. Errors use closed, localized
  categories, never arbitrary exception/provider text. Checks are serialized and invalidated on
  edits; changing a key also discards the old catalog. Missing key/model, denied access,
  unavailable model, network, timeout, rate limits and an empty catalog have explicit messages.
- **Continue locally without AI** first persists local privacy mode. A failed write leaves the
  guide open. **Skip for now** and **Finish setup** do not change privacy or delete credentials.
  Finishing requires a configured key/model, not a mandatory network test. None starts a question.
- Only the progress marker (`skipped`, `complete`, `local`, or `existing`) is added to
  `pcagent.prefs.v1`. If localStorage is unavailable, the choice lasts for this window only and
  a later launch may offer the guide again. Privacy remains owned by Rust, not this marker.

Run the no-dependency logic/IPC-fixture suite from the repository root:

```powershell
node --experimental-default-type=module --test apps/gui/dev/onboarding.test.mjs apps/gui/dev/storage.test.mjs
```

These are synthetic tests, not browser or live-service validation. For manual browser QA, use a
fresh browser profile/storage with `?cloud=none`, or explicitly open `#/setup`. The mock supports
`?connection=key|network|timeout|model|limit|empty` alongside `cloud`; it never contacts Nebius or
opens its website. Use only dummy keys in the mock. Check keyboard focus, both languages/themes,
skip/reload/resume, saved profiles, empty/error catalogs and clearing the password on leaving.

Native WebView2 adds its own password-reveal control after typing, even if the app provides no
reveal button. The Nebius field suppresses it with `::-ms-reveal`. Before entering a real key in
native QA, verify this with an unsaved dummy value; a Chrome mock alone cannot prove it.
Also verify that the official-console click actually opens the page, not just that IPC succeeds:
the existing `explorer.exe URL` transport can fail in an isolated secondary-logon desktop even
when opening HTTPS normally works. Confirm this in a fully interactive standard-user session.

## Web search

Typed web lookups through Tavily (contract C6): `lookup_driver_notes`, `lookup_known_issue`,
`lookup_upgrade_price` and `lookup_pc_specs`. The model never writes the query; the Rust side
composes it from measurements.

- **Composer and Settings**, *Web*: a remembered toggle, on by default (`web_search` in
  `privacy_get`/`privacy_set`). The Settings card says what a lookup sends (a short query from the
  hardware model, a driver version, an update number or an error code, a program or module name
  only in complete sharing, the country for prices) and never sends (the question, text written by
  the model, the user name, paths, command lines, file contents, serial numbers and identifiers).
  The switch works only in a cloud mode with a saved key; otherwise it is disabled and says why.
- `privacy_set` always receives the whole object, `web_search` and `file_names` included: Rust turns
  a missing toggle on, so a call that left one out could turn it back on.
- **The Tavily key** is optional; everything else works without it. `save_search_key` stores it in
  `%APPDATA%\pcagent\.env` beside the Nebius key and refuses keys that do not start with `tvly-`
  (sending `null` removes it). The page sees only `search_key_configured` and the last four
  characters (`search_key_hint`).
- **See what leaves**: when lookups can run, a line says that queries may also go to Tavily and
  appear in the steps.
- **Steps**: `tool_finished` carries the exact `query` of a lookup, shown under its step as *Sent to
  Tavily*. A reopened conversation shows the tool steps its turns recorded (`tool_events`), with
  their queries.
- **Citations**: `[W1]` chips carry a globe and an outline, unlike the filled chips of
  measurements. The source card shows the page title, the site, the publication date (*date
  unknown* when Tavily gives none), when it was read, the snippet, the query, and a note that a web
  page is not a measurement of this PC. *Open in browser* calls `open_web_source` with the
  conversation, turn and reference; Rust reads the address from the stored turn and opens only an
  https page. The page never renders the address, links it or opens it.
- In the mock, `?web=on` saves a synthetic key and turns web search on, so an AI investigation also
  calls `get_drivers` and `lookup_driver_notes` and the answer cites two web pages (one without a
  date); `?web=key` saves a key with web search off. `open_web_source` is recorded in
  `window.__pcagentMock.openedWebSources`, never navigated to. The tour captures the settings card,
  the composer with *Web* on, the query under its step and an open web source (`13` to `16`).

## Turns and events

A question and its answer form a turn. `send_message` returns a stable `turn_id`; the window
follows `turn:step`, `turn:delta` (the whole draft so far, replaced by the verified turn),
`turn:extension`, `turn:done` and `turn:failed`, and stops a turn with `stop_turn`. The Rust side
still emits the legacy `inv:*` events for older clients; this window does not listen to them, so
nothing is drawn twice. Failed turns are kept in the history and marked in the sidebar.

A window that reloads while a turn runs asks `current_turn` at start and reattaches it: the
question, Stop and an extension request still waiting for an answer come back; earlier steps are
not shown again. Without it the turn ran unseen, could wait forever for an answer and blocked
every new question. A saved conversation whose only question never got an answer (the window
closed mid-run) shows the question, *This question has no answer* and *Ask again*, never a blank
page. Mock switches: `?turn=orphan` and `?conversation=empty`.

## Model routing: Super, then Ultra

Chantier 14 and the routing complement of C3 (4 October). The routing mechanism and its
calibrated triggers belong to the engine ([selection.md](selection.md#local-routing-configuration));
the window only shows what happened and what it cost.

- **Choosing.** The composer's model menu offers *Auto*, then the models. *Auto* can be picked only
  when `model_routing_get` reports `automatic_available`: Auto is on with a valid calibration, or a
  reviewed calibration in the fixed local file matches the listed Super and Ultra models, which is
  exactly when `model_routing_set` accepts it (the file is read again each time the menu or Settings
  opens). Otherwise its note says it becomes available once the evaluation is reviewed. An Auto
  choice saved earlier and rejected at start-up (outdated calibration, malformed file) leaves the
  configured model in use: Settings says so under the model (the engine's reason, which exists in
  English only, is added on the English page) and the menu's *Auto* note points there. *Always
  Super* and *always Ultra* are not separate choices: picking *Nemotron 3 Super* or *Nemotron 3
  Ultra* in the menu gives exactly that behaviour (one model, no escalation). The `always_super` and
  `always_ultra` modes still work from a hand-written `model-routing.json`, and Settings says when
  one applies.
- **While a turn runs**, its header names the model in use (the configured model, or Super with
  Auto). On `model_changed` a step says that Ultra takes the question over with the measurements
  already taken, the header becomes *Nemotron 3 Super → Nemotron 3 Ultra*, and a draft already on
  screen stays, dimmed, until Ultra's text catches up. A window reloaded during an Auto turn cannot
  know whether the change already happened: the header says *Auto* until a step or the end of the
  turn tells.
- **Under the answer** a lasting line says that Ultra took the question over and why (Super's answer
  had low confidence, Super found too little evidence, Super's answer still had unverified passages
  after its corrections, or Super used its share of requests without answering). When Ultra wrote
  nothing it says so truthfully: its only request failed and the answer shown is Super's, its last
  request failed and the answer shown is the best one kept before the failure, the turn ended before
  it answered, or the person stopped the analysis (then no answer falls back to Super's). The header
  names the models that worked, in order, then the duration and the cost: the total of both models,
  or *≥* the priced requests when a request has no cost. A two-model cost is never one model's price
  applied to every token.
- **Sources** (*Requests and cost*) gives, after a change, one line per model: requests, tokens, cost
  and its dated Nebius price, then the total only when every request is priced (else *at least*).
  A turn of one model keeps the single cost and price lines.
- **Copy** adds the models, the change and the cost, with each model's own when there are two.
- **Reopened conversations** rebuild the switch from the saved report (*Nemotron 3 Ultra took the
  question over after 2 requests*), since the saved tool steps carry no request numbers; a run that
  reattached after a reload gets the switch back the same way when it ends.

The report's `usage` (`view::Usage`) keeps every earlier field and adds `models` (each model's
requests, tokens, exact cost only when all its requests are priced, and dated price),
`total_cost` (the display form of `total_cost_usd`, absent exactly when it is) and
`priced_cost_lower_bound` (what the priced requests cost at least, only without a total). Turns
saved before them keep their single cost. In the mock, `?routing=calibrated` offers Auto,
`?routing=escalated` turns Auto on and hands every AI answer from Super to Ultra,
`?routing=escalation_failed` makes Ultra's request time out (Super's answer stays, no total) and
`?routing=stale` shows an Auto choice rejected at start-up. The logic and these mock scenarios are
covered by `node --experimental-default-type=module --test apps/gui/dev/usage.test.mjs`.

## Architecture

```text
ui/ (HTML, CSS, JS modules, no build step) ──invoke──▶ src/app.rs (Tauri commands)
        ▲                                                   │
        └──────────── events (inv:*, scan:*) ◀──────────────┤
                                                            ▼
                          pcagent-telemetry · -agent · -storage · -policy · -actions
```

- `src/view.rs` turns core types into plain data for the page (raw value + text + status for every
  measurement). Portable and unit-tested.
- `src/session.rs` wraps the model and the toolbox so each real step is reported as it starts and
  ends, and so Stop refuses the next model request (`AgentError::Cancelled`). A request already sent
  cannot be recalled. Portable and unit-tested.
- `src/app.rs` (Windows only) holds the Tauri commands. One thread owns the live collector for the
  live measurements (and, every 30 s, the drives' free space); a second thread reads the battery
  (`pcagent_inspect::live_battery`, about every 10 s while measurements are polled), which the
  measurement thread never waits on. Each investigation and each scan runs on its own thread and
  reports through events.
  The Storage screen and investigations share one `StorageService`, so a completed scan can be
  reused by the model without rescanning.
- The page is untrusted input: commands validate every argument, paths and item references are
  resolved on the Rust side, and *Show in Explorer* uses the actions crate's argument sanitizer.
  The content security policy allows only the app's own files and Tauri's IPC. The only capability
  granted to the page is listening to events; the folder dialog is opened by a Rust command.

The interface is plain HTML, CSS and JavaScript modules served from `ui/` (no Node toolchain, no
npm dependencies), a deliberate departure from the React/TypeScript suggested in spec 15.1 to keep
the build to `cargo` and the supply chain small. Text is always inserted as text, never as HTML.

### Views and the view bar

`ui/js/main.js` mounts one view of `ui/js/views/` at a time with `mount(page, shell)`; the view
may return an unmount function. `shell` lends the view the sidebar slot that the chat uses for its
conversation list, and does nothing once the view is unmounted:

- `shell.setViewBar({ label, topics, render })` declares the view's bar. `label` is the i18n key
  of the sidebar landmark's name. `render()` returns the bar's content and is called once `mount`
  returns (at once when declared later), after each store topic listed in `topics`, and on
  `refreshViewBar()`. The shell
  adds the hide button above it and the model status below it.
- `shell.refreshViewBar()` redraws the bar after the view's own state changes.
- `shell.closeViewBar(focusTarget)` folds the bar after a pick on a narrow window, where it floats
  over the page, and moves focus to `focusTarget` (or the main region) so it never falls on an
  inert control.

A redraw keeps keyboard focus on the control with the same `data-key` (else the same position).
The bar is not a landmark of its own: the sidebar `aside` takes the view's label, so content inside
should not repeat it. Shared classes, all prefixed `viewbar-`: `viewbar-group`, `viewbar-label`,
`viewbar-item` (a button; `aria-current="true"` marks the shown choice) with `viewbar-item-icon`,
`viewbar-item-text`, `viewbar-item-title` and `viewbar-item-sub`, then `viewbar-hint` and
`viewbar-empty`. A view without a bar keeps the full width. The slot's width animates when the
route changes, so a view that measures its own layout listens for resizes instead of measuring
once at mount.

## Build and run

Windows 11 x64 with WebView2 (included in Windows 11), Rust 1.90 and the MSVC build tools:

```powershell
cargo run --release --locked -p pcagent-gui
```

The first build compiles Tauri and needs about 3 GB for `target\`. On other platforms the binary
only prints a message; the portable view and session code is still built and tested in CI. The
quick drive analysis and the deeper checks also need the helper next to the app
(`cargo build --release --locked -p pcagent-elevated`); without it the Storage view says so.
The CI keeps a `pcagent-gui-windows-x64` artifact: an unsigned prototype, not an installer.

Keyboard: `Ctrl+1` to `Ctrl+5` open Chat, History, Live PC, Storage and Settings, `Ctrl+N` starts a
new conversation, `Ctrl+K` jumps to the question box, `Ctrl+J` shows or hides the panel, `Enter`
sends and `Shift+Enter` adds a line, `Esc` closes a dialog or clears the selection. With an item
selected in Live PC, History or Storage, `E` explains it and `Enter` investigates it; the arrow
keys move between the action bar's buttons.

## Design review without the app

`dev/mock.js` answers every command with synthetic data, so the interface can be reviewed in a
browser:

```powershell
cd apps\gui
python -m http.server 5173 --bind 127.0.0.1
# open http://127.0.0.1:5173/ui/index.html (?cloud=none|problem, ?gpu=none, ?disk=ok,
# ?privacy=complete|anonymized|local for a confirmed choice, ?elevation=fail|slow,
# ?recording=short, ?battery=none|ac|charging|low|relative|two|unavailable for the live battery
# (none is a desktop PC, also without a battery in recordings), ?sources=short for selection
# sources that expire after 5 s, ?pc=desktop for a desktop without an NVIDIA GPU or a battery whose
# D: drive is exFAT, ?ntfs=none for no fixed NTFS drive, ?helper=missing for no pcagent-elevated)
# sources that expire after 5 s, ?routing=calibrated|escalated|escalation_failed|stale for model
# routing, see "Model routing" above)
```

Questions containing `extend`, `fail` or `slow` exercise the extension prompt, an unusable model
answer and a long run; `space` or `clean` proposes cleanup actions and `startup` a start-up entry
to turn off and restore; `health` (or `santé`) ends with the deeper checks card under the answer.
`dev/tour.mjs` drives a headless Chrome over the DevTools protocol and saves
screenshots of the main states (welcome, what leaves, running, answer, sources, live PC, storage, the
deeper checks with a review and a result, settings, the Live PC view with the action bar under an
application (`10a`) and under memory, Explain and its answer, an answer Ultra took over and its
costs in Sources (`16a`, `16b`), the action bar under a History
event (`17`) and a Storage folder (`18`), the battery card with its inspector discharging (`19`)
and plugged in (`20`), and the pill of a critically low battery (`21`)) in light and dark themes
(`node dev/tour.mjs <folder> [chrome.exe] [en|fr]` while the server runs; `TOUR_BASE` and
`TOUR_DEBUG_PORT` point it at a server on another port). The mock publishes selection sources for
live captures, history events, recordings and each page of scan results (revoked by a new scan).
It lives outside `ui/`, so the app never embeds it.

## Validation status

- Unit tests: view models (no invented zeros, local names only in the local window, key never
  exposed), step reporting and Stop.
- Visual review on 2026-09-28 with the mock in light, dark, English, French and narrow windows, and
  on the real app on the development PC: live cards, a local investigation, a folder scan and one AI
  investigation with Nemotron 3 Super ($0.0040).
- Conversation corrections on 2026-09-28: 13 browser checks at 1280 and 720 pixels cover preview
  refresh races, failed previews, cancellation, two answers, citation targets and reopening history.
  No JavaScript errors were reported. Review found and fixed a duplicate last answer when opening
  a saved conversation and unreadable history in the compact sidebar; a dialog now exposes the
  full conversation list there.
- The same corrective batch passed eight checks against the release Windows app through its
  actual Tauri bridge: real local measurements, two saved turns, history after a page reload,
  previewed history, names toggling and cancellation. Its temporary profile used a dummy credential;
  no provider request was made. These checks do not replace a live model comparison.
- Found on that PC: the GeForce MX350 sometimes refuses its load (NVML error 999) while its
  temperature and memory are readable; the card then shows the temperature and says the load is
  not reported.
- Redesign on 2026-10-01: the tour in French and English, light and dark, at 1280 × 800 and 860 ×
  640, plus a follow-up question, the sources of two answers and reopening from the history; no
  JavaScript errors. Not yet checked in the release Windows app.
- Rail, Live PC view and selection on 2026-10-05, with the mock: the tour in French, light and
  dark; Explain on memory through the preview to an answer; Explain on a history event; and a
  check that in anonymized sharing the question for an application carries neither its name nor
  a PID (in complete sharing: the name, never a PID). No JavaScript errors.
- Web search on 2026-10-05, with the mock: the tour in French and English, light and dark, plus
  checks of the switch (no key, local mode, a mode change keeping the consent), saving and removing
  a key (only the last four characters shown), a turn without consent (no lookup), a source without
  a date, reopening a conversation (recorded steps with the query) and *Open in browser* on the saved
  turn. No JavaScript errors. Not yet checked in the release Windows app or with a real Tavily key.
- Action bar on 2026-10-05, with the mock: the tour in French and English, light and dark, plus
  checks at 1280 × 800 and 860 × 640 of the bar under a card, an application, a process chosen in
  the inspector, an event, a recording and scan results (first page, a page from *Show more*, an
  expired source read again); alignment to the right edge and opening above near the bottom; one
  bar at a time; `E`, `Enter`, the arrow keys and Escape (focus back on the item, kept across the
  periodic refreshes); the disabled state without a reference. No JavaScript errors. Not yet
  checked in the release Windows app.
- Battery in Live PC on 2026-10-06, with the mock: the tour in French and English, light and dark,
  plus checks of every battery switch (two numbered cards, the consent dialog naming every battery
  shown, relative units, the plugged-in readings with *Not reported* values, a battery that cannot
  be read, none on a desktop), a held read showing its age without raising the pill, the drive
  suggestion kept beside a critical battery, `E` and Escape on the card, focus kept across the
  refreshes, and no horizontal scroll at 1280 × 800 and 860 × 640. No JavaScript errors. Then in
  the release Windows app on a laptop (6 %, charging on AC): the card after the GPU card, the
  action bar, the inspector with the driver's unreported power and cycle count shown as *Not
  reported*, and an Explain preview holding the 17 displayed battery metrics under one opaque
  reference, with no device path, tag or battery name. Natively too: a 30 s recording with the
  Live view open kept 28 battery points out of 30 ticks, with no gap over 2.5 s. Not checked
  natively: a discharging battery and the read's exact duration.
- Turn reattach on 2026-10-06, in the release Windows app: a paid driver question reloaded 20 s in
  was shown again as running, then answered (31 s, web query with NVIDIA's public 581.83, five
  nvidia.com sources, no repair), and `current_turn` was empty afterwards. With the mock:
  `?turn=orphan` (Stop ends it) and `?conversation=empty`, French and English, no JavaScript
  errors.
- Review corrections on 2026-10-10, with the mock: 52 browser assertions cover serialized sharing
  choices, keyboard focus, duplicate sends, draft recovery, reuse of the exact reviewed preview,
  the connection-test result after saving, delayed model menus, failed-turn disclosures, empty
  step lists and spaced citations after English and French abbreviations. Rendered checks in
  French and English at 1280 × 800 (light) and 860 × 640 (dark) found no horizontal overflow or
  JavaScript errors. A Rust regression checks that changing file-name sharing revokes a preview.
- The rebuilt release Windows app passed nine checks on 2026-10-10 through its actual Tauri
  bridge: current embedded JavaScript, real measurements, complete/anonymized previews, memory
  selection, serialized privacy settings, two local turns and history after a reload. The isolated
  temporary profile used dummy credentials and made no cloud request; no JavaScript errors were
  reported. Workspace validation passed formatting, Clippy and 715 tests (two ignored).
- Selection summaries on 2026-10-10: 57 rendered checks cover Live PC, History and Storage,
  streaming, citations, Stop, retry, same-conversation continuation, expired references, local
  mode, focus, deletion during preparation and reloading a running summary into Chat. French
  light and English dark layouts passed at desktop and narrow sizes without horizontal overflow
  or JavaScript errors. The native `local_evidence` mode is explicitly labelled as local in the
  inspector and Chat, including after a reload, without a model name or cloud request step.
  Another 28 controller regressions cover sharing and conversation races,
  deletion, late events and acknowledgements, reopened threads and duplicate completion; 12 label
  checks and eight History selection checks passed. These synthetic checks made no cloud call.
- The final release Windows build passed 12 native checks for that selection flow: current
  embedded assets, real memory measurements summarized without leaving Live PC, continuation in
  the same conversation, two compact selection labels after a reload, and explicit local labels
  in the inspector and Chat for `local_evidence`. The isolated, non-elevated profile made zero
  provider requests and reported no JavaScript errors; its app and WebView processes were closed.
- History performance on 2026-10-10: 42 checks cover delayed successes/errors and disposal (six),
  chart density, extrema, unknowns, exact timestamp joins and ambiguous operands (17), and rendered
  loading, period changes and resizing in French/light and English/dark desktop/narrow layouts
  (19). No horizontal overflow or JavaScript errors were reported. Resizing makes no extra IPC
  request. The Rust workspace passed formatting, Clippy and 732 tests (two ignored), including
  legacy/compact/mixed storage, exact u128 weights and cache invalidation after writes, retention,
  event-only changes and deletion. See [history performance](history-performance.md) for the
  reproducible offline benchmark and its limits. Twelve native checks (11 UI/IPC checks and one
  database check) passed on the rebuilt Windows GUI: the embedded asset matches the source,
  legacy history loads for all three periods, delayed replies cannot replace a newer period,
  and resizing makes no extra IPC request. French desktop and English narrow/dark captures were
  inspected. The isolated profile retained 114 legacy rows and added four compact live samples,
  with zero conversations or provider requests. Its app process and debug port were closed.
- Not validated: screen readers, high-contrast mode, other GPUs and screen scalings, very large
  scans in the window, installer and updates.
- Storage results first on 2026-10-26 (#67), with the mock: the scanned folder's totals and its
  largest folders and files now come right after the drop zone, beside an inspector of the
  selected item (sizes, shares, dates, location, Explorer, Explain and Investigate), and the two
  administrator cards follow under their own heading. 11 Node checks cover `storage-item.js`
  (shares, coverage, facts and notes); the tour in English and French, light and dark, adds the
  folder and file inspectors, the summary in the panel, the inspector under the row at 860 px
  and the administrator section (steps 22 to 26), with no page errors.
- Readable program names on 2026-10-26 (#70): Live PC, its inspector, answers, findings, sources
  and action reviews name a process as Task Manager does (*Google Chrome* for `chrome.exe`), the
  executable name as tooltip and the PID only in the inspector or when two cited processes would
  read the same. 74 telemetry tests (version-resource parsing on synthetic blocks, caches, the own
  process) and 68 app tests (labels, one-pass replacement, drafts named like the answer, the
  selection source unchanged); the tour in English, light and dark, against the mock's readable
  names, with no page errors.
- Escalation made visible on 2026-10-26 (#69), with the mock (`?routing=escalated`): the step when
  Ultra takes the question over, the header *Nemotron 3 Super → Nemotron 3 Ultra* with the total of
  both models (or a lower bound), the lasting note under the answer with the reason, and one line
  per model in Sources › Requests and cost; the CLI prints the same breakdown. 11 Node checks on
  `usage.js`, 73 app tests and the CLI's routed-turn tests; the tour in English, light and dark
  (steps 16a and 16b), with no page errors.

## Known gaps

Finding texts come from the core in English,
also when the interface is in French, and so do the coverage notes of the deeper checks. Their
disks and battery apps are numbered, not named: the inspection keeps names on the Rust side and the
window does not receive them. Conversation history is stored locally in SQLite; older
display-only reports remain readable but are not reused as evidence. There is no tray icon or
installer. See [answer verification](answer-verification.md) for covered and intentional limits of
the evidence checks and sharing previews.
