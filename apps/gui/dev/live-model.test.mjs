import test from "node:test";
import assert from "node:assert/strict";
import { almostFull, driveName, gpuName, health } from "../ui/js/live/model.js";
import { t } from "../ui/js/i18n.js";

const GiB = 1024 ** 3;
const dashboard = (patch = {}) => ({
  collected_at_unix_ms: 10000,
  volumes: [],
  batteries: [],
  memory: { used_ratio: null },
  cpu: { busy: { value: null } },
  gpus: [],
  ...patch,
});

test("drive labels and capacity thresholds keep their existing meaning", () => {
  assert.equal(driveName("C:\\"), "C:");
  assert.equal(driveName("D:///"), "D:");
  assert.equal(driveName(null), "");
  assert.equal(gpuName("NVIDIA Example GPU"), "Example");
  assert.equal(almostFull({ free_ratio: 0.05, free_bytes: 5 * GiB }), false);
  assert.equal(almostFull({ free_ratio: 0.049, free_bytes: 10 * GiB }), true);
  assert.equal(almostFull({ free_ratio: 0.5, free_bytes: 4 * GiB }), true);
});

test("the health hint handles initial and unknown measurements without a DOM", () => {
  assert.deepEqual(health(null), { tone: "neutral", text: t("health.measuring") });
  assert.deepEqual(health(dashboard()), { tone: "good", text: t("health.ok") });
});

test("drive hints rank by free ratio without mutating measured volumes", () => {
  const volumes = [
    { root: "D:\\", free_ratio: 0.04, free_bytes: 10 * GiB },
    { root: "C:\\", free_ratio: 0.01, free_bytes: 10 * GiB },
  ];
  const before = structuredClone(volumes);
  assert.deepEqual(health(dashboard({ volumes })), { tone: "warn", text: t("health.diskFull", { drive: "C:" }), drive: "C:" });
  assert.deepEqual(volumes, before);
});

test("memory precedes processor and GPU warnings", () => {
  const data = dashboard({ memory: { used_ratio: 0.9 }, cpu: { busy: { value: 9000 } }, gpus: [{ slowdown: { value: 1 } }] });
  assert.equal(health(data).text, t("health.memory"));
  data.memory.used_ratio = 0.89;
  assert.equal(health(data).text, t("health.cpu"));
  data.cpu.busy.value = 8999;
  assert.equal(health(data).text, t("health.gpu"));
});

test("critical battery takes precedence but preserves the drive suggestion", () => {
  const data = dashboard({
    volumes: [{ root: "C:\\", free_ratio: 0.01, free_bytes: GiB }],
    batteries: [{ critical: true, on_ac: false, measured_at_unix_ms: 10000 }],
  });
  assert.deepEqual(health(data), { tone: "warn", text: t("health.battery"), drive: "C:" });
  data.batteries[0].on_ac = true;
  assert.equal(health(data).text, t("health.diskFull", { drive: "C:" }));
  data.batteries[0].on_ac = false;
  data.collected_at_unix_ms = 40001;
  assert.equal(health(data).text, t("health.diskFull", { drive: "C:" }));
});
