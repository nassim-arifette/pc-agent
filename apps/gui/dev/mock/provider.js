import { wait, fail } from "./common.js";
import { SUPER_ID, ULTRA_ID } from "./models.js";

export function createProviderFixture(params) {
  const specialistReady = params.get("specialist") === "ready";
  let specialistEnabled = false;
  let cloudMode = params.get("cloud") || "ready";

  let cloudKeyConfigured = cloudMode !== "none";

  async function checkConnection() {
    await wait(800);
    if (!cloudKeyConfigured) throw new Error("NEBIUS_API_KEY is missing or empty");
    const errors = {
      key: "Provider returned HTTP 401; response body was withheld",
      network: "Provider request failed; details and response body were withheld",
      timeout: "Provider did not answer within the time limit",
      model: "The selected model is absent from the account catalog",
      limit: "Provider returned HTTP 429; response body was withheld",
    };
    const error = errors[params.get("connection")];
    if (error) throw new Error(error);
  }

  const webParam = params.get("web");

  let searchKey = webParam === "on" || webParam === "key" ? "tvly-synthetic-mock-key-x9Qa" : null;

  function searchKeyState() {
    return { search_key_configured: Boolean(searchKey), search_key_hint: searchKey && searchKey.length >= 12 ? searchKey.slice(-4) : null, search_key_source: searchKey ? "file" : null };
  }

  function cloud() {
    if (cloudMode === "none") {
      return { key_configured: false, key_hint: null, key_source: null, model_id: cloudModel, model_source: null, ready: false, problem: "NEBIUS_API_KEY is missing or empty", file: "C:\\Users\\Alex\\AppData\\Roaming\\pcagent\\.env", file_problem: null, ...searchKeyState() };
    }
    if (cloudMode === "problem") {
      return { key_configured: true, key_hint: "Q7fz", key_source: "file", model_id: "meta/llama", model_source: "file", ready: false, problem: "Select an explicit nvidia/ Nemotron model ID; no fallback model is allowed", file: "C:\\Users\\Alex\\AppData\\Roaming\\pcagent\\.env", file_problem: null, ...searchKeyState() };
    }
    return { key_configured: true, key_hint: "Q7fz", key_source: "file", model_id: cloudModel, model_source: "file", ready: true, reasoning: cloudReasoning, problem: null, file: "C:\\Users\\Alex\\AppData\\Roaming\\pcagent\\.env", file_problem: null, ...searchKeyState() };
  }

  let cloudModel = cloudMode === "none" ? null : "nvidia/nemotron-3-super-120b-a12b";

  let cloudReasoning = "on";

  const routingParam = params.get("routing");

  const escalation = ["escalated", "escalation_failed"].includes(routingParam) ? routingParam : null;

  const calibrated = routingParam === "calibrated" || Boolean(escalation);

  const calibrationReviewedAt = Date.now() - 86400000;

  let routingSettings = escalation ? { mode: "automatic", super_model_id: SUPER_ID, ultra_model_id: ULTRA_ID } : { mode: "configured", super_model_id: null, ultra_model_id: null };

  let routingLimitation = routingParam === "stale" ? "Automatic routing calibration belongs to a different engine version. Run and review a new campaign." : null;

  function routingView() {
    return {
      settings: { ...routingSettings },
      revision: 0,
      automatic_available: calibrated,
      calibration_reviewed_at_unix_ms: calibrated ? calibrationReviewedAt : null,
      calibrated_rules: calibrated ? [{ reason: "low_confidence", cases: 3, recoveries: 2, primary_request_limit: null }] : [],
      limitation: routingLimitation,
    };
  }

  const commands = {
    // ?specialist=ready: the published identity the real check reports; still no inference here.
    specialist_get: () => specialistReady
      ? { enabled: specialistEnabled, endpoint: "127.0.0.1:8737 (synthetic UI fixture; no inference)", error: null,
          identity: { protocol: 1, model: "nvidia/Llama-3.1-Nemotron-Nano-4B-v1.1", revision: "d552708a9d575fa8d4a690b988fd870d65279f98",
            adapter_sha256: "b85a03348ebeb27a327bcf53c5b3da3bde931a34337650d0eccb9d5887dbd459", device: "cuda", precision: "nf4", synthetic_training_only: true } }
      : { enabled: specialistEnabled, endpoint: "127.0.0.1:8737 (synthetic UI fixture; no inference)", identity: null, error: "unavailable" },
    specialist_set: ({ enabled }) => { specialistEnabled = Boolean(enabled); return commands.specialist_get(); },
    model_routing_get: () => routingView(),
    model_routing_set: ({ settings }) => {
      if (settings.mode === "automatic" && !calibrated) fail("routing_unavailable", "Automatic routing requires a reviewed local Nemotron calibration.");
      routingSettings = { mode: settings.mode, super_model_id: settings.super_model_id ?? null, ultra_model_id: settings.ultra_model_id ?? null };
      // A saved choice replaces the one rejected at start-up.
      routingLimitation = null;
      return routingView();
    },
    save_cloud_settings: ({ apiKey = null, modelId, reasoning = null }) => {
      if (typeof modelId !== "string" || modelId.length > 200
        || !modelId.toLowerCase().startsWith("nvidia/") || !modelId.toLowerCase().includes("nemotron")
        || !/^[a-z0-9/._-]+$/i.test(modelId)) throw new Error("Select an explicit nvidia/ Nemotron model ID; no fallback model is allowed");
      if (apiKey?.trim()) cloudKeyConfigured = true;
      cloudMode = cloudKeyConfigured ? "ready" : "none";
      cloudModel = modelId;
      if (reasoning) {
        if (!["on", "low", "off"].includes(reasoning)) throw new Error("unknown reasoning");
        cloudReasoning = reasoning;
      }
      return cloud();
    },
    test_cloud: async () => {
      await checkConnection();
      if (!cloudModel) throw new Error("NEBIUS_MODEL_ID is missing or empty");
      if (params.get("connection") === "empty" || cloudMode === "problem") throw new Error("The selected model is absent from the account catalog");
      return { price: { model_id: cloudModel, input_per_million: "0.30", output_per_million: "0.90", read_at_unix_ms: Date.now() } };
    },
    cloud_models: async () => {
      await checkConnection();
      return {
        source: "synthetic-mock", read_at_unix_ms: Date.now(),
        models: params.get("connection") === "empty" ? [] : [
          { id: "nvidia/nemotron-3-super-120b-a12b", supports_tools: true },
          { id: "nvidia/mock-nemotron-account-model", supports_tools: null },
          { id: "nvidia/mock-nemotron-no-tools", supports_tools: false },
        ],
      };
    },
    open_nebius_console: () => null,
    save_search_key: ({ apiKey = null }) => {
      const key = (apiKey || "").trim();
      if (key && !key.startsWith("tvly-")) fail("search_key_invalid", "A Tavily key starts with tvly-.");
      searchKey = key || null;
      return cloud();
    }
  };
  return { commands, cloud, escalation, calibrationReviewedAt, hasSearchKey: () => Boolean(searchKey), specialistActive: () => specialistReady && specialistEnabled, routingMode: () => routingSettings.mode };
}
