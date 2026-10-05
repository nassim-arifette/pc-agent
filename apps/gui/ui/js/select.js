// Explain and Investigate on a selected measurement. The page only names what was selected with
// a host-issued reference; the Rust side freezes that capture, builds the exact first message and
// returns a single-use preview ticket. A cloud mode shows that message before anything leaves.
import { h } from "./dom.js";
import { icon } from "./icons.js";
import { t } from "./i18n.js";
import { call } from "./api.js";
import { state, sendSelection, setPrivacy } from "./store.js";
import { button, dialog, toast } from "./ui.js";
import { choosePrivacy, effectiveMode, namesShared } from "./ask.js";
import { ago, bytes } from "./format.js";
import { modelName } from "./models.js";
import { webConsentNote } from "./web.js";
import { navigate } from "./router.js";

export const INTENTS = ["explain", "investigate"];

function running() {
  return Boolean(state.inv && (state.inv.status === "running" || state.inv.status === "stopping"));
}

/** What Explain and Investigate may do, in one sentence each. */
function scope(intent) {
  return t(`select.scope.${intent}`);
}

/**
 * The question sent with the selection, in the interface language so the answer comes back in it.
 * It follows C5 like the measurements: never a PID, and a program name only in complete sharing.
 */
function question(selection, intent) {
  const target = selection.reference.target;
  // Views can name the subject themselves ("of the recording from 14:02 to 14:03").
  const what =
    selection.subject ??
    (target.kind === "resource"
      ? t(`select.what.${target.resource}`)
      : namesShared() && selection.name
        ? t("select.what.named", { name: selection.name })
        : t("select.what.process"));
  return t(`select.ask.${intent}`, { what });
}

function reviewDialog(selection, intent, preview) {
  const names = namesShared();
  const exact = JSON.stringify(preview.message, null, 2);
  const measured = preview.measured_at_unix_ms != null ? t("select.measured", { ago: ago(preview.measured_at_unix_ms) }) : t("select.measured.unknown");
  const model = preview.configured_model_id ? modelName(preview.configured_model_id) : t("mode.cloud");
  const sent = [t("select.sent.item", { what: selection.label }), t("select.sent.measures"), names ? t("consent.sent.namesAny") : t("consent.sent.refs")];
  const never = [names ? t("consent.never.pids") : t("consent.never.names"), t("consent.never.paths"), t("consent.never.ids"), t("consent.never.key")];
  return dialog(
    (close) => [
      h("h2", { class: "dialog-title" }, t(`select.title.${intent}`, { what: selection.label })),
      h("p", null, scope(intent)),
      h("p", { class: "note" }, `${measured} · ${model} · ${t(`privacy.${state.privacy.mode}`)}`),
      h(
        "div",
        { class: "two-lists" },
        h("div", null, h("h4", null, icon("cloud"), t("consent.sent.title")), h("ul", null, sent.map((x) => h("li", null, x)))),
        h("div", null, h("h4", null, icon("lock"), t("consent.never.title")), h("ul", null, never.map((x) => h("li", null, x)))),
      ),
      // Explain reads only the selected measurements; Investigate may also look things up.
      intent === "investigate" ? webConsentNote() : null,
      h(
        "details",
        { class: "select-exact" },
        h("summary", null, icon("chevron"), t("select.exact", { size: bytes(new TextEncoder().encode(exact).length) })),
        h("pre", { class: "select-exact-text", tabindex: "0" }, exact),
      ),
      h(
        "div",
        { class: "dialog-actions" },
        button(t("common.cancel"), { onClick: () => close(false) }),
        button(t(`select.send.${intent}`), { variant: "primary", iconName: "send", onClick: () => close(true) }),
      ),
    ],
    { label: t(`select.title.${intent}`, { what: selection.label }), dismissValue: false },
  );
}

/**
 * The newest host-issued reference whose target satisfies `match`, for views whose data carries
 * no source ID (history events, recordings); null when none is still published.
 */
export async function findReference(match) {
  try {
    const sources = await call("selection_sources");
    return sources.find((source) => match(source.reference.target))?.reference ?? null;
  } catch {
    return null;
  }
}

/**
 * Runs Explain or Investigate on `selection` = { reference, label, name?, subject? }: prepares the
 * exact preview, asks before a cloud send, then starts the turn in a new conversation and opens
 * the chat. `label` is shown on this PC only; `name` is the program name a question may carry;
 * `subject` replaces the generic "of …" part of the question and must hold no name or ID.
 */
export async function runSelection(selection, intent) {
  if (!INTENTS.includes(intent) || !selection?.reference) return;
  if (running()) {
    toast(t("select.busy"));
    navigate("chat");
    return;
  }
  // First cloud use: the sharing choice is made deliberately, once, as for a typed question.
  if (effectiveMode() !== "local" && !state.privacy.confirmed && !state.privacyAsked) {
    state.privacyAsked = true;
    if (!(await choosePrivacy())) return;
  }
  const cloud = effectiveMode() !== "local";
  const asked = question(selection, intent);
  let preview;
  try {
    preview = await call("selection_preview", { reference: selection.reference, intent, question: asked, conversationId: null, names: namesShared(), fileNames: false });
  } catch (error) {
    toast(error.message, "bad");
    return;
  }
  if (cloud && !(state.skipConsent && state.privacy.confirmed)) {
    if (!(await reviewDialog(selection, intent, preview))) return;
    // Approving this exact preview confirms the stored choice; the same mode keeps the ticket.
    if (!state.privacy.confirmed) await setPrivacy(state.privacy.mode, true);
  }
  navigate("chat");
  await sendSelection(asked, { reference: selection.reference, intent, previewId: preview.preview_id });
}
