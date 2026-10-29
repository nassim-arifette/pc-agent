// Exercises the real view's async lifecycle with a small DOM stand-in. Visual/native acceptance
// is separate; this test controls IPC completion order without timing-dependent browser sleeps.
import test from "node:test";
import assert from "node:assert/strict";

class Node {
  constructor(tag = "", text = "") { this.tag = tag; this.text = text; this.children = []; this.attrs = {}; this.dataset = {}; this.listeners = {}; this.style = { setProperty() {} }; }
  append(...nodes) { for (const node of nodes) { node.parent = this; this.children.push(node); } }
  prepend(node) { node.parent = this; this.children.unshift(node); }
  remove() { if (this.parent) this.parent.children = this.parent.children.filter((n) => n !== this); }
  get firstChild() { return this.children[0]; }
  get textContent() { return this.text + this.children.map((n) => n.textContent).join(""); }
  set textContent(text) { this.text = text; this.children = []; }
  setAttribute(key, value) { this.attrs[key] = String(value); }
  getAttribute(key) { return this.attrs[key] ?? null; }
  hasAttribute(key) { return key in this.attrs; }
  addEventListener(key, run) { this.listeners[key] = run; }
  get classList() { return { contains: (cls) => (this.attrs.class || "").split(" ").includes(cls), toggle() {}, add() {}, remove() {} }; }
  contains(node) { return this === node || this.children.some((n) => n.contains(node)); }
  getBoundingClientRect() { return { width: 800 }; }
  querySelector(selector) { return this.querySelectorAll(selector)[0] ?? null; }
  querySelectorAll(selector) {
    const matches = (node) => selector.startsWith(".") ? node.classList.contains(selector.slice(1))
      : selector.startsWith("[") ? node.attrs[selector.match(/^\[([^=]+)=/)[1]] === selector.match(/="([^"]+)"/)[1]
      : node.tag === selector;
    return this.children.flatMap((node) => [...(matches(node) ? [node] : []), ...node.querySelectorAll(selector)]);
  }
}
globalThis.Node = Node;
Object.defineProperty(globalThis, "navigator", { value: { language: "en" }, configurable: true });
globalThis.document = {
  documentElement: { lang: "en", dataset: {}, classList: { toggle() {} } }, activeElement: null,
  createElement: (tag) => new Node(tag), createElementNS: (_, tag) => new Node(tag),
  createTextNode: (text) => new Node("", text), addEventListener() {}, removeEventListener() {},
};
globalThis.ResizeObserver = class { observe() {} disconnect() {} };
const pending = [];
const calls = [];
const timers = new Map();
let timer = 0;
let recordings = [];
globalThis.window = {
  location: { hash: "#/history", search: "" }, matchMedia: () => ({ matches: false, addEventListener() {} }),
  sessionStorage: { getItem: () => null, setItem() {} },
  setTimeout: (run, delay) => { timers.set(++timer, { run, delay }); return timer; }, clearTimeout: (id) => timers.delete(id),
  __TAURI__: { core: { invoke: async (command, args) => {
    calls.push({ command, args });
    if (command === "history_chart") return new Promise((resolve, reject) => pending.push({ args, resolve, reject }));
    if (command === "history_overview") return { periods: [], recordings, more_recordings: false };
    if (command === "recording_detail") return null;
    if (command === "history_status" || command === "history_collection") return null;
    if (command === "startup_get") return false;
    throw Error(`Unexpected IPC ${command}`);
  } }, event: { listen: async () => () => {} } },
};
const { connect } = await import("../ui/js/api.js");
const { mount } = await import("../ui/js/history/view.js");
await connect();
const flush = () => new Promise((resolve) => setImmediate(resolve));
const capture = (request, value) => ({
  from_unix_ms: request.args.fromUnixMs, to_unix_ms: request.args.toUnixMs,
  source_id: "selection-source-test", selection_unavailable: null, events: [], process_names: {}, gaps: [],
  series: [{ metric: "cpu_busy_basis_points", points: [[request.args.toUnixMs, value, value, value, 1, 1, 1000, "ok", "live", 1000, false]] }],
});
function start() {
  pending.length = 0;
  calls.length = 0;
  timers.clear();
  const page = new Node("main");
  let nav;
  const stop = mount(page, { setViewBar: (value) => { nav = value; }, refreshViewBar() {}, closeViewBar() {} });
  const choose = (label) => {
    const button = nav.render().querySelectorAll("button").find((b) => b.textContent.startsWith(label));
    assert.ok(button, label);
    button.listeners.click();
  };
  return { page, stop, choose, charts: page.children[4], events: page.children[5] };
}

test("initial loading is explicit; a period change issues exactly one grouped request and ignores the stale response", async () => {
  const { page, stop, choose, charts, events } = start();
  await flush();
  assert.equal(charts.getAttribute("aria-busy"), "true");
  assert.match(charts.textContent, /Loading/);
  assert.equal(events.inert, true);
  assert.equal(pending.length, 1);
  choose("1 hour");
  await flush();
  assert.equal(pending.length, 2);
  assert.equal(calls.filter((c) => c.command === "metric_history").length, 0);
  pending[1].resolve(capture(pending[1], 9000));
  await flush();
  assert.equal(charts.getAttribute("aria-busy"), "false");
  assert.equal(events.inert, false);
  assert.match(page.textContent, /90%/);
  pending[0].resolve(capture(pending[0], 1000));
  await flush();
  assert.match(page.textContent, /90%/);
  assert.doesNotMatch(page.textContent, /10%/);
  stop();
  assert.equal(timers.size, 0);
});

test("an older completed recording stays selected even when the sampler has no active recording", async () => {
  recordings = [{ id: "recording-old", started_at_unix_ms: 1000, ended_at_unix_ms: 61000, duration_seconds: 60, status: "completed", samples: 60 }];
  const { page, stop, choose } = start();
  await flush();
  choose("Recordings");
  page.children[2].querySelector("button").listeners.click();
  await flush();
  assert.equal(calls.filter((c) => c.command === "recording_detail").length, 1);
  assert.equal(page.children[3].hidden, false);
  assert.match(page.children[3].textContent, /no longer available/);
  assert.ok(page.children[2].querySelector("button").classList.contains("selected"));
  stop();
  recordings = [];
});

test("a late failure cannot clear a newer chart; unmount invalidates in-flight requests", async () => {
  const { page, stop, choose } = start();
  await flush();
  choose("7 days");
  await flush();
  pending[1].resolve(capture(pending[1], 7000));
  await flush();
  const displayed = page.textContent;
  pending[0].reject({ code: "history_unavailable" });
  await flush();
  assert.equal(page.textContent, displayed);
  choose("1 hour");
  await flush();
  stop();
  const stopped = page.textContent;
  pending[2].resolve(capture(pending[2], 1000));
  await flush();
  assert.equal(page.textContent, stopped);
});
