// Explain and Investigate on a selected measurement. The page only names what was selected with
// a host-issued reference; the Rust side freezes that capture, builds the exact first message and
// returns a single-use preview ticket. As for a typed question, no dialog stands before a cloud
// send (C5, 9 October): the message leaves as the composer's toggles say and stays in the steps.
import { t } from "./i18n.js";
import { call } from "./api.js";
import { state, notify, sendSelection, waitForPrivacy, beginTurnPreparation, endTurnPreparation, openPanel, closePanel, openConversation } from "./store.js";
import { toast } from "./ui.js";
import { effectiveMode, namesShared } from "./ask.js";
import { navigate } from "./router.js";

export const INTENTS = ["explain", "investigate"];

function running() {
  return Boolean(state.inv && (state.inv.status === "running" || state.inv.status === "stopping"));
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
 * exact preview in the current conversation. Explain stays in the inspector; Investigate opens
 * the chat. An inspector continuation explicitly keeps its summary's conversation. `label` is shown
 * on this PC only; `name` is the program name a question may carry; `subject` replaces the generic
 * "of …" part of the question and must hold no name or ID.
 */
export async function runSelection(selection, intent, { conversationId: requestedConversation } = {}) {
  if (!INTENTS.includes(intent) || !selection?.reference) return false;
  if (running()) {
    toast(t("select.busy"));
    if (state.summary?.inv === state.inv) openPanel("selection");
    else navigate("chat");
    return false;
  }
  if (!beginTurnPreparation()) return false;
  const selected = structuredClone(selection);
  const summary = intent === "explain" ? { selection: selected, status: "preparing", error: null, inv: null, conversationId: requestedConversation ?? state.conversation?.id ?? null } : null;
  if (summary) {
    state.summary = summary;
    openPanel("selection");
    notify("summary");
  }
  try {
    if (requestedConversation != null && requestedConversation !== state.conversation?.id) {
      if (!await openConversation(requestedConversation)) throw new Error(t("select.conversationGone"));
    }
    const conversation = state.conversation;
    const conversationId = conversation?.id ?? null;
    const conversationRevision = state.conversationRevision;
    const sameConversation = () => conversation === state.conversation && conversationId === (state.conversation?.id ?? null) && conversationRevision === state.conversationRevision;
    await waitForPrivacy();
    if (!sameConversation() || (summary && state.summary !== summary)) throw new Error(t("share.changed"));
    const privacyRevision = state.privacyRevision;
    const cloud = effectiveMode() !== "local";
    const asked = question(selected, intent);
    const preview = await call("selection_preview", { reference: selected.reference, intent, question: asked, conversationId, names: namesShared(), fileNames: false });
    if (privacyRevision !== state.privacyRevision || !sameConversation() || (summary && state.summary !== summary)) {
      throw new Error(t("share.changed"));
    }
    if (intent === "investigate") {
      closePanel();
      navigate("chat");
    }
    return await sendSelection(asked, { reference: selected.reference, intent, conversationId, label: selected.label || t("select.label.fallback"), previewId: preview.preview_id, sent: cloud ? (preview.message ?? null) : null, summary });
  } catch (error) {
    if (summary) {
      summary.status = "failed";
      summary.error = { message: error.message };
      notify("summary");
    } else toast(error.message, "bad");
    return false;
  } finally {
    endTurnPreparation();
  }
}

/** A fresh Investigate ticket for the same evidence and the summary's existing conversation. */
export function continueSummary() {
  const summary = state.summary;
  if (summary?.inv?.status !== "done" || !summary.inv.conversationId) return false;
  return runSelection(summary.selection, "investigate", { conversationId: summary.inv.conversationId });
}

export function retrySummary() {
  const summary = state.summary;
  if (!summary) return false;
  return runSelection(summary.selection, "explain", { conversationId: summary.inv?.conversationId ?? summary.conversationId });
}
