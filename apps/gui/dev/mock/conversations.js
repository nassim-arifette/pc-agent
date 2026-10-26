import { wait, fail } from "./common.js";
import { createImageFixture } from "../images.js";
import { sanitize, fileNameSharing } from "./sharing.js";
import { createInvestigationRunner } from "./investigate.js";

export function createConversationFixture({ params, emit, provider, proposePlans }) {
  const { cloud } = provider;
  const webParam = params.get("web");
  let orphanTurn = params.get("turn") === "orphan" ? { turnId: "turn-orphan", question: "Why is my PC slow?", startedAt: Date.now() - 42000 } : null;

  let nextId = 1;

  const conversations = [];

  let nextConversation = 1;
  // ?conversation=empty: a saved question whose turn never finished (window closed mid-run).
  if (params.get("conversation") === "empty") {
    const at = Date.now() - 3600000;
    conversations.push({ id: `conv-${nextConversation++}`, title: "Why is my battery stuck at 6%?", created_at_unix_ms: at, updated_at_unix_ms: at, turns: [] });
  }
  function saveTurn(conversationId, turnId, question, report, image = null) {
    const c = conversations.find((x) => x.id === conversationId);
    if (!c) return null;
    c.turns.push({ id: turnId, question, asked_at_unix_ms: Date.now(), report, image: image ? imageFixture.metadata(image) : null, tool_events: report.tool_events || [] });
    imageFixture.save(image, turnId, conversationId);
    c.updated_at_unix_ms = Date.now();
    return c.id;
  }

  function done(id, report) {
    const own = running && running.id === id;
    const conversationId = own ? saveTurn(running.conversationId, running.turnId, running.question, report, running.image) : null;
    if (own) {
      const turn = { id: running.turnId, question: running.question, asked_at_unix_ms: running.askedAt, answer: report.answer, tool_events: report.tool_events || [], report, image: running.image ? imageFixture.metadata(running.image) : null };
      if (report.error) emit("turn:failed", { turn_id: running.turnId, conversation_id: conversationId, code: report.error.code, message: report.error.message, turn });
      else emit("turn:done", { turn_id: running.turnId, conversation_id: conversationId, turn });
    }
    return emit("inv:done", { id, report, conversation_id: conversationId });
  }

  let running = null;

  let nextTurn = 1;

  const privacyParam = params.get("privacy");

  let privacy = ["complete", "anonymized", "local"].includes(privacyParam)
    ? { mode: privacyParam, confirmed: true, web_search: webParam !== "key", file_names: true }
    : { mode: "complete", confirmed: false, web_search: webParam !== "key", file_names: true };

  const webActive = () => privacy.mode !== "local" && privacy.web_search && provider.hasSearchKey();

  const openedWebSources = [];
  window.__pcagentMock = { openedWebSources, holdBattery: false };
  let pendingPreview = null;

  let previewSequence = 0;

  const imageFixture = createImageFixture(fail, () => { pendingPreview = null; previewSequence += 1; });
  const investigate = createInvestigationRunner({ emit, getRunning: () => running, done, webActive, provider });

  function createCommands(getScanRoot) {
    const commands = {
      image_prepare: ({ dataBase64 }) => imageFixture.prepare(dataBase64),
      image_discard: ({ imageId }) => imageFixture.discard(imageId),
      conversation_image_get: ({ conversationId, turnId }) => imageFixture.get(turnId, conversationId),
      privacy_get: () => ({ ...privacy }),
      privacy_set: ({ settings }) => {
        const unknown = Object.keys(settings).filter((key) => !["mode", "confirmed", "web_search", "file_names"].includes(key));
        if (unknown.length) throw new Error(`unknown field ${unknown[0]}`);
        if (settings.mode !== privacy.mode || !settings.confirmed || settings.web_search !== privacy.web_search) pendingPreview = null;
        privacy = { mode: settings.mode, confirmed: Boolean(settings.confirmed), web_search: settings.web_search ?? true, file_names: settings.file_names ?? true };
        return { ...privacy };
      },
      cloud_preview: async ({ question, names, conversationId = null, fileNames = false, imageId = null }) => {
        const sequence = ++previewSequence;
        const image = imageId ? imageFixture.read(imageId) : null;
        if (image && (privacy.mode !== "complete" || !names)) fail("image_sharing_required", "Images require Complete sharing.");
        await wait(300);
        if (fileNames && !names) throw new Error("File names require complete sharing.");
        if (fileNames && !getScanRoot()) fail("scan_unavailable", "Scan and authorize a folder before previewing its names.");
        const message = { question: sanitize(question), initial_observation: { schema_version: 1, origin: "synthetic_fixture", measurements: { values: { system: { physical_total_bytes: 16888365056, physical_available_bytes: 1653562000, cpu_busy_basis_points: 1850 }, "proc-1a2b3c4d": { process_working_set_bytes: 2480000000 } }, units: { physical_total_bytes: "bytes" } } }, findings: [{ id: "F1", classification: "low_available_memory" }], current_finding_ids: ["F1", "F2", "F3"] };
        if (names) message.process_names = { "proc-1a2b3c4d": "chrome.exe" };
        const conversation = conversations.find((c) => c.id === conversationId);
        if (conversation?.turns.length) message.previous_turns = conversation.turns.map((turn) => ({ question: turn.question, answer: "Earlier synthetic observation.", observations: [] }));
        if (sequence !== previewSequence) fail("preview_changed", "A newer sharing preview was requested.");
        const id = `preview-${sequence}`;
        pendingPreview = { id, question, names, conversationId, fileNames, imageId };
        return { id, message, image, bytes: JSON.stringify(message).length + (image?.data_url.length || 0), measured: 38, findings: 3, names: names ? 1 : 0, storage_folder: getScanRoot(), file_name_sharing: fileNames ? fileNameSharing(getScanRoot()) : { names: [], scope: null } };
      },
      send_message: async ({ conversationId = null, text, scopeIds = [], previewId = null, imageId = null }) => {
        const cloudRun = privacy.mode !== "local";
        if (scopeIds.length > 1) throw new Error("Attach one scanned folder to a message.");
        const image = imageId ? imageFixture.read(imageId) : null;
        if (image && privacy.mode !== "complete") fail("image_sharing_required", "Images require Complete sharing.");
        return commands.start_investigation({ request: { question: text, cloud: cloudRun, names: privacy.mode === "complete", conversationId, previewId, image } });
      },
      current_turn: () =>
        orphanTurn ? { turn_id: orphanTurn.turnId, conversation_id: null, question: orphanTurn.question, started_at_unix_ms: orphanTurn.startedAt, stopping: false, extension: null } : null,
      stop_turn: ({ turnId }) => {
        if (orphanTurn && orphanTurn.turnId === turnId) {
          orphanTurn = null;
          window.setTimeout(() => emit("turn:failed", { turn_id: turnId, conversation_id: null, code: "cancelled", message: "Stopped.", turn: null }), 300);
          return;
        }
        if (!running || running.turnId !== turnId || running.done) throw new Error("This turn is over.");
        running.stop = true;
        running.answer?.(false);
      },
      start_investigation: async ({ request: { question, cloud: cloudRun, names, conversationId = null, previewId, image = null } }) => {
        if (running && !running.done) fail("turn_running", "An investigation is already running.");
        if (cloudRun) {
          if (!pendingPreview || pendingPreview.id !== previewId || pendingPreview.question !== question || pendingPreview.names !== names || pendingPreview.conversationId !== conversationId) fail("preview_changed", "The question, conversation or sharing choice changed. Open the preview again.");
          if ((pendingPreview.imageId ?? null) !== (image?.id ?? null)) fail("preview_changed", "The image changed.");
          pendingPreview = null;
          // ?preview=expired: the ticket outlived its five minutes, so nothing leaves.
          if (params.get("preview") === "expired") fail("preview_expired", "The preview expired. Open it again for a fresh measurement.");
        }
        const id = nextId++;
        let conversation = conversations.find((c) => c.id === conversationId);
        if (!conversation) {
          const now = Date.now();
          conversation = { id: `conv-${nextConversation++}`, title: question.split("\n")[0].slice(0, 80), created_at_unix_ms: now, updated_at_unix_ms: now, turns: [] };
          conversations.unshift(conversation);
        }
        running = { id, turnId: `turn-${nextTurn++}`, askedAt: Date.now(), stop: false, answer: null, done: false, conversationId: conversation.id, question, image };
        if (image) imageFixture.discard(image.id);
        if (cloudRun) proposePlans(running.turnId, question);
        investigate(id, question, cloudRun).finally(() => {
          running.done = true;
        });
        return { id, conversation_id: conversation.id, turn_id: running.turnId };
      },
      conversations_list: async () =>
        conversations
          .slice()
          .sort((a, b) => b.updated_at_unix_ms - a.updated_at_unix_ms)
          .map(({ turns, ...c }) => ({ ...c, turn_count: turns.length, last_turn_status: turns.length && turns[turns.length - 1].report?.error ? "failed" : "completed" })),
      conversation_get: async ({ id }) => {
        const c = conversations.find((x) => x.id === id);
        return c ? { ...c, turn_count: c.turns.length, turns: c.turns.slice() } : null;
      },
      conversation_delete: async ({ id }) => {
        const i = conversations.findIndex((x) => x.id === id);
        if (i < 0) return false;
        conversations.splice(i, 1);
        imageFixture.delete(id);
        return true;
      },
      conversations_clear: async () => {
        const n = conversations.length;
        conversations.length = 0;
        imageFixture.clear();
        return n;
      },
      answer_extension: ({ accept }) => {
        running?.answer?.(accept);
      },
      stop_investigation: () => {
        if (running) {
          running.stop = true;
          running.answer?.(false);
        }
      },
      open_web_source: ({ conversationId, turnId, reference }) => {
        const conversation = conversations.find((c) => c.id === conversationId);
        if (!conversation) fail("web_source_unavailable", "This conversation no longer exists.");
        const turn = conversation.turns.find((x) => x.id === turnId);
        if (!turn) fail("web_source_unavailable", "This answer no longer exists.");
        const cited = (turn.report?.answer?.citations || []).find((c) => c.reference === reference && c.kind === "web");
        if (!cited?.web?.url) fail("web_source_unavailable", "This answer has no such web source.");
        openedWebSources.push({ conversationId, turnId, reference, at: Date.now() });
        return null;
      }
    };
    return commands;
  }

  return { createCommands, getPrivacy: () => privacy, nextPreviewSequence: () => ++previewSequence, preparePreview: (preview) => { pendingPreview = preview; } };
}
