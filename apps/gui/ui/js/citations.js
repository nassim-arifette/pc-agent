// Citations as people see them: each answer numbers its sources 1, 2, 3 in the order the model
// lists them, whatever their internal references (O2, F1).
import { t } from "./i18n.js";

export function toolLabel(name) {
  const key = `tool.${name}`;
  const label = t(key);
  return label === key ? name : label;
}

/** Reference → displayed number for one answer. */
export function numbering(answer) {
  return new Map((answer?.citations || []).map((c, i) => [c.reference, i + 1]));
}

export function citationTitle(citation) {
  return citation.kind === "observation" ? toolLabel(citation.title) : citation.title;
}
