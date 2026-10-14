// Settings, in one column: the Nebius connection, privacy, web search, history and data,
// appearance, then about. What the person types survives a redraw (a saved web key, a language
// change): the AI form keeps its own state until it is saved.
import { h, fill } from "../dom.js";
import { icon } from "../icons.js";
import { t } from "../i18n.js";
import { call } from "../api.js";
import { disclosure, modeCards, cloudReady } from "../ask.js";
import { getPrefs, setPref } from "../prefs.js";
import { state, subscribe, refreshInfo, clearConversations, setPrivacy, setFileNames } from "../store.js";
import { badge, button, dialog, segmented, toast, toggle } from "../ui.js";
import { price } from "../format.js";
import { modelName } from "../models.js";
import { REASONING, refreshRouting } from "../picker.js";
import { webSettingsCard } from "../web.js";

/** The AI form as typed and not saved yet; null fields follow the saved settings. */
const typed = { key: "", choice: null, custom: null };

function forgetTyped() {
  typed.key = "";
  typed.choice = null;
  typed.custom = null;
}

function settingRow(label, note, control) {
  return h("div", { class: "setting-row" }, h("div", { class: "setting-text" }, h("strong", null, label), note ? h("span", { class: "hint" }, note) : null), control);
}

function aiCard() {
  const info = state.info;
  const cloud = info?.cloud || {};
  const models = info?.models || [];
  const known = models.some((m) => m.id === cloud.model_id);
  const savedChoice = cloud.model_id ? (known ? cloud.model_id : "custom") : models[0]?.id || "custom";
  let choice = typed.choice ?? savedChoice;

  const status = cloud.ready
    ? badge(t("settings.ai.ready"), "good", "check")
    : cloud.key_configured || cloud.model_id
      ? badge(t("settings.ai.problem"), "warn", "alert")
      : badge(t("settings.ai.notReady"), "", "info");

  const note = h("p", { class: "hint" });
  const custom = h("input", {
    class: "input input-grow",
    placeholder: "nvidia/…",
    value: typed.custom ?? (known ? "" : cloud.model_id || ""),
    spellcheck: "false",
    oninput: (e) => {
      typed.custom = e.target.value;
    },
  });
  const customField = h("label", { class: "field", style: { "margin-top": "8px" } }, h("span", { class: "label" }, t("settings.model.customLabel")), custom);
  const select = h(
    "select",
    {
      class: "select",
      "aria-label": t("settings.model"),
      onchange: (e) => {
        choice = e.target.value;
        typed.choice = choice;
        sync();
      },
    },
    models.map((m) => h("option", { value: m.id }, modelName(m.id))),
    h("option", { value: "custom" }, t("settings.model.custom")),
  );
  select.value = choice;
  const sync = () => {
    const model = models.find((m) => m.id === choice);
    note.textContent = model ? `${t(`settings.model.note.${model.note}`)} ${model.id}` : "";
    customField.style.setProperty("display", choice === "custom" ? "flex" : "none");
  };
  sync();

  // The model a question uses can come from the composer's routing choice instead.
  const mode = state.routing?.settings?.mode;
  const routing = mode && mode !== "configured" ? h("p", { class: "hint" }, icon("info"), " ", t(`settings.routing.${mode}`)) : null;

  const reasoning = cloud.model_id
    ? settingRow(
        t("picker.reasoning"),
        t(`picker.reasoning.${cloud.reasoning || "on"}.note`),
        segmented(
          REASONING.map((r) => ({ value: r, label: t(`picker.reasoning.${r}`) })),
          cloud.reasoning || "on",
          async (value) => {
            try {
              state.info.cloud = await call("save_cloud_settings", { apiKey: null, modelId: cloud.model_id, reasoning: value });
              await refreshInfo();
            } catch (error) {
              toast(error.message, "bad");
            }
          },
          t("picker.reasoning"),
        ),
      )
    : null;

  const key = h("input", {
    class: "input input-grow",
    type: "password",
    autocomplete: "off",
    spellcheck: "false",
    value: typed.key,
    placeholder: cloud.key_configured ? (cloud.key_hint ? t("settings.key.saved", { hint: cloud.key_hint }) : t("settings.key.savedNoHint")) : t("settings.key.new"),
    "aria-label": t("settings.key"),
    oninput: (e) => {
      typed.key = e.target.value;
    },
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
  const modelId = () => (choice === "custom" ? custom.value.trim() : choice);
  const changed = () => Boolean(key.value.trim()) || modelId() !== (cloud.model_id || "");

  /** Saves the form; true when it was saved. */
  const save = async () => {
    try {
      const updated = await call("save_cloud_settings", { apiKey: key.value.trim() || null, modelId: modelId() });
      forgetTyped();
      state.info = { ...state.info, cloud: updated };
      toast(t("settings.saved"));
      await refreshInfo();
      return true;
    } catch (error) {
      toast(error.message, "bad");
      return false;
    }
  };

  // The test checks what the form shows: unsaved changes are saved first, never tested silently
  // against the previous settings.
  const testButton = button(t("settings.test"), {
    iconName: "plug",
    title: t("settings.test.title"),
    onClick: async () => {
      testButton.disabled = true;
      try {
        if (changed() && !(await save())) return;
        result.textContent = t("settings.testing");
        const check = await call("test_cloud");
        result.textContent = check.price
          ? t("settings.test.ok", { input: price(check.price.input_per_million), output: price(check.price.output_per_million) })
          : t("settings.test.okNoPrice");
      } catch (error) {
        result.textContent = t("settings.test.fail", { message: error.message });
      } finally {
        if (testButton.isConnected) testButton.disabled = false;
      }
    },
  });

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
    h("div", { class: "field" }, h("span", { class: "label" }, t("settings.model")), select, customField, note, routing),
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
    reasoning ? h("div", { class: "card-rows" }, reasoning) : null,
    cloud.file ? h("p", { class: "note", style: { "margin-top": "10px" } }, t("settings.file", { path: cloud.file })) : null,
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
        try {
          await setPrivacy(mode);
        } catch (error) {
          toast(error.message, "bad");
        }
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
      { class: "card-rows" },
      // The same toggle as under the composer once a scanned folder is attached.
      settingRow(
        t("settings.privacy.files"),
        t("settings.privacy.filesNote"),
        toggle(state.privacy.file_names !== false, async (on) => {
          try {
            await setFileNames(on);
          } catch (error) {
            toast(error.message, "bad");
          }
        }, t("settings.privacy.files")),
      ),
    ),
    h("details", { class: "more" }, h("summary", null, icon("chevron"), t("settings.privacy.what")), what),
  );
}

