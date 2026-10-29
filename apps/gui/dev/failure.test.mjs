// Logic tests for failed commands, not a browser test: the shapes api.js accepts, the sentences
// failure.js shows, and the mock's failures read in French.
// node --experimental-default-type=module --test apps/gui/dev/failure.test.mjs
import test from "node:test";
import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { en } from "../ui/js/i18n/en.js";
import { fr } from "../ui/js/i18n/fr.js";

// i18n.js reads the browser language only when asked; the mock reads its URL switches on load.
Object.defineProperty(globalThis, "navigator", { value: { language: "fr-FR" }, configurable: true });
globalThis.document = { documentElement: { lang: "en", dataset: {}, classList: { toggle() {} } } };
globalThis.window = { location: { search: "" }, setTimeout: (callback) => setTimeout(callback, 0), clearTimeout };

const { call, commandError, connect } = await import("../ui/js/api.js");
const { language, setLanguage, t } = await import("../ui/js/i18n.js");
const { failureDetail, failureText, pageFailure, refusalText, startFailure, turnFailureText } = await import("../ui/js/failure.js");

/** The `failure.*` entries and the two turn codes this change added, per dictionary. */
const tables = Object.fromEntries(Object.entries({ en, fr }).map(([locale, dictionary]) => {
  return [locale, Object.fromEntries(Object.entries(dictionary).filter(([key]) =>
    key.startsWith("failure.") || key === "error.start" || key === "error.preview_required"))];
}));

test("a failed command keeps one shape, whatever the host sent", () => {
  const plain = commandError("The measurement thread stopped; restart PC Agent.");
  assert.ok(plain instanceof Error);
  assert.equal(plain.message, "The measurement thread stopped; restart PC Agent.");
  assert.equal(plain.code, "unknown");
  assert.equal(plain.reason, undefined);
  const coded = commandError({ code: "uac_declined", message: "The Windows administrator prompt was declined (UacDeclined)." });
  assert.ok(coded instanceof Error);
  assert.equal(coded.code, "uac_declined");
  assert.equal(coded.message, "The Windows administrator prompt was declined (UacDeclined).");
  const refused = commandError({ code: "action_refused", reason: "changed", message: "target t1 changed since it was resolved" });
  assert.deepEqual([refused.code, refused.reason], ["action_refused", "changed"]);
  // A code that is not one of ours, or not a code at all, reads as unknown.
  for (const error of [new Error("boom"), { code: "UacDeclined", message: "x" }, { code: "<b>x</b>", reason: "a b", message: "x" }, { code: 42, message: 7 }, null, undefined]) {
    const normalized = commandError(error);
    assert.equal(normalized.code, "unknown");
    assert.equal(normalized.reason, undefined);
    assert.equal(typeof normalized.message, "string");
  }
});

test("call() rejects with the normalized error for both bridge shapes", async () => {
  let rejection = "Local history is unavailable.";
  window.__TAURI__ = { core: { invoke: async () => { throw rejection; } }, event: { listen: async () => () => {} } };
  await connect();
  await assert.rejects(call("metric_history"), (error) => error instanceof Error && error.code === "unknown" && error.message === "Local history is unavailable.");
  rejection = { code: "history_unavailable", message: "Local history is unavailable." };
  await assert.rejects(call("metric_history"), (error) => error.code === "history_unavailable" && error.message === "Local history is unavailable.");
  delete window.__TAURI__;
});

