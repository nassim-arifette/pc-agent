// IPC fixtures, not model inference or native acceptance.
import test from "node:test";
import assert from "node:assert/strict";
import { createProviderFixture } from "./mock/provider.js";
import { en } from "../ui/js/i18n/en.js";
import { fr } from "../ui/js/i18n/fr.js";
import { markSpecialistRequest, specialistRequestLabel } from "../ui/js/chat/specialist.js";

test("specialist preference is separate from the cloud provider and defaults off", () => {
  const { commands, cloud } = createProviderFixture(new URLSearchParams());
  const originalCloud = cloud();
  assert.equal(commands.specialist_get().enabled, false);
  assert.equal(commands.specialist_set({ enabled: true }).enabled, true);
  assert.deepEqual(cloud(), originalCloud);
  assert.equal(commands.specialist_set({ enabled: false }).enabled, false);
});

test("the design fixture never claims to load the real adapter", () => {
  const { commands } = createProviderFixture(new URLSearchParams());
  const view = commands.specialist_get({ check: true });
  assert.equal(view.identity, null);
  assert.equal(view.error, "unavailable");
  assert.match(view.endpoint, /synthetic UI fixture/);
  for (const dictionary of [en, fr]) {
    for (const reason of ["ready", "ask", "final", "unavailable", "invalid", "identity", "timeout", "scope"]) {
      assert.ok(dictionary[`specialist.reason.${reason}`]);
    }
  }
});

test("local request progress keeps its attribution when a later request hands off", () => {
  const local = { type: "model", state: "running" };
  const steps = [local, { type: "specialist", state: "running" }];
  markSpecialistRequest(steps, { kind: "started" });
  assert.equal(specialistRequestLabel(local), "specialist.running");
  markSpecialistRequest(steps, { kind: "decided" });
  local.state = "done";
  assert.equal(specialistRequestLabel(local), "specialist.request.done");
  const analyst = { type: "model", state: "running" };
  steps.push(analyst);
  markSpecialistRequest(steps, { kind: "started" });
  assert.equal(analyst.localSpecialist, true);
  markSpecialistRequest(steps, { kind: "handoff" });
  assert.equal(specialistRequestLabel(analyst), null);
  assert.equal(specialistRequestLabel(local), "specialist.request.done");
  assert.equal(specialistRequestLabel({ type: "model", state: "done" }), null);
});

test("failed and stopped local requests never claim a completed cloud answer", () => {
  for (const state of ["running", "done", "failed", "cancelled"]) {
    const label = specialistRequestLabel({ localSpecialist: true, state });
    for (const dictionary of [en, fr]) {
      assert.ok(dictionary[label]);
      assert.match(dictionary[label], /Nano 4B/);
    }
  }
  assert.equal(specialistRequestLabel({ localSpecialist: true, state: "failed" }), "specialist.request.failed");
  assert.equal(specialistRequestLabel({ localSpecialist: true, state: "cancelled" }), "specialist.request.cancelled");
  const steps = [{ type: "model", state: "done" }];
  markSpecialistRequest(steps, { kind: "started" });
  assert.equal(steps[0].localSpecialist, undefined);
  assert.doesNotThrow(() => markSpecialistRequest([], { kind: "handoff" }));
});
