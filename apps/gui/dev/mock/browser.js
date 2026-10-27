import { GiB, fmt, m } from "./common.js";

// Synthetic browser measurements; totals and every member agree with the type breakdown.
export function browserFixture(ref, name, family) {
  const types = family === "firefox"
    ? [["browser_main", 1, .415], ["renderer", 23, 3.6], ["gpu", 1, .592], ["network", 1, .04], ["media_decoding", 1, .08], ["utility", 2, .06], ["unknown", 1, .02]]
    : [["browser_main", 1, .3], ["renderer", 12, 2.2], ["extension", 2, .18], ["gpu", 1, .42], ["network", 1, .04], ["storage", 1, .03], ["audio", 1, .025], ["media_decoding", 1, .05], ["utility", 1, .03], ["crash_handler", 1, .01], ["unknown", 1, .015]];
  const browser_types = types.map(([process_type, process_count, ws]) => ({
    process_type, process_count, classified_count: process_type === "unknown" ? 0 : process_count,
    working_set: m(Math.round(ws * GiB), fmt(ws * GiB)), cpu: m(process_count * 10, ""),
  }));
  const members = [];
  for (const row of browser_types) {
    for (let i = 0; i < row.process_count; i++) {
      const ws = Math.floor(row.working_set.value / row.process_count) + (i < row.working_set.value % row.process_count ? 1 : 0);
      members.push({
        entity_ref: `proc-${ref.slice(4)}p${members.length}`, name, readable_name: family === "firefox" ? "Mozilla Firefox" : "Google Chrome",
        pid: (family === "firefox" ? 9000 : 4000) + members.length, cpu: m(10, ""), working_set: m(ws, fmt(ws)), services: [],
        browser: { family, process_type: row.process_type, status: row.process_type === "unknown" ? "permission_denied" : "ok" },
      });
    }
  }
  const ws = browser_types.reduce((sum, row) => sum + row.working_set.value, 0);
  return {
    entity_ref: ref, name, readable_name: family === "firefox" ? "Mozilla Firefox" : "Google Chrome",
    process_count: members.length, members_truncated: false, cpu: m(members.length * 10, ""), working_set: m(ws, fmt(ws)),
    private_commit: m(Math.round(ws * .8), ""), working_set_increase: m(0, "0"),
    io_read: m(120000, ""), io_write: m(40000, ""), gpu: m(310, ""), services: [], members, browser_types,
  };
}

export function localBrowserFixture(applicationRef, includeTabs, partial = false) {
  const family = applicationRef === "app-7" ? "firefox" : "chrome";
  return {
    status: partial ? "partial" : "ok",
    windows: [{
      process_ref: `proc-${applicationRef.slice(4)}p0`, family,
      title: "LOCAL-ONLY · Browser diagnostic demo",
      tabs_status: includeTabs ? (partial ? "partial" : "ok") : "unsupported",
      tabs: includeTabs ? [
        { title: "LOCAL-ONLY · Browser diagnostic demo", selected: true },
        { title: "LOCAL-ONLY · Synthetic private tab <script>alert(1)</script>", selected: false },
        { title: "LOCAL-ONLY · Documentation", selected: false },
      ] : [],
    }],
  };
}
