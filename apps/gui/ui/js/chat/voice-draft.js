const active = (phase) => ["starting", "listening", "finalizing"].includes(phase);

/** In-memory, conversation-bound dictation. Applying text never submits a question. */
export function createVoiceDraft({ invoke, changed = () => {}, apply = () => {} }) {
  let state = { phase: "idle" };
  let id = null;
  let generation = 0;
  let owner;
  let pending = false;
  let review = false;
  let base = "";
  let queued = null;
  const cancel = (sessionId) => {
    if (sessionId) void Promise.resolve(invoke("voice_stop", { sessionId, cancel: true })).catch(() => {});
  };
  const accept = (event) => {
    if (pending && !id) {
      if (!queued || event.sequence > queued.sequence) queued = event;
      return;
    }
    if (!id || event.session_id !== id || event.sequence <= (state.sequence ?? -1)) return;
    // An explicit local stop cannot be undone by an in-flight meter update.
    if (state.phase === "finalizing" && ["starting", "listening"].includes(event.phase)) return;
    state = event;
    if (["review", "error"].includes(event.phase) && event.text?.trim()) {
      apply([base.trimEnd(), event.text.trim()].filter(Boolean).join("\n"));
      review = true;
    }
    if (event.phase === "cancelled") { review = false; apply(base); }
    changed();
  };
  const clear = () => {
    generation += 1;
    const previous = id;
    id = null;
    pending = false;
    review = false;
    queued = null;
    base = "";
    state = { phase: "idle" };
    cancel(previous);
  };
  return {
    get state() { return state; },
    get busy() { return pending || active(state.phase); },
    get reviewRequired() { return review; },
    accept,
    bind(next) {
      if (owner !== next) { owner = next; clear(); changed(); }
    },
    async start(text) {
      if (pending || active(state.phase) || review) return;
      const current = ++generation;
      base = text;
      id = null;
      queued = null;
      pending = true;
      state = { phase: "starting" };
      changed();
      try {
        const started = await invoke("voice_start", { consent: true });
        if (current !== generation) { cancel(started.session_id); return; }
        id = started.session_id;
        pending = false;
        state = { phase: "starting", sequence: -1 };
        accept(started);
        if (queued) accept(queued);
        // Native events can arrive before the invoke resolves.
        const latest = await invoke("voice_state");
        if (current === generation) accept(latest);
      } catch (error) {
        if (current === generation) {
          cancel(id);
          id = null;
          pending = false;
          state = { phase: "error", error };
          changed();
        }
      }
    },
    async stop(discard = false) {
      if (!id) {
        if (discard) { const original = base; clear(); apply(original); changed(); }
        return;
      }
      const current = generation;
      state = { ...state, phase: "finalizing" };
      if (discard) { review = false; apply(base); }
      changed();
      try {
        const next = await invoke("voice_stop", { sessionId: id, cancel: discard });
        if (current === generation) accept(next);
      } catch (error) {
        if (current === generation) { state = { phase: "error", error }; changed(); }
      }
    },
    reviewed() { review = false; changed(); },
    dispose() { clear(); },
  };
}
