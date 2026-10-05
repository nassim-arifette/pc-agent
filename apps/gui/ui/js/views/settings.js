// Settings: the Nebius connection, privacy, web search and appearance.
import { h, fill } from "../dom.js";
import { icon } from "../icons.js";
import { t } from "../i18n.js";
import { call } from "../api.js";
import { disclosure, modeCards, cloudReady } from "../ask.js";
import { getPrefs, setPref } from "../prefs.js";
import { state, subscribe, refreshInfo, clearConversations, setPrivacy } from "../store.js";
import { badge, button, segmented, toast, toggle } from "../ui.js";
import { price } from "../format.js";
import { modelName } from "../models.js";
import { webSettingsCard } from "../web.js";

function aiCard() {
  const info = state.info;
  const cloud = info?.cloud || {};
  const models = info?.models || [];
  const known = models.some((m) => m.id === cloud.model_id);
  let choice = cloud.model_id ? (known ? cloud.model_id : "custom") : models[0]?.id || "custom";

  const status = cloud.ready
    ? badge(t("settings.ai.ready"), "good", "check")
    : cloud.key_configured || cloud.model_id
      ? badge(t("settings.ai.problem"), "warn", "alert")
      : badge(t("settings.ai.notReady"), "", "info");

  const note = h("p", { class: "hint" });
  const custom = h("input", { class: "input", placeholder: "nvidia/…", value: known ? "" : cloud.model_id || "", "aria-label": t("settings.model.customLabel") });
  const customField = h("div", { class: "field", style: { "margin-top": "8px" } }, custom);
  const select = h(
    "select",
    {
      class: "select",
      "aria-label": t("settings.model"),
      onchange: (e) => {
        choice = e.target.value;
        sync();
      },
    },
    models.map((m) => h("option", { value: m.id }, `${modelName(m.id)} — ${m.id}`)),
    h("option", { value: "custom" }, t("settings.model.custom")),
  );
  select.value = choice;
  const sync = () => {
    const model = models.find((m) => m.id === choice);
    note.textContent = model ? t(`settings.model.note.${model.note}`) : "";
    customField.style.setProperty("display", choice === "custom" ? "flex" : "none");
  };
  sync();

  const key = h("input", {
    class: "input",
    type: "password",
    autocomplete: "off",
    spellcheck: "false",
    placeholder: cloud.key_configured ? (cloud.key_hint ? t("settings.key.saved", { hint: cloud.key_hint }) : t("settings.key.savedNoHint")) : t("settings.key.new"),
    "aria-label": t("settings.key"),
  });
  const reveal = h(
    "button",
    {
      type: "button",
      class: "btn btn-icon btn-ghost",
      title: t("settings.key.show"),
      "aria-label": t("settings.key.show"),
      onclick: () => {
        const show = key.type === "password";
        key.type = show ? "text" : "password";
        reveal.title = t(show ? "settings.key.hide" : "settings.key.show");
        reveal.setAttribute("aria-label", reveal.title);
      },
    },
    icon("eye"),
  );
  const result = h("p", { class: "hint form-result", role: "status" });
  const testButton = button(t("settings.test"), {
    iconName: "plug",
    disabled: !cloud.ready,
    onClick: async () => {
      testButton.disabled = true;
      result.textContent = t("settings.testing");
      try {
        const check = await call("test_cloud");
        result.textContent = check.price
          ? t("settings.test.ok", { input: price(check.price.input_per_million), output: price(check.price.output_per_million) })
          : t("settings.test.okNoPrice");
      } catch (error) {
        result.textContent = t("settings.test.fail", { message: error.message });
      } finally {
        testButton.disabled = false;
      }
    },
  });
  const save = async () => {
    const modelId = choice === "custom" ? custom.value.trim() : choice;
    try {
      const updated = await call("save_cloud_settings", { apiKey: key.value.trim() || null, modelId });
      state.info = { ...state.info, cloud: updated };
      key.value = "";
      toast(t("settings.saved"));
      await refreshInfo();
    } catch (error) {
      toast(error.message, "bad");
    }
  };

  // A form, so Enter in the key field saves; nothing is submitted anywhere.
  return h(
    "form",
    {
      class: "card card-pad",
      autocomplete: "off",
      onsubmit: (e) => {
        e.preventDefault();
        save();
      },
    },
    h("div", { class: "card-head" }, h("div", null, h("h2", { class: "card-title" }, icon("sparkle"), t("settings.ai.title")), h("p", { class: "hint" }, t("settings.ai.sub"))), status),
    h("div", { class: "field" }, h("span", { class: "label" }, t("settings.model")), select, customField, note),
    h(
      "div",
      { class: "field", style: { "margin-top": "14px" } },
      h("span", { class: "label" }, t("settings.key")),
      h("div", { class: "row" }, key, reveal),
      cloud.key_source === "environment" || cloud.model_source === "environment" ? h("p", { class: "hint" }, icon("info"), " ", t("settings.key.env")) : null,
    ),
    h("div", { class: "row wrap", style: { "margin-top": "16px" } }, button(t("settings.save"), { variant: "primary", iconName: "check", type: "submit" }), testButton),
    result,
    cloud.problem && (cloud.key_configured || cloud.model_id) ? h("p", { class: "hint", style: { color: "var(--warn)" } }, cloud.problem) : null,
    cloud.file ? h("p", { class: "note", style: { "margin-top": "10px" } }, t("settings.file", { path: cloud.file })) : null,
  );
}

