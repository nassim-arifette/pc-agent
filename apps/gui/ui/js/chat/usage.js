// Which models a turn used and what each cost (chantier 14). After a calibrated escalation the
// report keeps Super's and Ultra's requests, tokens and costs apart (`usage.models`, built by
// view.rs), a total exists only when every request is priced, and `usage.routing.change` says
// when and why Ultra took the question over. A two-model cost is never one model's price applied
// to every token. Turns saved before the breakdown keep their single cost.
import { h } from "../dom.js";
import { icon } from "../icons.js";
import { t } from "../i18n.js";
import { labelled, number, usd } from "../format.js";
import { modelName } from "./models.js";

/** Each model's share of the turn, in the order the models were first used. */
export function turnModels(usage) {
  return Array.isArray(usage?.models) ? usage.models : [];
}

/** The models the turn used, in order; else the one its cost names, else `fallback`. */
export function usedModelIds(usage, fallback = null) {
  const shares = turnModels(usage).length ? turnModels(usage) : usage?.model_requests || [];
  const ids = [];
  for (const { model_id: id } of shares) if (id && !ids.includes(id)) ids.push(id);
  if (ids.length) return ids;
  const named = usage?.cost?.price?.model_id || usage?.cost_lower_bound?.price?.model_id || fallback;
  return named ? [named] : [];
}

/** "Nemotron 3 Super → Nemotron 3 Ultra". */
export function modelsLabel(ids) {
  return ids.map((id) => modelName(id)).join(" → ");
}

/** A share's model name, or "Unknown model" when the provider named none. */
export function shareName(share) {
  return share.model_id ? modelName(share.model_id) : t("usage.model.unknown");
}

/**
 * The turn's cost as one figure: the total, or a lower bound (`atLeast`) when a request has no
 * cost, or null.
 */
export function costFigure(usage) {
  if (!usage) return null;
  if ("total_cost" in usage || "priced_cost_lower_bound" in usage) {
    if (usage.total_cost) return { usd: usage.total_cost.usd, atLeast: false };
    if (usage.priced_cost_lower_bound) return { usd: usage.priced_cost_lower_bound.usd, atLeast: true };
    return null;
  }
  // Saved before the per-model breakdown: one model at one price.
  if (usage.cost) return { usd: usage.cost.usd, atLeast: false };
  if (usage.cost_lower_bound) return { usd: usage.cost_lower_bound.usd, atLeast: true };
  return null;
}

/** "$0.0215", "≥ $0.0030" for a lower bound, or null. */
export function costText(usage) {
  const figure = costFigure(usage);
  return figure ? `${figure.atLeast ? "≥ " : ""}${usd(figure.usd)}` : null;
}

/** One model's share in words: "Nemotron 3 Super: 2 requests · 9,700 tokens in, 100 out · $0.0030". */
export function modelShareText(share) {
  const parts = [
    t(share.requests === 1 ? "usage.model.request" : "usage.model.requests", { count: number(share.requests) }),
    share.prompt_tokens != null && share.completion_tokens != null
      ? t("usage.model.tokens", { in: number(share.prompt_tokens), out: number(share.completion_tokens) })
      : t("usage.model.tokensUnknown"),
    share.cost ? usd(share.cost.usd) : t("usage.model.noCost"),
  ];
  return labelled(shareName(share), parts.join(" · "));
}

/**
 * What the answer owes to a model change, or null when the turn kept one model. When the new
 * model wrote nothing (its only request failed, or the turn ended first), the text says whose
 * answer is shown; when its last request failed after others, it says the answer shown is the
 * best one kept before the failure, since the report does not say who wrote it. A Stop is not
 * called a failure. `tone` is "took", "stopped" or "failed".
 */
export function modelChange(report) {
  const usage = report?.usage;
  const change = usage?.routing?.change;
  if (!change) return null;
  const from = modelName(change.from_model_id);
  const to = modelName(change.to_model_id);
  const reasonKey = `routing.reason.${change.reason}`;
  const reason = t(reasonKey, { from });
  // Requests numbered after the change were sent to the new model.
  const requests = usage.model_requests;
  const taken = Array.isArray(requests) && requests.length
    ? requests.filter((r) => r.request > change.after_requests).length
    : Math.max(0, (usage.requests ?? 0) - change.after_requests);
  const error = usage.routing.last_request_error ?? null;
  const kept = report.answer ? ".kept" : "";
  let key = "routing.took";
  if (taken === 0) key = `routing.notReached${kept}`;
  else if (error === "cancelled") key = "routing.stopped";
  else if (error != null) key = `${taken === 1 ? "routing.failed" : "routing.lastFailed"}${kept}`;
  return {
    change,
    tone: key === "routing.took" ? "took" : key === "routing.stopped" ? "stopped" : "failed",
    text: t(key, { from, to, reason: reason === reasonKey ? change.reason : reason }),
  };
}

const NOTE_ICONS = { took: "sparkle", stopped: "info", failed: "alert" };

/** The lasting line under an answer when a stronger model took the question over, or tried to. */
export function routingNote(report) {
  const change = modelChange(report);
  if (!change) return null;
  return h("p", { class: ["routing-note", change.tone === "failed" ? "warn" : ""] }, icon(NOTE_ICONS[change.tone]), h("span", null, change.text));
}

/** The switch row of a saved turn's steps: its tool events carry no request numbers. */
export function recordedSwitch(report) {
  const change = report?.usage?.routing?.change;
  return change ? { key: "recorded-switch", type: "switch", state: "done", to: change.to_model_id, after: change.after_requests } : null;
}
