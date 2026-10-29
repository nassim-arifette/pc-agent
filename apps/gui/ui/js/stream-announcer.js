// Speak stable sentence batches without repeating the whole draft for every token.
export function streamAnnouncer(emit, schedule = setTimeout, cancel = clearTimeout) {
  let id = null;
  let latest = "";
  let spoken = "";
  let timer = null;
  const stop = () => {
    if (timer !== null) cancel(timer);
    timer = null;
  };
  const speak = (final = false) => {
    timer = null;
    const suffix = latest.slice(spoken.length);
    const batch = final ? suffix : suffix.match(/^[\s\S]*[.!?](?:\s|$)/)?.[0];
    if (!batch) return;
    spoken += batch;
    if (batch.trim()) emit(batch.trim());
  };
  return {
    update(turn, text) {
      if (turn !== id || !text.startsWith(latest)) {
        stop();
        spoken = "";
      }
      id = turn;
      latest = text;
      if (timer === null && /[.!?](?:\s|$)/.test(latest.slice(spoken.length))) {
        timer = schedule(() => speak(), 1200);
      }
    },
    finish() {
      stop();
      speak(true);
    },
    dispose: stop,
  };
}
