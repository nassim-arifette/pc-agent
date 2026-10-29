// The Surveillances view, the Live PC proposals and the window-wide runtime over the synthetic
// C7 fixture, with a small DOM stand-in. Native toasts and their clicks are Windows acceptance.
import test from "node:test";
import assert from "node:assert/strict";

class Node {
  constructor(tag = "", text = "") { this.tag = tag; this.text = text; this.children = []; this.attrs = {}; this.dataset = {}; this.listeners = {}; this.style = { setProperty() {} }; }
  append(...nodes) { for (const node of nodes) { node.parent = this; this.children.push(node); } }
  remove() { if (this.parent) this.parent.children = this.parent.children.filter((n) => n !== this); }
  get firstChild() { return this.children[0]; }
  get textContent() { return this.text + this.children.map((n) => n.textContent).join(""); }
  set textContent(text) { this.text = text; this.children = []; }
  setAttribute(key, value) { this.attrs[key] = String(value); }
  getAttribute(key) { return this.attrs[key] ?? null; }
  addEventListener(key, run) { this.listeners[key] = run; }
  get classList() { return { contains: (cls) => (this.attrs.class || "").split(" ").includes(cls), toggle() {}, add() {}, remove() {} }; }
  querySelectorAll(selector) {
    const matches = (node) => selector.startsWith(".") ? node.classList.contains(selector.slice(1)) : node.tag === selector;
    return this.children.flatMap((node) => [...(matches(node) ? [node] : []), ...node.querySelectorAll(selector)]);
  }
}
globalThis.Node = Node;
Object.defineProperty(globalThis, "navigator", { value: { language: "en" }, configurable: true });
const toasts = new Node("div");
globalThis.document = {
  documentElement: { lang: "en", dataset: {}, classList: { toggle() {} } }, activeElement: null,
  createElement: (tag) => new Node(tag), createElementNS: (_, tag) => new Node(tag),
  createTextNode: (text) => new Node("", text), getElementById: () => toasts,
  addEventListener() {}, removeEventListener() {},
};
const calls = [];
const handlers = new Map();
const windowListeners = new Map();
let commands = {};
globalThis.window = {
  location: { hash: "#/watches", search: "" }, matchMedia: () => ({ matches: false, addEventListener() {} }),
  sessionStorage: { getItem: () => null, setItem() {} }, localStorage: { getItem: () => null, setItem() {} },
  setTimeout: () => 0, clearTimeout() {}, setInterval: () => 1, clearInterval() {},
  addEventListener: (name, run) => windowListeners.set(name, run), removeEventListener: (name) => windowListeners.delete(name),
  dispatchEvent: (event) => windowListeners.get(event.type)?.(event),
  __TAURI__: { core: { invoke: async (command, args = {}) => {
    calls.push({ command, args });
    const run = commands[command];
    if (!run) throw Error(`Unexpected IPC ${command}`);
    return structuredClone(await run(args));
  } }, event: { listen: async (name, cb) => { handlers.set(name, (payload) => cb({ payload })); return () => handlers.delete(name); } } },
};
globalThis.CustomEvent = class { constructor(type) { this.type = type; } };
const { createWatchFixture } = await import("./mock/watches.js");
const { connect } = await import("../ui/js/api.js");
const { mount } = await import("../ui/js/watches/view.js");
const { liveWatch } = await import("../ui/js/watches/model.js");
const { describe } = await import("../ui/js/watches/text.js");
const runtime = await import("../ui/js/watches/runtime.js");
await connect();

const flush = () => new Promise((resolve) => setImmediate(resolve));
const byText = (node, text) => node.querySelectorAll("button").find((b) => b.textContent.includes(text));
function use(search) {
  calls.length = 0;
  let sequence = 0;
  commands = createWatchFixture(new URLSearchParams(search), () => `source-${++sequence}`).commands;
}

