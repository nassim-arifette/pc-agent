import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { noxAsset } from "../ui/js/nox.js";

test("Nox's bundled poses match the existing brand assets and stay self-contained", () => {
  for (const pose of ["hello", "investigate", "thinking", "local"]) {
    const asset = readFileSync(new URL(noxAsset(pose)), "utf8");
    assert.equal(asset, readFileSync(new URL(`../../../docs/brand/nox/nox-${pose}.svg`, import.meta.url), "utf8"));
    assert.doesNotMatch(asset, /<script\b|<foreignObject\b|\bon\w+\s*=|(?:href|src)\s*=\s*["'](?:https?:|data:|\/\/)/i);
  }
});

test("unknown poses cannot introduce remote or arbitrary asset URLs", () => {
  for (const pose of ["verified", "../../private", "https://example.com/mascot.svg", null]) {
    assert.equal(noxAsset(pose), noxAsset("hello"));
  }
});
