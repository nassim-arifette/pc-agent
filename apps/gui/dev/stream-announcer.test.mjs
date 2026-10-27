import test from "node:test";
import assert from "node:assert/strict";
import { streamAnnouncer } from "../ui/js/stream-announcer.js";

function harness() {
  const spoken = [];
  const timers = new Map();
  let next = 0;
  const speech = streamAnnouncer((text) => spoken.push(text), (fn) => {
    timers.set(++next, fn);
    return next;
  }, (id) => timers.delete(id));
  const tick = () => {
    const pending = [...timers.values()];
    timers.clear();
    pending.forEach((fn) => fn());
  };
  return { speech, spoken, tick, timers };
}

test("stream announcements batch all sentences received during the debounce", () => {
  const { speech, spoken, tick } = harness();
  speech.update("a", "First.");
  speech.update("a", "First. Second.");
  tick();
  assert.deepEqual(spoken, ["First. Second."]);
  speech.update("a", "First. Second. Third.");
  tick();
  assert.deepEqual(spoken, ["First. Second.", "Third."]);
});

test("completion flushes trailing text once, even without punctuation", () => {
  const { speech, spoken, tick } = harness();
  speech.update("a", "First. Not finished");
  tick();
  speech.finish();
  speech.finish();
  assert.deepEqual(spoken, ["First.", "Not finished"]);
});

test("new turns and revised drafts cancel old pending announcements", () => {
  const { speech, spoken, tick } = harness();
  speech.update("a", "Obsolete.");
  speech.update("a", "Replacement.");
  tick();
  speech.update("b", "Replacement.");
  tick();
  assert.deepEqual(spoken, ["Replacement.", "Replacement."]);
});

test("disposing the view cancels scheduled speech", () => {
  const { speech, spoken, tick, timers } = harness();
  speech.update("a", "Do not speak after navigation.");
  speech.dispose();
  tick();
  assert.equal(timers.size, 0);
  assert.deepEqual(spoken, []);
});
