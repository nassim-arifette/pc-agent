import { wait, fail } from "./common.js";
import { sanitize } from "./sharing.js";

export function createSelectionFixture({ params, cloud, getPrivacy, nextPreviewSequence, preparePreview }) {
  const SELECTION_TTL = params.get("sources") === "short" ? 5000 : 5 * 60 * 1000;

  const selectionSources = new Map();

  const selectionTickets = new Map();

  let selectionSequence = 0;

  function publishSource(targets, measuredAt, extra = {}) {
    const id = `selection-source-${++selectionSequence}`;
    selectionSources.set(id, { targets, measuredAt, expires: Date.now() + SELECTION_TTL, ...extra });
    return id;
  }

  function sameTarget(a, b) {
    return a.kind === b.kind && a.resource === b.resource && a.entity_ref === b.entity_ref && a.event_id === b.event_id && a.recording_id === b.recording_id && a.scope_id === b.scope_id && a.item_ref === b.item_ref && a.inspection_id === b.inspection_id && a.alert_id === b.alert_id;
  }

  function invalidateStorage() {
    for (const [id, source] of selectionSources) {
      if (source.targets.some((target) => target.kind === "storage")) selectionSources.delete(id);
    }
  }

  function createCommands(startInvestigation) {
    const commands = {
      selection_sources: () => {
        const now = Date.now();
        return [...selectionSources.entries()]
          .filter(([, source]) => source.expires > now)
          .reverse()
          .flatMap(([id, source]) => source.targets.map((target) => ({ reference: { source_id: id, target }, measured_at_unix_ms: source.measuredAt, historical: false })));
      },
      selection_preview: async ({ reference, intent, question = null, conversationId = null, names, fileNames = false }) => {
        await wait(250);
        if (!["explain", "investigate"].includes(intent)) throw new Error("Invalid local selection reference.");
        const source = selectionSources.get(reference?.source_id);
        if (!source || source.expires <= Date.now()) fail("selection_expired", "The selected capture expired or was invalidated. Select the measured item again.");
        if (!source.targets.some((target) => sameTarget(target, reference.target))) throw new Error("The target does not belong to this source type.");
        const verb = intent === "explain" ? "Explain" : "Investigate";
        const noun = reference.target.kind === "resource" ? `the selected ${reference.target.resource} measurements` : reference.target.kind === "storage" ? "the selected scan item" : `the selected ${reference.target.kind}`;
        // A scan item leaves under its alias: never its name or path without the file-name opt-in.
        const stored = reference.target.kind === "storage" ? source.storage?.[reference.target.item_ref] : null;
        const asked = question || `${verb} ${noun}, stating what is measured and what remains uncertain.`;
        const age = Date.now() - source.measuredAt;
        const evidence = {
          status: "ok",
          // Like the host's projection: a scan item is named by its alias, never by its scope or scan.
          selection: { target: stored ? { kind: "storage", alias: stored.alias } : source.elevated ? { kind: "elevated_inspection" } : reference.target, intent, fresh_collection: false },
          measured_at_unix_ms: source.measuredAt,
          age_ms: age,
          freshness: "already_measured",
          observations: source.elevated ? source.elevated.observations : stored
            ? [{ observation_id: "O0", entity_ref: stored.alias, metric: "allocated_bytes", value: stored.allocated_bytes, unit: "bytes", status: "ok" }]
            : reference.target.kind === "resource" && reference.target.resource === "battery"
              ? (source.battery || []).flatMap((b) =>
                  [["battery_charge_percent", b.charge_percent, "percent"], ["battery_power_rate_mw", b.power_rate_mw, "milliwatts"], ["battery_charge_state", b.charge_state, "category"]].map(([metric, value, unit]) => ({
                    observation_id: `${b.battery_ref}-measurement-${b.measured_at_unix_ms}-${metric}`,
                    measured_at_unix_ms: b.measured_at_unix_ms,
                    entity_ref: b.battery_ref,
                    metric,
                    value,
                    unit,
                    status: value == null ? "unsupported" : "ok",
                  })),
                )
            : reference.target.kind === "resource" && reference.target.resource === "memory"
              ? [{ observation_id: "O0", metric: "physical_available_bytes", value: 3326083072, unit: "bytes", status: "ok" }, { observation_id: "O0", metric: "physical_total_bytes", value: 16888365056, unit: "bytes", status: "ok" }]
              : [{ observation_id: "O0", metric: "cpu_busy_basis_points", value: 1850, unit: "basis_points", status: "ok" }],
          storage_items: stored ? [{ item_ref: stored.alias, alias_path: `<scope>/${stored.alias}`, kind: stored.kind, allocated_bytes: stored.allocated_bytes, files: stored.files, last_used: "unknown" }] : undefined,
          limitations: ["Measurements were taken before this turn; they cannot verify the current state."],
        };
        if (names && reference.target.kind === "process") evidence.process_names = { [reference.target.entity_ref]: "chrome.exe" };
        const message = { question: sanitize(asked), initial_observation: { selected_evidence: evidence } };
        const id = `selection-preview-${nextPreviewSequence()}`;
        selectionTickets.set(id, { reference, intent, question, conversationId, names, fileNames, asked });
        return { preview_id: id, message, selected_evidence: evidence, intent, measured_at_unix_ms: source.measuredAt, age_ms: age, context_revision: 0, file_name_preview: { names: [], scope: null }, model_routing: { mode: "configured" }, configured_model_id: cloud().model_id };
      },
      selection_send: async ({ reference, intent, question = null, conversationId = null, previewId }) => {
        const cloudRun = getPrivacy().mode !== "local" && cloud().ready;
        const ticket = selectionTickets.get(previewId);
        if (!ticket) fail("preview_required", "Open the selected-evidence sharing preview before sending.");
        if (JSON.stringify(ticket.reference) !== JSON.stringify(reference) || ticket.intent !== intent || ticket.question !== question || ticket.conversationId !== conversationId) {
          fail("preview_changed", "The selection, question, conversation, sharing or model settings changed. Open the preview again.");
        }
        selectionTickets.delete(previewId);
        preparePreview({ id: previewId, question: ticket.asked, names: getPrivacy().mode === "complete", conversationId });
        return startInvestigation({ request: { question: ticket.asked, cloud: cloudRun, names: getPrivacy().mode === "complete", conversationId, previewId } });
      }
    };
    return commands;
  }

  return { publishSource, invalidateStorage, createCommands };
}
