# Validation

What is checked, how to run the checks, and what has not been validated yet, as of 26 October
2026. Each work package records its dated results in [the plan's tracking table](plan.md#8-suivi)
(in French); the desktop app keeps its own record in [gui.md](gui.md#validation-status).

## Automated checks

The GitHub Actions workflow [`.github/workflows/ci.yml`](../.github/workflows/ci.yml) defines
these checks on Windows and Ubuntu. It has been disabled by the project owner since 10 October;
the same commands are run locally before each merge into `main`. A local run is not a published
CI run, and this page does not claim that any pending run has passed.

```powershell
# On the development PC, C: is nearly full: keep build outputs on D:.
$env:CARGO_TARGET_DIR = "D:\pcagent-target"
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked          # from a standard session: see below
cargo build --release --locked -p pcagent -p pcagent-gui -p pcagent-elevated -p pcagent-eval
python scripts/test-eval-mcp-client.py
node --experimental-default-type=module --test apps/gui/dev/onboarding.test.mjs apps/gui/dev/storage.test.mjs
```

- The toolchain is pinned to Rust 1.90.0 (`rust-toolchain.toml`). `Cargo.lock` is committed and
  every build uses `--locked`; no check rewrites it.
- Tests use synthetic data and scripted model replies. They need no credentials and must not see
  any: unset `NEBIUS_API_KEY`, `NEBIUS_MODEL_ID`, `NEBIUS_REASONING` and `TAVILY_API_KEY` first.
  The live tests (`crates/agent/tests/live_nebius.rs`, and one Tavily search in
  `crates/agent/src/tavily.rs`) are `#[ignore]`d: they make paid requests and run only when
  someone asks for them explicitly with `--ignored`.
- On Linux, the same `cargo test --workspace --locked` runs the portable tests; native collectors
  return `unsupported` there and are not substitutes for Windows measurements. Last Linux run:
  26 October 2026 on Ubuntu 22.04.5 x86_64 (Rust 1.90.0, Node 22, Python 3.10) as a non-root
  user: formatting, Clippy with `-D warnings`, 741 tests passed (0 failed, 3 ignored live
  tests, doctests included), the four release binaries, 3 Python and 56 Node tests, and the
  44-module link check. The Linux release binaries were then probed read-only: `doctor`,
  `snapshot` and `processes` report `unsupported` evidence with `null` values and exit 3,
  `diagnose --offline` answers from local rules without a provider call, `storage` scans a
  synthetic folder without reading content, `pcagent-gui` prints its Windows-only notice and
  exits 2, and `pcagent-elevated` answers `InvalidArguments` or `Unsupported` without
  touching the system. No credentials were present. Linux still has no native collector, no
  window and no elevated helper: these are stubs, not a port.
- On macOS (26 October 2026: macOS 26.6.2 on Apple silicon, Rust 1.90.0, branch
  `fix/macos-build`) the same commands pass: formatting, Clippy, 741 tests in 30 test executables
  (3 ignored: the paid live tests; no doctests), the four release binaries, 3 Python and 56 Node
  checks. Node 24 rejects `--experimental-default-type`; use Node 22. `pcagent-gui` prints its
  Windows-only notice and exits with code 2, `pcagent-elevated` answers `Unsupported`, native
  collectors return `unsupported` and `pcagent storage` runs on the portable scanner. These are
  compilation and portable-logic checks, not a macOS product target.
- What the tests cover, by area: the closed tool schemas and batch validation, the C1 verifier
  (wrong source, wrong process or metric, uncited quantity, question used as evidence,
  historical wording, web prices and versions), repairs, budgets and extensions, streaming and
  Stop, the C5 projections in each sharing mode (no user name, profile path, PID or command line),
  exact preview equality and single-use tickets, storage aliases and the finite name list, the
  elevated helper protocol (hostile arguments, truncated output, simulated UAC refusal, slot and
  shutdown), action approval, revalidation and journal recovery, history storage, retention and
  gaps, application grouping, and the page's onboarding logic. The detailed lists are in each
  design note ([answer-verification.md](answer-verification.md), [selection.md](selection.md),
  [elevation.md](elevation.md), [actions.md](actions.md),
  [history-performance.md](history-performance.md), [telemetry.md](telemetry.md)).

### Tests as a standard user

PC Agent refuses to run elevated: the CLI exits with code 2 and the window shows a notice
(`ensure_unelevated_host`). Several tests exercise that guard, so they fail by design when the
suite runs in an elevated session: an elevated terminal, the built-in Administrator account, or a
hosted CI runner. Never skip those tests and never relax the guard to make them pass.

- **Normal case.** Run `cargo test --workspace --locked` from a standard, unelevated terminal. An
  administrator's ordinary session under UAC is unelevated too; *Run as administrator* is not.
- **When only an elevated session exists** (for example a virtual machine signed in as the
  built-in Administrator): compile as yourself, then run the compiled tests as a temporary
  standard account, as [`scripts/ci-windows-tests.ps1`](../scripts/ci-windows-tests.ps1) does on
  the disposable GitHub runner. That script creates accounts only inside GitHub Actions and
  refuses to on any other PC, so follow its steps by hand on a machine you control:
  1. `cargo test --workspace --locked --no-run --message-format=json`, and keep the `executable`
     path of every test artifact;
  2. create a temporary local account in the Users group (`S-1-5-32-545`) and give it modify
     access to the checkout and the target directory only;
  3. run each test executable as that account, from the checkout, with `USERPROFILE`, `APPDATA`,
     `LOCALAPPDATA`, `TEMP` and `TMP` pointing into a disposable folder and no provider variable
     set;
  4. run the doctests separately, `cargo test --workspace --doc --locked`;
  5. delete the account and the disposable folder, including when a test failed.
- The script also checks the guard on the real binary: in the elevated session,
  `pcagent doctor --json` must exit with code 2, write nothing to stdout and print
  *Start PC Agent from a standard, unelevated session.* on stderr.

## Browser QA with the design mock

The interface can be reviewed without compiling: `apps/gui/dev/mock.js` answers every command
with synthetic data and never contacts Nebius, Tavily or a website.

```powershell
cd apps\gui
python -m http.server 5173 --bind 127.0.0.1
# open http://127.0.0.1:5173/ui/index.html; URL switches are listed at the top of dev/mock.js
```

- `node dev/tour.mjs <folder> [chrome.exe] [en|fr]` drives a headless Chrome over the DevTools
  protocol and saves screenshots of the main states; `TOUR_BASE` and `TOUR_DEBUG_PORT` point it at
  another server and debugging port. Node 20 needs `node --experimental-websocket`.
- For every screen a change touches, check English and French, light and dark, 1280 × 800 and
  860 × 640, keyboard focus, no JavaScript error in the console and no horizontal overflow.
- Use dummy keys only. The mock checks rendering and page logic; it proves nothing about the Rust
  side, the native WebView or a model. Switches and states: [gui.md](gui.md#design-review-without-the-app).
- The README's screenshots come from this mock, so they show synthetic data; retake them when a
  view they show changes.

## Native QA of the release app

- Build the release executables and run them from a standard session, never elevated (starting
  the window as administrator must show the refusal notice: that is a check in itself).
- To keep a test from touching your own data, start the app with `APPDATA` and `LOCALAPPDATA`
  pointing at a disposable folder: settings, keys, history and the action journal follow these
  variables. The WebView keeps its own data under the real `%LOCALAPPDATA%\dev.pcagent.desktop`.
- Use a dummy key unless a live check is intended. A live request is billed to the key's account
  and needs the owner's explicit authorization; record its date, model, duration and cost, never
  the key, a user name or personal data.
- Scripted native checks can attach to the WebView through
  `WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--remote-debugging-port=<port>`; close the app and that
  port afterwards.
- Some behaviour only the native app can show: the Nebius key field without WebView2's reveal
  button, the *Open Nebius Token Factory* link opening a browser in an interactive session,
  Windows' consent prompt for the helper, the notification-area icon, real measurements, and
  NVIDIA readings compared with `nvidia-smi`.

## Manual acceptance on a second Windows 11 PC

Not done yet. Run it on a Windows 11 x64 PC that has never run PC Agent, signed in with a standard
account (or an administrator's ordinary session), with the build published as a GitHub release.
Record the date, the Windows build, the hardware class (no serial numbers, names or paths), the
release version and commit, and each result in the plan's tracking table. A Windows 10 PC, if one
is available, can run the same list; note whether WebView2 had to be installed.

**Install and first launch**

- [ ] Download `pcagent-gui.exe` and `pcagent-elevated.exe` (and `pcagent.exe` for the CLI) from
  the release into one folder.
- [ ] SmartScreen shows *Windows protected your PC*; *More info* then *Run anyway* starts the app.
  Note what Windows shows exactly, and whether Smart App Control is on (Windows Security, App &
  browser control).
- [ ] *Run as administrator*: a notice says PC Agent does not run as administrator, and no window
  opens.
- [ ] A normal start opens the setup guide. *Open Nebius Token Factory* opens the browser at
  `https://tokenfactory.nebius.com/`.
- [ ] The pasted key stays masked, with no reveal button. *Load account models* lists the
  account's Nemotron models; *Test connection* succeeds; *Finish setup* opens the chat.
- [ ] A wrong key and a disconnected network give clear messages. *Continue locally without AI*
  works without a key, and Settings reopens the guide.

**Chat**

- [ ] A suggested question: the steps appear live with their durations, the draft answer streams,
  then the verified answer replaces it with its model, duration and cost.
- [ ] Citations open the *Sources* panel on the measurement they cite; unverified passages, if
  any, are marked.
- [ ] *See what leaves* before sending: the question, the measurements and program names; with
  *Names* off, references only. No user name, profile path, PID or command line in the JSON.
- [ ] A follow-up question builds on the previous answer; *Stop* ends a running turn; a budget
  extension, if asked, shows the cost so far.
- [ ] After closing and reopening the app, the conversation is in the sidebar and can be deleted.

**Live PC, History and Storage**

- [ ] Live PC values are plausible next to Task Manager (memory in use, processor, drives,
  graphics card), keeping in mind that both tools define some counters differently. Apps that
  embed WebView2 (Teams, Outlook, Widgets) carry their `msedgewebview2.exe` processes.
- [ ] Selecting memory then *Summarize these measurements* answers in the inspector; *Find the
  cause* continues in the Chat.
- [ ] *Record a problem* for 30 seconds while reproducing a load; the recording appears and can
  be summarized.
- [ ] *Start with Windows* adds and removes the `PcAgent` value under
  `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`; while it is on, closing the window keeps
  PC Agent in the notification area, and *Quit PC Agent* stops it.
- [ ] A scan of Downloads lists the largest folders and files; selecting one offers Summarize and
  Find the cause; nothing is deleted.
- [ ] *Quick drive analysis* of C: shows its review, then Windows' consent prompt for
  `pcagent-elevated.exe` (an unknown publisher, since it is unsigned); the result keeps its
  partial coverage. Declining the prompt and *Stop* both end cleanly.
- [ ] Each deeper check (*Windows caches*, *Restore points and shadow copies*, *Disk health*,
  *Battery use by app* on a laptop) runs after its own consent prompt.

**Actions**

- [ ] A question about freeing space proposes cards; *Review and confirm* lists every exact
  target; *Run now* on a TEMP cleanup moves the files to the Recycle Bin and shows the free space
  before and after; *Ignore* drops another proposal; nothing runs without a click.
- [ ] A test entry in the user's startup programs can be turned off from a proposal, then
  restored with *Restore*.

**Optional: web lookups**

- [ ] With a Tavily key saved, a question about the graphics driver shows the exact query under
  its step, and a `[W1]` source opens in the browser.

**Privacy and data**

- [ ] `%APPDATA%\pcagent` holds `.env`, `privacy.json` and `pcagent.sqlite`; the window never
  shows the saved key again, only its last four characters.
- [ ] In local mode, questions are answered by the local rules and nothing goes to Nebius
  (optionally confirmed in Resource Monitor's Network tab).
- [ ] *Clear the history* and deleting all conversations both work.

**Command line**

- [ ] `pcagent doctor`, `pcagent snapshot`, `pcagent processes --sort-by cpu`.
- [ ] `pcagent diagnose "Why is my PC slow?" --preview-cloud` prints the first message and sends
  nothing; `pcagent doctor --check-cloud`, then the same question with `--allow-cloud --verbose`.

**Hardware, failures and footprint**

- [ ] With an NVIDIA GPU, compare its temperature, memory, clock and slowdown reasons with
  `nvidia-smi --query-gpu=temperature.gpu,memory.used,memory.total,clocks.sm,clocks_event_reasons.active --format=csv`,
  idle and under load. `memory.used` there excludes the driver's reserve, while PC Agent reports
  total minus free, reserve included.
- [ ] Bad credentials, an unavailable model, a disconnected network and a cancelled turn each give
  an explicit message, and nothing runs that was not approved.
- [ ] Measure the app's memory and processor use, window open and hidden, and the size of
  `pcagent.sqlite` after a known duration, for the README.
- [ ] Rehearse the video's path: a real problem, the conversation, the tools running, the cited
  answer, the approved action and its before/after.

Never put a key, a user name or personal measurements in screenshots, issues or logs.

## Not validated yet

- The second-PC acceptance above, including installation from the release.
- **Answer quality.** The evaluation harness exists ([eval.md](eval.md)), but no human-scored
  campaign against Claude or ChatGPT has been run, so the choice of default model is not yet
  backed by scores. Scripted tests validate the controller, not the model's diagnoses.
- **Windows 10.** Announced as supported; not tested yet.
- **Linux.** Only the portable core is validated (26 October, see above); collectors, the window
  and the elevated helper are intentional `unsupported` stubs. The CI's Ubuntu job has been
  disabled since 10 October, so the Linux commands are replayed by hand.
- **macOS.** Only the compilation and portable-logic checks above; there is no native collector,
  window or elevated helper for it, and no acceptance run.
- **Administrator helper.** Windows' consent prompt (accept, decline, cancel) has not been through
  an acceptance run; MFT totals have not been compared with a controlled NTFS volume, so every
  result keeps `native_accuracy_unvalidated`; the ETW performance trace stays unavailable on
  purpose ([etw.md](etw.md)).
- **Measurement accuracy** against Performance Monitor for CPU share, I/O rates, disk latency and
  per-process GPU use, and the grouping of WebView2 processes with real Teams, Outlook, Widgets or
  Search trees ([telemetry.md](telemetry.md#validation-status)).
- **Accessibility and hardware breadth:** screen readers, high-contrast mode, other GPUs and
  display scalings, very large scans in the window.
- **Web lookups:** price extraction and the per-country shop lists against real results, and
  Tavily's terms before any claim about data retention ([web-search.md](web-search.md#not-done-yet)).
- **Distribution:** no installer, no automatic update and no code signing.

## Earlier records

Short dated pointers; the details stay where they were recorded.

- **Since 26 October 2026.** Each branch merged into `main` before the submission passes the
  commands above locally, with the tests run under a standard account; the plan's
  [section 8](plan.md#8-suivi) records each merge and its test count.
- **26 October 2026.** Linux replay of the full command list on Ubuntu 22.04.5 x86_64
  (`fix/linux-build`): 741 Rust tests, 56 Node tests, release binaries and read-only probes of
  the Linux stubs; the same patch replayed on the agent-boundaries refactor tree gave 760 Rust
  and 100 Node tests.
- **26 October 2026.** First macOS run (Apple silicon): the portable scanner's device and link
  counts are converted from Darwin's `dev_t` and `nlink_t` types; formatting, Clippy, 741 tests,
  release builds, Python and Node checks pass (`fix/macos-build`).
- **10 October 2026.** Review corrections and the history performance work: formatting, Clippy,
  715 then 732 tests, browser and native checks ([gui.md](gui.md#validation-status),
  [history-performance.md](history-performance.md)). GitHub Actions has been disabled since then.
- **4 October 2026.** Local backend integration: 559 Windows and 510 Linux tests, release builds
  on both systems and non-elevated native probes
  ([backend-delivery.md](backend-delivery.md#validation-status)).
- **Until 10 October 2026.** GitHub Actions ran these checks on Windows and Ubuntu; its Windows
  job compiled as the runner and ran every test executable as an ephemeral standard user. Its last
  runs had failed when it was disabled ([plan, phase 0](plan.md#phase-0--remise-en-route-1718-oct)).
- **27 September 2026.** First live Nemotron runs on the development PC, under an earlier answer
  contract ([archived model comparison](archive/model-comparison.md)).
