// The elevated helper runs one prepared or running task at a time (elevation_status). The cards
// that start one (quick drive analysis, deeper checks) publish whether they hold that slot, so the
// other card can disable its buttons with a hint instead of failing on elevation_prepare.
const holders = new Map();
const listeners = new Set();

/** Records whether `owner` currently holds the slot (prepared, running, stopping or cleaning up). */
export function holdSlot(owner, held) {
  if (Boolean(holders.get(owner)) === Boolean(held)) return;
  holders.set(owner, Boolean(held));
  for (const listener of [...listeners]) listener();
}

/** Whether another card holds the slot. */
export function slotHeldElsewhere(owner) {
  return [...holders].some(([name, held]) => held && name !== owner);
}

/** Calls `listener` when any card takes or releases the slot; returns the unsubscribe. */
export function onSlotChange(listener) {
  listeners.add(listener);
  return () => listeners.delete(listener);
}
