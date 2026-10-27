import { wait, fail, GiB } from "./common.js";

export function createActionFixture(params) {
  const turnPlans = new Map();

  function proposePlans(turnId, question) {
    if (/startup|start-up|boot|démarr/i.test(question)) {
      turnPlans.set(turnId, [
        { plan_id: `plan-${turnId}-startup`, kind: "disable_user_startup", targets: [{ kind: "startup", label: "Spotify", files: null, bytes: null }], recovery: "restore_previous_startup_approval", checks: ["user_startup_entry_unchanged", "known_startup_approval_format"], decision: null },
      ]);
      return;
    }
    if (!/supprim|delete|clean|nettoy|place|space|free/i.test(question)) return;
    turnPlans.set(turnId, [
      { plan_id: `plan-${turnId}-1`, kind: "run_cleanup_adapter", targets: [{ kind: "cleanup", label: "C:\\Users\\Alex\\AppData\\Local\\Temp", files: 1840, bytes: 18.7 * GiB }], recovery: "none_synthetic_regenerable_data", checks: ["identity_revalidated", "not_protected"], decision: null },
      { plan_id: `plan-${turnId}-2`, kind: "recycle_selected_files", targets: [{ kind: "file", label: "C:\\Users\\Alex\\Downloads\\ubuntu-24.04.iso", files: 1, bytes: null }, { kind: "file", label: "C:\\Users\\Alex\\Downloads\\setup-old.exe", files: 1, bytes: null }], recovery: "recycle_bin_if_operation_supported", checks: ["identity_revalidated", "recycle_bin_available"], decision: null },
    ]);
  }

  const commands = {
    browser_manager_prepare: async ({ applicationRef, processRef }) => {
      await wait(300);
      if (!["app-1", "app-7"].includes(applicationRef) || processRef !== `proc-${applicationRef.slice(4)}p0`) fail("action_refused", "Refresh local browser windows first.", "stale");
      const turnId = `local-browser-${Date.now()}`;
      const plan = {
        plan_id: `plan-${turnId}`, kind: "open_browser_process_manager",
        targets: [{ kind: "browser_manager", label: applicationRef === "app-7" ? "Mozilla Firefox" : "Google Chrome", pid: applicationRef === "app-7" ? 9000 : 4000 }],
        recovery: "not_applicable", checks: ["identity_revalidated", "same_process_instance", "browser_manager_allowlisted"], decision: null,
      };
      turnPlans.set(turnId, [plan]);
      return { turn_id: turnId, plan: { ...plan } };
    },
    action_plans: ({ turnId }) => (turnPlans.get(turnId) || []).map((p) => ({ ...p })),
    startup_restore_prepare: async ({ sourcePlanId, itemIndex }) => {
      await wait(300);
      const source = [...turnPlans.values()].flat().find((p) => p.plan_id === sourcePlanId);
      if (!source || source.kind !== "disable_user_startup" || source.decision !== "completed" || itemIndex !== 0) fail("action_refused", "startup disable is not durably completed", "restore_unavailable");
      const turnId = `local-restore-${sourcePlanId}`;
      const plan = { plan_id: `plan-restore-${Date.now()}`, kind: "restore_user_startup", targets: source.targets.map((x) => ({ ...x })), recovery: "restore_previous_startup_approval", checks: ["user_startup_entry_unchanged", "known_startup_approval_format"], decision: null };
      turnPlans.set(turnId, [plan]);
      return { turn_id: turnId, plan: { ...plan } };
    },
    action_decline: ({ turnId, planId }) => {
      const plan = (turnPlans.get(turnId) || []).find((p) => p.plan_id === planId);
      if (!plan) fail("action_refused", "These proposals are no longer current.", "stale");
      plan.decision = plan.decision || "declined";
    },
    action_run: async ({ turnId, planId }) => {
      const plan = (turnPlans.get(turnId) || []).find((p) => p.plan_id === planId);
      if (!plan || plan.decision) fail("action_refused", "This proposal was already decided.", "already_decided");
      await wait(1200);
      // ?action=refuse: the broker's last check finds the target changed since it was proposed.
      if (params.get("action") === "refuse") {
        plan.decision = "refused";
        fail("action_refused", "target t1 changed since it was resolved", "changed");
      }
      plan.decision = "completed";
      const gained = plan.kind === "run_cleanup_adapter" ? 18.6 * GiB : null;
      return { plan_id: planId, overall: "completed", items: [], verification: { volume: gained ? { gained_bytes: gained, status: "ok" } : null, comparisons: [] } };
    }
  };
  return { commands, proposePlans };
}