function historyRow() {
  const status = h("span", { class: "hint" });
  const clear = button(t("settings.history.clear"), {
    iconName: "x",
    size: "sm",
    disabled: !(state.conversations || []).length,
    onClick: async () => {
      const removed = await clearConversations();
      status.textContent = t("settings.history.cleared", { count: removed });
      clear.disabled = true;
    },
  });
  return h(
    "div",
    { class: "setting-row" },
    h("div", { class: "setting-text" }, h("strong", null, t("settings.history.title")), h("span", { class: "hint" }, t("settings.history.note")), status),
    clear,
  );
}

function privacyCard() {
  const cards = h("div");
  const what = h("div");
  const draw = () => {
    fill(
      cards,
      modeCards(state.privacy.mode, async (mode) => {
        if (mode !== "local" && !cloudReady()) {
          toast(t("mode.cloud.setup"), "bad");
          return;
        }
        // Keeping the same mode keeps its confirmation; a new cloud mode is confirmed by its
        // first reviewed preview.
        await setPrivacy(mode, mode === "local" || (mode === state.privacy.mode && state.privacy.confirmed));
        draw();
      }),
    );
    fill(what, disclosure(null, state.privacy.mode));
  };
  draw();
  return h(
    "section",
    { class: "card card-pad" },
    h("div", { class: "card-head" }, h("h2", { class: "card-title" }, icon("shield"), t("settings.privacy.title"))),
    h("p", { class: "hint", style: { "margin-bottom": "12px" } }, t("privacy.lead")),
    cards,
    h(
      "div",
      { class: "setting-row", style: { "margin-top": "12px" } },
      h("div", { class: "setting-text" }, h("strong", null, t("settings.privacy.ask")), h("span", { class: "hint" }, t("settings.privacy.askNote"))),
      toggle(!state.skipConsent, (on) => {
        state.skipConsent = !on;
      }, t("settings.privacy.ask")),
    ),
    historyRow(),
    h("details", { class: "more" }, h("summary", null, icon("chevron"), t("settings.privacy.what")), what),
  );
}

function appearanceCard(redraw) {
  const prefs = getPrefs();
  const row = (label, note, control) =>
    h("div", { class: "setting-row" }, h("div", { class: "setting-text" }, h("strong", null, label), note ? h("span", { class: "hint" }, note) : null), control);
  return h(
    "section",
    { class: "card card-pad" },
    h("div", { class: "card-head" }, h("h2", { class: "card-title" }, icon("sun"), t("settings.appearance.title"))),
    row(
      t("settings.theme"),
      null,
      segmented(
        [
          { value: "system", label: t("theme.system"), icon: "pc" },
          { value: "light", label: t("theme.light"), icon: "sun" },
          { value: "dark", label: t("theme.dark"), icon: "moon" },
        ],
        prefs.theme,
        (v) => {
          setPref("theme", v);
          redraw();
        },
        t("settings.theme"),
      ),
    ),
    row(
      t("settings.language"),
      null,
      segmented(
        [
          { value: "system", label: t("lang.system"), icon: "globe" },
          { value: "en", label: t("lang.en") },
          { value: "fr", label: t("lang.fr") },
        ],
        prefs.language,
        (v) => {
          setPref("language", v);
          window.dispatchEvent(new Event("pcagent:redraw"));
        },
        t("settings.language"),
      ),
    ),
    row(
      t("settings.motion"),
      t("settings.motion.note"),
      segmented(
        [
          { value: "system", label: t("motion.system") },
          { value: "on", label: t("motion.on") },
          { value: "off", label: t("motion.off") },
        ],
        prefs.motion,
        (v) => {
          setPref("motion", v);
          redraw();
        },
        t("settings.motion"),
      ),
    ),
  );
}

export function mount(page) {
  let web = null;
  const render = () => {
    web = webSettingsCard();
    fill(
      page,
      h("div", { class: "page-head" }, h("div", null, h("h1", { class: "page-title" }, t("settings.title")), h("p", { class: "page-sub" }, t("settings.sub")))),
      h(
        "div",
        { class: "split" },
        h("div", { class: "stack" }, aiCard(), privacyCard(), web.el),
        h(
          "div",
          { class: "stack" },
          appearanceCard(render),
          h(
            "section",
            { class: "card card-pad" },
            h("div", { class: "card-head" }, h("h2", { class: "card-title" }, icon("info"), t("settings.about.title"))),
            h("p", { class: "muted" }, t("settings.about.text", { version: state.info?.version || "" })),
          ),
        ),
      ),
    );
  };
  render();
  return subscribe((topic) => {
    if (topic === "info") render();
    // Web search exists only with AI analysis: its card follows the sharing choice.
    else if (topic === "privacy") web?.redraw();
  });
}
