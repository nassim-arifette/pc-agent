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
