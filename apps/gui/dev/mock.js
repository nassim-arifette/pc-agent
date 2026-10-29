// Design-time stand-in for the Rust side, used when ui/index.html is opened in a plain browser
// (python -m http.server in apps/gui, then /ui/index.html). Every value is synthetic. The app
// never loads this file: it is outside the embedded ui folder.
//
// URL switches: ?cloud=none|ready|problem, ?connection=key|network|timeout|model|limit|empty,
// ?gpu=none, ?disk=ok, ?privacy=complete|anonymized|local
// (a confirmed choice; without it the first cloud question asks for one), ?elevation=fail|slow
// (fail: Windows' administrator prompt is declined), ?recording=short (3-second recordings),
// ?battery=none|ac|charging|low|relative|two|unavailable
// (the Live PC battery read: by default one discharging laptop battery; none is a desktop PC, also
// without a battery in recordings), ?sources=short (selection sources expire after 5 s instead of
// five minutes, to exercise a scan page read again on Explain or Investigate), ?pc=desktop (a
// desktop without an NVIDIA GPU or a battery, whose D: drive is exFAT: app_info's capabilities say
// so once the first dashboard is measured), ?ntfs=none (no fixed NTFS drive for the quick drive
// analysis), ?helper=missing (no pcagent-elevated.exe next to the app). Questions containing
// "extend" ask for a budget extension; "fail" makes the model's answer unusable; "slow"
// stretches the steps; "space" or "clean" propose cleanups and "startup" a start-up entry to turn
// off; "health" or "santé" lacks administrator readings, so the deeper checks card follows the
// answer. Both event families are emitted, like the Rust side: turn:* and inv:*.
// ?routing=calibrated|escalated|escalation_failed|stale covers model routing (chantier 14): Auto
// offered, every AI answer handed from Super to Ultra (or Ultra's request failing), and a saved
// Auto choice rejected at start-up; see mock/provider.js.
// Live captures, history reads and scan pages publish selection sources; Explain and Investigate
// use selection_preview/send.
// Failures have the Rust shape { code, message } (src/failure.rs), translated by the page. Error
// switches: ?scan=denied (the folder cannot be read) or ?scan=fail (the scan stops midway),
// ?action=refuse (Run now is refused: the target changed), ?preview=expired (a question's preview
// ticket expired, nothing is sent), ?history=unavailable and ?live=fail.

import { createProviderFixture } from "./mock/provider.js";
import { createLiveFixture } from "./mock/live.js";
import { createActionFixture } from "./mock/actions.js";
import { createHistoryFixture } from "./mock/history.js";
import { createElevationFixture } from "./mock/elevation.js";
import { createStorageFixture } from "./mock/storage.js";
import { createSelectionFixture } from "./mock/selection.js";
import { createConversationFixture } from "./mock/conversations.js";

const params = new URLSearchParams(window.location.search);

const listeners = new Map();

function emit(name, payload) {
  for (const cb of listeners.get(name) || []) cb(payload);
}

const provider = createProviderFixture(params);
const actions = createActionFixture(params);
const conversation = createConversationFixture({ params, emit, provider, proposePlans: actions.proposePlans });
const selection = createSelectionFixture({
  params, cloud: provider.cloud, getPrivacy: conversation.getPrivacy,
  nextPreviewSequence: conversation.nextPreviewSequence, preparePreview: conversation.preparePreview,
});
const live = createLiveFixture({ params, publishSource: selection.publishSource, cloud: provider.cloud });
const history = createHistoryFixture({ params, batteryMode: live.batteryMode, publishSource: selection.publishSource });
const storage = createStorageFixture({ params, emit, publishSource: selection.publishSource, invalidateStorage: selection.invalidateStorage });
const elevation = createElevationFixture(params, selection.publishSource);

export function createMock() {
  const scan = storage.createCommands();
  const turns = conversation.createCommands(scan.getRoot);
  const commands = {
    ...provider.commands, ...live.commands, ...history.commands, ...actions.commands, ...elevation.commands,
    ...scan.commands, ...turns, ...selection.createCommands(turns.start_investigation),
  };
  return {
    mock: true,
    invoke: async (command, args = {}) => {
      const fn = commands[command];
      if (!fn) throw new Error(`Unknown command ${command}`);
      return fn(args);
    },
    listen: (name, cb) => {
      if (!listeners.has(name)) listeners.set(name, new Set());
      listeners.get(name).add(cb);
      return Promise.resolve(() => listeners.get(name).delete(cb));
    },
    onDrop: () => () => {},
  };
}
