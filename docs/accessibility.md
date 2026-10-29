# Language and accessibility (#71)

What the interface and the CLI do for French readers and assistive technologies, and what has
actually been measured. Nothing below claims a screen-reader or native Windows acceptance.

## Language

- **One closed catalogue.** Interface text, core measurement phrases and CLI sentences come from
  the EN/FR dictionaries (`apps/gui/ui/js/i18n/`, `ui/js/phrases.js`), generated into
  `crates/shared/src/phrases.json` for the Rust renderers (`node apps/gui/dev/core-catalog.mjs`).
  Tests check that both languages have the same keys and placeholders.
- **Never translated:** local names (folders, files, programs), the model's prose and web
  quotations. They are rendered as received and marked with `data-i18n-source` (`local_name`,
  `model`, `web`) so the audit can tell them from interface text.
- **CLI.** `pcagent --language fr` renders the storage report, content comparison, storage
  context, doctor report, action review and approval prompt, investigation extension prompt, chat
  intro and question errors in French; `oui` is accepted where `yes` is. Fixed storage, policy
  and storage-context sentences are rendered by exact match from a closed table
  (`apps/cli/src/text.rs`, `CORE_TEXT`); any other text is shown unchanged. Machine output
  (`--json`) stays English.
- **Rendered audit.** `apps/gui/dev/i18n-audit.mjs` renders every state of the UI tour in EN and
  FR (light, dark and high contrast) and reports English interface text in the French rendering.
  On Linux: `CHROME_NO_SANDBOX=1 node --experimental-default-type=module
  apps/gui/dev/i18n-audit.mjs <out> /path/to/chrome`.

## Accessibility

- Dialogs trap focus and make the background inert; focus is visible; forced-colors (Windows
  high contrast) has its own styles; reduced motion is respected; streamed answers are announced
  in batches through a polite live region.
- Switches use `role="switch"` with `aria-checked`; disabled action-bar buttons describe why
  through `aria-describedby`; the Watches rail badge carries an accessible count.

## Measured

- 29 October, with the mock in headless Chromium (Linux): the rendered audit covered 222 states.
  The first run reported 27 English leaks in the French rendering, all folder names of the
  Storage map, tree and inspector that were not marked as local names; after marking them, the
  result is recorded in the plan's follow-up table.
- Node tests: dictionary parity and placeholders, the audit's classification, the CLI closed
  sentences.

## Not measured yet

Narrator and NVDA in the native app, an axe or Accessibility Insights pass, contrast ratios,
keyboard-only navigation of the native window, and the French typography review by a person.
