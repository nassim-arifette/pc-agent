// Only a progress marker is persisted, never credentials or permission to send a diagnostic.
export function shouldOfferOnboarding(cloud, progress, { initialHash = "", hash = initialHash, hasTurn = false } = {}) {
  return Boolean(cloud) && !cloud.ready && !["skipped", "complete", "local", "existing"].includes(progress) && !initialHash && hash === initialHash && !hasTurn;
}

export async function leaveOnboarding(choice, { setPrivacy, remember, navigate }) {
  if (!["skipped", "complete", "local"].includes(choice)) throw new Error("Unknown setup choice");
  if (choice === "local") await setPrivacy("local");
  remember(choice);
  navigate("chat");
}

// A refreshed catalog must never silently replace a saved or explicitly typed model ID.
export function cloudModelSelection(modelId, models, { choice = null, custom = null } = {}) {
  const known = models.some((model) => model.id === modelId);
  const requested = choice ?? (modelId ? (known ? modelId : "custom") : models[0]?.id || "custom");
  const unavailable = requested !== "custom" && !models.some((model) => model.id === requested);
  return {
    choice: unavailable ? "custom" : requested,
    custom: unavailable ? requested : custom ?? (known ? "" : modelId || ""),
  };
}

// Backend errors intentionally contain no provider body. Display only our own translations,
// including for an unexpected IPC failure; never echo the form or an arbitrary error string.
export function cloudError(error) {
  const message = error?.message || String(error);
  if (message.includes("NEBIUS_API_KEY is missing")) return "missingKey";
  if (message.includes("NEBIUS_MODEL_ID is missing")) return "missingModel";
  if (message.includes("explicit nvidia/") || message.includes("absent from the account catalog")) return "model";
  if (/HTTP (401|403)\b/.test(message)) return "key";
  if (/HTTP 429\b/.test(message)) return "limit";
  if (message.includes("time limit")) return "timeout";
  if (message.includes("Provider request failed") || /HTTP 5\d\d\b/.test(message)) return "network";
  return "unknown";
}

/** Shared by Settings and first launch: no network until run(), one request at a time. */
export function createCloudCheck(call, changed = () => {}) {
  const state = { running: false, check: null, catalog: null, error: null };
  let revision = 0;
  return {
    state,
    invalidate({ key = false } = {}) {
      revision += 1;
      state.check = null;
      state.error = null;
      if (key) state.catalog = null;
    },
    async run(kind, save) {
      if (!["catalog", "test", "save"].includes(kind)) throw new Error("Unknown cloud check");
      if (state.running) return false;
      state.running = true;
      state.check = null;
      state.error = null;
      if (kind === "catalog") state.catalog = null;
      const started = revision;
      changed();
      try {
        if (!(await save()) || started !== revision) return false;
        if (kind === "save") return true;
        const result = await call(kind === "catalog" ? "cloud_models" : "test_cloud");
        if (started !== revision) return false;
        if (kind === "catalog") state.catalog = { ...result, models: result.models.filter((model) => model.supports_tools !== false) };
        else state.check = result;
        return true;
      } catch (error) {
        if (started === revision) state.error = cloudError(error);
        return false;
      } finally {
        state.running = false;
        changed();
      }
    },
  };
}
