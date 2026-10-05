// Web sources (C6): typed lookups through Tavily. The page shows where a cited page comes from,
// when it was read and the exact query that found it. It never opens or links an address itself:
// "Open in browser" asks the Rust side, which reads the address from the stored turn.
import { h, fill } from "./dom.js";
import { icon } from "./icons.js";
import { t } from "./i18n.js";
import { state, setWebSearch, saveSearchKey, openWebSource, webSearchActive } from "./store.js";
import { citationTitle, isWeb, publishedDate, sourceCounts } from "./citations.js";
import { badge, button, toast, toggle } from "./ui.js";
import { clock } from "./format.js";

// ---------- In the conversation ----------

/** The exact query a lookup sent, under its step. */
export function webStepQuery(query) {
  if (!query) return null;
  return h("span", { class: "web-step-query" }, h("span", { class: "web-step-to" }, t("web.step.sent")), h("q", { class: "web-step-text" }, query));
}

/** Reference → the site of a web source, for its chip's tooltip. */
export function webDescriber(answer) {
  const sites = new Map((answer?.citations || []).filter(isWeb).map((c) => [c.reference, c.web?.domain || ""]));
  return (reference) => sites.get(reference) || null;
}

/** " (site, date)" after a web source's title in copied text; nothing for a measurement. */
export function webOrigin(citation) {
  if (!isWeb(citation)) return "";
  return ` (${[citation.web?.domain, publishedDate(citation.web?.published_date)].filter(Boolean).join(", ")})`;
}

/** "3 measurements cited · 1 web source" for an answer's sources button. */
export function sourcesLabel(answer) {
  const { measured, web } = sourceCounts(answer);
  const parts = [];
  if (measured) parts.push(t(measured === 1 ? "chat.cited.one" : "chat.cited", { count: measured }));
  if (web) parts.push(t(web === 1 ? "web.cited.one" : "web.cited", { count: web }));
  return parts.join(" · ");
}

async function open(target) {
  try {
    await openWebSource(target.conversationId, target.turnId, target.reference);
    toast(t("web.opened"));
  } catch (error) {
    toast(error.message, "bad");
  }
}

/**
 * A cited web page in the Sources panel: title, site, dates, the snippet the answer relied on and
 * the query that found it. `target` = { conversationId, turnId, reference } of a saved turn.
 */
export function webSourceCard(citation, n, focused, target) {
  const web = citation.web || {};
  const meta = [
    web.domain ? h("span", { class: "web-domain" }, web.domain) : null,
    h("span", null, web.published_date ? t("web.published", { date: publishedDate(web.published_date) }) : t("web.date.unknown")),
    web.searched_at_unix_ms ? h("span", null, t("web.readAt", { time: clock(web.searched_at_unix_ms) })) : null,
  ].filter(Boolean);
  const canOpen = Boolean(target?.conversationId && target?.turnId);
  return h(
    "article",
    { class: ["source-card", "web-source", focused ? "focused" : ""], dataset: { reference: citation.reference } },
    h("div", { class: "source-head" }, h("span", { class: "num web-num" }, icon("globe"), String(n)), h("strong", { class: "web-title" }, citationTitle(citation))),
    h("p", { class: "web-meta" }, meta.flatMap((part, i) => (i ? [h("span", { class: "web-dot", "aria-hidden": "true" }, "·"), part] : [part]))),
    citation.facts.length ? h("blockquote", { class: "web-snippet" }, citation.facts.map((fact) => h("p", null, fact))) : null,
    web.query ? h("p", { class: "web-query" }, h("span", { class: "web-query-label" }, t("web.query")), h("q", null, web.query)) : null,
    h("p", { class: "web-note" }, icon("info"), h("span", null, t("web.notMeasured"))),
    h(
      "div",
      { class: "web-actions" },
      button(t("web.open"), {
        iconName: "reveal",
        size: "sm",
        disabled: !canOpen,
        title: canOpen ? null : t("web.open.unsaved"),
        onClick: () => open(target),
      }),
    ),
  );
}

/** One line in a consent dialog when lookups may also go to Tavily during the turn. */
export function webConsentNote() {
  if (!webSearchActive()) return null;
  return h("p", { class: "web-consent" }, icon("globe"), h("span", null, t("web.consent")));
}

// ---------- Settings ----------

/** What a lookup sends and never sends, in the current sharing mode (C6). */
function webDisclosure(mode) {
  const names = mode === "complete";
  const sent = [t("web.sent.query"), names ? t("web.sent.names") : null, t("web.sent.country")].filter(Boolean);
  const never = [t("web.never.text"), names ? null : t("web.never.names"), t("web.never.paths"), t("web.never.ids")].filter(Boolean);
  return h(
    "div",
    { class: "two-lists" },
    h("div", null, h("h4", null, icon("globe"), t("web.sent.title")), h("ul", null, sent.map((x) => h("li", null, x)))),
    h("div", null, h("h4", null, icon("lock"), t("consent.never.title")), h("ul", null, never.map((x) => h("li", null, x)))),
  );
}

