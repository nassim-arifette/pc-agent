import test from "node:test";
import assert from "node:assert/strict";
import { actionBar } from "../ui/js/actionbar.js";

// Minimal DOM for the component's callback and accessibility contract; layout is tested in the UI.
class Element {
  constructor(tag) { this.tag = tag; this.attributes = {}; this.children = []; this.events = {}; }
  setAttribute(name, value) { this.attributes[name] = value; }
  hasAttribute(name) { return name in this.attributes; }
  addEventListener(name, handler) { this.events[name] = handler; }
  append(node) { this.children.push(node); }
}
globalThis.Node = Element;
globalThis.document = {
  createElement: (tag) => new Element(tag),
  createElementNS: (_, tag) => new Element(tag),
  createTextNode: (text) => Object.assign(new Element("#text"), { text }),
};
const monitorButton = (bar) => bar.children.find((node) => node.attributes["data-key"] === "actbar:monitor");

test("monitor invokes the view callback without requiring a cloud reference", async () => {
  let calls = 0;
  const bar = actionBar({ key: "volume", label: "C:", ready: false, monitor: async () => { calls++; } });
  const button = monitorButton(bar);
  assert.equal(button.hasAttribute("aria-disabled"), false);
  assert.equal(button.tabIndex, 0);
  button.events.click();
  await Promise.resolve();
  assert.equal(calls, 1);
  assert.equal(button.hasAttribute("aria-describedby"), false);
});

test("unavailable monitor has a visible, associated reason and no callback", () => {
  const props = { key: "folder", label: "Folder", ready: true, monitorUnavailable: "The volume is not fixed." };
  const first = actionBar(props);
  const second = actionBar(props);
  const button = monitorButton(first);
  assert.equal(button.attributes["aria-disabled"], "true");
  const id = button.attributes["aria-describedby"];
  const reason = first.children.find((node) => node.attributes.id === id);
  assert.equal(reason.children[0].text, props.monitorUnavailable);
  assert.notEqual(monitorButton(second).attributes["aria-describedby"], id);
  assert.doesNotThrow(() => button.events.click());
});
