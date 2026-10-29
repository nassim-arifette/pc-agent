import assert from "node:assert/strict";

// A virtual clock lets scan loops, source expiry and recordings run without real delays.
export function virtualClock() {
  let now = 1_800_000_000_000;
  let sequence = 0;
  const timers = [];
  const microtasks = async () => { for (let i = 0; i < 20; i++) await Promise.resolve(); };
  const tick = async () => {
    timers.sort((a, b) => a.at - b.at || a.id - b.id);
    const timer = timers.shift();
    if (!timer) return false;
    now = Math.max(now, timer.at);
    timer.callback();
    await microtasks();
    return true;
  };
  return {
    now: () => now,
    setTimeout: (callback, ms = 0) => timers.push({ callback, at: now + ms, id: sequence++ }),
    async settle(promise) {
      let result;
      promise.then(value => { result = { value }; }, error => {
        result = { error: { name: error?.name, code: error?.code, reason: error?.reason, message: error?.message ?? String(error) } };
      });
      for (let i = 0; i < 10000; i++) {
        await microtasks();
        if (result) return result;
        assert.ok(await tick(), "Mock command stalled without a timer");
      }
      assert.fail("Mock exceeded the timer budget");
    },
    async drain() {
      await microtasks();
      for (let i = 0; i < 10000; i++) if (!(await tick())) return;
      assert.fail("Mock exceeded the timer budget");
    },
  };
}

export const scenarios = [
  "", "?pc=desktop&ntfs=none&helper=missing", "?gpu=none&disk=ok", "?battery=ac", "?battery=charging",
  "?battery=low", "?battery=relative", "?battery=two", "?battery=unavailable", "?battery=none",
  "?cloud=none", "?cloud=problem", "?connection=key", "?connection=network", "?connection=timeout",
  "?connection=model", "?connection=limit", "?connection=empty", "?privacy=local",
  "?privacy=anonymized&web=key", "?elevation=fail", "?elevation=slow", "?recording=short",
  "?sources=short", "?conversation=empty&turn=orphan", "?history=unavailable&live=fail",
  "?scan=denied", "?scan=fail", "?action=refuse", "?preview=expired", "?routing=stale",
  "?report=none", "?report=quiet", "?report=unsupported", "?report=stale",
];
