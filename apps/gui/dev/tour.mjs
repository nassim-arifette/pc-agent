// Screenshots of the main states (welcome, what leaves, running, answer, sources, live PC, history,
// storage with the deeper checks, settings, the Live PC view with Explain and its summary, web
// search, the action bar under a selected item in Live PC, History and Storage, the battery: its
// card and inspector discharging and plugged in, then the status pill of a critically low battery,
// and the Storage inspector: a folder, a file, its summary, a narrow window and the administrator
// cards after the results) in light and dark themes, against the mock (dev/mock.js).
// Usage, with `python -m http.server 5173 --bind 127.0.0.1` running in apps/gui:
//   node dev/tour.mjs <output folder> [chrome.exe path] [language: en|fr]
// TOUR_BASE and TOUR_DEBUG_PORT override the page URL and Chrome's debugging port, for a server
// on another port (e.g. when several worktrees are reviewed at once).
import { launch } from "./cdp.mjs";
import { mkdirSync } from "node:fs";
import { join } from "node:path";

const out = process.argv[2] || "tour";
const chrome = process.argv[3] || undefined;
const language = process.argv[4] || "en";
mkdirSync(out, { recursive: true });
const base = process.env.TOUR_BASE || "http://127.0.0.1:5173/ui/index.html";
const port = Number(process.env.TOUR_DEBUG_PORT) || undefined;
const { proc, page } = await launch({ ...(chrome ? { chrome } : {}), ...(port ? { port } : {}) });
const shot = (name) => page.shot(join(out, `${name}.png`));
const prefs = JSON.stringify({ language, mode: "cloud" });
// The pill's words for a critically low battery (health.battery in ui/js/i18n.js).
const lowBatteryPill = language === "fr" ? "Batterie très faible" : "Battery critically low";
const batteryCard = ".livepage-card[data-key^='resource:battery:']";
// A page of the same origin that does not run the app: preferences written there are read by the
// next load, never overwritten by an app still starting (it saves its own once its status is known).
const quietPage = new URL("styles/tokens.css", base).href;
// Explain's answer is the summary in the panel's inspector, once the draft is replaced and checked.
const summaryAnswer = "document.querySelector('.panel.open .selection-summary .summary-answer:not(.draft)') && document.querySelector('.panel.open .selection-summary .verified-line, .panel.open .selection-summary .unverified')";
try {
  await page.viewport(1280, 800);
  for (const dark of [false, true]) {
    const theme = dark ? "dark" : "light";
    await page.scheme(dark);
    await page.goto(quietPage);
    await page.eval(`localStorage.setItem('pcagent.prefs.v1', ${JSON.stringify(prefs)}); sessionStorage.clear()`);
    await page.goto(`${base}?cloud=ready&privacy=complete#/chat`);
    await page.waitFor("document.querySelector('.welcome') && document.querySelector('.status-text.warn')");
    await page.sleep(400);
    await shot(`1-home-${theme}`);
    // "See what leaves" from the composer, then ask: no dialog stands before a send (C5).
    await page.click(".share-preview");
    await page.waitFor("document.querySelector('.dialog .two-lists')", 10000);
    await page.sleep(400);
    await shot(`2-preview-${theme}`);
    await page.click(".dialog .btn-primary");
    await page.click(".suggestion", 0);
    await page.waitFor("document.querySelector('.progress .step')", 10000);
    await page.sleep(700);
    await shot(`3-running-${theme}`);
    await page.waitFor("document.querySelector('.answer-foot') && document.querySelector('.answer-text .cite')", 20000);
    await page.sleep(300);
    await shot(`4-answer-${theme}`);
    await page.click(".answer-text .cite", 0);
    await page.waitFor("document.querySelector('.panel.open .source-card.focused')");
    await page.sleep(350);
    await shot(`5-sources-${theme}`);
    await page.click(".status-pill");
    await page.waitFor("document.querySelector('.panel.open .live-row')");
    await page.sleep(5600);
    await shot(`6-live-${theme}`);
    await page.goto(`${base}?cloud=ready&privacy=complete#/history`);
    await page.waitFor("document.querySelectorAll('.history-chart').length >= 4", 10000);
    await page.sleep(300);
    await shot(`7-history-${theme}`);
    await page.goto(`${base}?cloud=ready#/storage`);
    await page.waitFor("document.querySelectorAll('.chip').length >= 6");
    await page.click(".chip", 0);
    await page.waitFor("document.querySelector('.tabs')", 10000);
    await shot(`8-storage-${theme}`);
    // Deeper checks: the list, the review of the disk health check, then its result.
    await page.goto(`${base}?cloud=ready&privacy=complete&disk=ok#/storage`);
    await page.waitFor("document.querySelectorAll('.admin-check').length === 4");
    await page.eval("document.querySelector('.admin-card').scrollIntoView({ block: 'start' })");
    await page.sleep(300);
    await shot(`8a-admin-${theme}`);
    await page.click(".admin-check:nth-child(3) .admin-check-actions .btn:last-child");
    await page.waitFor("document.querySelector('.admin-review')");
    await page.eval("document.querySelector('.admin-card').scrollIntoView({ block: 'start' })");
    await page.sleep(300);
    await shot(`8b-admin-review-${theme}`);
    await page.click(".admin-review .btn-primary");
    await page.waitFor("document.querySelector('.admin-result .admin-group')", 10000);
    await page.eval("document.querySelector('.admin-result').scrollIntoView({ block: 'start' })");
    await page.sleep(300);
    await shot(`8c-admin-result-${theme}`);
    await page.goto(`${base}?cloud=ready#/settings`);
    await page.waitFor("document.querySelector('select')");
    await shot(`9-settings-${theme}`);
    // Live PC: select an application, then memory, whose action bar Explains it with the exact
    // preview of what leaves the PC.
    await page.goto(`${base}?cloud=ready&privacy=complete#/live`);
    await page.waitFor("document.querySelectorAll('.livepage-card').length >= 3 && document.querySelector('.livepage-app')", 10000);
    await page.click(".livepage-app", 1);
    await page.waitFor("document.querySelector('.livepage-app-slot .actbar .actbar-primary:not([aria-disabled])')");
    await page.sleep(300);
    await shot(`10a-live-app-${theme}`);
    await page.click(".livepage-card", 0);
    await page.eval("document.querySelector('.main').scrollTo(0, 0)");
    await page.waitFor("document.querySelector('.livepage-card-slot .actbar .actbar-primary:not([aria-disabled])')");
    await page.sleep(300);
    await shot(`10-live-${theme}`);
    await page.click(".actbar .actbar-primary");
    await page.waitFor(summaryAnswer, 20000);
    await page.sleep(300);
    await shot(`12-explain-answer-${theme}`);
    // Web search (C6): the settings card with a saved key, the consent note, the exact query
    // under its step, then a cited web page in the Sources panel.
    await page.goto(`${base}?cloud=ready&privacy=complete&web=on#/settings`);
    await page.waitFor("document.querySelector('.web-card .web-key')");
    await page.eval("document.querySelector('.web-card').scrollIntoView({ block: 'start' })");
    await page.sleep(300);
    await shot(`13-settings-web-${theme}`);
    await page.goto(`${base}?cloud=ready&privacy=complete&web=on#/chat`);
    await page.waitFor("document.querySelector('.welcome') && document.querySelector('.share-toggle[aria-pressed=\"true\"] + .share-toggle[aria-pressed=\"true\"]')");
    await page.sleep(300);
    await shot(`14-web-toggles-${theme}`);
    await page.click(".suggestion", 0);
    await page.waitFor("document.querySelector('.answer-foot') && document.querySelector('.answer-text .web-cite')", 30000);
    await page.click(".steps-fold > summary");
    await page.eval("document.querySelector('.web-step-query').scrollIntoView({ block: 'center' })");
    await page.sleep(300);
    await shot(`15-web-steps-${theme}`);
    await page.click(".answer-text .web-cite", 0);
    await page.waitFor("document.querySelector('.panel.open .web-source.focused')");
    await page.sleep(400);
    await shot(`16-web-source-${theme}`);
    // "Open in browser" asks the Rust side (open_web_source); the mock records it, never navigates.
    await page.click(".panel.open .web-source.focused .web-actions .btn");
    await page.waitFor("window.__pcagentMock.openedWebSources.length === 1");
    // The action bar under a selected history event, then under a folder of a scan.
    await page.goto(`${base}?cloud=ready&privacy=complete#/history`);
    await page.waitFor("document.querySelector('.event-main')", 10000);
    await page.click(".event-main", 0);
    await page.waitFor("document.querySelector('.event-row.selected .actbar')");
    await page.sleep(300);
    await shot(`17-history-bar-${theme}`);
    await page.goto(`${base}?cloud=ready&privacy=complete#/storage`);
    await page.waitFor("document.querySelectorAll('.chip').length >= 6");
    await page.click(".chip", 0);
    await page.waitFor("document.querySelector('.item-select')", 10000);
    await page.click(".item-select", 1);
    await page.waitFor("document.querySelector('.item-row.selected .actbar .actbar-primary:not([aria-disabled])')");
    await page.sleep(300);
    await shot(`18-storage-bar-${theme}`);
    // The battery card with its action bar and inspector, then the plugged-in readings with their
    // unreported values. A reload clears the selection, so the card is selected again each time.
    for (const [mode, name] of [["", "19-live-battery"], ["&battery=ac", "20-live-battery-ac"]]) {
      await page.goto(`${base}?cloud=ready&privacy=complete${mode}#/live`);
      await page.waitFor(`document.querySelector(${JSON.stringify(batteryCard)})`, 10000);
      await page.click(batteryCard);
      await page.waitFor(`document.querySelector('.livepage-card-slot .actbar .actbar-primary:not([aria-disabled])') && document.querySelector('.insp .battery-facts')${mode ? "" : " && document.querySelector('.insp .battery-capacity')"}`, 10000);
      await page.eval("document.querySelector('.main').scrollTo(0, 0)");
      await page.sleep(300);
      await shot(`${name}-${theme}`);
    }
    // With the drives fine, the pill can only be the battery's.
    await page.goto(`${base}?cloud=ready&privacy=complete&disk=ok&battery=low#/chat`);
    await page.waitFor(`document.querySelector('.status-text.warn')?.textContent === ${JSON.stringify(lowBatteryPill)}`, 10000);
    await page.sleep(300);
    await shot(`21-pill-battery-${theme}`);
    // Storage (steps 22 to 29): the inspector beside the results for a folder, then for a file
    // whose size on disk is unknown, the summary Explain writes in its place, the inspector under
    // the selected row in a narrow window, and the administrator cards after the results.
    await page.goto(`${base}?cloud=ready&privacy=complete#/storage`);
    await page.waitFor("document.querySelectorAll('.chip').length >= 6");
    await page.click(".chip", 0);
    await page.waitFor("document.querySelector('.item-select')", 10000);
    await page.click(".item-select", 0);
    await page.waitFor("document.querySelector('.storage-layout.beside .storage-insp:not(.insp-empty)') && document.querySelector('.item-row.selected .actbar .actbar-primary:not([aria-disabled])')");
    await page.eval("document.querySelector('.storage-results').scrollIntoView({ block: 'start' })");
    await page.sleep(400);
    await shot(`22-storage-insp-folder-${theme}`);
    await page.click(".storage-results .tabs [role='tab']", 1);
    await page.waitFor("!document.querySelector('.item-row.selected')");
    await page.click(".item-select", 1);
    await page.waitFor("document.querySelector('.storage-insp:not(.insp-empty) .insp-kv .unknown')");
    await page.eval("document.querySelector('.storage-results').scrollIntoView({ block: 'start' })");
    await page.sleep(400);
    await shot(`23-storage-insp-file-${theme}`);
    // The inspector is sticky: clicked in place, without the helper's scrolling.
    await page.eval(`document.querySelector(".storage-insp [data-key='insp:explain']").click()`);
    await page.waitFor(`${summaryAnswer} && !document.querySelector('.storage-insp')`, 20000);
    await page.sleep(300);
    await shot(`24-storage-insp-summary-${theme}`);
    await page.click(".panel.open .panel-top .icon-btn");
    await page.waitFor("document.querySelector('.storage-insp:not(.insp-empty)')");
    await page.viewport(860, 640);
    await page.waitFor("document.querySelector('.item-row.selected + .storage-insp-under')");
    await page.eval("document.querySelector('.item-row.selected').scrollIntoView({ block: 'start' })");
    await page.sleep(400);
    await shot(`25-storage-insp-narrow-${theme}`);
    await page.viewport(1280, 800);
    await page.waitFor("document.querySelector('.storage-layout.beside')");
    await page.eval("document.querySelector('.storage-admin').scrollIntoView({ block: 'start' })");
    await page.sleep(300);
    await shot(`26-storage-admin-${theme}`);
  }
  await page.eval("localStorage.removeItem('pcagent.prefs.v1')");
} finally {
  const problems = page.logs.filter((l) => /exception|error/i.test(l) && !l.includes("favicon"));
  console.log(problems.length ? problems.join("\n") : "No page errors.");
  proc.kill();
}