test("a code reads as a French sentence; anything else as the generic one", () => {
  setLanguage("fr");
  assert.equal(language(), "fr");
  const declined = commandError({ code: "uac_declined", message: "The Windows administrator prompt was declined (UacDeclined)." });
  assert.equal(failureText(declined), tables.fr["failure.uac_declined"]);
  assert.doesNotMatch(failureText(declined), /UacDeclined|declined/);
  for (const error of [commandError({ code: "made_up_code", message: "Some new English sentence." }), commandError("A bare English sentence."), null]) {
    assert.equal(failureText(error), tables.fr["failure.unknown"]);
  }
  // A fallback key replaces the generic sentence when the caller has a better one.
  assert.equal(failureText(commandError("x"), "error.start"), tables.fr["error.start"]);
  // The page's own errors are already in the interface language.
  assert.equal(failureText(pageFailure("share.changed")), t("share.changed"));
  assert.match(t("share.changed"), /Renvoyez/);
  // A refused action says why, or only that an error occurred.
  const changed = commandError({ code: "action_refused", reason: "changed", message: "target t1 changed since it was resolved" });
  assert.equal(failureText(changed), tables.fr["failure.action_refused"].replace("{reason}", tables.fr["failure.action_refused.changed"]));
  assert.equal(refusalText(changed), tables.fr["failure.action_refused.changed"]);
  assert.equal(refusalText(commandError({ code: "action_refused", reason: "made_up", message: "x" })), tables.fr["failure.action_refused.other"]);
  assert.equal(refusalText(undefined), tables.fr["failure.action_refused.other"]);
  setLanguage("en");
  assert.equal(failureText(declined), tables.en["failure.uac_declined"]);
  setLanguage("fr");
});

test("a failed turn reads its C3 code, or why its start was refused", () => {
  setLanguage("fr");
  assert.equal(turnFailureText({ code: "preview_required", message: "Open the model sharing preview before sending." }), tables.fr["error.preview_required"]);
  assert.equal(turnFailureText({ code: "cancelled", message: "Stopped by the user" }), t("error.cancelled"));
  assert.equal(turnFailureText({ code: "error", message: "Something new" }), tables.fr["failure.unknown"]);
  assert.equal(turnFailureText(startFailure(commandError("Cannot start the investigation: os error 8"))), tables.fr["error.start"]);
  assert.equal(turnFailureText(startFailure(commandError({ code: "preview_expired", message: "The preview expired. Open it again for a fresh measurement." }))), tables.fr["failure.preview_expired"]);
  assert.equal(turnFailureText(startFailure(pageFailure("select.conversationGone"))), t("select.conversationGone"));
  // Do not leak raw English through accessible title attributes in French mode.
  assert.equal(failureDetail(startFailure(commandError({ code: "turn_running", message: "An investigation is already running." }))), null);
  assert.equal(failureDetail({ code: "cancelled", message: "Stopped by the user" }), null);
  assert.equal(failureDetail(startFailure(pageFailure("share.changed"))), null);
  assert.equal(failureDetail(commandError({ code: "unknown", message: "  " })), null);
  assert.equal(failureDetail(null), null);
  setLanguage("en");
  assert.equal(failureDetail({ code: "cancelled", message: "Stopped by the user" }), "Stopped by the user");
  setLanguage("fr");
});

test("every failure sentence exists in English and French, with the same placeholders", () => {
  const [en, fr] = [tables.en, tables.fr];
  assert.deepEqual(Object.keys(fr).sort(), Object.keys(en).sort());
  for (const key of Object.keys(en)) {
    assert.ok(en[key] && fr[key], key);
    assert.deepEqual(en[key].match(/\{\w+\}/g), fr[key].match(/\{\w+\}/g), key);
    assert.notEqual(fr[key], en[key], `${key} is translated`);
  }
});

test("every code and refusal the host declares has a sentence", async () => {
  const rust = (await readFile(new URL("../src/failure.rs", import.meta.url), "utf8")).replace(/\r\n/g, "\n");
  const variants = (name) => {
    const body = rust.match(new RegExp(`\\n    ${name} \\{\\n([\\s\\S]*?)\\n    \\}`))[1];
    return [...body.matchAll(/^\s+([A-Z]\w*),$/gm)].map((m) => m[1].replace(/(?<!^)([A-Z])/g, "_$1").toLowerCase());
  };
  const codes = variants("Code");
  const reasons = variants("Refusal");
  assert.ok(codes.includes("uac_declined") && codes.includes("action_refused") && codes.length >= 30);
  assert.ok(reasons.includes("restore_unavailable"));
  for (const code of codes) assert.ok(tables.en[`failure.${code}`] && tables.fr[`failure.${code}`], code);
  for (const reason of reasons) assert.ok(tables.fr[`failure.action_refused.${reason}`], reason);
});

