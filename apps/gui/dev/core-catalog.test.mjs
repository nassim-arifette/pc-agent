import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { catalogPath, catalogText } from "./core-catalog.mjs";
import { cliEn, cliFr } from "../ui/js/i18n/cli.js";
import { DICTIONARIES } from "../ui/js/phrases.js";

test("native and GUI renderers use exactly the same closed phrase catalogue", () => {
  assert.equal(readFileSync(catalogPath, "utf8").replace(/\r\n/g, "\n"), catalogText());
});

test("core and CLI catalogues have matching closed keys and placeholders", () => {
  for (const [en, fr] of [[cliEn, cliFr], [DICTIONARIES.en, DICTIONARIES.fr]]) {
    assert.deepEqual(Object.keys(en).sort(), Object.keys(fr).sort());
    for (const key of Object.keys(en)) {
      const names = (text) => [...new Set(text.match(/\{\w+\}/g) || [])].sort();
      assert.deepEqual(names(en[key]), names(fr[key]), key);
    }
  }
});

test("native CLI literal message keys must exist in both generated catalogues", () => {
  const source = readFileSync(new URL("../../cli/src/text.rs", import.meta.url), "utf8");
  const catalogue = JSON.parse(catalogText());
  for (const match of source.matchAll(/(?:say!|label)\(\s*"([^"]+)"/g)) {
    for (const language of ["en", "fr"]) assert.ok(catalogue[language][match[1]], match[1]);
  }
});
