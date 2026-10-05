// The restricted Markdown the model writes, rendered as DOM nodes: paragraphs, lists, short
// headings, bold, inline code and citation chips ([O2], [F1], and [W1] for a web source). Text
// always stays text: nothing is ever parsed as HTML, and links are not rendered.
import { h } from "./dom.js";
import { icon } from "./icons.js";
import { t } from "./i18n.js";

const CITATION = /\[((?:[OFW]\d{1,5})(?:\s*[,;]\s*[OFW]\d{1,5})*)\]/g;

function chip(reference, cite) {
  const text = String(cite.label?.(reference) ?? reference);
  // A web page (C6) is marked with a globe so it is never mistaken for a measurement of this PC.
  const web = reference.startsWith("W");
  const about = web ? cite.describe?.(reference) : null;
  const aria = web ? t("cite.web.label", { n: text, source: about || t("web.source") }) : t("cite.label", { n: text });
  const content = web ? [icon("globe"), text] : [text];
  const classes = ["cite", web ? "web-cite" : "", cite.onCite ? "" : "static"];
  // Without a handler (a draft still being written) the chip is a plain mark, not a button.
  if (!cite.onCite) return h("span", { class: classes, "aria-label": aria, title: about }, content);
  return h("button", { class: classes, type: "button", dataset: { ref: reference }, "aria-label": aria, title: about, onClick: () => cite.onCite(reference) }, content);
}

function citations(text, cite) {
  const nodes = [];
  let last = 0;
  for (const match of text.matchAll(CITATION)) {
    if (match.index > last) nodes.push(text.slice(last, match.index));
    for (const reference of match[1].split(/\s*[,;]\s*/)) nodes.push(chip(reference, cite));
    last = match.index + match[0].length;
  }
  if (last < text.length) nodes.push(text.slice(last));
  return nodes;
}

function bold(text, cite) {
  const parts = text.split("**");
  return parts.map((part, i) => {
    const closed = i % 2 === 1 && i < parts.length - 1;
    if (closed) return h("strong", null, citations(part, cite));
    // An unmatched marker stays as written.
    return citations(i % 2 === 1 ? `**${part}` : part, cite);
  });
}

function inline(text, cite) {
  const parts = text.split("`");
  return parts.map((part, i) => {
    const closed = i % 2 === 1 && i < parts.length - 1;
    if (closed) return h("code", null, part);
    return bold(i % 2 === 1 ? `\`${part}` : part, cite);
  });
}

/**
 * Returns block elements for the text; `onCite(reference)` runs when a citation is clicked,
 * `label(reference)` gives the chip's text (the reference itself by default) and
 * `describe(reference)` names a web source (its domain) for the chip's tooltip.
 */
export function markdown(text, onCite, label, describe) {
  const cite = { onCite, label, describe };
  const blocks = [];
  let list = null;
  let paragraph = [];
  const flush = () => {
    if (paragraph.length) blocks.push(h("p", null, inline(paragraph.join(" "), cite)));
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
      list.el.append(h("li", null, inline(item[2], cite)));
      continue;
    }
    const heading = line.match(/^#{1,4}\s+(.*)$/);
    if (heading) {
      flush();
      list = null;
      blocks.push(h("h4", { class: "md-heading" }, inline(heading[1], cite)));
      continue;
    }
    list = null;
    paragraph.push(line);
  }
  flush();
  return blocks;
}
