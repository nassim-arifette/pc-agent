// The latest daily report as the home card and the settings row show it: the answer first
// (how many points to look at), then each point by its local rule's heading.
import { h } from "../dom.js";
import { icon } from "../icons.js";
import { t, classificationLabel } from "../i18n.js";
import { badge } from "../ui.js";
import { date } from "../format.js";

/** When the measurement was taken, in the interface language. */
function when(report) {
  return Number.isFinite(report.generated_at_unix_ms) ? date(report.generated_at_unix_ms, true) : report.date;
}

export function historyNote(report) {
  return t(`report.history.${report.history_status}`);
}

export function reportSummary(report, { compact = false } = {}) {
  const count = report.points_to_review;
  const tone = count === 0 ? "good" : "warn";
  const items = report.items.map((item) =>
    h("li", { class: "report-item" }, icon("alert"), h("span", null, classificationLabel(item))));
  if (report.alerts_total > 0) {
    items.push(h("li", { class: "report-item" }, icon("bell"), h("span", null, t("report.alerts", { count: report.alerts_total }))));
  }
  return h(
    "div",
    { class: `report-summary${compact ? " compact" : ""}` },
    h(
      "div",
      { class: "report-headline" },
      badge(count === 0 ? t("report.ok") : t("report.points", { count }), tone, count === 0 ? "check" : "alert"),
      h("span", { class: "hint" }, t(report.scheduled ? "report.when.scheduled" : "report.when.manual", { when: when(report) })),
    ),
    items.length ? h("ul", { class: "report-items" }, ...items) : null,
    compact ? null : h("p", { class: "hint" }, historyNote(report)),
  );
}
