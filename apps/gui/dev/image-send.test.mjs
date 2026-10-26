import test from "node:test";
import assert from "node:assert/strict";

Object.defineProperty(globalThis, "navigator", { value: { language: "en" }, configurable: true });
globalThis.document = { documentElement: { lang: "en", dataset: {}, classList: { toggle() {} } } };
const requests = [];
globalThis.window = {
  location: { hash: "#/chat", search: "" },
  matchMedia: () => ({ matches: false, addEventListener() {} }),
  sessionStorage: { getItem: () => null, setItem() {} },
  __TAURI__: {
    core: { invoke: async (command, args) => {
      requests.push({ command, args });
      if (command === "cloud_preview") return { id: "ticket", message: { question: args.question }, storage_folder: null };
      if (command === "send_message") return { turn_id: "turn", conversation_id: "conversation" };
      if (command === "conversations_list") return [];
      throw Error(`Unexpected command: ${command}`);
    } },
    event: { listen: async () => () => {} },
  },
};
const { connect } = await import("../ui/js/api.js");
const { state } = await import("../ui/js/store.js");
const { ask } = await import("../ui/js/chat/composer.js");
const { IMAGE_MODEL_ID } = await import("../ui/js/chat/image-draft.js");
await connect();

test("the same opaque image reference reaches preview and send, without copying pixels to steps", async () => {
  state.info = { cloud: { ready: true, model_id: "nvidia/nemotron-3-super-120b-a12b" } };
  state.privacy = { mode: "complete", confirmed: true, web_search: false, file_names: false };
  state.inv = null;
  state.conversation = null;
  requests.length = 0;
  assert.equal(await ask("Look at this error", { imageId: "opaque-image" }), true);
  const preview = requests.find((r) => r.command === "cloud_preview").args;
  const sent = requests.find((r) => r.command === "send_message").args;
  assert.equal(preview.imageId, "opaque-image");
  assert.equal(sent.imageId, preview.imageId);
  assert.equal(sent.previewId, "ticket");
  assert.equal(state.inv.model, IMAGE_MODEL_ID);
  assert.deepEqual(state.inv.sent, { question: "Look at this error" });
  assert.equal(state.info.cloud.model_id, "nvidia/nemotron-3-super-120b-a12b");

  state.inv.status = "done";
  requests.length = 0;
  assert.equal(await ask("And now?", { followUp: true }), true);
  const next = requests.find((r) => r.command === "send_message").args;
  assert.equal(next.imageId, null);
  assert.equal(next.conversationId, "conversation");
  assert.equal(state.inv.model, state.info.cloud.model_id);
});