/** Conversations, the performance history and starting with Windows: what PC Agent keeps here. */
function dataCard() {
  const conversationsStatus = h("span", { class: "hint" });
  const clearConversationsButton = button(t("settings.history.clear"), {
    iconName: "x",
    size: "sm",
    disabled: !(state.conversations || []).length,
    onClick: async () => {
      const removed = await clearConversations();
      conversationsStatus.textContent = t("settings.history.cleared", { count: removed });
      clearConversationsButton.disabled = true;
    },
  });
  const startupHost = h("span", { class: "hint" }, t("history.collection.unknown"));
  call("startup_get")
    .then((on) => {
      fill(
        startupHost,
        toggle(on, async (next) => {
          try {
            await call("startup_set", { enabled: next });
          } catch (error) {
            toast(error.message, "bad");
          }
        }, t("startup.title")),
      );
      startupHost.classList.remove("hint");
    })
    .catch(() => {});
  const clearHistoryButton = button(t("history.clear"), {
    iconName: "x",
    size: "sm",
    onClick: async () => {
      const yes = await dialog(
        (close) => [
          h("h2", { class: "dialog-title" }, t("history.clear.title")),
          h("p", null, t("history.clear.body")),
          h("div", { class: "dialog-actions" }, button(t("common.cancel"), { onClick: () => close(false) }), button(t("history.clear"), { variant: "danger", onClick: () => close(true) })),
        ],
        { label: t("history.clear.title"), dismissValue: false },
      );
      if (!yes) return;
      try {
        await call("history_clear");
        toast(t("history.cleared"));
      } catch (error) {
        toast(error.message, "bad");
      }
    },
  });
  return h(
    "section",
    { class: "card card-pad" },
    h("div", { class: "card-head" }, h("h2", { class: "card-title" }, icon("history"), t("settings.data.title"))),
    h(
      "div",
      { class: "card-rows" },
      h(
        "div",
        { class: "setting-row" },
        h("div", { class: "setting-text" }, h("strong", null, t("settings.history.title")), h("span", { class: "hint" }, t("settings.history.note")), conversationsStatus),
        clearConversationsButton,
      ),
      settingRow(t("settings.perf.title"), t("settings.perf.note"), clearHistoryButton),
      settingRow(t("startup.title"), t("settings.startup.note"), startupHost),
    ),
  );
}

function appearanceCard(redraw) {
  const prefs = getPrefs();
  return h(
    "section",
    { class: "card card-pad" },
    h("div", { class: "card-head" }, h("h2", { class: "card-title" }, icon("sun"), t("settings.appearance.title"))),
    h(
      "div",
      { class: "card-rows" },
      settingRow(
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
      settingRow(
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
      settingRow(
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
    ),
  );
}

function aboutCard() {
  return h(
    "section",
    { class: "card card-pad" },
    h("div", { class: "card-head" }, h("h2", { class: "card-title" }, icon("info"), t("settings.about.title"))),
    h("p", { class: "muted" }, t("settings.about.text", { version: state.info?.version || "" })),
  );
}

export function mount(page) {
  let web = null;
  const render = () => {
    web = webSettingsCard();
    fill(
      page,
      h("div", { class: "page-head" }, h("div", null, h("h1", { class: "page-title" }, t("settings.title")), h("p", { class: "page-sub" }, t("settings.sub")))),
      h("div", { class: "stack settings-column" }, aiCard(), privacyCard(), web.el, dataCard(), appearanceCard(render), aboutCard()),
    );
  };
  render();
  // The composer's routing choice decides the model too: Settings says so when it does. A
  // changed policy notifies "info", which draws the page again.
  refreshRouting();
  const unsubscribe = subscribe((topic) => {
    if (topic === "info") render();
    // Web search exists only with AI analysis: its card follows the sharing choice.
    else if (topic === "privacy") web?.redraw();
  });
  return unsubscribe;
}