test("Live PC proposes only rules C7 can evaluate, and says why otherwise", () => {
  const GiB = 1024 ** 3;
  assert.deepEqual(liveWatch({ kind: "resource", card: { resource: "cpu" } }).rule, { kind: "resource_above", resource: "cpu", threshold_basis_points: 9000, sustained_seconds: 60 });
  assert.deepEqual(liveWatch({ kind: "resource", card: { resource: "disk", volume: { kind: "fixed", root: "c:\\", total_bytes: 500 * GiB } } }).rule, { kind: "volume_free_below", root: "C:\\", threshold_bytes: 10 * GiB });
  assert.equal(liveWatch({ kind: "resource", card: { resource: "disk", volume: { kind: "fixed", root: "E:\\", total_bytes: 8 * GiB } } }).rule.threshold_bytes, GiB);
  assert.equal(liveWatch({ kind: "resource", card: { resource: "disk", volume: { kind: "ram_disk", root: "R:\\", total_bytes: 8 * GiB } } }).reason, "watch.reason.volume");
  assert.equal(liveWatch({ kind: "resource", card: { resource: "gpu", gpu: { nvml_index: null, temperature: { value: 60 } } } }).reason, "watch.reason.gpu");
  assert.equal(liveWatch({ kind: "resource", card: { resource: "gpu", gpu: { nvml_index: 1, temperature: { value: 60 } } } }).rule.nvml_index, 1);
  assert.equal(liveWatch({ kind: "resource", card: { resource: "battery" } }).reason, "watch.reason.battery");
  const app = liveWatch({ kind: "app", app: { name: "chrome.exe", working_set: { value: 3 * GiB } } }).rule;
  assert.equal(app.kind, "app_memory_above");
  assert.equal(app.executable, "chrome.exe");
  assert.ok(app.threshold_bytes >= 4.5 * GiB && app.threshold_bytes % (64 * 1024 ** 2) === 0);
  assert.equal(liveWatch({ kind: "process", member: { name: "", named: false } }).reason, "watch.reason.unnamed");
  assert.equal(liveWatch({ kind: "process", member: { name: "C:\\x\\a.exe" } }).reason, "watch.reason.unnamed");
});

test("rules read as sentences, a threshold never as a measurement", () => {
  assert.equal(describe({ kind: "resource_above", resource: "cpu", threshold_basis_points: 9000, sustained_seconds: 60 }), "CPU at or above 90% · 1 minute");
  assert.match(describe({ kind: "volume_free_below", root: "C:\\", threshold_bytes: 10 * 1024 ** 3 }), /^Free space on C: below 10/);
});

test("the view shows each rule's live state, toggles it, pages alerts and marks them seen", async () => {
  use("");
  const page = new Node("main");
  const stop = mount(page);
  await flush();
  const text = page.textContent;
  assert.match(text, /CPU at or above 90%/);
  assert.match(text, /In alert/);
  assert.match(text, /Off/);
  assert.match(text, /still open/);
  assert.match(text, /not notified: one per rule every 30 min/);
  assert.match(text, /interrupted \(PC Agent closed\)/);
  page.querySelectorAll(".switch")[0].listeners.click();
  await flush();
  assert.deepEqual(calls.find((c) => c.command === "watch_set_enabled").args.enabled, false);
  byText(page, "Mark 1 as seen").listeners.click();
  await flush();
  assert.ok(calls.some((c) => c.command === "watch_alerts_seen"));
  assert.equal(byText(page, "Mark 1 as seen"), undefined);
  byText(page, "Show the summary").listeners.click();
  await flush();
  assert.match(page.textContent, /4 alert\(s\): 1 open, 2 back to normal, 1 interrupted\./);
  const alerts = calls.filter((c) => c.command === "watch_alerts");
  assert.ok(alerts.every((c) => c.args.toUnixMs - c.args.fromUnixMs < 7 * 86_400_000 && c.args.limit <= 200));
  stop();
  assert.equal(windowListeners.has("watches:changed"), false);
});

test("a paused window and Windows-disabled notifications are said, an empty list guides", async () => {
  use("?watches=paused&notifications=disabled");
  const paused = new Node("main");
  mount(paused);
  await flush();
  assert.match(paused.textContent, /Another PC Agent window is checking the rules/);
  assert.match(paused.textContent, /turned off in Windows/);
  assert.match(paused.textContent, /Paused/);
  use("?watches=none");
  const empty = new Node("main");
  mount(empty);
  await flush();
  assert.match(empty.textContent, /No watch yet\./);
  assert.match(empty.textContent, /No alert in the last 7 days\./);
});

test("the runtime keeps the unseen count and opens Watches from a notification click", async () => {
  use("?activation=alert");
  let seen = null;
  runtime.onUnseenChange((count) => { seen = count; });
  window.location.hash = "#/chat";
  await runtime.startWatchRuntime();
  assert.equal(window.location.hash, "#/watches");
  assert.equal(runtime.unseenAlerts(), 1);
  handlers.get("watch:changed")(5);
  assert.equal(seen, 5);
  window.location.hash = "#/live";
  handlers.get("watch:open")("watch-alert-mock-1");
  assert.equal(window.location.hash, "#/watches");
  handlers.get("watch:alert")({ alert: {}, unseen_alerts: 6 });
  assert.equal(runtime.unseenAlerts(), 6);
  assert.match(toasts.textContent, /threshold was crossed/);
});
