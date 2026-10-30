// Nox reduced to a mark: an owl head in the accent colour, with two round eyes and a small beak.
// It signs the window; it never animates on its own.
import { s } from "./dom.js";

// Drawn on a 24-unit grid and scaled to the requested size.
const HEAD = "M3 6.5 L6.5 2.5 L9.5 5.2 Q12 4.4 14.5 5.2 L17.5 2.5 L21 6.5 V16 Q21 22 12 22 Q3 22 3 16 Z";

export function mark(size = 20, label) {
  const eye = (cx) => [
    s("circle", { class: "mark-eye", cx, cy: 11.6, r: 3.7 }),
    s("circle", { class: "mark-iris", cx: cx + 0.4, cy: 11.9, r: 2 }),
    s("circle", { class: "mark-pupil", cx: cx + 0.5, cy: 12, r: 0.95 }),
  ];
  return s(
    "svg",
    {
      class: "mark",
      width: size,
      height: size,
      viewBox: "0 0 24 24",
      "aria-hidden": label ? null : "true",
      role: label ? "img" : null,
      "aria-label": label || null,
    },
    s("path", { class: "mark-tile", d: HEAD }),
    ...eye(8.1),
    ...eye(15.9),
    s("path", { class: "mark-beak", d: "M10.9 15.2 H13.1 L12 17.4 Z" }),
  );
}
