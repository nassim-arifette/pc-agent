// IPC fixtures, not model inference or native acceptance.
import test from "node:test";
import assert from "node:assert/strict";
import { en } from "../ui/js/i18n/en.js";
import { fr } from "../ui/js/i18n/fr.js";
import { markSpecialistRequest, specialistRequestLabel } from "../ui/js/chat/specialist.js";

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
