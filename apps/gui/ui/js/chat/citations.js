// Citations as people see them: each answer numbers its sources 1, 2, 3 in the order the model
// lists them, whatever their internal references (O2, F1, W1).
import { t, classificationLabel } from "../i18n.js";
import { date } from "../format.js";

export function toolLabel(name) {
  const key = `tool.${name}`;
  const label = t(key);
  return label === key ? name : label;
}

/** Reference → displayed number for one answer. */
export function numbering(answer) {
  return new Map((answer?.citations || []).map((c, i) => [c.reference, i + 1]));
}

/** A web page found by a lookup (C6), not a measurement of this PC. */
export function isWeb(citation) {
  return citation?.kind === "web";
}

export function citationTitle(citation) {
  if (isWeb(citation)) return citation.title || citation.web?.domain || citation.reference;
  if (citation.kind === "observation") return toolLabel(citation.title);
  // A finding is titled by its classification; earlier and older sources keep their stored title.
  return classificationLabel({ classification: citation.classification, label: citation.title });
}

/**
 * A source's own publication date, as the search reported it. A date that cannot be read is
 * shown as written; a missing one stays unknown.
 */
export function publishedDate(value) {
  if (!value) return t("web.date.unknown");
  // A bare day is that calendar day wherever the PC is, not midnight UTC.
  const day = /^(\d{4})-(\d{2})-(\d{2})$/.exec(value);
  const at = day ? new Date(Number(day[1]), Number(day[2]) - 1, Number(day[3])).getTime() : Date.parse(value);
  return Number.isFinite(at) ? date(at) : value;
}

/** Counts of measured and web sources of one answer. */
export function sourceCounts(answer) {
  const all = answer?.citations || [];
  const web = all.filter(isWeb).length;
  return { measured: all.length - web, web };
}
