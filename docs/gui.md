# Desktop app (`apps/gui`, package `pcagent-gui`)

A Tauri 2 window over the same Rust core as the `pcagent` CLI (spec 6). Every measurement,
finding, scan and Nebius request goes through the crates the CLI uses; the window adds no
diagnostic logic of its own.

## Layout

### Browser details in Live PC

A browser application's row includes process count, working set and the largest closed
process categories. Its inspector adds an EN/FR table sorted by working set, process count
or type, plus classification coverage and the shared-working-set warning. No column claims
to measure exact RAM per tab/site. Application selection survives fresh captures using a
local stable process reference; model selections still use the current snapshot's references.

**Read window titles** and **Read tab titles** are explicit local requests. Captions and
accessible tab names appear only in this view, not in selection chips, saved conversations,
SQLite, evaluation output or provider requests. Leaving Live PC discards them. Unknown,
exited and inaccessible windows keep honest status messages. Use **See what is sent** on
Explain/Investigate to inspect the exact measured breakdown without these titles.

Each supported browser window offers **Open browser process manager**. The dialog lists the
fixed destination and identity checks, offers decline, and requires a separate confirmation
before execution. It never closes processes or grants approval to a model. Existing
in-flight plans block preparation rather than silently replacing the pending decision.

One window after the hybrid direction chosen on 2026-10-01 (chat first, context on demand), with
the rail of the desktop layout:

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

Storage starts with a squarified map of direct child folders. Map, Tree and List share the same
scan index and inspector. Folders can be opened by double-clicking a map tile, by the explicit
Open folder button (keyboard accessible), or by a breadcrumb. Map tiles support Tab, arrows,
Home/End and Enter/Space selection. The list below the map includes zero-size and unknown items
that cannot receive an area. **Other** explains the difference between the parent and displayed
pages; it never creates an AI selection. Partial coverage is explicit.

Tree uses nested lists with native disclosure buttons and `aria-expanded`, loading folder and
file pages only as expanded. List reads the current folder rather than mixing descendants.
All pages remain reachable through Load more; no visual top-N cap hides indexed children.
Selection loads cancellable, scan-time facts and an exact host-issued C3 reference before
enabling Explain/Investigate. Its inspector adds 182-day untouched bytes/count, undated files,
largest file and extension-only type totals. Unknown allocation is displayed as unknown.

The shared action toolbar participates in normal layout and wraps on narrow windows, rather
than reserving a fixed-height overlay; this applies to Live PC too. Number, size and date
formatting follows the selected interface language. Scan completion time and the permanent
read-only notice distinguish metadata measurement from current use or deletion.

Recent scans are summaries for this session only; selecting an old root explicitly rescans it.
Watch proposes volume-free-below through `ui/js/watches-controller.js`'s `requestWatch(rule)`.
The threshold is 10 GiB, bounded by capacity and at least 1 GiB. An absent form, non-fixed
volume or unknown/too-small capacity has a visible disabled reason, never a fake success.

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
calls `elevation_run`. The result is drawn generically from the inspection (`elevation/checks.js`):
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

These are isolated unit tests. In the native application, check keyboard focus, both
languages and themes, skip/reload/resume, saved profiles, empty/error catalogs and clearing
the password on leaving. Use a fresh local profile for first-launch acceptance.

Native WebView2 adds its own password-reveal control after typing, even if the app provides no
reveal button. The Nebius field suppresses it with `::-ms-reveal`. Before entering a real key in
native QA, verify this with an unsaved dummy value.
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
page.

## Errors

A command that fails rejects with `{code, message}` (`src/failure.rs`): `code` comes from a closed
set (`uac_declined`, `folder_unreadable`, `preview_expired`…), `message` is an English detail for
diagnosis, and a refused action adds a closed `reason` (`expired`, `changed`, `protected`…).
`ui/js/api.js` gives every rejection one shape, an `Error` with `code` (`unknown` for a command
that still sends a bare sentence) and `reason`; `ui/js/failure.js` words it with `failure.<code>`
or `failure.action_refused.<reason>` from `i18n.js`, and an unknown error reads as a generic
sentence. The English detail is at most a tooltip, never the main text, so the French window shows
no English sentence and no Rust name such as `UacDeclined`. Failed turns keep their C3 codes
(`turn:failed {code, message}`, worded by `error.<code>`); a turn refused before it starts says why
with the command's code, else `error.start`. `scan:failed` carries the same `code` as `start_scan`.

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
saved before them keep their single cost.
Presentation helpers are covered by `node --test apps/gui/dev/usage.test.mjs`.

### Watches

`#/watches` lists the rules with their live state, the alerts of the last seven days and a local
weekly summary; Live PC, History and Storage open the prefilled Monitor form. See
[watches.md](watches.md). Covered by
`node --test apps/gui/dev/watches-view.test.mjs`.

### Daily report

