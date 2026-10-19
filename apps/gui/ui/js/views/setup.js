// First launch reuses Settings; leaving never starts a conversation or grants elevation.
import { h, fill } from "../dom.js";
import { t } from "../i18n.js";
import { call } from "../api.js";
import { state, subscribe, setPrivacy } from "../store.js";
import { getPrefs, setPref } from "../prefs.js";
import { navigate } from "../router.js";
import { leaveOnboarding } from "../onboarding.js";
import { aiCard, privacyCard, clearCloudDraft } from "./settings.js";
import { button, segmented, toast } from "../ui.js";
import { keepFocus } from "../actionbar.js";

export function mount(page) {
  let leaving = false;
  const leave = async (choice) => {
    if (leaving) return;
    leaving = true;
    render();
    try {
      await leaveOnboarding(choice, { setPrivacy, remember: (value) => setPref("onboarding", value), navigate });
    } catch {
      toast(t("onboarding.error.local"), "bad");
    } finally {
      leaving = false;
      if (page.isConnected) render();
    }
  };
  const render = () => keepFocus(page, () => fill(
    page,
    h("div", { class: "page-head" }, h("div", null,
      h("h1", { class: "page-title", tabindex: "-1", "data-key": "setup-title" }, t("onboarding.title")),
      h("p", { class: "page-sub" }, t("onboarding.lead")))),
    h("div", { class: "stack settings-column" },
      h("section", { class: "card card-pad" },
        segmented([{ value: "en", label: "English" }, { value: "fr", label: "Français" }], document.documentElement.lang, (language) => {
          setPref("language", language);
          window.dispatchEvent(new Event("pcagent:redraw"));
        }, t("settings.language")),
        h("h2", { class: "card-title" }, t("onboarding.key.title")),
        h("p", null, t("onboarding.key.steps")),
        h("p", { class: "hint" }, t("onboarding.key.privacy")),
        button(t("onboarding.key.open"), { onClick: async () => {
          try { await call("open_nebius_console"); }
          catch { toast(t("onboarding.error.browser"), "bad"); }
        } }),
        h("p", { class: "note" }, "https://tokenfactory.nebius.com/")),
      h("section", null,
        h("h2", { class: "card-title" }, t("onboarding.connect.title")),
        h("p", { class: "hint" }, t("onboarding.connect.steps")),
        aiCard()),
      privacyCard(),
      h("section", { class: "card card-pad" },
        h("h2", { class: "card-title" }, t("onboarding.continue.title")),
        h("p", { class: "hint" }, t("onboarding.continue.note")),
        h("div", { class: "row wrap" },
          button(t("onboarding.finish"), { variant: "primary", disabled: leaving || !state.info?.cloud?.ready, onClick: () => leave("complete") }),
          button(t("onboarding.local"), { disabled: leaving, onClick: () => leave("local") }),
          button(t("onboarding.skip"), { disabled: leaving, onClick: () => leave("skipped") })),
        h("p", { class: "hint" }, t("onboarding.resume.note")),
        getPrefs().onboarding ? h("p", { class: "note" }, t("onboarding.preserved")) : null),
    ),
  ));
  render();
  page.querySelector("h1")?.focus();
  const off = subscribe((topic) => {
    if (["info", "connection-test"].includes(topic)) render();
  });
  return () => { off(); clearCloudDraft(page); };
}
