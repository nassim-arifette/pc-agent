import { wait, fail } from "./common.js";
import { createInspectionFixtures } from "./inspections.js";

export function createElevationFixture(params, publishSource) {
  let elevation = null;
  let sequence = 0;
  let records = [];
  try { records = JSON.parse(window.sessionStorage?.getItem("pcagent-mock-elevated") || "[]"); } catch { /* No browser storage in command tests. */ }
  const { mftResult, CHECK_RESULTS } = createInspectionFixtures();
  const options = () => ({
    tasks: [
      ...["system_caches", "shadow_storage", "disk_reliability", ...(params.get("battery") === "none" ? [] : ["battery_energy"])].map(kind => ({ kind })),
      { kind: "mft_inventory", volume: "C" }, { kind: "mft_inventory", volume: "D" },
      { kind: "process_images", targets: [{ pid: 1234, creation_time_filetime: 133500000000000000 }] },
    ],
    unavailable: params.get("battery") === "none" ? ["battery_energy"] : [],
    process_targets_omitted: 0,
  });
  const validSection = (task) => {
    if (!task || typeof task !== "object") return false;
    if (Object.hasOwn(CHECK_RESULTS, task.kind)) return Object.keys(task).length === 1;
    if (task.kind === "mft_inventory") return /^[A-Z]$/.test(task.volume) && Object.keys(task).length === 2;
    return task.kind === "process_images" && Object.keys(task).length === 2 &&
      Array.isArray(task.targets) && task.targets.length > 0 && task.targets.length <= 16 &&
      new Set(task.targets.map(p => p.pid)).size === task.targets.length &&
      task.targets.every(p => Object.keys(p).length === 2 && Number.isInteger(p.pid) && p.pid > 0 && Number.isInteger(p.creation_time_filetime) && p.creation_time_filetime > 0);
  };
  const validTask = task => validSection(task) || (task?.kind === "deep_analysis" && Object.keys(task).length === 2 &&
    Array.isArray(task.tasks) && task.tasks.length > 0 && task.tasks.length <= 31 && task.tasks.every(validSection) &&
    new Set(task.tasks.map(t => `${t.kind}:${t.volume || ""}`)).size === task.tasks.length);
  const purge = () => {
    records = records.filter(r => {
      const age = Date.now() - r.inspection.measured_at_unix_ms;
      return age >= 0 && age < 24 * 60 * 60 * 1000;
    });
    try { window.sessionStorage?.setItem("pcagent-mock-elevated", JSON.stringify(records)); } catch { /* Storage can be disabled. */ }
  };
  const measure = task => {
    if (task.kind === "mft_inventory") return mftResult(task.volume);
    if (task.kind === "process_images") {
      const now = Date.now();
      return { status: "partial", measured_at_unix_ms: now, window: { started_at_unix_ms: now - 100, finished_at_unix_ms: now, duration_ms: 100 },
        origin: "synthetic_fixture", observations: [{ observation_id: "mock-image", metric: "process_image_count", value: task.targets.length, unit: "count", status: "partial", entity_ref: "image-set", measured_at_unix_ms: now }],
        coverage: [{ scope: "process_images", status: "partial", detail: "Synthetic process image metadata; protected processes may remain unavailable." }] };
    }
    return CHECK_RESULTS[task.kind]();
  };
  const commands = {
    elevation_options: () => options(),
    elevation_results: () => { purge(); return structuredClone(records); },
    elevation_select: ({ inspectionId }) => {
      purge();
      const record = records.find(r => r.id === inspectionId);
      if (!record) fail("selection_expired", "The administrator measurement expired or was removed.");
      const target = { kind: "elevated_inspection", inspection_id: record.id };
      return { source_id: publishSource([target], record.inspection.measured_at_unix_ms, { elevated: record.inspection }), target };
    },
    elevation_status: () => (elevation ? { request_id: elevation.id, task: elevation.task, budget_ms: elevation.budget, state: elevation.state } : null),
    elevation_prepare: async ({ task, budgetMs }) => {
      if (elevation) fail("helper_busy", "An elevated inspection is already prepared or running.");
      if (!validTask(task)) fail("unsupported", "This elevated task is unsupported here (Unsupported).");
      const budget = task.kind === "deep_analysis" ? task.tasks.length * 125000 : 120000;
      if (!Number.isInteger(budgetMs) || !(budgetMs >= 1 && budgetMs <= budget)) fail("unknown", "The elevated request was refused (InvalidRequest).");
      await wait(300);
      elevation = { id: `elevation-${Date.now()}`, task, budget: budgetMs, state: "awaiting_local_approval", cancelled: false };
      return { request_id: elevation.id, task, budget_ms: budgetMs, state: elevation.state };
    },
    elevation_run: async ({ requestId }) => {
      if (!elevation || elevation.id !== requestId || elevation.state !== "awaiting_local_approval") fail("request_expired", "This elevated inspection is no longer awaiting approval.");
      const job = elevation;
      job.state = "running";
      const slow = params.get("elevation") === "slow" && job.task.kind !== "mft_inventory";
      for (let waited = 0; waited < (slow ? 20000 : 2500) && !job.cancelled; waited += 100) await wait(100);
      const finish = () => {
        if (elevation === job) elevation = null;
      };
      if (job.cancelled) {
        finish();
        fail("cancelled", "The elevated task was cancelled (Cancelled).");
      }
      if (params.get("elevation") === "fail") {
        finish();
        fail("uac_declined", "The Windows administrator prompt was declined (UacDeclined).");
      }
      finish();
      const sections = job.task.kind === "deep_analysis" ? job.task.tasks.map(task => ({ task, ...measure(task) })) : null;
      const now = Date.now();
      const result = sections ? { status: sections.every(s => s.status === "ok") ? "ok" : "partial",
        measured_at_unix_ms: now, window: { started_at_unix_ms: now - 2500, finished_at_unix_ms: now, duration_ms: 2500 }, origin: "synthetic_fixture",
        observations: sections.flatMap(s => s.observations), coverage: sections.flatMap(s => s.coverage), sections } : measure(job.task);
      const id = `elevated-mock-${now}-${++sequence}`;
      records.push({ id, task: job.task, inspection: result, local_details: {} });
      purge();
      return { ...result, stored_id: id, persistence_error: null };
    },
    elevation_cancel: ({ requestId }) => {
      if (!elevation || elevation.id !== requestId) fail("request_expired", "This elevated inspection is no longer available.");
      if (elevation.state === "awaiting_local_approval") elevation = null;
      else {
        elevation.cancelled = true;
        elevation.state = "cancelling";
      }
    }
  };
  return { commands };
}
