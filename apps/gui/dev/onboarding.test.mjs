// Logic/IPC-fixture tests, not a browser or a live Nebius test.
// node --experimental-default-type=module --test apps/gui/dev/onboarding.test.mjs
import test from "node:test";
import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { shouldOfferOnboarding, leaveOnboarding, cloudError, createCloudCheck, cloudModelSelection } from "../ui/js/settings/onboarding.js";

test("first launch offers setup only for an unconfigured profile", () => {
  assert.equal(shouldOfferOnboarding(null), false);
  assert.equal(shouldOfferOnboarding({ ready: false }), true);
  assert.equal(shouldOfferOnboarding({ ready: false, key_configured: true }), true);
  assert.equal(shouldOfferOnboarding({ ready: true }), false);
  for (const marker of ["skipped", "complete", "local", "existing"]) {
    assert.equal(shouldOfferOnboarding({ ready: false }, marker), false);
  }
});

test("a deep link, navigation during boot or a resumed turn is not displaced", () => {
  for (const initialHash of ["#/settings", "#/chat", "#/setup", "#/live", "#/history", "#/storage"]) {
    assert.equal(shouldOfferOnboarding({ ready: false }, null, { initialHash }), false);
  }
  assert.equal(shouldOfferOnboarding({ ready: false }, null, { hash: "#/settings" }), false);
  assert.equal(shouldOfferOnboarding({ ready: false }, null, { hasTurn: true }), false);
});

test("catalog changes preserve saved and unsaved model IDs as visible custom choices", () => {
  const first = "nvidia/nemotron-first";
  const saved = "nvidia/nemotron-saved";
  assert.deepEqual(cloudModelSelection(saved, [{ id: first }, { id: saved }]), { choice: saved, custom: "" });
  assert.deepEqual(cloudModelSelection(saved, [{ id: first }]), { choice: "custom", custom: saved });
  assert.deepEqual(cloudModelSelection(saved, []), { choice: "custom", custom: saved });
  assert.deepEqual(cloudModelSelection(null, [{ id: first }]), { choice: first, custom: "" });
  assert.deepEqual(cloudModelSelection(null, []), { choice: "custom", custom: "" });
  assert.deepEqual(cloudModelSelection(first, [{ id: first }], { choice: saved }), { choice: "custom", custom: saved });
  assert.deepEqual(cloudModelSelection(saved, [{ id: saved }], { choice: "custom", custom: "typed-id" }), { choice: "custom", custom: "typed-id" });
});

for (const choice of ["skipped", "complete", "local"]) {
  test(`${choice} remembers progress, navigates, and only local changes privacy`, async () => {
    const calls = [];
    await leaveOnboarding(choice, {
      setPrivacy: async (mode) => calls.push(["privacy", mode]),
      remember: (value) => calls.push(["remember", value]),
      navigate: (route) => calls.push(["navigate", route]),
    });
    assert.deepEqual(calls, [...(choice === "local" ? [["privacy", "local"]] : []), ["remember", choice], ["navigate", "chat"]]);
    assert.equal(shouldOfferOnboarding({ ready: false }, choice), false);
  });
}

test("a failed local-mode write leaves the guide open and does not mark completion", async () => {
  await assert.rejects(leaveOnboarding("local", {
    setPrivacy: async () => { throw new Error("local write failed"); },
    remember: () => assert.fail("must not remember"),
    navigate: () => assert.fail("must not navigate"),
  }), /local write failed/);
  await assert.rejects(leaveOnboarding("unknown", {}), /Unknown setup choice/);
});

test("creating, invalidating or abandoning a cloud check never sends a request", () => {
  const check = createCloudCheck(() => assert.fail("unexpected IPC"));
  check.invalidate({ key: true });
  assert.deepEqual(check.state, { running: false, check: null, catalog: null, error: null });
});

test("account catalog is requested only after successful save; incompatible models are filtered", async () => {
  const calls = [];
  const fixture = { source: "synthetic", models: [
    { id: "nvidia/nemotron-synthetic", supports_tools: true },
    { id: "nvidia/nemotron-unknown" },
    { id: "nvidia/nemotron-incompatible", supports_tools: false },
  ] };
  const check = createCloudCheck(async (command) => { calls.push(command); return fixture; });
  assert.equal(await check.run("catalog", async () => { calls.push("save"); return true; }), true);
  assert.deepEqual(calls, ["save", "cloud_models"]);
  assert.deepEqual(check.state.catalog.models, fixture.models.slice(0, 2));
  assert.equal(fixture.models.length, 3);
  assert.equal(check.state.running, false);
  check.invalidate();
  assert.equal(check.state.catalog.models.length, 2);
  check.invalidate({ key: true });
  assert.equal(check.state.catalog, null);
});

