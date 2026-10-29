// Shared Live PC / Storage / History entry point. No selection ID and no automatic write.
import { watchForm } from "./watches/form.js";
import { validRule } from "./watches/model.js";
import { pageFailure } from "./failure.js";

let editing = false;

/** Opens a C7 proposal. Returns nothing; only the dialog's explicit confirmation may persist. */
export function requestWatch(rule) {
  if (!validRule(rule)) throw pageFailure("watch.invalid");
  openForm(rule, null);
}

/** The Surveillances view uses the same form for an existing durable rule. */
export function editWatch(watch) {
  if (!watch?.id || !validRule(watch.rule)) throw pageFailure("watch.invalid");
  openForm(watch.rule, watch.id);
}

function openForm(rule, id) {
  if (editing) return;
  editing = true;
  watchForm(rule, id).finally(() => { editing = false; });
}
