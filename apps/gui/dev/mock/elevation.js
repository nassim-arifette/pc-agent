import { wait, fail } from "./common.js";
import { createInspectionFixtures } from "./inspections.js";

export function createElevationFixture(params) {
  let elevation = null;
  const { mftResult, CHECK_RESULTS } = createInspectionFixtures();
  const commands = {
    elevation_status: () => (elevation ? { request_id: elevation.id, task: elevation.task, budget_ms: elevation.budget, state: elevation.state } : null),
    elevation_prepare: async ({ task, budgetMs }) => {
      if (elevation) fail("helper_busy", "An elevated inspection is already prepared or running.");
      const mft = task?.kind === "mft_inventory" && /^[A-Z]$/.test(task.volume) && Object.keys(task).length === 2;
      const check = Boolean(CHECK_RESULTS[task?.kind]) && Object.keys(task).length === 1;
      if (!mft && !check) fail("unsupported", "This elevated task is unsupported here (Unsupported).");
      if (!(budgetMs >= 1 && budgetMs <= 120000)) fail("unknown", "The elevated request was refused (InvalidRequest).");
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
      return job.task.kind === "mft_inventory" ? mftResult(job.task.volume) : CHECK_RESULTS[job.task.kind]();
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
