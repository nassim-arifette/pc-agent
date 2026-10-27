import test from "node:test";
import assert from "node:assert/strict";
import { KINDS, DURATIONS, MIB, defaultRule, formRule, validRule } from "../ui/js/watches/model.js";
import { watchesEn, watchesFr } from "../ui/js/i18n/watches.js";

test("proposals use the closed snake_case rule DTO directly, never a wrapper or selection", () => {
  const cpu = defaultRule();
  assert.equal(validRule(cpu), true);
  for (const rule of [null, [], {}, { rule: cpu }, { ...cpu, id: "source" }, { ...cpu, threshold: 80 }, { ...cpu, kind: "recording" }]) {
    assert.equal(validRule(rule), false);
  }
  assert.equal(validRule({ ...cpu, sustained_seconds: undefined }), false);
});

test("all five kinds have translated fields and preserve backend numeric units", () => {
  for (const kind of KINDS) {
    assert.ok(watchesEn[`watch.kind.${kind}`]);
    assert.ok(watchesFr[`watch.kind.${kind}`]);
    const rule = defaultRule(kind);
    if (kind.startsWith("app_")) rule.executable = "blender.exe";
    assert.equal(validRule(rule), true, kind);
    const input = {
      target: String(rule.root ?? rule.executable ?? rule.nvml_index ?? rule.resource),
      threshold: String(rule.threshold_bytes != null ? rule.threshold_bytes / MIB : rule.threshold_celsius ?? rule.threshold_basis_points / 100),
      duration: String(rule.sustained_seconds ?? 0),
    };
    assert.deepEqual(formRule(kind, input), rule);
  }
});

test("durations include immediate evaluation and reject durations outside C7", () => {
  for (const sustained_seconds of DURATIONS) assert.equal(validRule({ ...defaultRule(), sustained_seconds }), true);
  for (const sustained_seconds of [-1, 1, 59, 900.1, "60", Infinity]) assert.equal(validRule({ ...defaultRule(), sustained_seconds }), false);
});

test("volume roots never accept paths, UNC, device namespaces or unrepresentable byte counts", () => {
  const rule = defaultRule("volume_free_below");
  assert.equal(validRule({ ...rule, root: "d:\\" }), true);
  for (const root of ["C:", "C:\\Users", "\\\\server\\share\\", "\\\\?\\C:\\", "C:\\\n"]) assert.equal(validRule({ ...rule, root }), false);
  for (const threshold_bytes of [1024 ** 3 - 1, 2 ** 53, NaN, -1, "1073741824"]) assert.equal(validRule({ ...rule, threshold_bytes }), false);
  assert.equal(formRule(rule.kind, { target: "d:\\", threshold: "1024", duration: "0" }).root, "D:\\");
});

test("application names are basenames and each threshold range matches C7", () => {
  const cpu = { ...defaultRule("app_cpu_above"), executable: "Blender.EXE" };
  assert.equal(validRule(cpu), true);
  for (const executable of ["", "  ", "C:\\app.exe", "../app.exe", "app\0.exe", "a".repeat(261)]) {
    assert.equal(validRule({ ...cpu, executable }), false);
  }
  assert.equal(validRule({ ...cpu, threshold_basis_points: 100 }), true);
  assert.equal(validRule({ ...cpu, threshold_basis_points: 99 }), false);
  const memory = { ...defaultRule("app_memory_above"), executable: "x.exe" };
  for (const threshold_bytes of [64 * MIB, 2 ** 40]) assert.equal(validRule({ ...memory, threshold_bytes }), true);
  for (const threshold_bytes of [64 * MIB - 1, 2 ** 40 + 1]) assert.equal(validRule({ ...memory, threshold_bytes }), false);
});

test("system and GPU thresholds reject out-of-range and fractional native values", () => {
  for (const threshold_basis_points of [999, 10001, 1000.1]) assert.equal(validRule({ ...defaultRule(), threshold_basis_points }), false);
  const gpu = defaultRule("gpu_temperature_above");
  for (const threshold_celsius of [50, 105]) assert.equal(validRule({ ...gpu, threshold_celsius }), true);
  for (const threshold_celsius of [49, 106, 80.5]) assert.equal(validRule({ ...gpu, threshold_celsius }), false);
  for (const nvml_index of [-1, 0.5, 2 ** 32]) assert.equal(validRule({ ...gpu, nvml_index }), false);
  assert.equal(validRule(formRule(gpu.kind, { target: "", threshold: "80", duration: "60" })), false);
  assert.equal(validRule(formRule("resource_above", { target: "cpu", threshold: "", duration: "60" })), false);
});

test("watch translations have matching keys and placeholders", () => {
  assert.deepEqual(Object.keys(watchesEn).sort(), Object.keys(watchesFr).sort());
  for (const key of Object.keys(watchesEn)) {
    const placeholders = (value) => value.match(/\{[^}]+\}/g)?.sort() ?? [];
    assert.deepEqual(placeholders(watchesEn[key]), placeholders(watchesFr[key]), key);
  }
});
