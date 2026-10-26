// Parse and link the UI's native ES modules without evaluating them, creating a DOM or connecting
// to Tauri. Run with --experimental-vm-modules; the Node test suite supplies that flag.
import { readdirSync, readFileSync } from "node:fs";
import { resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import { SourceTextModule } from "node:vm";

const root = process.argv[2] ? pathToFileURL(`${resolve(process.argv[2])}/`) : new URL("../ui/js/", import.meta.url);
const modules = new Map();

function load(url) {
  if (!modules.has(url.href)) {
    modules.set(url.href, new SourceTextModule(readFileSync(url, "utf8"), { identifier: url.href }));
  }
  return modules.get(url.href);
}

function discover(folder) {
  for (const entry of readdirSync(folder, { withFileTypes: true })) {
    const url = new URL(entry.name + (entry.isDirectory() ? "/" : ""), folder);
    if (entry.isDirectory()) discover(url);
    else if (entry.name.endsWith(".js")) load(url);
  }
}

discover(root);
for (const module of modules.values()) {
  if (module.status !== "unlinked") continue;
  await module.link((specifier, parent) => {
    if (!specifier.startsWith(".")) throw new Error(`Nonlocal UI import: ${specifier} in ${parent.identifier}`);
    return load(new URL(specifier, parent.identifier));
  });
}
console.log(`Linked ${modules.size} modules from ${fileURLToPath(root)} without executing application code.`);
