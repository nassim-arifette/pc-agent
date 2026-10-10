// Explain and Investigate on a selected measurement. The page only names what was selected with
// a host-issued reference; the Rust side freezes that capture, builds the exact first message and
// returns a single-use preview ticket. As for a typed question, no dialog stands before a cloud
// send (C5, 9 October): the message leaves as the composer's toggles say and stays in the steps.
import { t } from "./i18n.js";
import { call } from "./api.js";
import { state, sendSelection } from "./store.js";
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
 * exact preview, then starts the turn in a new conversation and opens the chat. `label` is shown
 * on this PC only; `name` is the program name a question may carry; `subject` replaces the generic
 * "of …" part of the question and must hold no name or ID.
 */
export async function runSelection(selection, intent) {
  if (!INTENTS.includes(intent) || !selection?.reference) return;
  if (running()) {
    toast(t("select.busy"));
    navigate("chat");
    return;
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
  navigate("chat");
  await sendSelection(asked, { reference: selection.reference, intent, previewId: preview.preview_id, sent: cloud ? (preview.message ?? null) : null });
}
