// Failures in the interface language. A failed command rejects with an Error whose `code` comes
// from a closed set (api.js, src/failure.rs); `failure.<code>` in i18n.js says it in English and
// French. The host's English `message` is kept for a tooltip only, never shown as the main text,
// and a code without a sentence reads as a generic one. A failed turn keeps its C3 codes
// (`error.<code>`); a turn the host refused to start keeps the command's own code as its cause.
import { t } from "./i18n.js";

/** The translation of `key`, or of `fallback` when the dictionaries have none. */
function translated(key, fallback) {
  const text = t(key);
  return text === key ? t(fallback) : text;
}

/** Why an approved action did not run, as the words after "Not run:" (action.refused). */
export function refusalText(error) {
  return error?.reason ? translated(`failure.action_refused.${error.reason}`, "failure.action_refused.other") : t("failure.action_refused.other");
}

/** The sentence for a failed command; `fallback` is the key read for a code without a sentence. */
export function failureText(error, fallback = "failure.unknown") {
  if (!error) return t(fallback);
  if (error.localized) return error.message;
  if (error.code === "action_refused") return t("failure.action_refused", { reason: refusalText(error) });
  if (typeof error.code !== "string" || error.code === "unknown") return t(fallback);
  return translated(`failure.${error.code}`, fallback);
}

/** A turn the host refused to start, in the shape of a turn:failed payload: why stays its cause. */
export function startFailure(error) {
  return { code: "start", message: typeof error?.message === "string" ? error.message : "", cause: error ?? null };
}

/** The sentence for a failed turn: a turn:failed code (C3), or a refused start. */
export function turnFailureText(failure) {
  if (failure?.code === "start") return failureText(failure.cause, "error.start");
  return translated(`error.${failure?.code}`, "failure.unknown");
}

/** The host's English detail, for a title attribute; null for the page's own sentences. */
export function failureDetail(error) {
  const source = error?.code === "start" ? error.cause : error;
  if (!source || source.localized) return null;
  const detail = typeof source.message === "string" ? source.message.trim() : "";
  return detail || null;
}

/** An error the page raises itself, already written in the interface language. */
export function pageFailure(key, vars) {
  const error = new Error(t(key, vars));
  error.code = "page";
  error.localized = true;
  return error;
}
