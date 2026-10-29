import test from "node:test";
import assert from "node:assert/strict";
import { setLanguage } from "../ui/js/i18n.js";
import { volumeComparison } from "../ui/js/phrases.js";

globalThis.document = { documentElement: {} };
Object.defineProperty(globalThis, "navigator", { value: { language: "en" }, configurable: true });

test("before/after space preserves zero, missing sides and French units", () => {
  setLanguage("en");
  assert.equal(volumeComparison(null), "");
  assert.equal(volumeComparison({ free_before_bytes: 0, free_after_bytes: 1024 }), "Free space: 0 B → 1.0 KiB.");
  assert.equal(volumeComparison({}), "Free space: not measured → not measured.");
  setLanguage("fr");
  assert.equal(volumeComparison({ free_before_bytes: 0, free_after_bytes: 1024 }), "Espace libre : 0\u00a0o → 1,0\u00a0Kio.");
  assert.equal(volumeComparison({ free_before_bytes: -1, free_after_bytes: Infinity }), "Espace libre : non mesuré → non mesuré.");
});
