import test from "node:test";
import assert from "node:assert/strict";
import { exerciseMock } from "./mock-contracts.mjs";
import { scenarios, virtualClock } from "./mock-cases.mjs";

for (const [index, search] of scenarios.entries()) {
  test(`mock feature contracts: ${search || "default"}`, async (t) => {
    const clock = virtualClock();
    t.mock.method(Date, "now", clock.now);
    t.mock.method(Math, "random", () => 0.5);
    globalThis.window = { location: { search }, setTimeout: clock.setTimeout };
    const { createMock } = await import(`./mock.js?contracts-${index}`);
    const result = await exerciseMock(createMock(), clock, search);
    if (!search) assert.equal(result.commands.length, 55); // 54 IPC commands and one rejected name.
  });
}
