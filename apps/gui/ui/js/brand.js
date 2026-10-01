// Iris reduced to a mark: an accent tile with two eyes. It signs the assistant's answers and the
// window; it never animates on its own.
import { s } from "./dom.js";

export function mark(size = 20, label) {
  const eyeWidth = size * 0.15;
  const eyeHeight = size * 0.3;
  const eyeTop = size * 0.33;
  const eye = (x) => s("rect", { class: "mark-eye", x: x.toFixed(1), y: eyeTop.toFixed(1), width: eyeWidth.toFixed(1), height: eyeHeight.toFixed(1), rx: (eyeWidth / 2).toFixed(1) });
  return s(
    "svg",
    {
      class: "mark",
      width: size,
      height: size,
      viewBox: `0 0 ${size} ${size}`,
      "aria-hidden": label ? null : "true",
      role: label ? "img" : null,
      "aria-label": label || null,
    },
    s("rect", { class: "mark-tile", x: 0, y: 0, width: size, height: size, rx: (size * 0.3).toFixed(1) }),
    eye(size * 0.28),
    eye(size * 0.57),
  );
}
