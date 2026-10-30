import { h } from "./dom.js";

const POSES = new Set(["hello", "investigate", "thinking", "local"]);

export function noxAsset(pose = "hello") {
  const name = POSES.has(pose) ? pose : "hello";
  return new URL(`../assets/nox/nox-${name}.svg`, import.meta.url).href;
}

export function nox(pose = "hello", width = 32) {
  return h("img", { class: "nox", src: noxAsset(pose), width, height: Math.round(width * 7 / 8),
    alt: "", "aria-hidden": "true", decoding: "async", draggable: "false" });
}