/** A fresh mock with these URL switches, its failures normalized as call() does. */
async function mock(search) {
  window.location.search = search;
  const { createMock } = await import(`./mock.js?failure-${encodeURIComponent(search)}`);
  const bridge = createMock();
  return {
    bridge,
    invoke: async (command, args) => {
      try {
        return await bridge.invoke(command, args);
      } catch (error) {
        throw commandError(error);
      }
    },
  };
}

/** Closed-code French presentation leaves the original error payload unchanged. */
function assertFrench(error, code, detail) {
  assert.equal(error.code, code);
  const sentence = tables.fr[`failure.${code}`];
  assert.equal(failureText(error), code === "action_refused" ? sentence.replace("{reason}", refusalText(error)) : sentence);
  assert.notEqual(failureText(error), error.message);
  assert.equal(failureDetail(error), null);
  assert.match(error.message, detail);
}

test("the mock's error switches reject like the host and read in French", async () => {
  setLanguage("fr");
  {
    const { invoke } = await mock("?elevation=fail");
    const request = await invoke("elevation_prepare", { task: { kind: "system_caches" }, budgetMs: 120000 });
    await assert.rejects(invoke("elevation_prepare", { task: { kind: "shadow_storage" }, budgetMs: 120000 }), (error) => (assertFrench(error, "helper_busy", /already prepared/), true));
    await assert.rejects(invoke("elevation_run", { requestId: request.request_id }), (error) => (assertFrench(error, "uac_declined", /UacDeclined/), true));
    await assert.rejects(invoke("elevation_cancel", { requestId: request.request_id }), (error) => (assertFrench(error, "request_expired", /no longer available/), true));
  }
  {
    const { invoke } = await mock("?scan=denied");
    await assert.rejects(invoke("start_scan", { path: "C:\\Users\\Alex\\Downloads" }), (error) => (assertFrench(error, "folder_unreadable", /PermissionDenied/), true));
  }
  {
    const { bridge, invoke } = await mock("?scan=fail");
    const failed = new Promise((resolve) => bridge.listen("scan:failed", resolve));
    await invoke("start_scan", { path: "C:\\Users\\Alex\\Downloads" });
    const payload = await failed;
    assertFrench(commandError({ code: payload.code, message: payload.message }), "scan_failed", /scan worker panicked/);
  }
  {
    const { invoke } = await mock("?preview=expired");
    const question = "Pourquoi mon PC est-il lent ?";
    const preview = await invoke("cloud_preview", { question, names: true, conversationId: null });
    await assert.rejects(invoke("send_message", { conversationId: null, text: question, scopeIds: [], previewId: preview.id }), (error) => (assertFrench(error, "preview_expired", /preview expired/), true));
  }
  {
    const { invoke } = await mock("?action=refuse");
    const question = "Comment libérer de la place ?";
    const preview = await invoke("cloud_preview", { question, names: true, conversationId: null });
    const started = await invoke("send_message", { conversationId: null, text: question, scopeIds: [], previewId: preview.id });
    const [plan] = await invoke("action_plans", { turnId: started.turn_id });
    await assert.rejects(invoke("action_run", { turnId: started.turn_id, planId: plan.plan_id }), (error) => (assertFrench(error, "action_refused", /changed/), assert.equal(error.reason, "changed"), true));
    const [after] = await invoke("action_plans", { turnId: started.turn_id });
    assert.equal(after.decision, "refused");
    await assert.rejects(invoke("action_run", { turnId: started.turn_id, planId: plan.plan_id }), (error) => (assert.equal(error.reason, "already_decided"), true));
  }
  {
    const { invoke } = await mock("?history=unavailable&live=fail");
    await assert.rejects(invoke("metric_history", { metric: "cpu_busy_basis_points", fromUnixMs: 0, toUnixMs: 1 }), (error) => (assertFrench(error, "history_unavailable", /unavailable/), true));
    await assert.rejects(invoke("dashboard"), (error) => (assertFrench(error, "live_unavailable", /too long/), true));
  }
});
