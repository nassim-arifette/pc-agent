//! The model-facing submission schema, lenient parsing and bounded repair messages.
use super::{
    Category, CitationKind, Confidence, Issue, IssueKind, MAX_ACTION_REFS, MAX_ANSWER_CHARS,
    MAX_FOLLOW_UPS, MAX_FOLLOW_UP_CHARS, SUBMIT_ANSWER,
};
use serde::Deserialize;
use serde_json::{json, Value};

const CATEGORIES: [&str; 13] = [
    "memory",
    "cpu",
    "disk",
    "gpu",
    "thermal_power",
    "storage_space",
    "startup_background",
    "updates_system",
    "software_fault",
    "hardware_limit",
    "no_problem_found",
    "insufficient_evidence",
    "other",
];

/// The arguments of `submit_answer`. Reading is lenient on form, never on content: an extra field
/// is ignored, a list sent as one text is split into lines, and a missing or unknown confidence or
/// category falls back to `medium` or `other`. Only the answer text itself is required.
#[derive(Debug, Clone, Deserialize)]
pub(super) struct Submission {
    pub(super) answer: String,
    #[serde(default = "medium", deserialize_with = "lenient_confidence")]
    pub(super) confidence: Confidence,
    #[serde(default = "other", deserialize_with = "lenient_category")]
    pub(super) category: Category,
    #[serde(default, deserialize_with = "text_list")]
    pub(super) action_refs: Vec<String>,
    #[serde(default, deserialize_with = "text_list")]
    pub(super) follow_ups: Vec<String>,
}

fn medium() -> Confidence {
    Confidence::Medium
}
fn other() -> Category {
    Category::Other
}

/// A known value written in another case or with spaces or dashes, like `Medium` or `storage space`.
fn known_word<T: serde::de::DeserializeOwned>(value: &Value) -> Option<T> {
    let word = value
        .as_str()?
        .trim()
        .to_lowercase()
        .replace([' ', '-'], "_");
    serde_json::from_value(json!(word)).ok()
}

fn lenient_confidence<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Confidence, D::Error> {
    Ok(known_word(&Value::deserialize(d)?).unwrap_or(Confidence::Medium))
}

fn lenient_category<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Category, D::Error> {
    Ok(known_word(&Value::deserialize(d)?).unwrap_or(Category::Other))
}

/// A list of texts, or one text with an item per line (bullets and numbering removed).
fn text_list<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<String>, D::Error> {
    Ok(match Value::deserialize(d)? {
        Value::Array(items) => items
            .into_iter()
            .filter_map(|item| item.as_str().map(str::to_owned))
            .collect(),
        Value::String(text) => text
            .lines()
            .map(|line| {
                line.trim()
                    .trim_start_matches(|c: char| {
                        c.is_ascii_digit() || matches!(c, '-' | '*' | '•' | '.' | ')')
                    })
                    .trim()
                    .to_owned()
            })
            .filter(|line| !line.is_empty())
            .collect(),
        _ => Vec::new(),
    })
}

/// The canonical definition, part of the global registry identity (`routing`).
pub(crate) fn definition() -> Value {
    definition_for(true)
}

