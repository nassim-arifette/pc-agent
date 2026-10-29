// Render every tour state, then audit visible text and accessible names. No translation or
// replacement of model/user text: findings report the original string and where it appeared.
import { en } from "../ui/js/i18n/en.js";
import { fr } from "../ui/js/i18n/fr.js";
import { DICTIONARIES } from "../ui/js/phrases.js";
import { readFileSync, writeFileSync, mkdirSync } from "node:fs";
import { resolve, join } from "node:path";
import { fileURLToPath } from "node:url";
import { spawnSync } from "node:child_process";

const normalize = (text) => text.replace(/\s+/g, " ").trim();
const escape = (text) => text.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
const english = { ...en, ...DICTIONARIES.en };
const french = { ...fr, ...DICTIONARIES.fr };
const frenchTexts = new Set(Object.values(french).map(normalize));
const templates = Object.entries(english)
  .filter(([key, text]) => text !== french[key] && /[A-Za-z]{3}/.test(text.replace(/\{\w+\}/g, "")))
  .map(([key, text]) => ({
    key,
    pattern: new RegExp(`^${normalize(text).split(/\{\w+\}/).map(escape).join(".+?")}$`, "i"),
  }));

// Core prose which can escape dictionaries via legacy fallbacks or incomplete mock fixtures.
export const KNOWN_ENGLISH = [
  /\bavailable physical memory\b/i,
  /\bsystem commit\b/i,
  /\bworking sets? can share\b/i,
  /\b(?:one short|one-second) sample\b/i,
  /\b(?:not measured|permission denied|not supported)\b/i,
  /\b(?:was|were) (?:not |the |read |measured |available |used |busy)/i,
  /\b(?:no|first|last) (?:observation|measurement|sample|record)\b/i,
  /\b(?:keep|run|try|close|open) (?:the |your |unused )/i,
  /\b(?:memory|processor|disk|battery|files?) (?:usage|use|response|reads|size|available|unknown)/i,
  /\b(?:derived from|cannot prove|do not add up|read from disk|not a live measurement)\b/i,
  /\b(?:the|this|these) (?:PC|target|action|measurement|check|result|request|file|folder)/i,
];

export function findEnglish(text) {
  const value = normalize(text);
  if (!value) return [];
  // Shared technical names and genuine French translations are not English leaks.
  if (frenchTexts.has(value)) return [];
  const keys = templates.filter(({ pattern }) => pattern.test(value)).map(({ key }) => key);
  if (keys.length) return keys.map((key) => `dictionary:${key}`);
  return KNOWN_ENGLISH.filter((pattern) => pattern.test(value)).map((pattern) => `phrase:${pattern.source}`);
}

export function auditSnapshots(snapshots) {
  const findings = [];
  for (const snapshot of snapshots) {
    if (snapshot.language !== "fr") continue;
    for (const entry of snapshot.texts) {
      const reasons = findEnglish(entry.text);
      if (reasons.length) findings.push({
        state: snapshot.name, ...entry, reasons,
        intentional: ["local_name", "model", "web"].includes(entry.owner),
      });
    }
  }
  return findings;
}

function main() {
  const out = resolve(process.argv[2] || "i18n-audit");
  mkdirSync(out, { recursive: true });
  const root = fileURLToPath(new URL(".", import.meta.url));
  const all = [];
  for (const language of ["en", "fr"]) {
    const folder = join(out, language);
    const args = ["--experimental-default-type=module", join(root, "tour.mjs"), folder, process.argv[3] || "C:\\Program Files\\Google\\Chrome\\Application\\chrome.exe", language];
    const result = spawnSync(process.execPath, args, { env: { ...process.env, TOUR_AUDIT: "1" }, stdio: "inherit" });
    if (result.error || result.status !== 0) throw result.error || new Error(`${language} tour failed (${result.status})`);
    const snapshots = JSON.parse(readFileSync(join(folder, "rendered-text.json"), "utf8"));
    if (!snapshots.length || !snapshots.some((s) => s.name === "29-action-result-high-contrast")) throw new Error(`${language} tour incomplete`);
    all.push(...snapshots);
  }
  const findings = auditSnapshots(all);
  const leaks = findings.filter((finding) => !finding.intentional);
  writeFileSync(join(out, "findings.json"), JSON.stringify(findings, null, 2));
  for (const finding of findings) console.error(`${finding.state} [${finding.source}/${finding.context}/${finding.owner || "interface"}] ${finding.text} (${finding.intentional ? "preserved verbatim; " : ""}${finding.reasons.join(", ")})`);
  console.log(`${all.length} rendered states; ${leaks.length} English interface leaks; ${findings.length - leaks.length} intentionally preserved local names/model/web texts.`);
  if (leaks.length) process.exitCode = 1;
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) main();
