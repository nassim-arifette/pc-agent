// Friendly names for the Nemotron models the settings screen offers.
const NAMES = {
  "nvidia/nemotron-3-super-120b-a12b": "Nemotron 3 Super",
  "nvidia/Nemotron-3-Ultra-550b-a55b": "Nemotron 3 Ultra",
  "nvidia/NVIDIA-Nemotron-3-Nano-30B-A3B": "Nemotron 3 Nano",
  "nvidia/Nemotron-3_5-Lightning": "Nemotron 3.5 Lightning",
};

export function modelName(id) {
  return NAMES[id] || (id || "").replace(/^nvidia\//, "");
}

/** The first listed model of a family ("super", "ultra"): what the picker's choices save. */
export function familyId(info, family) {
  return (info?.models || []).map((m) => m.id).find((id) => id.toLowerCase().includes(family)) || null;
}

/** The model a question starts with under the routing policy: Auto starts with Super. */
export function startModelId(info, routing) {
  const settings = routing?.settings;
  if (settings?.mode === "always_super" || settings?.mode === "automatic") return settings.super_model_id || familyId(info, "super");
  if (settings?.mode === "always_ultra") return settings.ultra_model_id || familyId(info, "ultra");
  return info?.cloud?.model_id ?? null;
}