/// The definition a turn declares: `action_refs` names `prepare_action_plan` only when the turn
/// offers it. The schema itself never changes (C1).
pub(crate) fn definition_for(proposals: bool) -> Value {
    let action_refs = if proposals {
        "plan_id values returned by prepare_action_plan in this investigation that the answer recommends."
    } else {
        "No action can be proposed in this investigation: leave it empty."
    };
    json!({"type":"function","function":{"name":SUBMIT_ANSWER,
        "description":"Finish the investigation with your answer. Call it alone, after at least one inspection; selected evidence in the first message counts as one.",
        "parameters":{"type":"object","additionalProperties":false,
            "required":["answer","confidence","category"],
            "properties":{
                "answer":{"type":"string","maxLength":MAX_ANSWER_CHARS,
                    "description":"The answer in the language the question is written in (an English question gets an English answer): the direct answer first, then what the user can do, then what supports it and what remains uncertain. Plain prose and short lists; cite the source of every number right after it, like [O1] or [F1]. Numbers from an earlier measurement are written in the past."},
                "confidence":{"type":"string","enum":["high","medium","low"],
                    "description":"How well the measurements support the answer."},
                "category":{"type":"string","enum":CATEGORIES,
                    "description":"Main topic of the answer."},
                "action_refs":{"type":"array","maxItems":MAX_ACTION_REFS,"items":{"type":"string"},
                    "description":action_refs},
                "follow_ups":{"type":"array","maxItems":MAX_FOLLOW_UPS,
                    "items":{"type":"string","maxLength":MAX_FOLLOW_UP_CHARS},
                    "description":"Up to three short follow-up questions or checks the user could ask next, in the same language as the answer."}}}}})
}

/// The tool reply that sends an answer back for correction. When a citation is wrong, it lists
/// the references the answer may use.
pub(crate) fn repair_request(
    issues: &[Issue],
    references: &[(String, CitationKind, String)],
) -> Value {
    let mut valid: Vec<&str> = Vec::new();
    for (reference, _, _) in references {
        if !valid.contains(&reference.as_str()) {
            valid.push(reference);
        }
    }
    // Examples name a reference that exists in this turn, never a made-up one.
    let example = references
        .iter()
        .find(|(_, kind, _)| *kind == CitationKind::Observation)
        .map_or("O1", |(reference, _, _)| reference.as_str());
    let problems: Vec<Value> = issues
        .iter()
        .map(|i| json!({"kind":i.kind,"excerpt":i.excerpt,"fix":i.kind.fix().replace("{ref}", example)}))
        .collect();
    let kinds = |kind: IssueKind| issues.iter().any(|i| i.kind == kind);
    let next = if kinds(IssueKind::NoInspection) {
        "Call an available inspection tool now; call submit_answer alone only after usable measurements are returned, correcting the listed problems."
    } else if kinds(IssueKind::NotAlone) {
        "Call submit_answer alone, with the complete answer."
    } else if kinds(IssueKind::NoToolCall) {
        "Call submit_answer with your complete answer."
    } else {
        "Correct only these problems and call submit_answer again with the complete answer."
    };
    let mut request = json!({"accepted":false,"problems":problems,"next":next});
    if issues.iter().any(|i| {
        matches!(
            i.kind,
            IssueKind::UnknownCitation
                | IssueKind::MissingCitation
                | IssueKind::UncitedNumber
                | IssueKind::UnsupportedNumber
                | IssueKind::HistoricalWording
        )
    }) {
        request["valid_references"] = json!(valid);
    }
    request
}

/// Fields of the submit_answer schema, the only names a malformed-submission note may repeat.
const SUBMISSION_FIELDS: [&str; 5] = [
    "answer",
    "confidence",
    "category",
    "action_refs",
    "follow_ups",
];

/// A closed description of why a submission does not parse, like `missing_field category`: a
/// class and a declared field name, never text the model wrote.
pub(super) fn malformed(error: &serde_json::Error) -> String {
    let message = error.to_string();
    let class = if message.starts_with("unknown field") {
        "unknown_field"
    } else if message.starts_with("missing field") {
        "missing_field"
    } else if message.starts_with("invalid type") {
        "invalid_type"
    } else if message.starts_with("unknown variant") || message.starts_with("invalid value") {
        "invalid_value"
    } else {
        "syntax"
    };
    // Only the part before serde's list of expected names says which field is at fault.
    let subject = message.split(", expected").next().unwrap_or_default();
    let field = SUBMISSION_FIELDS
        .iter()
        .find(|field| subject.contains(&format!("`{field}`")))
        .copied()
        .unwrap_or("unknown");
    format!("{class} {field}")
}
