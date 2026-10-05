// Screenshots of the main states (welcome, consent, running, answer, sources, live PC, history,
// storage, settings, the Live PC view with Explain) in light and dark themes, against the mock
// (dev/mock.js).
// Usage, with `python -m http.server 5173 --bind 127.0.0.1` running in apps/gui:
//   node dev/tour.mjs <output folder> [chrome.exe path] [language: en|fr]
import { launch } from "./cdp.mjs";
import { mkdirSync } from "node:fs";
import { join } from "node:path";

const out = process.argv[2] || "tour";
const chrome = process.argv[3] || undefined;
const language = process.argv[4] || "en";
mkdirSync(out, { recursive: true });
const base = "http://127.0.0.1:5173/ui/index.html";
const { proc, page } = await launch(chrome ? { chrome } : {});
const shot = (name) => page.shot(join(out, `${name}.png`));
const prefs = JSON.stringify({ language, mode: "cloud" });
try {
  await page.viewport(1280, 800);
  for (const dark of [false, true]) {
    const theme = dark ? "dark" : "light";
    await page.scheme(dark);
    await page.goto(`${base}#/chat`);
    await page.eval(`localStorage.setItem('pcagent.prefs.v1', ${JSON.stringify(prefs)}); sessionStorage.clear()`);
    await page.goto(`${base}?cloud=ready&privacy=complete#/chat`);
    await page.waitFor("document.querySelector('.welcome') && document.querySelector('.status-text.warn')");
    await page.sleep(400);
    await shot(`1-home-${theme}`);
    // Ask, approve the consent dialog, and catch the analysis while it runs.
    await page.click(".suggestion", 0);
    await page.waitFor("document.querySelector('.dialog .btn-primary') && !document.querySelector('.dialog .btn-primary').disabled", 10000);
    await page.sleep(400);
    await shot(`2-consent-${theme}`);
    await page.click(".dialog .btn-primary");
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
    await page.goto(`${base}?cloud=ready#/settings`);
    await page.waitFor("document.querySelector('select')");
    await shot(`9-settings-${theme}`);
    // Live PC: select memory, then Explain it with the exact preview of what leaves the PC.
    await page.goto(`${base}?cloud=ready&privacy=complete#/live`);
    await page.waitFor("document.querySelectorAll('.livepage-card').length >= 3 && document.querySelector('.livepage-app')", 10000);
    await page.click(".livepage-card", 0);
    await page.waitFor("document.querySelector('.insp-actions .btn-primary:not([disabled])')");
    await page.sleep(300);
    await shot(`10-live-${theme}`);
    await page.click(".insp-actions .btn-primary");
    await page.waitFor("document.querySelector('.dialog .select-exact')", 10000);
    await page.sleep(400);
    await shot(`11-explain-preview-${theme}`);
    await page.click(".dialog .btn-primary");
    await page.waitFor("document.querySelector('.answer-foot')", 20000);
    await page.sleep(300);
    await shot(`12-explain-answer-${theme}`);
  }
  await page.eval("localStorage.removeItem('pcagent.prefs.v1')");
} finally {
  const problems = page.logs.filter((l) => /exception|error/i.test(l) && !l.includes("favicon"));
  console.log(problems.length ? problems.join("\n") : "No page errors.");
  proc.kill();
}