function keyForm(cloud) {
  const configured = Boolean(cloud.search_key_configured);
  const hint = cloud.search_key_hint ? `••••${cloud.search_key_hint}` : null;
  const input = h("input", {
    class: "input web-key-input",
    type: "password",
    autocomplete: "off",
    spellcheck: "false",
    placeholder: configured ? t("web.key.replace") : t("web.key.new"),
    "aria-label": t("web.key"),
  });
  const reveal = h(
    "button",
    {
      type: "button",
      class: "btn btn-icon btn-ghost",
      title: t("settings.key.show"),
      "aria-label": t("settings.key.show"),
      onclick: () => {
        const show = input.type === "password";
        input.type = show ? "text" : "password";
        reveal.title = t(show ? "settings.key.hide" : "settings.key.show");
        reveal.setAttribute("aria-label", reveal.title);
      },
    },
    icon("eye"),
  );
  const save = async () => {
    const key = input.value.trim();
    if (!key) {
      toast(t("web.key.empty"), "bad");
      return;
    }
    if (!key.startsWith("tvly-")) {
      toast(t("web.key.invalid"), "bad");
      return;
    }
    try {
      await saveSearchKey(key);
      input.value = "";
      toast(t("web.key.saved"));
    } catch (error) {
      toast(error.message, "bad");
    }
  };
  const remove = async () => {
    try {
      await saveSearchKey(null);
      toast(t("web.key.removed"));
    } catch (error) {
      toast(error.message, "bad");
    }
  };
  // A form, so Enter in the key field saves; nothing is submitted anywhere.
  return h(
    "form",
    {
      class: "field web-key",
      autocomplete: "off",
      onsubmit: (e) => {
        e.preventDefault();
        save();
      },
    },
    h("span", { class: "label" }, t("web.key")),
    h("div", { class: "row" }, input, reveal),
    h(
      "p",
      { class: ["web-key-state", configured ? "web-key-saved" : ""], role: "status" },
      icon(configured ? "check" : "key"),
      configured ? (hint ? t("web.key.savedHint", { hint }) : t("web.key.savedNoHint")) : t("web.key.none"),
    ),
    cloud.search_key_source === "environment" ? h("p", { class: "hint" }, icon("info"), " ", t("web.key.env")) : null,
    h(
      "div",
      { class: "row wrap" },
      button(t("settings.save"), { variant: "primary", iconName: "check", type: "submit" }),
      configured ? button(t("web.key.remove"), { variant: "danger", iconName: "x", onClick: remove }) : null,
    ),
    h("p", { class: "note" }, t("web.key.note")),
  );
}

/**
 * The "Web search (Tavily)" settings card. Returns { el, redraw }: the settings view redraws it
 * when the sharing choice changes, since web search exists only with AI analysis.
 */
export function webSettingsCard() {
  const el = h("section", { class: "card card-pad web-card" });
  const redraw = () => {
    const cloud = state.info?.cloud || {};
    const aiMode = state.privacy.mode !== "local" && Boolean(cloud.ready);
    const key = Boolean(cloud.search_key_configured);
    const active = webSearchActive();
    const status = active ? badge(t("web.status.on"), "good", "check") : !key ? badge(t("web.status.noKey"), "", "key") : badge(t("web.status.off"));
    const hint = !aiMode ? t("web.hint.local") : !key ? t("web.hint.noKey") : t("web.hint.ready");
    const control = toggle(active, async (on) => {
      try {
        await setWebSearch(on);
      } catch (error) {
        toast(error.message, "bad");
      }
      redraw();
    }, t("web.toggle"));
    // The consent is remembered, but it only has an effect with AI analysis and a saved key.
    control.disabled = !aiMode || !key;
    control.classList.add("web-switch");
    fill(
      el,
      h("div", { class: "card-head" }, h("div", null, h("h2", { class: "card-title" }, icon("globe"), t("web.title")), h("p", { class: "hint" }, t("web.sub"))), status),
      h(
        "div",
        { class: "setting-row" },
        h("div", { class: "setting-text" }, h("strong", null, t("web.toggle")), h("span", { class: "hint" }, hint)),
        control,
      ),
      webDisclosure(state.privacy.mode),
      h("p", { class: "note web-steps-note" }, icon("search"), h("span", null, t("web.stepsNote"))),
      keyForm(cloud),
    );
  };
  redraw();
  return { el, redraw };
}
