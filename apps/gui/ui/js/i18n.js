// Locale selection, interpolation and fallbacks shared by the interface.
import { en } from "./i18n/en.js";
import { fr, classFr } from "./i18n/fr.js";

const dictionaries = { en, fr };
let current = "en";

export function setLanguage(choice) {
  const system = (navigator.language || "en").toLowerCase().startsWith("fr") ? "fr" : "en";
  current = choice === "en" || choice === "fr" ? choice : system;
  document.documentElement.lang = current;
}
export function language() {
  return current;
}

/** Translates a key, filling {placeholders}. Unknown keys fall back to English, then the key. */
export function t(key, vars) {
  let text = dictionaries[current][key] ?? en[key] ?? key;
  if (vars) {
    for (const [name, value] of Object.entries(vars)) {
      text = text.split(`{${name}}`).join(String(value));
    }
  }
  return text;
}

/** A finding's heading in the interface language. */
export function classificationLabel(finding) {
  if (current === "fr" && classFr[finding.classification]) return classFr[finding.classification];
  return finding.label;
}
