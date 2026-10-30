import { h, fill } from "../dom.js";
import { call } from "../api.js";
import { t } from "../i18n.js";
import { button, toggle } from "../ui.js";

export function specialistSettingsCard() {
  let disposed = false;
  const body = h("div", { class: "card-rows" });
  const status = h("p", { class: "hint", role: "status" });
  const el = h("section", { class: "card card-pad", dataset: { specialist: "settings" } },
    h("h2", { class: "card-title" }, t("specialist.title")),
    h("p", { class: "hint" }, t("specialist.note")), body, status);
  async function load(check = false) {
    try {
      const view = await call("specialist_get", { check });
      if (!disposed) render(view);
    } catch {
      if (!disposed) status.textContent = t("specialist.unavailable");
    }
  }
  function render(view) {
    const identity = view.identity;
    fill(body,
      h("div", { class: "setting-row" }, h("div", { class: "setting-text" },
        h("strong", null, t("specialist.enable")), h("span", { class: "hint" }, t("specialist.scope"))),
      toggle(view.enabled, async (enabled) => {
        try { await call("specialist_set", { enabled }); }
        catch { status.textContent = t("specialist.saveFailed"); await load(); }
      }, t("specialist.enable"))),
      h("p", { class: "hint" }, view.endpoint),
      button(t("specialist.check"), { onClick: () => load(true) }),
      identity ? h("div", { class: "callout info", style: "overflow-wrap:anywhere" }, h("div", null,
        h("strong", null, t("specialist.ready", { device: identity.device })),
        h("p", null, identity.model),
        h("p", { class: "hint" }, t("specialist.revision", { revision: identity.revision })),
        h("p", { class: "hint" }, t("specialist.adapter", { hash: identity.adapter_sha256 })))) : null,
    );
    status.textContent = view.error ? t("specialist.unavailable") : identity ? t("specialist.synthetic") : t("specialist.start");
  }
  load();
  return { el, dispose: () => { disposed = true; } };
}
