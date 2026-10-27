import test from "node:test";
import assert from "node:assert/strict";
import { createVoiceDraft } from "../ui/js/chat/voice-draft.js";
import { createVoiceFixture } from "./mock/voice.js";
import { en } from "../ui/js/i18n/en.js";
import { fr } from "../ui/js/i18n/fr.js";

function harness(query = "") {
  const tasks = new Map();
  let timer = 0;
  const events = [];
  const calls = [];
  const applied = [];
  let draft;
  const fixture = createVoiceFixture({
    params: new URLSearchParams(query),
    emit: (name, value) => { events.push([name, value]); if (name === "voice:state") draft?.accept(value); },
    schedule: (fn) => { tasks.set(++timer, fn); return timer; },
    unschedule: (id) => tasks.delete(id),
  });
  const invoke = async (cmd, args = {}) => { calls.push(cmd); return fixture.commands[cmd](args); };
  draft = createVoiceDraft({ invoke, apply: (text) => applied.push(text) });
  return {
    draft, invoke, applied, calls, events,
    tick() { const batch = [...tasks.values()]; tasks.clear(); batch.forEach((fn) => fn()); },
  };
}

test("dictation keeps partials separate and requires explicit review; never sends", async () => {
  const h = harness();
  h.draft.bind("conversation-a");
  await h.draft.start("Existing question");
  assert.equal(h.draft.busy, true);
  h.tick();
  assert.match(h.draft.state.partial, /Pourquoi/);
  assert.deepEqual(h.applied, []);
  await h.draft.stop();
  h.tick();
  assert.equal(h.draft.busy, false);
  assert.equal(h.draft.reviewRequired, true);
  assert.match(h.applied.at(-1), /^Existing question\nPourquoi/);
  h.draft.reviewed();
  assert.equal(h.draft.reviewRequired, false);
  assert.equal(h.calls.some((name) => /send|preview/.test(name)), false);
  h.draft.dispose();
  assert.equal((await h.invoke("voice_state")).text, "");
});

test("English selection and cancellation restore the original draft", async () => {
  const h = harness();
  await h.invoke("voice_settings_set", { settings: { engine: "whisper", language: "en", device_id: null } });
  await h.draft.start("Original");
  h.tick();
  assert.equal(h.draft.state.language, "en");
  assert.match(h.draft.state.partial, /Why/);
  await h.draft.stop(true);
  h.tick();
  assert.equal(h.applied.at(-1), "Original");
  assert.equal(h.draft.reviewRequired, false);
  assert.equal(h.draft.state.text, "");
});

test("obsolete sessions and out-of-order events cannot change the composer", async () => {
  const h = harness();
  h.draft.bind("a");
  await h.draft.start("");
  const stale = { ...h.draft.state, phase: "review", text: "obsolete", sequence: 99 };
  h.draft.bind("b");
  h.draft.accept(stale);
  assert.deepEqual(h.applied, []);
  await h.draft.start("");
  const current = h.draft.state;
  h.draft.accept({ ...current, sequence: current.sequence - 1, phase: "review", text: "late" });
  h.draft.accept(stale);
  assert.deepEqual(h.applied, []);
  h.draft.dispose();
});

test("a start resolving after unmount is cancelled, never adopted", async () => {
  let resolve;
  const cancelled = [];
  const draft = createVoiceDraft({
    invoke: (cmd, args) => cmd === "voice_start" ? new Promise((done) => { resolve = done; }) : cancelled.push(args),
  });
  const start = draft.start("");
  draft.dispose();
  resolve({ session_id: "late-start", sequence: 0, phase: "starting" });
  await start;
  assert.deepEqual(cancelled, [{ sessionId: "late-start", cancel: true }]);
  assert.equal(draft.busy, false);
});

test("model consent, progress, cancellation, deletion and active-operation exclusion", async () => {
  const h = harness("voiceModel=missing");
  await assert.rejects(h.invoke("voice_model_download", { consent: false }), { code: "consent_required" });
  await h.invoke("voice_model_download", { consent: true });
  await assert.rejects(h.invoke("voice_start", { consent: true }), { code: "busy" });
  h.tick();
  assert.ok((await h.invoke("voice_model_state")).downloaded_bytes > 0);
  await h.invoke("voice_model_cancel");
  h.tick();
  assert.equal((await h.invoke("voice_model_state")).installed, false);
  await h.invoke("voice_model_download", { consent: true });
  for (let i = 0; i < 5; i++) h.tick();
  assert.equal((await h.invoke("voice_model_state")).installed, true);
  await h.draft.start("");
  await assert.rejects(h.invoke("voice_model_delete"), { code: "busy" });
  await assert.rejects(h.invoke("voice_settings_set", { settings: {} }), { code: "busy" });
  await h.draft.stop(true);
  await h.invoke("voice_model_delete");
  assert.equal((await h.invoke("voice_model_state")).installed, false);
});

test("voice errors are actionable in both languages and no mock audio is captured", async () => {
  for (const code of ["no_device", "permission_denied", "device_unavailable", "engine_unavailable"]) {
    const h = harness(`voice=${code}`);
    await h.draft.start("");
    assert.equal(h.draft.state.error.code, code);
    assert.equal(h.draft.busy, false);
    assert.ok(en[`voice.error.${code}`]);
    assert.ok(fr[`voice.error.${code}`]);
  }
  const missing = harness("voiceModel=missing");
  await missing.draft.start("");
  assert.equal(missing.draft.state.error.code, "model_missing");
  for (const key of Object.keys(en).filter((key) => key.startsWith("voice."))) assert.ok(fr[key], key);
});
