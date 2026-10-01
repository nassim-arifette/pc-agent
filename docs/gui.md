# Desktop app (`apps/gui`, package `pcagent-gui`)

A Tauri 2 window over the same Rust core as the `pcagent` CLI (spec 6). Every measurement,
finding, scan and Nebius request goes through the crates the CLI uses; the window adds no
diagnostic logic of its own.

## Layout

One window, three areas, after the hybrid direction chosen on 2026-10-01 (chat first, context on
demand):

- **Sidebar**: *New conversation*, a search box, saved conversations grouped by day, then
  *Storage*, *Settings* and the model with the current sharing choice. It folds away (button or
  narrow window).
- **Conversation**: a welcome screen with the composer and four suggested questions (one names a
  drive that is almost full), then the thread. While a run is going, its real steps appear with
  their durations and Stop replaces Send. An answer shows the model's text with numbered citation
  chips, the model, duration and cost, the steps and cited measurements, follow-up questions, Copy,
  Ask again, and whether every figure was checked against the measurements. A local analysis shows
  the rules' best finding and offers to ask the AI.
- **Context panel**, closed by default. It opens on the right tab from what the person is looking
  at: a citation chip (*Sources*, with that measurement highlighted), the *PC status* pill (*Live
  PC*), *Analyze a folder* or *Storage* (*Storage*). *Sources* also keeps each answer's limits and
  its requests, tokens and price. *Live PC* refreshes every 5 s while the window is visible
  (pause button) and draws short sparklines. Below 1100 px the panel floats over the conversation.

The *PC status* pill in the top bar names the most important thing in a few words: a drive under
5 % or 5 GiB free, memory at 90 % or more, a processor at 90 % or more, or a slowed graphics card.
These thresholds are hints for the status line, not findings.

The full *Storage* and *Settings* screens open in the conversation area. Storage picks or drops a
folder, follows the scan and lists the largest folders and files; nothing is deleted there.

## Visual system

Warm neutrals, a single accent (*Encre* `#3B5BDB`, `#8FA2F5` in dark), Segoe UI Variable and 1 px
strokes rather than shadows, close to Windows 11. Colours are tokens in `ui/styles/tokens.css`,
light and dark. Iris, the former mascot, is reduced to a mark (`ui/js/brand.js`): an accent tile
with two eyes that signs answers and never animates on its own. The design exploration and the
clickable prototype live on a private design canvas; the decisions are recorded in
[the plan](plan.md).

## Consent and privacy

- Local analysis is the default. *With AI* requires a configured key and model.
- Before an AI investigation, a dialog previews the exact first data message, including the bounded
  conversation history and its retained observations. Program names follow the person's choice.
  The reviewed snapshot is frozen in a single-use ticket; sending waits for preview updates and
  rejects a mismatched or expired ticket. Subsequent tool results follow the same sharing settings.
  *Don't ask again until I close PC Agent* lasts for the session only; it is never saved.
- A folder scanned in Storage is offered to the model only when that box is ticked; otherwise the
  investigation gets an empty storage service.
- When the budget runs out, the window asks before extending it and shows the cost so far.
- The key is written to `%APPDATA%\pcagent\.env` (the CLI's file) by the Rust side and never sent
  back to the page, which only sees whether a key exists and its last four characters.

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

Keyboard: `Ctrl+1` to `Ctrl+3` open the conversation, Storage and Settings, `Ctrl+N` starts a new
conversation, `Ctrl+K` jumps to the question box, `Ctrl+J` shows or hides the panel, `Enter` sends
and `Shift+Enter` adds a line, `Esc` closes a dialog.

## Design review without the app

`dev/mock.js` answers every command with synthetic data, so the interface can be reviewed in a
browser:

```powershell
cd apps\gui
python -m http.server 5173 --bind 127.0.0.1
# open http://127.0.0.1:5173/ui/index.html (?cloud=none|problem, ?gpu=none, ?disk=ok)
```

Questions containing `extend`, `fail` or `slow` exercise the extension prompt, an unusable model
answer and a long run. `dev/tour.mjs` drives a headless Chrome over the DevTools protocol and saves
screenshots of the main states (welcome, consent, running, answer, sources, live PC, storage,
settings) in light and dark themes (`node dev/tour.mjs <folder> [chrome.exe] [en|fr]` while the
server runs). The mock lives outside `ui/`, so the app never embeds it.

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
- Not validated: screen readers, high-contrast mode, other GPUs and screen scalings, very large
  scans in the window, installer and updates.

## Known gaps

Action proposals and approvals are CLI-only for now. Finding texts come from the core in English,
also when the interface is in French. Conversation history is stored locally in SQLite; older
display-only reports remain readable but are not reused as evidence. There is no tray icon or
installer. See [answer verification](answer-verification.md) for covered and intentional limits of
the evidence checks and sharing previews.
