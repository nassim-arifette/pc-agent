import test from "node:test";
import assert from "node:assert/strict";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";

const checker = fileURLToPath(new URL("./check-modules.mjs", import.meta.url));
const check = (root) => spawnSync(process.execPath, ["--experimental-vm-modules", checker, ...(root ? [root] : [])], { encoding: "utf8" });

test("every UI module parses and all static imports resolve to real exports", () => {
  const result = check();
  assert.equal(result.status, 0, result.stderr || String(result.error));
  assert.match(result.stdout, /Linked \d+ modules/);
});

test("every mock feature module links without loading a browser", () => {
  const result = check(fileURLToPath(new URL("./mock/", import.meta.url)));
  assert.equal(result.status, 0, result.stderr || String(result.error));
  assert.match(result.stdout, /Linked \d+ modules/);
});

function fixture(files, verify) {
  const root = mkdtempSync(join(tmpdir(), "pcagent-module-check-"));
  try {
    for (const [name, source] of Object.entries(files)) writeFileSync(join(root, name), source);
    verify(check(root));
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
}

test("the module check rejects imports of missing exports", () => {
  fixture({ "main.js": 'import { absent } from "./data.js";', "data.js": "export const present = 1;" }, (result) => {
    assert.notEqual(result.status, 0);
    assert.match(result.stderr, /does not provide an export named 'absent'/);
  });
});

test("the module check rejects imports of missing files", () => {
  fixture({ "main.js": 'import "./missing.js";' }, (result) => {
    assert.notEqual(result.status, 0);
    assert.match(result.stderr, /ENOENT/);
  });
});

test("the module check rejects duplicate bindings after a merge", () => {
  fixture({ "main.js": "const name = 1; const name = 2;" }, (result) => {
    assert.notEqual(result.status, 0);
    assert.match(result.stderr, /already been declared/);
  });
});

test("the module check does not execute top-level code", () => {
  fixture({ "main.js": 'throw new Error("must not execute"); export const value = 1;' }, (result) => {
    assert.equal(result.status, 0, result.stderr);
  });
});
