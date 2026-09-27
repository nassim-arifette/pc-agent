// Iris, the mascot: a small robot with a glowing visor. Its state always mirrors something
// real (see mascot.css); it never pretends that work is happening.
import { h, s } from "./dom.js";

let instances = 0;

const STATES = new Set(["idle", "listening", "working", "thinking", "happy", "unsure", "concerned", "error", "sleeping"]);

function sparkle(x, y, r) {
  return `M${x} ${y - r} L${x + r * 0.28} ${y - r * 0.28} L${x + r} ${y} L${x + r * 0.28} ${y + r * 0.28} L${x} ${y + r} L${x - r * 0.28} ${y + r * 0.28} L${x - r} ${y} L${x - r * 0.28} ${y - r * 0.28}Z`;
}

/** The full character. `size` is in CSS pixels. */
export function createMascot({ size = 160, label = "Iris" } = {}) {
  const n = ++instances;
  const shell = `iris-shell-${n}`;
  const visor = `iris-visor-${n}`;
  const svg = s(
    "svg",
    {
      class: "iris",
      viewBox: "0 0 160 160",
      width: size,
      height: size,
      role: "img",
      "aria-label": label,
      dataset: { state: "idle" },
    },
    s(
      "defs",
      null,
      s("linearGradient", { id: shell, x1: 0, y1: 0, x2: 0, y2: 1 }, s("stop", { offset: 0, class: "stop-shell-top" }), s("stop", { offset: 1, class: "stop-shell-bottom" })),
      s("linearGradient", { id: visor, x1: 0, y1: 0, x2: 0.6, y2: 1 }, s("stop", { offset: 0, class: "stop-visor-top" }), s("stop", { offset: 1, class: "stop-visor-bottom" })),
    ),
    s("ellipse", { class: "iris-shadow", cx: 80, cy: 151, rx: 30, ry: 5 }),
    s(
      "g",
      { class: "iris-float" },
      s("rect", { class: "iris-torso", x: 56, y: 104, width: 48, height: 30, rx: 15 }),
      s("circle", { class: "iris-core", cx: 80, cy: 121, r: 4.5 }),
      s("circle", { class: "iris-hand", cx: 46, cy: 122, r: 7.5 }),
      s(
        "g",
        { class: "iris-lens" },
        s("line", { class: "iris-lens-handle", x1: 117, y1: 117, x2: 124, y2: 110 }),
        s("circle", { class: "iris-lens-glass", cx: 131, cy: 102, r: 10 }),
        s("path", { class: "iris-lens-shine", d: "M125.5 99 a6.5 6.5 0 0 1 5 -3.6" }),
      ),
      s("circle", { class: "iris-hand", cx: 114, cy: 122, r: 7.5 }),
      s(
        "g",
        { class: "iris-head" },
        s("line", { class: "iris-antenna", x1: 80, y1: 30, x2: 80, y2: 17 }),
        s("circle", { class: "iris-tip", cx: 80, cy: 12, r: 6 }),
        s("rect", { class: "iris-ear", x: 19, y: 55, width: 11, height: 26, rx: 5.5 }),
        s("rect", { class: "iris-ear", x: 130, y: 55, width: 11, height: 26, rx: 5.5 }),
        s("rect", { class: "iris-shell", x: 26, y: 26, width: 108, height: 86, rx: 41, fill: `url(#${shell})` }),
        s("rect", { class: "iris-visor", x: 38, y: 42, width: 84, height: 50, rx: 25, fill: `url(#${visor})` }),
        s(
          "g",
          { class: "iris-eyes" },
          s("rect", { class: "iris-eye iris-eye-l", x: 57, y: 55, width: 13, height: 21, rx: 6.5 }),
          s("rect", { class: "iris-eye iris-eye-r", x: 90, y: 55, width: 13, height: 21, rx: 6.5 }),
          s("path", { class: "iris-eye-happy", d: "M56.5 70 q7 -10 14 0" }),
          s("path", { class: "iris-eye-happy", d: "M89.5 70 q7 -10 14 0" }),
        ),
        s("path", { class: "iris-mouth", d: "M73 100 q7 5 14 0" }),
        s("ellipse", { class: "iris-cheek", cx: 41, cy: 96, rx: 6, ry: 3.5 }),
        s("ellipse", { class: "iris-cheek", cx: 119, cy: 96, rx: 6, ry: 3.5 }),
      ),
    ),
    s(
      "g",
      { class: "iris-fx" },
      s("text", { class: "iris-question", x: 132, y: 38 }, "?"),
      s("g", { class: "iris-dots" }, s("circle", { cx: 118, cy: 24, r: 3 }), s("circle", { cx: 128, cy: 17, r: 3.6 }), s("circle", { cx: 139, cy: 9, r: 4.2 })),
      s("g", { class: "iris-zz" }, s("text", { x: 118, y: 30 }, "z"), s("text", { x: 130, y: 20 }, "z")),
      s("g", { class: "iris-sparkles" }, s("path", { d: sparkle(22, 30, 7) }), s("path", { d: sparkle(142, 50, 5.5) })),
    ),
  );
  return {
    el: svg,
    set(state) {
      svg.dataset.state = STATES.has(state) ? state : "idle";
    },
    get state() {
      return svg.dataset.state;
    },
  };
}

/** Iris with a speech bubble below (or beside, with `side`). */
export function mascotWithBubble({ size = 150, side = false, label } = {}) {
  const mascot = createMascot({ size, label });
  const bubble = h("p", { class: side ? "bubble side" : "bubble" });
  const wrap = h("div", { class: side ? "row" : "hero-mascot" }, mascot.el, bubble);
  return {
    el: wrap,
    set: mascot.set,
    say(text) {
      if (bubble.textContent === text) return;
      bubble.textContent = text;
      // Restart the entrance animation for a new sentence.
      bubble.style.setProperty("animation", "none");
      void bubble.offsetWidth;
      bubble.style.removeProperty("animation");
    },
  };
}

/** The compact head used as the app's mark in the sidebar. */
export function createMark() {
  return s(
    "svg",
    { class: "brand-mark", viewBox: "0 0 64 64", "aria-hidden": "true" },
    s("defs", null, s("linearGradient", { id: "mark-bg", x1: 0, y1: 0, x2: 0, y2: 1 }, s("stop", { offset: 0, "stop-color": "#8476ff" }), s("stop", { offset: 1, "stop-color": "#5646de" }))),
    s("rect", { x: 0, y: 0, width: 64, height: 64, rx: 16, fill: "url(#mark-bg)" }),
    s("line", { x1: 32, y1: 20, x2: 32, y2: 12, stroke: "#fff", "stroke-width": 2.4, "stroke-linecap": "round" }),
    s("circle", { cx: 32, cy: 10.5, r: 3.6, fill: "#ff8a65" }),
    s("rect", { x: 10, y: 29, width: 5, height: 11, rx: 2.5, fill: "#d6d0ff" }),
    s("rect", { x: 49, y: 29, width: 5, height: 11, rx: 2.5, fill: "#d6d0ff" }),
    s("rect", { x: 13, y: 19, width: 38, height: 32, rx: 15, fill: "#faf9ff" }),
    s("rect", { x: 18, y: 25, width: 28, height: 18, rx: 9, fill: "#1e1a4d" }),
    s("rect", { x: 24, y: 29.5, width: 5, height: 8.5, rx: 2.5, fill: "#7cf3e6" }),
    s("rect", { x: 35, y: 29.5, width: 5, height: 8.5, rx: 2.5, fill: "#7cf3e6" }),
  );
}
