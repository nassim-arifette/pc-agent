# Watches (C7)

Local threshold rules checked by the window's sampler, with a Windows notification when one is
crossed. Contract: `docs/plan.md`, C7. Nothing here calls the model or the network.

## Rules and evaluation

- Five closed rule kinds (`resource_above`, `volume_free_below`, `gpu_temperature_above`,
  `app_cpu_above`, `app_memory_above`), validated in the page (`ui/js/watches/model.js`) and again
  in Rust, which also proves the target is observable (`Sampler::validate_watch_target`).
- Evaluated in the window's sampler only (`crates/store/src/watch_engine.rs`), on a monotonic
  clock, with hysteresis on return and "no reading" instead of zero. One window evaluates at a time
  (SQLite lease); another window shows itself paused.
- Rules, alerts (7 days), the notification language and the lease live in `pcagent.sqlite`
  (migration 7). Clear history removes alerts and keeps rules.

## Surfaces

- **Watches view** (`#/watches`, `ui/js/watches/view.js`): each rule with its live phase (normal,
  waiting, in alert, no reading with its reason, off, paused), the latest reading with its date and
  whether it is a lower bound, an on/off switch, Edit and Delete (confirmed); the alerts of the last
  7 days, paged by 50, with their extreme value, how they ended and why a notification was held
  back; Mark as seen; Explain; the local weekly summary on click; a test notification.
- **Monitor** in the action bar of Live PC (CPU, memory, fixed drives, an NVIDIA GPU whose
  temperature is read, a named application or process), History (CPU, memory and disk events) and
  Storage (the scanned item's fixed drive). It opens the shared form prefilled; nothing persists
  before its confirmation. What cannot be watched keeps the button disabled with its reason.
- **Rail badge** with the unseen-alert count, a toast when an alert opens in this window, and the
  Watches view when a notification is clicked (`watch:open`, or `watch_activation_take` at start).
- **Explain** on an alert publishes the frozen `watch_alert` source (`watch_alert_detail`) and
  goes through the selection preview, so the exact message is shown before anything is sent; the
  program name leaves only when program names are shared.
- **Closing the window** hides it in the notification area while at least one rule is enabled, so
  watching continues; Quit PC Agent stops the sampler.
- The page reports its language (`watch_language_set`), which notifications and the daily report
  use.

## Validation

Linux: Rust tests of the store, engine and projections; Node tests of the view, the Live PC
proposals, the runtime and the mock contracts (`apps/gui/dev/watches-view.test.mjs`,
`mock.test.mjs`); captures of the view in EN/FR light/dark and of Live PC → Monitor → form →
rule, and Explain on an alert, with the mock (`?watches=none|paused`, `?notifications=disabled`,
`?activation=alert`).

Not validated (Windows, standard account): a real CPU load raising an alert, the toast, its click
opening Watches, the notification-area behaviour on close, and Explain with Nebius.
