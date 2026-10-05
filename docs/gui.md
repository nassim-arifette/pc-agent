# Desktop app (`apps/gui`, package `pcagent-gui`)

A Tauri 2 window over the same Rust core as the `pcagent` CLI (spec 6). Every measurement,
finding, scan and Nebius request goes through the crates the CLI uses; the window adds no
diagnostic logic of its own.

## Layout

One window after the hybrid direction chosen on 2026-10-01 (chat first, context on demand), with
the rail of the phase 2 desktop mockups:

- **Rail**: *Chat*, *History*, *Live PC*, *Storage* and, at the bottom, *Settings* with a dot for
  the AI status.
- **Conversation list** (on the chat only): *New conversation*, a search box, saved conversations
  grouped by day, then the model with the current sharing choice. It folds away (button or
  narrow window); the other views use the full width.
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
  its requests, tokens and price. *Live PC* refreshes every 5 s while the window is visible
  (pause button) and draws short sparklines. Below them, programs are grouped by application
  (`process_activity`, a 2-second window measured every 10 s only while the tab is visible), with
  their own sums, sortable by memory or processor; opening a group shows its services and its
  processes with their own values, so a group is never mistaken for one process. Below 1100 px the panel floats over the conversation.

The *PC status* pill in the top bar names the most important thing in a few words: a drive under
5 % or 5 GiB free, memory at 90 % or more, a processor at 90 % or more, or a slowed graphics card.
These thresholds are hints for the status line, not findings.

The full *Storage* and *Settings* screens open in the conversation area. Storage picks or drops a
folder, follows the scan and lists the largest folders and files; nothing is deleted there.

## Live PC, selection, Explain and Investigate

The *Live PC* view (`#/live`) shows the resources as selectable cards (memory, processor, each
fixed drive, each graphics card) and the applications as selectable rows. Selecting one opens the
inspector on the right with its details and two actions:

- **Explain** uses only the measurements already taken for that item: nothing new is collected and
  no action is proposed (`get_selected_evidence` only).
- **Investigate** starts from them with the usual read-only tools; actions stay proposals.

The page never sends measurements or names itself. It passes a host-issued reference
(`selection_source_id` of the dashboard or of `process_activity`, plus a closed target) to
`selection_preview`, which freezes that capture and returns the exact first message and a
single-use ticket; `selection_send` must match it exactly ([selection.md](selection.md)). In a
cloud mode the preview dialog shows the item, its measurement age, the model, what is sent and
never sent, and the exact JSON; in local mode the turn starts directly. The question is written in
the interface language and follows C5 like the data: never a PID, a program name only in complete
sharing. An application is explained through its largest measured process, which the inspector
names. History events and finished recordings use the same flow through `selection_sources()`; if
their source has expired, the typed question of the previous version is used instead.

## Quick drive analysis

The Storage view also offers a *Quick drive analysis* of a fixed NTFS drive. It runs the closed
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
trace are not offered here.

## History, recording and startup

The *History* view reads the local sampler (`history_status`, `metric_history`): processor,
memory, disk activity and graphics card over the last hour, 24 hours or 7 days, as four charts on
one time axis. Unmeasured periods (PC Agent closed, PC asleep) stay breaks in the lines and are
counted, never drawn through. Detected anomalies are listed with their peak, duration and the
program measured at the peak, which is not presented as the proven cause; *Explain* and
*Investigate* send that event through the selected-evidence preview.

*Record a problem* takes one measurement per second for 30, 60 or 120 seconds
(`record_activity_start`), with a countdown and *Stop* (`record_activity_cancel`, which keeps the
measurements); then *Explain* or *Investigate* sends that exact recording. The *Collection* card
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

## Visual system

Warm neutrals, a single accent (*Encre* `#3B5BDB`, `#8FA2F5` in dark), Segoe UI Variable and 1 px
strokes rather than shadows, close to Windows 11. Colours are tokens in `ui/styles/tokens.css`,
light and dark. Iris, the former mascot, is reduced to a mark (`ui/js/brand.js`): an accent tile
with two eyes that signs answers and never animates on its own. The design exploration and the
clickable prototype live on a private design canvas; the decisions are recorded in
[the plan](plan.md).

## Consent and privacy

