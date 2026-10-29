import { h, fill } from "../dom.js";
import { call } from "../api.js";
import { t } from "../i18n.js";
import { button, dialog, toast } from "../ui.js";
import { failureText, failureDetail } from "../failure.js";
import { KINDS, DURATIONS, MIB, defaultRule, formRule, validRule } from "./model.js";

/** Local confirmation is the only write path. Opening, editing and cancelling do not persist. */
export function watchForm(initial, id = null) {
  return dialog((close) => {
    let draft = structuredClone(initial);
    let busy = false;
    const fields = h("div", { class: "settings-fields" });
    const error = h("p", { class: "muted", role: "alert" });
    const kind = h("select", { class: "select", "aria-label": t("watch.kind"), "data-autofocus": true },
      KINDS.map((value) => h("option", { value, selected: value === draft.kind }, t(`watch.kind.${value}`))));
    let target;
    let threshold;
    let duration;
    const field = (label, control) => h("label", { class: "field" }, h("span", null, label), control);
    const draw = () => {
      const volume = draft.kind === "volume_free_below";
      const app = draft.kind.startsWith("app_");
      const bytes = volume || draft.kind === "app_memory_above";
      const gpu = draft.kind === "gpu_temperature_above";
      target = draft.kind === "resource_above"
        ? h("select", { class: "select", name: "target" }, ["cpu", "memory", "disk_busy"].map((value) => h("option", { value, selected: value === draft.resource }, t(`watch.resource.${value}`))))
        : h("input", {
          class: "input",
          name: "target", type: gpu ? "number" : "text", required: true, min: gpu ? 0 : null, step: gpu ? 1 : null,
          max: gpu ? 2 ** 32 - 1 : null, maxlength: app ? 260 : null,
          value: draft.root ?? draft.executable ?? draft.nvml_index, autocomplete: "off",
        });
      threshold = h("input", {
        class: "input",
        name: "threshold", type: "number", required: true,
        min: volume ? 1024 : bytes ? 64 : gpu ? 50 : app ? 1 : 10,
        max: volume ? Number.MAX_SAFE_INTEGER / MIB : bytes ? 2 ** 20 : gpu ? 105 : 100,
        step: bytes ? "any" : gpu ? 1 : 0.01,
        value: bytes ? draft.threshold_bytes / MIB : gpu ? draft.threshold_celsius : draft.threshold_basis_points / 100,
      });
      duration = h("select", { class: "select", name: "duration" },
        DURATIONS.map((value) => h("option", { value, selected: value === (draft.sustained_seconds ?? 0) }, t(`watch.duration.${value}`))));
      fill(fields,
        field(t(volume ? "watch.root" : app ? "watch.executable" : gpu ? "watch.gpuIndex" : "watch.resource"), target),
        field(t(volume ? "watch.belowBytes" : bytes ? "watch.aboveBytes" : gpu ? "watch.aboveCelsius" : "watch.abovePercent"), threshold),
        !volume && field(t("watch.duration"), duration),
        h("p", { class: "muted" }, t(volume ? "watch.volumeNote" : app ? "watch.appNote" : "watch.sampleNote")),
      );
      error.textContent = "";
    };
    kind.addEventListener("change", () => { draft = defaultRule(kind.value); draw(); });
    const cancel = button(t("common.cancel"), { onClick: () => { if (!busy) close(false); } });
    const save = button(t(id ? "watch.save" : "watch.create"), { variant: "primary", type: "submit" });
    const form = h("form", { class: "watch-form", onsubmit: async (event) => {
      event.preventDefault();
      if (busy) return;
      const rule = formRule(kind.value, { target: target.value, threshold: threshold.value, duration: duration.value });
      if (!validRule(rule)) { error.textContent = t("watch.invalid"); return; }
      busy = true;
      save.disabled = cancel.disabled = true;
      error.textContent = "";
      try {
        await call(id ? "watch_update" : "watch_create", id ? { id, rule } : { rule });
        window.dispatchEvent(new CustomEvent("watches:changed"));
        close(true);
        toast(t("watch.saved"));
      } catch (reason) {
        error.textContent = failureText(reason, "watch.saveFailed");
        error.title = failureDetail(reason) ?? "";
      } finally {
        busy = false;
        save.disabled = cancel.disabled = false;
      }
    } },
    h("h2", null, t(id ? "watch.edit" : "watch.new")),
    h("p", { class: "muted" }, t("watch.local")),
    field(t("watch.kind"), kind), fields, error,
    h("div", { class: "dialog-actions" }, cancel, save));
    draw();
    return form;
  }, { label: t(id ? "watch.edit" : "watch.new"), dismissValue: false });
}