Settings has a Daily report card (switch, time, the exact command Windows will start, "Write a
report now") and the home page shows the latest report under the composer, answer first. See
[daily-report.md](daily-report.md). Covered by
`node --test apps/gui/dev/daily-report.test.mjs`.

## Architecture

```text
ui/ (HTML, CSS, JS modules, no build step) ──invoke──▶ src/app/ (Tauri commands)
        ▲                                                   │
        └──────────── events (inv:*, scan:*) ◀──────────────┤
                                                            ▼
                          pcagent-telemetry · -agent · -storage · -policy · -actions
```

- `src/view.rs` exposes feature modules under `src/view/` for measurements/dashboard, battery,
  actions, activity, local labels, reports, usage, storage and settings. They turn core types
  into plain data for the page (raw value + text + status for every
  measurement). Portable and unit-tested.
- `src/session.rs` wraps the model and the toolbox so each real step is reported as it starts and
  ends, and so Stop refuses the next model request (`AgentError::Cancelled`). A request already sent
  cannot be recalled. Portable and unit-tested.
- `src/app.rs` (Windows only) composes the shell, registers Tauri commands and owns startup/shutdown.
  Its `app/` modules separate shared state, feature commands, measurement threads and the
  investigation worker. Command names, payloads, consent checks and helper lifecycle stay at the
  same host-owned boundary. One thread owns the live collector for the
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

### Feature modules

The native ES modules under `ui/js/` are grouped by responsibility:

| Directory | Owns |
| --- | --- |
| `chat/` | conversation view, composer, model picker, citations, costs and web sources |
| `selection/` | selected-item actions, inspector and local selection metadata |
| `settings/` | preferences view, first-launch setup and its testable state transitions |
| `live/` | live view and panel, battery presentation and health hints |
| `storage/` | storage view, item facts and quick drive analysis |
| `elevation/` | deeper-check cards and the shared helper slot |
| `history/` | history view, charts and recording controls |
| `i18n/` | pure English/French dictionaries and translated finding headings |

Shared infrastructure stays at the root: the shell (`main.js`), router, store, Tauri bridge,
preferences, localization, DOM helpers and common controls. Each view imports the modules it
needs directly; there are no compatibility wrappers for the old paths. In `live/`, `model.js`
owns the health priorities and labels without importing the store, DOM or Tauri; `panel.js`
renders them. This lets Node test the decisions without mounting the interface.

`i18n.js` keeps locale selection, placeholder interpolation and fallback behavior. Its dictionaries
under `i18n/` have no browser dependencies; tests import them directly to check key and parameter
parity. Measurement sentences remain in `phrases.js`.

The page connects only to the native Tauri host. Test helpers under `dev/test-support/`
are Node-only modules imported by unit tests; they are never part of the UI module graph.
Nothing under `dev/` is embedded in the app.

### Views and the view bar

`ui/js/main.js` mounts one feature view at a time with `mount(page, shell)`; the view
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

### Frontend regression checks

Node is needed only for development checks, not to build or run the application:

```powershell
node --experimental-default-type=module --test apps/gui/dev/*.test.mjs
node --experimental-vm-modules apps/gui/dev/check-modules.mjs
```

The suite checks the presentation and state helpers and runs the module checker in a separate
process with `--experimental-vm-modules`. It parses **every** UI module and links static imports,
catching missing files, missing named exports and duplicate declarations. It does not evaluate
application code, create a DOM or invoke Tauri. Native app validation remains required for rendered flows and Windows integration.

## Image attachments

The chat composer accepts one explicitly selected or pasted static PNG/JPEG per question.
Rust validates the bytes (8 MiB input, 24 million pixels, 16,384 pixels per axis), applies
orientation, flattens transparency on white, strips metadata and reduces the longest side to
1,600 pixels. The prepared JPEG is at most 2 MiB. The thumbnail and expanded preview use these
exact prepared bytes; the page sends only an opaque `image_id` with the question.

Images require **Complete** sharing, with a visible warning about personal information in the
pixels. The host uses exactly `nvidia/Nemotron-3.5-Super-VL` for that turn, without escalation
or changing the saved text model. Availability is checked in the authenticated account catalog.
Missing modality metadata must be confirmed by an active, exact `image2text` flavor with tool
support in Nebius's [public JSON catalog](https://tokenfactory.nebius.com/api/public/models_info);
an unknown, conflicting, unavailable or explicitly incompatible capability prevents sending.
The public catalog request carries no API key, question, telemetry or image.

The single-use preview binds the question, image digest, conversation, sharing and routing.
Removing, replacing or expiring the draft invalidates it. Stored pixels are separate from the
text history, committed with the turn and deleted with it (including retention and clear-all).
History loads images only when requested; follow-up questions never automatically include them.
An image is untrusted context, not measured evidence: numerical claims still require tools.

`dev/image-draft.test.mjs` covers byte-only
IPC preparation, size/type rejection, clipboard filtering, single-image replacement, removal
and stale asynchronous results. Rust tests cover decoding/orientation, animation/dimensions,
preview equality and consumption, routing restrictions, tool-context retention and SQLite
rollback/deletion/retention.

## Visual review

Run the desktop application with its native host. Opening `ui/index.html` in a plain
browser displays an instruction to open PC Agent, without loading readings or conversations.
Check the affected views in English and French, light and dark, at 1280 x 800 and 860 x 640.
Record the actual host, measurements and any unavailable capabilities with each result.

## Validation status

- Unit tests: view models (no invented zeros, local names only in the local window, key never
  exposed), step reporting and Stop.
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
## Known gaps

Finding texts come from the core in English,
also when the interface is in French, and so do the coverage notes of the deeper checks. Their
disks and battery apps are numbered, not named: the inspection keeps names on the Rust side and the
window does not receive them. Conversation history is stored locally in SQLite; older
display-only reports remain readable but are not reused as evidence. There is no tray icon or
installer. See [answer verification](answer-verification.md) for covered and intentional limits of
the evidence checks and sharing previews.
