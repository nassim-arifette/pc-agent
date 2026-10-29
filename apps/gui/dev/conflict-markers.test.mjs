// Merge conflict markers in files no compiler or parser reads (HTML, CSS, Markdown) would ship
// silently: the page would show them and skip the stylesheets between them.
import test from "node:test";
import assert from "node:assert/strict";
import { readdirSync, readFileSync, statSync } from "node:fs";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

const root = fileURLToPath(new URL("..", import.meta.url));
const MARKER = /^(<{7}|={7}|>{7})( |$)/m;

function files(dir) {
  return readdirSync(dir).flatMap((name) => {
    const path = join(dir, name);
    if (statSync(path).isDirectory()) return name === "node_modules" ? [] : files(path);
    return /\.(html|css|js|mjs|json|md)$/.test(name) ? [path] : [];
  });
}

test("no merge conflict marker is left in the interface or its tooling", () => {
  const found = [...files(join(root, "ui")), ...files(join(root, "dev"))]
    .filter((path) => MARKER.test(readFileSync(path, "utf8")));
  assert.deepEqual(found, []);
});
