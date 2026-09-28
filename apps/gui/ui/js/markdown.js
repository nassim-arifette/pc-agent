// The restricted Markdown the model writes, rendered as DOM nodes: paragraphs, lists, short
// headings, bold, inline code and citation chips ([O2], [F1]). Text always stays text: nothing is
// ever parsed as HTML, and links are not rendered.
import { h } from "./dom.js";

const CITATION = /\[((?:[OF]\d{1,5})(?:\s*[,;]\s*[OF]\d{1,5})*)\]/g;

function chip(reference, onCite) {
  return h("button", { class: "cite", type: "button", dataset: { ref: reference }, onClick: () => onCite?.(reference) }, reference);
}

function citations(text, onCite) {
  const nodes = [];
  let last = 0;
  for (const match of text.matchAll(CITATION)) {
    if (match.index > last) nodes.push(text.slice(last, match.index));
    for (const reference of match[1].split(/\s*[,;]\s*/)) nodes.push(chip(reference, onCite));
    last = match.index + match[0].length;
  }
  if (last < text.length) nodes.push(text.slice(last));
  return nodes;
}

function bold(text, onCite) {
  const parts = text.split("**");
  return parts.map((part, i) => {
    const closed = i % 2 === 1 && i < parts.length - 1;
    if (closed) return h("strong", null, citations(part, onCite));
    // An unmatched marker stays as written.
    return citations(i % 2 === 1 ? `**${part}` : part, onCite);
  });
}

function inline(text, onCite) {
  const parts = text.split("`");
  return parts.map((part, i) => {
    const closed = i % 2 === 1 && i < parts.length - 1;
    if (closed) return h("code", null, part);
    return bold(i % 2 === 1 ? `\`${part}` : part, onCite);
  });
}

/** Returns block elements for the text; `onCite(reference)` runs when a citation is clicked. */
export function markdown(text, onCite) {
  const blocks = [];
  let list = null;
  let paragraph = [];
  const flush = () => {
    if (paragraph.length) blocks.push(h("p", null, inline(paragraph.join(" "), onCite)));
    paragraph = [];
  };
  for (const raw of String(text).split(/\r?\n/)) {
    const line = raw.trim();
    if (!line) {
      flush();
      list = null;
      continue;
    }
    const item = line.match(/^([-*•]|\d{1,2}[.)])\s+(.*)$/);
    if (item) {
      flush();
      const ordered = /^\d/.test(item[1]);
      if (!list || list.ordered !== ordered) {
        list = { ordered, el: h(ordered ? "ol" : "ul", { class: "md-list" }) };
        blocks.push(list.el);
      }
      list.el.append(h("li", null, inline(item[2], onCite)));
      continue;
    }
    const heading = line.match(/^#{1,4}\s+(.*)$/);
    if (heading) {
      flush();
      list = null;
      blocks.push(h("h4", { class: "md-heading" }, inline(heading[1], onCite)));
      continue;
    }
    list = null;
    paragraph.push(line);
  }
  flush();
  return blocks;
}
