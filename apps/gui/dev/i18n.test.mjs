import test from "node:test";
import assert from "node:assert/strict";
import { en } from "../ui/js/i18n/en.js";
import { fr, classFr } from "../ui/js/i18n/fr.js";
import { setLanguage, language, t, classificationLabel } from "../ui/js/i18n.js";

globalThis.document = { documentElement: { lang: "" } };
const systemLanguage = (value) => Object.defineProperty(globalThis, "navigator", {
  value: { language: value }, configurable: true,
});

test("both dictionaries have the same keys and interpolation parameters", () => {
  assert.deepEqual(Object.keys(fr).sort(), Object.keys(en).sort());
  for (const key of Object.keys(en)) {
    assert.equal(typeof en[key], "string", key);
    assert.equal(typeof fr[key], "string", key);
    assert.ok(en[key].length && fr[key].length, key);
    const parameters = (text) => [...new Set(text.match(/\{\w+\}/g) || [])].sort();
    assert.deepEqual(parameters(en[key]), parameters(fr[key]), key);
  }
});

test("explicit language overrides the system and updates the document language", () => {
  systemLanguage("fr-FR");
  setLanguage("en");
  assert.equal(language(), "en");
  assert.equal(document.documentElement.lang, "en");
  assert.equal(t("common.close"), en["common.close"]);
  setLanguage("fr");
  assert.equal(language(), "fr");
  assert.equal(document.documentElement.lang, "fr");
  assert.equal(t("common.close"), fr["common.close"]);
});

test("system language selects French by prefix, otherwise English", () => {
  for (const [system, expected] of [["FR-ca", "fr"], ["en-GB", "en"], ["de-DE", "en"], ["", "en"], [undefined, "en"]]) {
    systemLanguage(system);
    for (const choice of ["system", "invalid", undefined]) {
      setLanguage(choice);
      assert.equal(language(), expected);
    }
  }
});

test("interpolation preserves literal values and missing placeholders", () => {
  systemLanguage("en");
  setLanguage("en");
  assert.equal(t("image.details", { width: 640, height: 480, size: "<b>$&</b>" }), "640 × 480 · JPEG · <b>$&</b>");
  assert.equal(t("image.details", { width: 640 }), "640 × {height} · JPEG · {size}");
  assert.equal(t("unknown.{value}.{value}", { value: "$&" }), "unknown.$&.$&");
  assert.equal(t("missing.translation"), "missing.translation");
});

test("a missing French value falls back to English without changing the locale", () => {
  const key = "common.close";
  const saved = fr[key];
  systemLanguage("fr-FR");
  setLanguage("fr");
  try {
    delete fr[key];
    assert.equal(t(key), en[key]);
    assert.equal(language(), "fr");
  } finally {
    fr[key] = saved;
  }
});

test("finding headings translate known classifications only in French", () => {
  systemLanguage("en");
  for (const [classification, label] of Object.entries(classFr)) {
    const finding = { classification, label: "Core heading" };
    setLanguage("en");
    assert.equal(classificationLabel(finding), finding.label);
    setLanguage("fr");
    assert.equal(classificationLabel(finding), label);
  }
  assert.equal(classificationLabel({ classification: "unknown", label: "Core fallback" }), "Core fallback");
});
