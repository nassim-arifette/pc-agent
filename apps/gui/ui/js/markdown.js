// The restricted Markdown the model writes, rendered as DOM nodes: paragraphs, lists, short
// headings, bold, inline code and citation chips ([O2], [F1], and [W1] for a web source). Text
// always stays text: nothing is ever parsed as HTML, and links are not rendered.
import { h } from "./dom.js";
import { icon } from "./icons.js";
import { t } from "./i18n.js";

// One bracket group of references: [O2], [O2, F1], [O2; W1] or [O1 O2], as the verifier accepts.
const REF = "[OFW]\\d{1,5}";
const GROUP = `\\[\\s*(${REF}(?:(?:\\s*[,;]\\s*|\\s+)${REF})*)\\s*\\]`;
const CITATION = new RegExp(GROUP, "g");
const SPACED_CITATION = new RegExp(`\\s*${GROUP}`, "g");
// A full stop (or !, ?, …) followed by citation groups: the groups belong to the sentence it ends.
const AFTER_STOP = new RegExp(`([.!?…]+)((?:\\s*${GROUP})+)`, "g");
// A sentence ends at a stop followed by a space or the end; a decimal point ("1.54") does not.
const SENTENCE_END = /[.!?…]+(?=\s|$)/g;
const ABBREVIATION = /\b(?:e\.g|i\.e|env|approx|cf|vs|p\.\s*ex)\.$/iu;

function abbreviationAt(text, stopIndex) {
  const before = text.slice(0, stopIndex + 1);
  const after = text.slice(stopIndex + 1).trimStart();
  return after.length > 0 && (
    ABBREVIATION.test(before)
    || (/\bp\.$/iu.test(before) && /^ex\./iu.test(after))
    || (/\betc\.$/iu.test(before) && /^\p{Ll}/u.test(after))
  );
}

function references(group) {
  return group.split(/[\s,;]+/).filter(Boolean);
}

/** The labels of some references in display order: 1, 2 rather than 2, 1. */
function ordered(refs, label) {
  // Labels are asked for in the order written, since a draft numbers references as they appear.
  const labelled = refs.map((reference) => ({ reference, text: String(label?.(reference) ?? reference) }));
  const rank = (x) => (/^\d+$/.test(x.text) ? Number(x.text) : Infinity);
  return labelled.sort((a, b) => rank(a) - rank(b));
}

function gatherSentence(sentence) {
  const refs = [];
  const stripped = sentence.replace(SPACED_CITATION, (_, group) => {
    for (const reference of references(group)) if (!refs.includes(reference)) refs.push(reference);
    return "";
  });
  if (!refs.length) return sentence;
  const group = ` [${refs.join(", ")}]`;
  const stop = /[.!?…]+$/.exec(stripped);
  return stop ? `${stripped.slice(0, stop.index)}${group}${stop[0]}` : stripped.replace(/\s*$/, (space) => `${group}${space}`);
}

/**
 * Gathers the citations of each sentence into one group at its end, each reference once, as chat
 * apps show sources: "a [O1] b [O1], c [O2]." becomes "a b, c [O1, O2].". Only the display
 * changes; the verifier reads the answer as the model wrote it.
 */
export function gatherCitations(text) {
  const moved = String(text).replace(AFTER_STOP, (match, stop, groups, _reference, offset, original) =>
    stop === "." && abbreviationAt(original, offset) ? match : `${groups}${stop}`,
  );
  const sentences = [];
  let start = 0;
  for (const end of moved.matchAll(SENTENCE_END)) {
    if (end[0] === "." && abbreviationAt(moved, end.index)) continue;
    sentences.push(moved.slice(start, end.index + end[0].length));
    start = end.index + end[0].length;
  }
  if (start < moved.length) sentences.push(moved.slice(start));
  return sentences.map(gatherSentence).join("");
}

/** The text with its citations gathered and written as their labels: "… [1, 2].". */
export function labelCitations(text, label) {
  return gatherCitations(text).replace(CITATION, (_, group) => `[${ordered(references(group), label).map((x) => x.text).join(", ")}]`);
}

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

/** One group of chips, numbered in order; a single source is a single chip. */
function chipGroup(refs, cite) {
  const chips = ordered(refs, cite.label).map((x) => chip(x.reference, cite));
  return chips.length === 1 ? chips[0] : h("span", { class: "cite-group" }, chips);
}

function citations(text, cite) {
  const nodes = [];
  let last = 0;
  for (const match of text.matchAll(CITATION)) {
    if (match.index > last) nodes.push(text.slice(last, match.index));
    nodes.push(chipGroup(references(match[1]), cite));
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
 * `describe(reference)` names a web source (its domain) for the chip's tooltip. The citations of
 * each sentence are shown once, together, at its end.
 */
export function markdown(text, onCite, label, describe) {
  const cite = { onCite, label, describe };
  const write = (line) => inline(gatherCitations(line), cite);
  const blocks = [];
  let list = null;
  let paragraph = [];
  const flush = () => {
    if (paragraph.length) blocks.push(h("p", null, write(paragraph.join(" "))));
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
      list.el.append(h("li", null, write(item[2])));
      continue;
    }
    const heading = line.match(/^#{1,4}\s+(.*)$/);
    if (heading) {
      flush();
      list = null;
      blocks.push(h("h4", { class: "md-heading" }, write(heading[1])));
      continue;
    }
    list = null;
    paragraph.push(line);
  }
  flush();
  return blocks;
}
