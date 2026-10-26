// Logic/IPC-fixture tests of which models answered and what each cost (chantier 14): the page's
// helpers on synthetic reports, and the mock's routing scenarios. Not a browser or a live test.
// node --experimental-default-type=module --test apps/gui/dev/usage.test.mjs
import test from "node:test";
import assert from "node:assert/strict";

Object.defineProperty(globalThis, "navigator", { value: { language: "en" }, configurable: true });
globalThis.document = { documentElement: { dataset: {}, classList: { toggle() {} } } };
globalThis.window = { location: { search: "", hash: "" }, setTimeout: (callback) => setTimeout(callback, 0) };

const { setLanguage } = await import("../ui/js/i18n.js");
const { costFigure, costText, modelChange, modelShareText, modelsLabel, recordedSwitch, usedModelIds } = await import("../ui/js/chat/usage.js");

const SUPER = "nvidia/nemotron-3-super-120b-a12b";
const ULTRA = "nvidia/Nemotron-3-Ultra-550b-a55b";
const PICO = 10n ** 12n;
const toPico = (usd) => {
  const [whole, fraction = ""] = usd.split(".");
  return BigInt(whole) * PICO + BigInt(fraction.padEnd(12, "0"));
};

/** A mock bridge for these URL switches, evaluated afresh. */
async function mock(search) {
  window.location.search = search;
  const { createMock } = await import(`./mock.js?usage-${search.replace(/\W/g, "-")}`);
  return createMock();
}

/** Asks one AI question through the exact preview, as the composer does, until the turn ends. */
async function ask(bridge, onStep = () => {}) {
  const steps = [];
  const ended = new Promise((resolve) => {
    bridge.listen("turn:done", (payload) => resolve(payload));
    bridge.listen("turn:failed", (payload) => resolve(payload));
  });
  bridge.listen("turn:step", ({ turn_id: turnId, step }) => {
    steps.push(step);
    onStep(step, turnId);
  });
  const question = "Why is my PC hot?";
  const preview = await bridge.invoke("cloud_preview", { question, names: true });
  await bridge.invoke("send_message", { text: question, previewId: preview.id });
  const { turn } = await ended;
  return { steps, report: turn.report };
}

/** One synthetic turn: Super's requests, then the change and Ultra's (tokens, or null for a failure). */
function routed(ultra, { answered = true, error = undefined, reason = "low_confidence" } = {}) {
  const requests = [[SUPER, 4627], [SUPER, 8969], ...ultra.map((tokens) => [ULTRA, tokens])];
  return {
    answer: answered ? { text: "See [O1].", confidence: "medium", citations: [] } : null,
    usage: {
      requests: requests.length,
      model_requests: requests.map(([model, tokens], i) => ({ request: i + 1, model_id: model, usage: { prompt_tokens: tokens }, cost: tokens == null ? null : { usd: "0.001" } })),
      routing: {
        mode: "automatic",
        current_model_id: ULTRA,
        change: { from_model_id: SUPER, to_model_id: ULTRA, reason, after_requests: 2 },
        last_request_error: error === undefined ? (ultra.at(-1) === null ? "timeout" : null) : error,
      },
    },
  };
}

test("one figure per turn: the total, else a lower bound, never one model's price on all tokens", () => {
  // Saved before the per-model breakdown: the single cost, or its lower bound.
  assert.deepEqual(costFigure({ cost: { usd: "0.0044" } }), { usd: "0.0044", atLeast: false });
  assert.deepEqual(costFigure({ cost: null, cost_lower_bound: { usd: "0.0021" } }), { usd: "0.0021", atLeast: true });
  // A routed turn: the exact total of both models, or what the priced requests cost at least.
  assert.equal(costText({ cost: null, total_cost: { usd: "0.0110" }, priced_cost_lower_bound: null }), "$0.0110");
  assert.equal(costText({ cost: null, total_cost: null, priced_cost_lower_bound: { usd: "0.0044" } }), "≥ $0.0044");
  assert.equal(costText({ cost: null, cost_lower_bound: null, total_cost: null, priced_cost_lower_bound: null }), null);
  assert.equal(costText(null), null);
});

