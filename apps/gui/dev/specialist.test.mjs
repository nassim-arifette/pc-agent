// IPC fixtures, not model inference or native acceptance.
import test from "node:test";
import assert from "node:assert/strict";
import { createProviderFixture } from "./mock/provider.js";
import { en } from "../ui/js/i18n/en.js";
import { fr } from "../ui/js/i18n/fr.js";

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