test("save-only does not call Nebius, and a rejected save prevents a connection request", async () => {
  const check = createCloudCheck(() => assert.fail("unexpected network"));
  assert.equal(await check.run("save", async () => true), true);
  assert.equal(await check.run("test", async () => false), false);
  assert.equal(check.state.running, false);
});

test("a successful test is retained across redraw notifications, not across edits", async () => {
  let draws = 0;
  const check = createCloudCheck(async (command) => {
    assert.equal(command, "test_cloud");
    return { price: null };
  }, () => { draws++; });
  assert.equal(await check.run("test", async () => true), true);
  assert.deepEqual(check.state.check, { price: null });
  assert.equal(draws, 2);
  check.invalidate();
  assert.equal(check.state.check, null);
});

test("only one operation may run; stale results are discarded", async () => {
  let finish;
  const response = new Promise((resolve) => { finish = resolve; });
  const check = createCloudCheck(() => response);
  const pending = check.run("test", async () => true);
  await Promise.resolve();
  assert.equal(check.state.running, true);
  assert.equal(await check.run("catalog", () => assert.fail("duplicate save")), false);
  check.invalidate({ key: true });
  finish({ price: null });
  assert.equal(await pending, false);
  assert.equal(check.state.check, null);
  assert.equal(check.state.running, false);
});

test("invalidation during save prevents even the subsequent catalog request", async () => {
  let finish;
  const saving = new Promise((resolve) => { finish = resolve; });
  const check = createCloudCheck(() => assert.fail("stale request"));
  const pending = check.run("catalog", () => saving);
  check.invalidate({ key: true });
  finish(true);
  assert.equal(await pending, false);
});

test("empty catalog is explicit and a failed refresh does not retain a stale catalog", async () => {
  let fail = false;
  const check = createCloudCheck(async () => {
    if (fail) throw new Error("Provider request failed");
    return { models: [] };
  });
  assert.equal(await check.run("catalog", async () => true), true);
  assert.deepEqual(check.state.catalog.models, []);
  fail = true;
  assert.equal(await check.run("catalog", async () => true), false);
  assert.equal(check.state.catalog, null);
  assert.equal(check.state.error, "network");
});

const errors = [
  ["NEBIUS_API_KEY is missing or empty", "missingKey"],
  ["NEBIUS_MODEL_ID is missing or empty", "missingModel"],
  ["Select an explicit nvidia/ Nemotron model ID; no fallback model is allowed", "model"],
  ["The selected model is absent from the account catalog", "model"],
  ["Provider returned HTTP 401; response body was withheld", "key"],
  ["Provider returned HTTP 403; response body was withheld", "key"],
  ["Provider returned HTTP 429; response body was withheld", "limit"],
  ["Provider returned HTTP 503; response body was withheld", "network"],
  ["Provider did not answer within the time limit", "timeout"],
  ["Provider request failed; details and response body were withheld", "network"],
  ["synthetic-private-value-not-for-display", "unknown"],
];
test("known errors are localized categories; unexpected error text is never exposed", async () => {
  for (const [message, category] of errors) {
    assert.equal(cloudError(new Error(message)), category);
    assert.equal(cloudError(message), category);
    const check = createCloudCheck(async () => { throw new Error(message); });
    assert.equal(await check.run("test", async () => true), false);
    assert.equal(check.state.error, category);
    assert.equal(JSON.stringify(check.state).includes(message), false);
  }
});

test("all new strings exist in French and English and use the same placeholders", async () => {
  const source = await readFile(new URL("../ui/js/i18n.js", import.meta.url), "utf8");
  const tables = [...source.matchAll(/const (en|fr) = \{([\s\S]*?)\n\};/g)].map((match) => {
    const lines = match[2].split("\n").filter((line) => line.trim().startsWith('"onboarding.'));
    return JSON.parse(`{${lines.join("\n").trim().replace(/,$/, "")}}`);
  });
  assert.equal(tables.length, 2);
  const [en, fr] = tables;
  assert.deepEqual(Object.keys(en).sort(), Object.keys(fr).sort());
  for (const key of Object.keys(en)) {
    assert.ok(en[key] && fr[key]);
    assert.deepEqual(en[key].match(/\{\w+\}/g), fr[key].match(/\{\w+\}/g));
  }
  for (const category of new Set(errors.map(([, category]) => category))) assert.ok(en[`onboarding.error.${category}`]);
});

