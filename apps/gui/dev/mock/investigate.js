import { wait } from "./common.js";
import { report, DRIVER_QUERY, PRICE_QUERY, RAM_QUESTION } from "./reports.js";
import { SUPER_ID, ULTRA_ID } from "./models.js";
import { createUsageFixture } from "./usage.js";

export function createInvestigationRunner({ emit, getRunning, done, webActive, provider }) {
  const { escalation, calibrationReviewedAt } = provider;
  const routedUsage = createUsageFixture();
  async function investigate(id, question, cloudRun) {
    const slow = /slow|lent/i.test(question) ? 1.6 : 1;
    const step = (s) => {
      const at = Date.now();
      emit("turn:step", { turn_id: getRunning().turnId, at_unix_ms: at, step: s });
      if (s.kind === "answer_draft") emit("turn:delta", { turn_id: getRunning().turnId, request: s.request, text: s.text, replace: true, unverified: true });
      emit("inv:step", { id, at_unix_ms: at, step: s });
    };
    step({ kind: "snapshot_started" });
    await wait(900 * slow);
    step({ kind: "snapshot_finished", duration_ms: 1012, measured: 38 });
    if (!cloudRun) {
      await wait(250);
      return done(id, report(question, false));
    }
    let request = 0;
    const model = async (ms, prompt, completion, outcome = "ok") => {
      request += 1;
      step({ kind: "model_started", request });
      await wait(ms * slow);
      step({ kind: "model_finished", request, duration_ms: ms, prompt_tokens: prompt, completion_tokens: completion, outcome });
    };
    const toolEvents = [];
    const tool = async (name, ms, status = "ok", query = null) => {
      step({ kind: "tool_started", tool: name });
      await wait(ms * slow);
      // A web lookup reports the exact query it sent; other tools have none (skipped like serde).
      const event = { tool: name, status, duration_ms: ms, ...(query ? { query } : {}) };
      toolEvents.push(event);
      step({ kind: "tool_finished", ...event });
    };
    if (getRunning().stop) return done(id, report(question, true, "cancelled"));
    // ?specialist=ready with the setting on: the local Nano 4B chooses the first measurement,
    // the controller runs it, then the specialist hands the evidence to the Nebius analyst.
    if (provider.specialistActive()) {
      const local = (event) => emit("specialist:step", { turn_id: getRunning().turnId, event });
      request += 1;
      step({ kind: "model_started", request });
      local({ kind: "started" });
      await wait(1500);
      local({ kind: "decided", decision: { decision: "collect", next_call: { tool: "get_processes", arguments: {} } }, duration_ms: 1480, device: "cuda" });
      step({ kind: "model_finished", request, duration_ms: 1480, prompt_tokens: 1310, completion_tokens: 42, outcome: "ok" });
      await tool("get_processes", 420);
      request += 1;
      step({ kind: "model_started", request });
      local({ kind: "started" });
      await wait(1100);
      local({ kind: "decided", decision: { decision: "summarize" }, duration_ms: 1090, device: "cuda" });
      step({ kind: "model_finished", request, duration_ms: 1090, prompt_tokens: 1780, completion_tokens: 51, outcome: "ok" });
      local({ kind: "handoff", reason: "summarize" });
    }
    await model(1400, 4627, 83);
    if (/fail|échec/i.test(question)) {
      await model(1600, 9880, 8192, "output_limit");
      return done(id, report(question, true, "output_limit"));
    }
    if (getRunning().stop) return done(id, report(question, true, "cancelled"));
    await tool("get_processes", 420);
    await tool("get_metric_history", 180);
    const web = webActive();
    if (web && RAM_QUESTION.test(question)) {
      await tool("get_hardware_inventory", 300);
      await tool("lookup_upgrade_price", 1200, "ok", PRICE_QUERY);
    } else if (web) {
      await tool("get_drivers", 260);
      await tool("lookup_driver_notes", 1100, "ok", DRIVER_QUERY);
    }
    if (/extend|prolong/i.test(question)) {
      const answer = new Promise((resolve) => {
        getRunning().answer = resolve;
      });
      const extension = {
        request: { reason: "rounds", available: true, additional_rounds: 6, additional_tool_calls: 12, additional_seconds: 90 },
        usage: { requests: 6, tool_calls: 6, extensions: 0, prompt_tokens: 30210, completion_tokens: 812, reasoning_tokens: 600, cost: { usd: "0.0098", exact_usd: "0.0097938", price: null } },
      };
      emit("turn:extension", { turn_id: getRunning().turnId, ...extension });
      emit("inv:extension", { id, ...extension });
      const accepted = await answer;
      if (!accepted) return done(id, { ...report(question, true, "budget"), execution: "budget_exhausted" });
    }
    if (getRunning().stop) return done(id, report(question, true, "cancelled"));
    // ?routing=escalated|escalation_failed with Auto on: Super's answer has low confidence, so the
    // calibrated routing hands the question to Ultra with the measurements already taken.
    if (escalation && provider.routingMode() === "automatic") return escalated(id, question, { step, slow, web, toolEvents });
    // The last request writes the answer: its text arrives in pieces, like Nebius's stream.
    const final = report(question, true, null, { web, toolEvents });
    request += 1;
    step({ kind: "model_started", request });
    await wait(500 * slow);
    const text = final.answer?.text || "";
    for (let end = 12; end < text.length + 12; end += 12) {
      step({ kind: "answer_draft", request, text: text.slice(0, end) });
      await wait(70 * slow);
    }
    step({ kind: "model_finished", request, duration_ms: 1300, prompt_tokens: 8969, completion_tokens: 271, outcome: "ok" });
    done(id, final);
  }

  async function escalated(id, question, { step, slow, web, toolEvents }) {
    const full = report(question, true, null, { web, toolEvents });
    // Super's own first paragraph, less sure of itself.
    const draft = full.answer.text.split("\n\n")[0].replace("I'm moderately sure", "I'm not sure").replace("J'en suis moyennement sûr", "Je n'en suis pas sûr");
    const superAnswer = { ...full.answer, text: draft, confidence: "low", citations: full.answer.citations.filter((c) => draft.includes(`[${c.reference}]`)) };
    const write = async (request, text) => {
      for (let end = 12; end < text.length + 12; end += 12) {
        step({ kind: "answer_draft", request, text: text.slice(0, end) });
        await wait(70 * slow);
      }
    };
    const requests = [{ model: SUPER_ID, prompt: 4627, completion: 83, reasoning: 60 }, { model: SUPER_ID, prompt: 8969, completion: 271, reasoning: 210 }];
    step({ kind: "model_started", request: 2 });
    await wait(500 * slow);
    await write(2, draft);
    step({ kind: "model_finished", request: 2, duration_ms: 1100, prompt_tokens: 8969, completion_tokens: 271, outcome: "ok" });
    const change = { from_model_id: SUPER_ID, to_model_id: ULTRA_ID, reason: "low_confidence", after_requests: 2 };
    step({ kind: "model_changed", change });
    const finish = (outcome, answer) => {
      const routing = { mode: "automatic", current_model_id: ULTRA_ID, calibration_reviewed_at_unix_ms: calibrationReviewedAt, change, last_request_error: outcome === "ok" ? null : outcome };
      const base = outcome === "cancelled" ? report(question, true, "cancelled") : full;
      return done(id, { ...base, answer, usage: routedUsage(requests, { toolCalls: toolEvents.length, routing }) });
    };
    if (getRunning().stop) {
      requests.push({ model: ULTRA_ID });
      return finish("cancelled", null);
    }
    step({ kind: "model_started", request: 3 });
    await wait(900 * slow);
    if (getRunning().stop || escalation === "escalation_failed") {
      const outcome = getRunning().stop ? "cancelled" : "timeout";
      requests.push({ model: ULTRA_ID });
      step({ kind: "model_finished", request: 3, duration_ms: 900, prompt_tokens: null, completion_tokens: null, outcome });
      return finish(outcome, outcome === "cancelled" ? null : superAnswer);
    }
    await write(3, full.answer.text);
    requests.push({ model: ULTRA_ID, prompt: 9734, completion: 512, reasoning: 380 });
    step({ kind: "model_finished", request: 3, duration_ms: 2100, prompt_tokens: 9734, completion_tokens: 512, outcome: "ok" });
    return finish("ok", full.answer);
  }
  return investigate;
}