- The person chooses how questions are analyzed, once and then whenever they like (composer pill,
  Settings): **AI with program names** (complete sharing, recommended), **AI with names hidden**
  (anonymized) or **on this PC only** (local rules, no request). The choice is stored by the Rust
  side (`privacy_get`, `privacy_set`); choosing a cloud mode is not consent by itself.
- Before a cloud question, a dialog shows the stored mode (with *Change*), the question exactly as
  it will be sent after identities, paths and secrets were removed (and says when something was
  removed), what is sent and never sent, and the exact first message. Approving it confirms the
  choice. The reviewed snapshot is frozen in a single-use ticket that `send_message` requires.
- File and folder names of the scanned folder are a separate opt-in, offered only in complete
  sharing. The dialog lists the exact names and folder-relative paths (`%SCOPE%\...`) that may be
  sent; never file contents or paths outside the folder.
- *Don't ask again until I close PC Agent* lasts for the session only and still requests a fresh
  ticket per question; it applies to the folder actually approved.
- When the budget runs out, the window asks before extending it and shows the cost so far.
- The key is written to `%APPDATA%\pcagent\.env` (the CLI's file) by the Rust side and never sent
  back to the page, which only sees whether a key exists and its last four characters.

## Turns and events

A question and its answer form a turn. `send_message` returns a stable `turn_id`; the window
follows `turn:step`, `turn:delta` (the whole draft so far, replaced by the verified turn),
`turn:extension`, `turn:done` and `turn:failed`, and stops a turn with `stop_turn`. The Rust side
still emits the legacy `inv:*` events for older clients; this window does not listen to them, so
nothing is drawn twice. Failed turns are kept in the history and marked in the sidebar.

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
  live measurements (and, every 30 s, the drives' free space); each investigation and each scan runs on its own thread and reports through events.
  The Storage screen and investigations share one `StorageService`, so a completed scan can be
  reused by the model without rescanning.
- The page is untrusted input: commands validate every argument, paths and item references are
  resolved on the Rust side, and *Show in Explorer* uses the actions crate's argument sanitizer.
  The content security policy allows only the app's own files and Tauri's IPC. The only capability
  granted to the page is listening to events; the folder dialog is opened by a Rust command.

The interface is plain HTML, CSS and JavaScript modules served from `ui/` (no Node toolchain, no
npm dependencies), a deliberate departure from the React/TypeScript suggested in spec 15.1 to keep
the build to `cargo` and the supply chain small. Text is always inserted as text, never as HTML.

## Build and run

Windows 11 x64 with WebView2 (included in Windows 11), Rust 1.90 and the MSVC build tools:

```powershell
cargo run --release --locked -p pcagent-gui
```

The first build compiles Tauri and needs about 3 GB for `target\`. On other platforms the binary
only prints a message; the portable view and session code is still built and tested in CI.
The CI keeps a `pcagent-gui-windows-x64` artifact: an unsigned prototype, not an installer.

Keyboard: `Ctrl+1` to `Ctrl+5` open Chat, History, Live PC, Storage and Settings, `Ctrl+N` starts a
new conversation, `Ctrl+K` jumps to the question box, `Ctrl+J` shows or hides the panel, `Enter`
sends and `Shift+Enter` adds a line, `Esc` closes a dialog or clears the Live PC selection.

## Design review without the app

`dev/mock.js` answers every command with synthetic data, so the interface can be reviewed in a
browser:

```powershell
cd apps\gui
python -m http.server 5173 --bind 127.0.0.1
# open http://127.0.0.1:5173/ui/index.html (?cloud=none|problem, ?gpu=none, ?disk=ok,
# ?privacy=complete|anonymized|local for a confirmed choice, ?elevation=fail|slow)
```

Questions containing `extend`, `fail` or `slow` exercise the extension prompt, an unusable model
answer and a long run. `dev/tour.mjs` drives a headless Chrome over the DevTools protocol and saves
screenshots of the main states (welcome, consent, running, answer, sources, live PC, storage, the
deeper checks with a review and a result, settings, the Live PC view with Explain and its preview)
in light and dark themes (`node dev/tour.mjs <folder> [chrome.exe] [en|fr]` while the server runs;
`TOUR_BASE` and `TOUR_DEBUG_PORT` point it at a server on another port). The mock publishes
selection sources for live captures, history events and recordings. It lives outside `ui/`, so
the app never embeds it.

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