test("the models are named in the order they worked, with the saved turn's fallbacks", () => {
  const usage = { models: [{ model_id: SUPER }, { model_id: ULTRA }] };
  assert.deepEqual(usedModelIds(usage), [SUPER, ULTRA]);
  assert.equal(modelsLabel(usedModelIds(usage)), "Nemotron 3 Super → Nemotron 3 Ultra");
  // Saved before the breakdown: the request list, then the priced model, then the configured one.
  assert.deepEqual(usedModelIds({ model_requests: [{ model_id: SUPER }, { model_id: SUPER }, { model_id: ULTRA }] }), [SUPER, ULTRA]);
  assert.deepEqual(usedModelIds({ cost: { price: { model_id: SUPER } } }), [SUPER]);
  assert.deepEqual(usedModelIds(null, ULTRA), [ULTRA]);
  assert.deepEqual(usedModelIds(null), []);
  assert.equal(
    modelShareText({ model_id: ULTRA, requests: 1, prompt_tokens: 9734, completion_tokens: 512, cost: { usd: "0.0066" } }),
    "Nemotron 3 Ultra: 1 request · 9,734 tokens in, 512 out · $0.0066",
  );
  assert.equal(
    modelShareText({ model_id: null, requests: 2, prompt_tokens: null, completion_tokens: 3, cost: null }),
    "Unknown model: 2 requests · tokens not reported · cost not shown",
  );
});

