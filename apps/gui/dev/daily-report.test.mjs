// The daily report cards (chantier 17) over the synthetic fixture, with a small DOM stand-in.
// Native Windows acceptance (task creation, run, catch-up, removal) is separate.
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
let commands = {};
globalThis.window = {
  location: { hash: "#/chat", search: "" }, matchMedia: () => ({ matches: false, addEventListener() {} }),
  sessionStorage: { getItem: () => null, setItem() {} }, localStorage: { getItem: () => null, setItem() {} },
  setTimeout: () => 0, clearTimeout() {},
  __TAURI__: { core: { invoke: async (command, args = {}) => {
    calls.push({ command, args });
    const run = commands[command];
    if (!run) throw Error(`Unexpected IPC ${command}`);
    return run(args);
  } }, event: { listen: async () => () => {} } },
};
const { createDailyReportFixture } = await import("./mock/daily-report.js");
const { connect } = await import("../ui/js/api.js");
const { dailyReportCard } = await import("../ui/js/report/schedule.js");
const { homeReportCard } = await import("../ui/js/report/home.js");
await connect();

const flush = () => new Promise((resolve) => setImmediate(resolve));
const buttons = (node) => node.querySelectorAll("button");
const byText = (node, text) => buttons(node).find((b) => b.textContent.includes(text));
function use(search) {
  calls.length = 0;
  commands = createDailyReportFixture(new URLSearchParams(search)).commands;
}

test("settings show the exact command, turn the task on at the chosen time and off again", async () => {
  use("");
  const card = dailyReportCard();
  await flush();
  assert.match(card.el.textContent, /Off\./);
  assert.match(card.el.textContent, /pcagent-gui\.exe" --scheduled-report/);
  const time = card.el.querySelectorAll("input")[0];
  assert.equal(time.value, "09:00");
  time.value = "07:30";
  card.el.querySelectorAll(".switch")[0].listeners.click();
  await flush();
  assert.deepEqual(calls.at(-1), { command: "report_schedule_set", args: { enabled: true, time: "07:30" } });
  assert.match(card.el.textContent, /On, every day at 07:30\./);
  const move = () => byText(card.el, "Move to this time");
  const disabled = (b) => b.disabled ?? b.getAttribute("disabled") !== null;
  assert.equal(disabled(move()), true);
  time.value = "21:15";
  time.listeners.input();
  assert.equal(disabled(move()), false);
  move().listeners.click();
  await flush();
  assert.match(card.el.textContent, /On, every day at 21:15\./);
  card.el.querySelectorAll(".switch")[0].listeners.click();
  await flush();
  assert.equal(calls.at(-1).args.enabled, false);
  assert.match(card.el.textContent, /Off\./);
  card.dispose();
});

test("a report can be written on request, and Windows-only scheduling says so elsewhere", async () => {
  use("?report=unsupported");
  const card = dailyReportCard();
  await flush();
  assert.match(card.el.textContent, /needs Windows/);
  assert.equal(card.el.querySelectorAll(".switch").length, 0);
  byText(card.el, "Write a report now").listeners.click();
  await flush();
  assert.ok(calls.some((c) => c.command === "report_run_now"));
  assert.match(card.el.textContent, /2 point\(s\) to review/);
  assert.ok(!calls.some((c) => c.command === "report_schedule_set"));
});

test("home shows the answer first and only drafts a question; nothing is sent", async () => {
  use("");
  let drafted = null;
  const card = homeReportCard({ ask: (text) => { drafted = text; } });
  await flush();
  assert.equal(card.el.hidden, false);
  assert.match(card.el.textContent, /3 point\(s\) to review/);
  assert.match(card.el.textContent, /Low available memory/);
  assert.match(card.el.textContent, /1 watch alert\(s\)/);
  byText(card.el, "Ask PC Agent about it").listeners.click();
  assert.match(drafted, /daily report found points/);
  assert.deepEqual(calls.map((c) => c.command).sort(), ["report_latest", "report_schedule_get"]);
});

test("home stays out of the way when there is nothing to show", async () => {
  use("?report=quiet");
  const quiet = homeReportCard({ ask: () => assert.fail("nothing to ask") });
  await flush();
  assert.match(quiet.el.textContent, /All clear/);
  assert.equal(byText(quiet.el, "Ask PC Agent"), undefined);
  use("?report=none&");
  commands.report_schedule_set({ enabled: true, time: "09:00" });
  const hidden = homeReportCard();
  await flush();
  assert.equal(hidden.el.getAttribute("hidden"), "");
  assert.notEqual(hidden.el.hidden, false);
  use("?report=none");
  const invite = homeReportCard();
  await flush();
  assert.match(invite.el.textContent, /Set up the daily report/);
});
