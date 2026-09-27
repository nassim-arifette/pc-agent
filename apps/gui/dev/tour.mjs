// Screenshots of every screen in light and dark themes, against the mock (dev/mock.js).
// Usage, with `python -m http.server 5173 --bind 127.0.0.1` running in apps/gui:
//   node dev/tour.mjs <output folder> [chrome.exe path]
import { launch } from "./cdp.mjs";
import { mkdirSync } from "node:fs";
import { join } from "node:path";

const out = process.argv[2] || "tour";
const chrome = process.argv[3];
mkdirSync(out, { recursive: true });
const base = "http://127.0.0.1:5173/ui/index.html";
const { proc, page } = await launch(chrome ? { chrome } : {});
const shot = (name) => page.shot(join(out, `${name}.png`));
try {
  await page.viewport(1280, 900);
  for (const dark of [false, true]) {
    const theme = dark ? "dark" : "light";
    await page.scheme(dark);
    await page.goto(`${base}#/home`);
    await page.eval("localStorage.setItem('pcagent.prefs.v1', JSON.stringify({language: 'en', mode: 'local'}))");
    await page.goto(`${base}?cloud=ready#/home`);
    await page.waitFor("document.querySelectorAll('.metric-value').length >= 4");
    await page.sleep(3500);
    await shot(`home-${theme}`);
    await page.clickText(".chip", "Why is my PC slow?");
    await page.waitFor("document.querySelector('.answer-title')", 10000);
    await shot(`investigate-${theme}`);
    await page.goto(`${base}?cloud=ready#/storage`);
    await page.waitFor("document.querySelectorAll('.chip').length >= 6");
    await page.clickText(".chip", "Downloads");
    await page.waitFor("document.querySelector('.tabs')", 10000);
    await shot(`storage-${theme}`);
    await page.goto(`${base}?cloud=ready#/settings`);
    await page.waitFor("document.querySelector('select')");
    await shot(`settings-${theme}`);
  }
  await page.eval("localStorage.removeItem('pcagent.prefs.v1')");
} finally {
  const problems = page.logs.filter((l) => /exception|error/i.test(l) && !l.includes("favicon"));
  console.log(problems.length ? problems.join("\n") : "No page errors.");
  proc.kill();
}
