import test from "node:test";
import assert from "node:assert/strict";
import { browserFixture, localBrowserFixture } from "./mock/browser.js";
import { BROWSER_TYPES, browserSummary, sortedBrowserTypes } from "../ui/js/live/browser.js";
import { en } from "../ui/js/i18n/en.js";
import { fr } from "../ui/js/i18n/fr.js";
import { readFile } from "node:fs/promises";

test("realistic browser fixtures cover every closed type with matching member totals", () => {
  const chrome = browserFixture("app-1", "chrome.exe", "chrome");
  const firefox = browserFixture("app-7", "firefox.exe", "firefox");
  assert.deepEqual(chrome.browser_types.map((row) => row.process_type).sort(), [...BROWSER_TYPES].sort());
  assert.equal(firefox.process_count, 30);
  for (const app of [chrome, firefox]) {
    assert.equal(app.members.length, app.process_count);
    assert.equal(app.members.reduce((n, p) => n + p.working_set.value, 0), app.working_set.value);
    assert.equal(app.browser_types.reduce((n, p) => n + p.process_count, 0), app.process_count);
    assert.equal(app.browser_types.reduce((n, p) => n + p.working_set.value, 0), app.working_set.value);
    assert.equal(app.browser_types.find((p) => p.process_type === "unknown").classified_count, 0);
    assert.ok(!JSON.stringify(app).includes("LOCAL-ONLY"));
  }
});

test("browser sorting is stable, non-mutating and puts missing measurements last", () => {
  const rows = browserFixture("app-1", "chrome.exe", "chrome").browser_types;
  const before = JSON.stringify(rows);
  for (const sort of ["memory", "count", "cpu"]) assert.equal(sortedBrowserTypes(rows, sort)[0].process_type, "renderer");
  assert.equal(JSON.stringify(rows), before);
  const missing = { ...rows[0], working_set: { value: null } };
  assert.equal(sortedBrowserTypes([...rows.slice(1), missing]).at(-1), missing);
  assert.ok(browserSummary({ browser_types: rows }).includes("Unknown"));
  for (const type of BROWSER_TYPES) for (const dict of [en, fr]) assert.ok(dict[`browser.type.${type}`]);
});

test("local titles are on-demand and do not use the app store, selection or persistence", async () => {
  const windows = localBrowserFixture("app-7", false);
  assert.equal(windows.windows[0].tabs.length, 0);
  const tabs = localBrowserFixture("app-7", true, true);
  assert.equal(tabs.status, "partial");
  assert.equal(tabs.windows[0].tabs[0].selected, true);
  assert.ok(tabs.windows[0].tabs[1].title.includes("<script>"));
  const source = await readFile(new URL("../ui/js/live/browser.js", import.meta.url), "utf8");
  assert.doesNotMatch(source, /import .*store\.js|localStorage\.|sessionStorage\.|console\.|innerHTML|runSelection\(/);
  const view = await readFile(new URL("../ui/js/live/view.js", import.meta.url), "utf8");
  assert.match(view, /clearLocalBrowsers\(\)/);
});
