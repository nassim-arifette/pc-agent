// Tiny DOM helpers. Text always goes through textContent or text nodes, never innerHTML, so
// names read from the PC or words from a model can never become markup.
const SVG_NS = "http://www.w3.org/2000/svg";

function apply(el, props) {
  for (const [key, value] of Object.entries(props || {})) {
    if (value == null || value === false) continue;
    if (key === "class") {
      const names = Array.isArray(value) ? value.filter(Boolean).join(" ") : value;
      if (names) el.setAttribute("class", names);
    } else if (key === "dataset") {
      Object.assign(el.dataset, value);
    } else if (key === "style") {
      for (const [name, v] of Object.entries(value)) el.style.setProperty(name, v);
    } else if (key === "text") {
      el.textContent = value;
    } else if (key === "value" || key === "checked") {
      el[key] = value;
    } else if (key.startsWith("on") && typeof value === "function") {
      el.addEventListener(key.slice(2).toLowerCase(), value);
    } else if (value === true) {
      el.setAttribute(key, "");
    } else {
      el.setAttribute(key, String(value));
    }
  }
}

function append(el, children) {
  for (const child of children.flat(Infinity)) {
    if (child == null || child === false || child === "") continue;
    el.append(child instanceof Node ? child : document.createTextNode(String(child)));
  }
}

/** Creates an HTML element: h("div", { class: "card" }, child, "text"). */
export function h(tag, props, ...children) {
  const el = document.createElement(tag);
  apply(el, props);
  append(el, children);
  return el;
}

/** Creates an SVG element with the same conventions as h(). */
export function s(tag, props, ...children) {
  const el = document.createElementNS(SVG_NS, tag);
  apply(el, props);
  append(el, children);
  return el;
}

export function clear(el) {
  while (el.firstChild) el.firstChild.remove();
  return el;
}

/** Replaces an element's children. */
export function fill(el, ...children) {
  clear(el);
  append(el, children);
  return el;
}

/** Announces a short status for screen readers. */
export function announce(text) {
  const region = document.getElementById("announcer");
  if (!region) return;
  region.textContent = "";
  window.setTimeout(() => {
    region.textContent = text;
  }, 60);
}
