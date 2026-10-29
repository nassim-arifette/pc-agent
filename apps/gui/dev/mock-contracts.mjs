// Synthetic IPC contracts only: no browser, Windows calls, network or real credentials.
import assert from "node:assert/strict";

export async function exerciseMock(bridge, clock, search) {
  const params = new URLSearchParams(search);
  const transcript = [];
  const seen = new Set();
  const call = async (command, args = {}) => {
    seen.add(command);
    const result = await clock.settle(bridge.invoke(command, args));
    transcript.push({ command, args, ...result });
    if (result.error?.name === "ReferenceError") assert.fail(result.error.message);
    return result;
  };
  const ok = async (command, args) => {
    const result = await call(command, args);
    assert.equal(result.error, undefined, `${command}: ${JSON.stringify(result.error)}`);
    return result.value;
  };
  const reject = async (command, args, code) => {
    const result = await call(command, args);
    assert.ok(result.error, `${command} should reject`);
    if (code) assert.equal(result.error.code, code, command);
  };
  const events = [];
  const names = ["scan:progress", "scan:done", "scan:failed", "turn:step", "turn:done", "turn:failed", "turn:delta", "turn:extension", "inv:step", "inv:done", "inv:extension"];
  const off = await Promise.all(names.map(name => bridge.listen(name, payload => {
    events.push({ name, payload });
    if (name === "turn:extension") bridge.invoke("answer_extension", { accept: false });
  })));
  assert.equal(bridge.mock, true);
  assert.equal(typeof bridge.onDrop(() => {}), "function");
  await reject("nonexistent");
  const before = await ok("app_info");
  assert.equal(before.capabilities.nvidia_gpu, null);
  const live = await call("dashboard");
  if (params.get("live") === "fail") assert.equal(live.error.code, "live_unavailable");
  else {
    assert.equal((await ok("app_info")).capabilities.nvidia_gpu, params.get("gpu") !== "none" && params.get("pc") !== "desktop");
    assert.equal(live.value.cpu.busy.status, "partial");
    assert.equal((await ok("dashboard")).cpu.busy.status, "ok");
  }
  await ok("process_activity");
  const localWindows = await ok("local_browser_windows", { applicationRef: "app-7", includeTabs: false });
  assert.equal(localWindows.windows[0].tabs.length, 0);
  const localTabs = await ok("local_browser_windows", { applicationRef: "app-7", includeTabs: true });
  assert.ok(localTabs.windows[0].tabs[0].title.includes("LOCAL-ONLY"));
  const browserPlan = await ok("browser_manager_prepare", { applicationRef: "app-7", processRef: "proc-7p0" });
  assert.equal(browserPlan.plan.kind, "open_browser_process_manager");
  assert.equal(browserPlan.plan.decision, null);
  await ok("action_decline", { turnId: browserPlan.turn_id, planId: browserPlan.plan.plan_id });
  const sources = await ok("selection_sources");
  assert.ok(sources.length);
  const selected = sources[0].reference;
  const preview = await ok("selection_preview", { reference: selected, intent: "explain", names: true });
  assert.ok(!JSON.stringify(preview).includes("LOCAL-ONLY"));
  assert.equal(preview.selected_evidence.selection.fresh_collection, false);
  const sendSelection = { reference: selected, intent: "explain", previewId: preview.preview_id };
  if (params.get("preview") === "expired") await reject("selection_send", sendSelection, "preview_expired");
  else await ok("selection_send", sendSelection);
  await clock.drain();
  await reject("selection_send", { reference: selected, intent: "explain", previewId: preview.preview_id }, "preview_required");

  await ok("history_status");
  const rec = await call("record_activity_start", { durationSeconds: 30 });
  if (params.get("history") === "unavailable") assert.equal(rec.error.code, "history_unavailable");
  else {
    await reject("record_activity_start", { durationSeconds: 30 });
    await ok("battery_recording", { recordingId: rec.value.id });
    await ok("record_activity_cancel", { recordingId: rec.value.id });
  }
  await call("metric_history", { metric: "cpu_busy_basis_points", fromUnixMs: clock.now() - 86400000, toUnixMs: clock.now() });
  await ok("history_clear");
  await ok("startup_get");
  assert.equal(await ok("startup_set", { enabled: true }), true);
  await ok("privacy_get");
  await reject("privacy_set", { settings: { extra: true } });
  await ok("privacy_set", { settings: { mode: "complete", confirmed: true, web_search: true } });

  await ok("quick_folders");
  const root = await ok("pick_folder");
  const scan = await call("start_scan", { path: root });
  if (params.get("scan") === "denied") assert.equal(scan.error.code, "folder_unreadable");
  else {
    await clock.drain();
    if (params.get("scan") === "fail") {
      assert.ok(events.some(e => e.name === "scan:failed"));
      await reject("scan_page", { scopeId: scan.value.scope_id, kind: "files", offset: 0 }, "scan_unavailable");
    } else {
      const page = await ok("scan_page", { scopeId: scan.value.scope_id, kind: "files", offset: 0 });
      const reference = { source_id: page.selection_source_id, target: { kind: "storage", scope_id: scan.value.scope_id, item_ref: page.items[0].item_ref } };
      const shown = await ok("selection_preview", { reference, intent: "explain", names: false });
      assert.equal(shown.selected_evidence.selection.target.alias, "file-1");
      assert.ok(!JSON.stringify(shown.message).includes(root));
      const children = await ok("scan_children", { scopeId: scan.value.scope_id, itemRef: null, kind: "dirs", offset: 0 });
      assert.equal(children.items.length, 50);
      assert.equal(children.next_offset, 50);
      assert.equal(children.selection_source_id, null);
      const remaining = await ok("scan_children", { scopeId: scan.value.scope_id, kind: "dirs", offset: 50 });
      assert.equal(remaining.items.length, 14);
      assert.equal(remaining.next_offset, null);
      const child = children.items[0];
      const nested = await ok("scan_children", { scopeId: scan.value.scope_id, itemRef: child.item_ref, kind: "files" });
      assert.equal(nested.parent.item_ref, child.item_ref);
      assert.ok(nested.items.every(item => item.parent_ref === child.item_ref));
      const facts = await ok("scan_item_facts", { scopeId: scan.value.scope_id, itemRef: child.item_ref });
      assert.ok(facts.selection_source_id);
      assert.equal(facts.rank_scope, "siblings_of_parent");
      assert.equal(facts.parent_ref, children.parent.item_ref);
      assert.equal(facts.item_facts.scanned_at_unix_ms - facts.item_facts.untouched_before_unix_ms, 182 * 86400000);
      assert.ok(facts.item_facts.file_types.every(type => type.files_count >= 0));
      await reject("scan_item_facts", { scopeId: scan.value.scope_id, itemRef: children.parent.item_ref }, "item_unavailable");
      await ok("start_scan", { path: root });
      await ok("cancel_scan");
      await clock.drain();
      await reject("selection_preview", { reference, intent: "explain", names: false }, "selection_expired");
      await reject("scan_item_facts", { scopeId: scan.value.scope_id, itemRef: child.item_ref }, "scan_unavailable");
    }
  }
  await ok("reveal");
  await ok("model_routing_get");
  await call("model_routing_set", { settings: { mode: "automatic" } });
  await ok("model_routing_set", { settings: { mode: "configured" } });
  await call("test_cloud");
  await call("cloud_models");
  await ok("save_cloud_settings", { apiKey: "synthetic-fixture-key", modelId: "nvidia/nemotron-3-super-120b-a12b", reasoning: "low" });
  await ok("save_search_key", { apiKey: "tvly-synthetic-fixture-key" });
  await ok("open_nebius_console");

  await ok("elevation_status");
  const availableChecks = await ok("elevation_options");
  await ok("elevation_results");
  await reject("elevation_select", { inspectionId: "expired-or-invented" }, "selection_expired");
  for (const tasks of [[], [{ kind: "performance_trace" }], [{ kind: "system_caches" }, { kind: "system_caches" }], [{ kind: "deep_analysis", tasks: [{ kind: "system_caches" }] }]]) {
    await reject("elevation_prepare", { task: { kind: "deep_analysis", tasks }, budgetMs: 1 }, "unsupported");
  }
  await reject("elevation_prepare", { task: { kind: "other" }, budgetMs: 1 }, "unsupported");
  const pending = await ok("elevation_prepare", { task: { kind: "system_caches" }, budgetMs: 10000 });
  await reject("elevation_prepare", { task: { kind: "system_caches" }, budgetMs: 10000 }, "helper_busy");
  await ok("elevation_cancel", { requestId: pending.request_id });
  for (const task of [{ kind: "deep_analysis", tasks: availableChecks.tasks }, { kind: "mft_inventory", volume: "C" }, ...["system_caches", "shadow_storage", "disk_reliability", "battery_energy"].map(kind => ({ kind }))]) {
    const request = await ok("elevation_prepare", { task, budgetMs: 120000 });
    const result = await call("elevation_run", { requestId: request.request_id });
    if (params.get("elevation") === "fail") assert.equal(result.error.code, "uac_declined");
    else {
      assert.ok(result.value.observations.length);
      const saved = await ok("elevation_results");
      assert.ok(saved.some(record => record.id === result.value.stored_id));
      const reference = await ok("elevation_select", { inspectionId: result.value.stored_id });
      assert.equal(reference.target.kind, "elevated_inspection");
      if (task.kind === "deep_analysis") assert.equal(result.value.sections.length, task.tasks.length);
    }
    assert.equal(await ok("elevation_status"), null);
  }

  await reject("image_prepare", { dataBase64: "invalid" }, "image_unsupported");
  await ok("image_discard", { imageId: "unknown" });
  assert.equal(await ok("conversation_image_get", { conversationId: "unknown", turnId: "unknown" }), null);
  await ok("current_turn");
  await call("stop_turn", { turnId: "turn-orphan" });
  await clock.drain();
  await reject("send_message", { text: "No preview", scopeIds: ["one", "two"] });
  await reject("start_investigation", { request: { question: "No preview", cloud: true, names: true } }, "preview_changed");
  for (const question of ["Why is my PC hot?", "startup", "clean space", "fail", "extend"]) {
    const shown = await ok("cloud_preview", { question, names: true });
    const started = await call("send_message", { text: question, previewId: shown.id });
    if (params.get("preview") === "expired") {
      assert.equal(started.error.code, "preview_expired");
      continue;
    }
    assert.equal(started.error, undefined);
    await clock.drain();
    const { turn_id: turnId, conversation_id: conversationId } = started.value;
    const plans = await ok("action_plans", { turnId });
    if (plans.length) {
      const run = await call("action_run", { turnId, planId: plans[0].plan_id });
      if (params.get("action") === "refuse") assert.equal(run.error.reason, "changed");
      else assert.equal(run.value.overall, "completed");
      await reject("action_run", { turnId, planId: plans[0].plan_id }, "action_refused");
      if (plans[0].kind === "disable_user_startup") {
        const restore = await call("startup_restore_prepare", { sourcePlanId: plans[0].plan_id, itemIndex: 0 });
        if (!run.error) assert.equal(restore.value.plan.kind, "restore_user_startup");
      }
      if (plans[1]) await ok("action_decline", { turnId, planId: plans[1].plan_id });
    }
    await ok("conversation_get", { id: conversationId });
    await call("open_web_source", { conversationId, turnId, reference: "W1" });
  }
  await ok("answer_extension", { accept: false });
  await ok("stop_investigation");
  const saved = await ok("conversations_list");
  if (params.get("preview") === "expired") assert.equal(saved.length, 0);
  else {
    assert.ok(saved.length);
    assert.equal(await ok("conversation_delete", { id: saved[0].id }), true);
  }
  await ok("conversations_clear");
  assert.equal((await ok("conversations_list")).length, 0);
  off.forEach(unlisten => unlisten());
  transcript.push({ events });
  return { transcript: JSON.parse(JSON.stringify(transcript)), commands: [...seen].sort() };
}