test("the Nebius password field suppresses WebView2's native reveal control", async () => {
  const css = await readFile(new URL("../ui/styles/app.css", import.meta.url), "utf8");
  assert.match(css, /\[data-key="cloud-key"\]::-ms-reveal\s*\{\s*display:\s*none;/);
});

test("progress survives reload without credentials; setup remains manually reachable", async () => {
  const saved = new Map();
  Object.defineProperty(globalThis, "navigator", { value: { language: "en" }, configurable: true });
  globalThis.document = { documentElement: { dataset: {}, classList: { toggle() {} } } };
  globalThis.window = {
    localStorage: { getItem: (key) => saved.get(key), setItem: (key, value) => saved.set(key, value) },
    matchMedia: () => ({ matches: false, addEventListener() {} }),
    location: { hash: "" },
  };
  const { loadPrefs, getPrefs, setPref } = await import("../ui/js/prefs.js");
  const { navigate, currentRoute } = await import("../ui/js/router.js");
  loadPrefs();
  setPref("onboarding", "local");
  loadPrefs();
  assert.equal(getPrefs().onboarding, "local");
  assert.equal(shouldOfferOnboarding({ ready: false }, getPrefs().onboarding), false);
  navigate("setup");
  assert.equal(currentRoute(), "setup");
  assert.doesNotMatch(saved.get("pcagent.prefs.v1"), /apiKey|NEBIUS|model_id/);
  window.localStorage.setItem = () => { throw new Error("storage unavailable"); };
  setPref("onboarding", "skipped");
  assert.equal(getPrefs().onboarding, "skipped");
});

test("the synthetic IPC fixture covers a new account without granting readiness on model save alone", async () => {
  window.location.search = "?cloud=none";
  window.setTimeout = (callback) => setTimeout(callback, 0);
  const { createMock } = await import("./mock.js?onboarding-none");
  const bridge = createMock();
  await assert.rejects(bridge.invoke("cloud_models"), /NEBIUS_API_KEY/);
  const modelId = "nvidia/nemotron-3-super-120b-a12b";
  const withoutKey = await bridge.invoke("save_cloud_settings", { modelId });
  assert.equal(withoutKey.ready, false);
  assert.equal(withoutKey.key_configured, false);
  const saved = await bridge.invoke("save_cloud_settings", { modelId, apiKey: "synthetic-input-not-a-real-key" });
  assert.equal(saved.ready, true);
  assert.doesNotMatch(JSON.stringify(saved), /synthetic-input-not-a-real-key/);
  const check = createCloudCheck((command) => bridge.invoke(command));
  assert.equal(await check.run("catalog", async () => true), true);
  assert.equal(check.state.catalog.models.length, 2);
  assert.equal(await check.run("test", async () => true), true);
});

test("the mock accepts every suggested NVIDIA Nemotron ID without changing its case", async () => {
  window.location.search = "?cloud=ready";
  const { createMock } = await import("./mock.js?onboarding-model-case");
  const bridge = createMock();
  const { models } = await bridge.invoke("app_info");
  for (const { id } of models) {
    const saved = await bridge.invoke("save_cloud_settings", { modelId: id });
    assert.equal(saved.model_id, id);
    assert.equal(saved.ready, true);
  }
  for (const modelId of ["meta/llama", "nvidia/another-model", "nvidia/nemotron test", "nvidia/nemotron?key=x", "nvidia/nemotron" + "x".repeat(200), null]) {
    await assert.rejects(bridge.invoke("save_cloud_settings", { modelId }), /Select an explicit nvidia/);
  }
});

test("the mock exposes deterministic error and empty-catalog fixtures without network", async () => {
  for (const failure of ["key", "network", "timeout", "model", "limit", "empty"]) {
    window.location.search = `?connection=${failure}`;
    const { createMock } = await import(`./mock.js?onboarding-${failure}`);
    const bridge = createMock();
    const check = createCloudCheck((command) => bridge.invoke(command));
    const ok = await check.run("catalog", async () => true);
    assert.equal(ok, failure === "empty");
    if (ok) assert.deepEqual(check.state.catalog.models, []);
    else assert.equal(check.state.error, failure);
  }
});