test("the model change says truthfully whose answer is shown", () => {
  const took = modelChange(routed([9734]));
  assert.equal(took.text, "Nemotron 3 Ultra took over this question: Nemotron 3 Super's answer had low confidence.");
  assert.equal(took.tone, "took");
  // Ultra's only request failed: the answer is Super's.
  const failed = modelChange(routed([null]));
  assert.equal(failed.tone, "failed");
  assert.match(failed.text, /but its request failed: the answer shown is the one Nemotron 3 Super wrote\.$/);
  assert.doesNotMatch(modelChange(routed([null], { answered: false })).text, /answer shown/);
  // Ultra answered, then its last request failed: who wrote the kept answer is unknown.
  assert.match(modelChange(routed([9734, null])).text, /its last request failed: the answer shown is the best one kept before the failure\.$/);
  // The turn ended before Ultra's first request.
  assert.match(modelChange(routed([], { error: null })).text, /^Nemotron 3 Ultra was to take over this question .+ the answer shown is the one Nemotron 3 Super wrote\.$/);
  assert.equal(modelChange(routed([], { error: null })).tone, "failed");
  // A Stop is not a failure, and the four calibrated reasons have their own words.
  const stopped = modelChange(routed([null], { answered: false, error: "cancelled", reason: "verification_failure" }));
  assert.equal(stopped.tone, "stopped");
  assert.equal(
    stopped.text,
    "Nemotron 3 Ultra was taking over this question (Nemotron 3 Super's answer still had unverified passages after its corrections) when you stopped the analysis.",
  );
  assert.match(modelChange(routed([9734], { reason: "insufficient_evidence" })).text, /Nemotron 3 Super found too little evidence to conclude\.$/);
  assert.match(modelChange(routed([9734], { reason: "primary_request_limit" })).text, /Nemotron 3 Super used its share of requests without answering\.$/);
  assert.match(modelChange(routed([9734], { reason: "a_future_reason" })).text, /: a_future_reason\.$/);
  assert.equal(modelChange({ usage: { routing: { change: null } } }), null);
  assert.deepEqual(recordedSwitch(routed([9734])), { key: "recorded-switch", type: "switch", state: "done", to: ULTRA, after: 2 });
  // French addresses the person as « vous » and uses its own punctuation.
  setLanguage("fr");
  try {
    assert.equal(modelChange(routed([9734])).text, "Nemotron 3 Ultra a repris cette question : la réponse de Nemotron 3 Super avait un niveau de confiance faible.");
    assert.match(modelChange(routed([null], { answered: false, error: "cancelled" })).text, /quand vous avez arrêté l'analyse\.$/);
    assert.equal(costText({ total_cost: null, priced_cost_lower_bound: { usd: "0.0044" } }), "≥ 0,0044 $");
  } finally {
    setLanguage("en");
  }
});

test("the mock offers Auto exactly when a reviewed calibration exists, as model_routing.rs does", async () => {
  const plain = await mock("?cloud=ready");
  const none = await plain.invoke("model_routing_get");
  assert.equal(none.automatic_available, false);
  assert.deepEqual(none.calibrated_rules, []);
  await assert.rejects(plain.invoke("model_routing_set", { settings: { mode: "automatic", super_model_id: SUPER, ultra_model_id: ULTRA } }), (error) => error.code === "routing_unavailable" && /calibration/.test(error.message));
  const calibrated = await mock("?cloud=ready&routing=calibrated");
  const offered = await calibrated.invoke("model_routing_get");
  assert.equal(offered.settings.mode, "configured");
  assert.equal(offered.automatic_available, true);
  assert.ok(offered.calibrated_rules.length && offered.calibration_reviewed_at_unix_ms);
  assert.equal((await calibrated.invoke("model_routing_set", { settings: { mode: "automatic", super_model_id: SUPER, ultra_model_id: ULTRA } })).settings.mode, "automatic");
  // A saved Auto choice rejected at start-up: a limitation until another choice is saved.
  const stale = await mock("?cloud=ready&routing=stale");
  const rejected = await stale.invoke("model_routing_get");
  assert.equal(rejected.settings.mode, "configured");
  assert.equal(rejected.automatic_available, false);
  assert.match(rejected.limitation, /different engine version/);
  assert.equal((await stale.invoke("model_routing_set", { settings: { mode: "configured", super_model_id: null, ultra_model_id: null } })).limitation, null);
});

test("an escalated mock turn signals the change and keeps each model's exact cost and the total", async () => {
  const bridge = await mock("?cloud=ready&routing=escalated");
  assert.equal((await bridge.invoke("model_routing_get")).settings.mode, "automatic");
  const { steps, report } = await ask(bridge);
  const order = steps.filter((s) => s.kind.startsWith("model_")).map((s) => `${s.kind}${s.request ?? ""}`);
  assert.deepEqual(order, ["model_started1", "model_finished1", "model_started2", "model_finished2", "model_changed", "model_started3", "model_finished3"]);
  const change = steps.find((s) => s.kind === "model_changed").change;
  assert.deepEqual(change, { from_model_id: SUPER, to_model_id: ULTRA, reason: "low_confidence", after_requests: 2 });
  const usage = report.usage;
  assert.equal(usage.cost, null, "two prices never give the legacy single cost");
  assert.deepEqual(usage.models.map((m) => [m.model_id, m.requests, m.unpriced_requests]), [[SUPER, 2, 0], [ULTRA, 1, 0]]);
  assert.deepEqual(usage.model_requests.map((r) => r.model_id), [SUPER, SUPER, ULTRA]);
  // The total is the exact sum of each request at its own model's price.
  const sum = usage.model_requests.reduce((total, r) => total + toPico(r.cost.usd), 0n);
  assert.equal(toPico(usage.total_cost.exact_usd), sum);
  assert.equal(toPico(usage.models[0].cost.exact_usd) + toPico(usage.models[1].cost.exact_usd), sum);
  assert.equal(usage.total_cost_usd, usage.total_cost.exact_usd);
  assert.equal(usage.priced_cost_lower_bound, null);
  assert.deepEqual([usage.models[0].price.model_id, usage.models[1].price.model_id], [SUPER, ULTRA]);
  assert.equal(usage.routing.last_request_error, null);
  assert.equal(report.answer.confidence, "medium");
  assert.equal(modelChange(report).tone, "took");
  assert.equal(costText(usage), `$${usage.total_cost.usd}`);
});

test("a failed Ultra request keeps Super's answer and leaves the turn without a total", async () => {
  const { steps, report } = await ask(await mock("?cloud=ready&routing=escalation_failed"));
  assert.equal(steps.findLast((s) => s.kind === "model_finished").outcome, "timeout");
  assert.equal(report.error, null);
  assert.equal(report.answer.confidence, "low");
  const usage = report.usage;
  assert.equal(usage.routing.last_request_error, "timeout");
  assert.equal(usage.total_cost, null);
  assert.equal(usage.total_cost_usd, null);
  assert.deepEqual([usage.models[1].cost, usage.models[1].unpriced_requests, usage.models[1].prompt_tokens], [null, 1, null]);
  // At least Super's own requests, never presented as the total.
  assert.equal(usage.priced_cost_lower_bound.exact_usd, usage.models[0].cost.exact_usd);
  assert.equal(costText(usage), `≥ $${usage.models[0].cost.usd}`);
  assert.match(modelChange(report).text, /the answer shown is the one Nemotron 3 Super wrote/);
});

test("a Stop after the change refuses Ultra's request and never falls back to Super's answer", async () => {
  const bridge = await mock("?cloud=ready&routing=escalated");
  const { steps, report } = await ask(bridge, (step, turnId) => {
    if (step.kind === "model_changed") bridge.invoke("stop_turn", { turnId });
  });
  assert.equal(steps.filter((s) => s.kind === "model_started").length, 2, "Ultra's request is never sent");
  assert.equal(report.error.code, "cancelled");
  assert.equal(report.answer, null);
  assert.equal(report.usage.routing.last_request_error, "cancelled");
  assert.equal(modelChange(report).text, "Nemotron 3 Ultra was taking over this question (Nemotron 3 Super's answer had low confidence) when you stopped the analysis.");
});
