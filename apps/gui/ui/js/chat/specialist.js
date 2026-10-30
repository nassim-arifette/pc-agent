export function markSpecialistRequest(steps, event) {
  if (!["started", "decided", "handoff"].includes(event.kind)) return;
  const request = steps.findLast((step) => step.type === "model" && step.state === "running");
  if (request) request.localSpecialist = event.kind !== "handoff";
}

export function specialistRequestLabel(step) {
  if (!step.localSpecialist) return null;
  return ({ running: "specialist.running", done: "specialist.request.done",
    failed: "specialist.request.failed", cancelled: "specialist.request.cancelled" })[step.state] || "specialist.request.failed";
}
