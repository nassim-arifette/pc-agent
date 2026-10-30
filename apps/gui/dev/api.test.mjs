import test from "node:test";
import assert from "node:assert/strict";
import { connect, call, on, onDrop } from "../ui/js/api.js";

test("without the native host, connect refuses to supply application data", async () => {
  globalThis.window = {};
  await assert.rejects(connect(), { code: "host_unavailable" });
  await assert.rejects(call("dashboard"));
  window.__TAURI__ = { core: {} };
  await assert.rejects(connect(), { code: "host_unavailable" });
});

test("the native bridge preserves commands, event payloads and unsubscribe", async () => {
  const calls = [];
  let listener;
  let unsubscribed = false;
  window.__TAURI__ = {
    core: { invoke: async (...args) => { calls.push(args); return { measured: true }; } },
    event: { listen: async (name, cb) => {
      calls.push([name]);
      listener = cb;
      return () => { unsubscribed = true; };
    } },
  };
  await connect();
  const args = { id: "local-selection" };
  assert.deepEqual(await call("selection_sources", args), { measured: true });
  assert.deepEqual(calls[0], ["selection_sources", args]);
  let received;
  const off = await on("turn:done", (payload) => { received = payload; });
  listener({ payload: { turn_id: "turn-1" } });
  assert.deepEqual(received, { turn_id: "turn-1" });
  off();
  assert.equal(unsubscribed, true);
  assert.equal(typeof onDrop(() => {}), "function");
  delete window.__TAURI__;
  await assert.rejects(connect(), { code: "host_unavailable" });
  await assert.rejects(call("dashboard"));
  assert.equal(calls.length, 2);
});
