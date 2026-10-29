// Synchronize the native presentation catalogue from the GUI's closed dictionaries.
// No runtime source text is translated: both renderers address the same keys and typed values.
import { DICTIONARIES } from "../ui/js/phrases.js";
import { en } from "../ui/js/i18n/en.js";
import { fr } from "../ui/js/i18n/fr.js";
import { readFileSync, writeFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { resolve } from "node:path";

export const catalogPath = new URL("../../../crates/shared/src/phrases.json", import.meta.url);
export const catalogText = () => `${JSON.stringify({
  en: { ...en, ...DICTIONARIES.en },
  fr: { ...fr, ...DICTIONARIES.fr },
}, null, 2)}\n`;

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  if (process.argv.includes("--check")) {
    if (readFileSync(catalogPath, "utf8").replace(/\r\n/g, "\n") !== catalogText()) {
      throw new Error("Native phrase catalogue is stale; run node apps/gui/dev/core-catalog.mjs.");
    }
  } else {
    writeFileSync(catalogPath, catalogText());
  }
}
