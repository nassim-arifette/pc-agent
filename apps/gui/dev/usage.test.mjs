// Logic/IPC-fixture tests of which models answered and what each cost (chantier 14): the page's
// helpers on isolated test reports. Not a browser or a live test.
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

test("web credits add to the turn estimate without inventing missing usage", () => {
  assert.deepEqual(costFigure({ web: { lookups: 1 }, combined_estimated_cost: { usd: "0.009" }, total_cost: { usd: "0.001" } }), { usd: "0.009", atLeast: false });
  assert.deepEqual(costFigure({ web: { lookups: 1 }, combined_estimated_cost: null, total_cost: { usd: "0.001" } }), { usd: "0.001", atLeast: true });
  assert.equal(costFigure({ web: { lookups: 1 }, combined_estimated_cost: null, total_cost: null }), null);
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
