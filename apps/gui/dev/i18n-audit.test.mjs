import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { findEnglish, auditSnapshots } from "./i18n-audit.mjs";
import { setLanguage } from "../ui/js/i18n.js";
import { homeStatus } from "../ui/js/live/model.js";

test("source dictionaries cannot silently overwrite duplicate keys", () => {
  for (const locale of ["en", "fr"]) {
    const source = readFileSync(new URL(`../ui/js/i18n/${locale}.js`, import.meta.url), "utf8");
    const keys = [...source.matchAll(/^\s*"([^"]+)":/gm)].map((match) => match[1]);
    assert.equal(new Set(keys).size, keys.length, `${locale} duplicate key`);
  }
});

test("audit detects dictionary templates and known English core fallbacks", () => {
  assert.ok(findEnglish("Memory 81%").length);
  assert.ok(findEnglish("Available physical memory: 1.54 GiB of 15.73 GiB.").length);
  assert.ok(findEnglish("Working sets can share pages; they do not add up.").length);
  for (const text of [
    "Mémoire 81 %", "PC Agent", "NVIDIA GeForce RTX 4060", "C:\\Windows", "NVMe", "Non mesuré",
    "Fait. 18,60 Gio libérés, mesurés sur le disque. Espace libre : 4,10 Gio → 22,70 Gio.",
    "Nemotron 3 Ultra a repris cette question : la réponse de Nemotron 3 Super avait un niveau de confiance faible.",
  ]) assert.deepEqual(findEnglish(text), [], text);
});

test("rendered audit records the state and accessible attribute, without modifying the text", () => {
  const entry = { text: "Available physical memory: 42 GiB", source: "aria-label", context: "card" };
  const findings = auditSnapshots([{ name: "live-dark", language: "fr", texts: [entry] }, { name: "live-light", language: "en", texts: [entry] }]);
  assert.equal(findings.length, 1);
  assert.equal(findings[0].text, entry.text);
  assert.equal(findings[0].state, "live-dark");
  assert.equal(findings[0].source, "aria-label");
});

test("audit lists verbatim content separately, never exempting an untagged interface label", () => {
  const texts = ["interface", "local_name", "model", "web"].map((owner) => ({
    text: "Downloads", source: "text", context: "label", owner,
  }));
  const findings = auditSnapshots([{ name: "storage", language: "fr", texts }]);
  assert.equal(findings.length, 4);
  assert.deepEqual(findings.map((finding) => finding.intentional), [false, true, true, true]);
});

test("home status always includes memory, CPU and C:, preserving unknowns and zero", () => {
  globalThis.document = { documentElement: {} };
  Object.defineProperty(globalThis, "navigator", { value: { language: "en" }, configurable: true });
  setLanguage("fr");
  assert.equal(homeStatus(null), "Mémoire non mesuré · Processeur non mesuré · C: plein à non mesuré");
  const dash = { memory: { used_ratio: 0.81 }, cpu: { busy: { status: "ok", value: 1700 } }, volumes: [{ root: "C:\\", free_ratio: 0.08 }] };
  assert.match(homeStatus(dash), /^Mémoire 81\s% · Processeur 17\s% · C: plein à 92\s%$/);
  dash.cpu.busy.value = 0;
  assert.match(homeStatus(dash), /Processeur 0\s%/);
  dash.cpu.busy.status = "partial";
  assert.match(homeStatus(dash), /Processeur non mesuré/);
  setLanguage("en");
  assert.match(homeStatus(dash), /Memory 81% · Processor not measured · C: 92% full/);
});
