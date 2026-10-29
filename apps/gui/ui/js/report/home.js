// Home: the latest daily report under the composer, or an invitation to turn it on. Nothing is
// sent: "Ask" only writes a question in the composer, which the person may edit or not send.
import { h, fill } from "../dom.js";
import { icon } from "../icons.js";
import { t } from "../i18n.js";
import { call } from "../api.js";
import { button } from "../ui.js";
import { navigate } from "../router.js";
import { reportSummary } from "./summary.js";
import { reveal } from "./schedule.js";

export function homeReportCard({ ask } = {}) {
  let disposed = false;
  const el = h("section", { class: "card card-pad report-home", hidden: true, "aria-label": t("report.home.title") });
  Promise.all([call("report_latest").catch(() => null), call("report_schedule_get").catch(() => null)])
    .then(([report, schedule]) => {
      if (disposed) return;
      const head = h("div", { class: "report-home-head" }, h("span", { class: "metric-icon report-icon" }, icon("clock")), h("h2", { class: "card-title" }, t("report.home.title")));
      if (report) {
        fill(
          el,
          head,
          reportSummary(report),
          h(
            "div",
            { class: "report-actions" },
            report.points_to_review > 0 && ask ? button(t("report.ask"), { iconName: "sparkle", size: "sm", onClick: () => ask(t("report.ask.question")) }) : null,
            button(t("report.reveal"), { iconName: "reveal", size: "sm", variant: "ghost", onClick: reveal }),
            schedule?.supported && !schedule.installed ? button(t("report.home.enable"), { iconName: "clock", size: "sm", variant: "ghost", onClick: () => navigate("settings") }) : null,
          ),
        );
      } else if (schedule?.supported && !schedule.installed) {
        fill(
          el,
          head,
          h("p", { class: "hint" }, t("report.home.invite")),
          h("div", { class: "report-actions" }, button(t("report.home.enable"), { iconName: "clock", size: "sm", onClick: () => navigate("settings") })),
        );
      } else {
        return;
      }
      el.hidden = false;
    });
  return { el, dispose: () => { disposed = true; } };
}
