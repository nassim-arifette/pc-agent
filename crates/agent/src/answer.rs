//! The model's answer (contract C1 of docs/plan.md). The model writes the answer itself, in the
//! language of the question, and ends the investigation by calling `submit_answer`.
//!
//! The controller checks the text against what the model actually received:
//! - citations must name existing observations or findings;
//! - numbers must come from those results, allowing rounding, unit variants and simple ratios;
//! - program names must have been sent;
//! - the text carries no commands, links or HTML.
//!
//! Problems go back to the model for a bounded number of repairs. Whatever remains is reported as
//! unverified, never hidden.
use serde::{Deserialize, Serialize};
use serde_json::Value;

mod grounding;
mod quantities;
mod review;
mod submission;
mod text;

pub(crate) use review::{review, unverified_claims};
pub(crate) use submission::{definition, definition_for, repair_request};
pub(crate) use text::is_reference;

#[cfg(test)]
mod tests;

pub const SUBMIT_ANSWER: &str = "submit_answer";
pub const MAX_ANSWER_CHARS: usize = 6_000;
pub const MAX_FOLLOW_UPS: usize = 3;
pub const MAX_FOLLOW_UP_CHARS: usize = 200;
pub const MAX_ACTION_REFS: usize = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Confidence {
    High,
    Medium,
    Low,
}

/// Main topic of an answer, for the interface and the evaluation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Category {
    Memory,
    Cpu,
    Disk,
    Gpu,
    ThermalPower,
    StorageSpace,
    StartupBackground,
    UpdatesSystem,
    SoftwareFault,
    HardwareLimit,
    NoProblemFound,
    InsufficientEvidence,
    Other,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CitationKind {
    Observation,
    Finding,
    /// A web source returned by an external lookup (C6), cited `[W1]`.
    Web,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Citation {
    /// As written in the answer: `O2`, `F1`.
    pub reference: String,
    pub kind: CitationKind,
    /// The local observation ID or finding ID it resolves to.
    pub target: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IssueKind {
    UnknownCitation,
    /// A number without a source in its sentence, not found in any measurement either.
    MissingCitation,
    /// A number without a source in its sentence, but present in a measurement this turn could
    /// cite: shown as is, marked, never sent back for correction (26 Oct 2026 decision).
    UncitedNumber,
    UnsupportedNumber,
    /// The value is in a source measured before this turn, but the sentence presents it as current.
    HistoricalWording,
    UnseenName,
    Command,
    Link,
    Html,
    UnknownAction,
    TooLong,
    Empty,
    /// The arguments do not follow the schema.
    Malformed,
    /// `submit_answer` came before any successful inspection.
    NoInspection,
    /// `submit_answer` shared its turn with other tool calls.
    NotAlone,
    /// The model wrote text instead of calling a tool.
    NoToolCall,
}
impl IssueKind {
    /// What the model must change, sent with the repair request. `{ref}` stands for a reference
    /// the answer may really cite.
    fn fix(self) -> &'static str {
        match self {
            Self::UnknownCitation => "Cite only references listed in valid_references, written like [{ref}].",
            Self::MissingCitation => "Put the source of each number right after the number, before the period, like [{ref}]. Only the references in valid_references exist.",
            Self::UncitedNumber => "Put the source of each number right after the number, before the period, like [{ref}].",
            Self::UnsupportedNumber => "The cited source does not hold this value for the named entity, even rounded or as a simple ratio. Copy the value from the measurement and cite that measurement right after it, or remove the number. Questions and previous answers are not measurements.",
            Self::HistoricalWording => "This value is in the cited source, but that source was measured before this turn. Keep the number and its citation, and rewrite this sentence in the past, tied to the measurement, like 'When it was measured, ... was ... [{ref}].' or 'Lors de la mesure, ... était de ... [{ref}].' Do not write now, currently, still, actuellement, maintenant or toujours in it.",
            Self::UnseenName => "No tool result contains this name. Do not guess program names; use the names or references the tools returned.",
            Self::Command => "Do not give commands to run. Describe the step in words, or propose an action with prepare_action_plan when it is offered.",
            Self::Link => "Remove links and images.",
            Self::Html => "Remove HTML; write plain text and short lists.",
            Self::UnknownAction => "action_refs may only contain plan_id values returned by prepare_action_plan in this investigation.",
            Self::TooLong => "Shorten the answer to at most 6000 characters.",
            Self::Empty => "Write the answer.",
            Self::Malformed => "Follow the declared submit_answer schema: answer, confidence and category are required; action_refs and follow_ups are optional arrays of strings. Do not add undeclared fields.",
            Self::NoInspection => "Call an inspection tool that returns usable measurements before answering. An unsupported or failed inspection, including one awaiting administrator approval, does not count.",
            Self::NotAlone => "Call submit_answer alone, in a turn without other tool calls.",
            Self::NoToolCall => "Use the declared tool calls, not a plain message. Call submit_answer only after an inspection returned usable measurements.",
        }
    }
    /// Marked in the shown answer without sending it back: an unknown action reference is already
    /// dropped from `action_refs`.
    pub(crate) fn mark_only(self) -> bool {
        matches!(self, Self::UnknownAction | Self::UncitedNumber)
    }
    /// A problem of the text itself: the answer can still be shown, marked as unverified.
    pub(crate) fn in_text(self) -> bool {
        !matches!(
            self,
            Self::Malformed | Self::NoInspection | Self::NotAlone | Self::NoToolCall | Self::Empty
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Issue {
    pub kind: IssueKind,
    /// The part of the answer concerned, as written.
    pub excerpt: String,
}
impl Issue {
    pub(crate) fn new(kind: IssueKind, excerpt: impl Into<String>) -> Self {
        Self {
            kind,
            excerpt: excerpt.into(),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Answer {
    /// Written by the model: restricted Markdown in the language of the question, with inline
    /// citations like `[O2]`. Untrusted text: render it as text, never as markup.
    pub text: String,
    pub confidence: Confidence,
    pub category: Category,
    /// Valid citations in order of first appearance.
    pub citations: Vec<Citation>,
    /// Plans prepared during this investigation that the answer recommends.
    pub action_refs: Vec<String>,
    pub follow_ups: Vec<String>,
    /// What the controller could not verify in the final text; empty when everything checked out.
    pub unverified: Vec<Issue>,
    /// How many times an answer or a tool call was sent back to the model for correction.
    pub repairs: usize,
}

/// What the model may cite and what it received, for one check.
pub(crate) struct Context<'a> {
    /// Every payload the model received: the first message and each tool reply, as sent.
    pub received: &'a [Value],
    /// Citable references: (as written, kind, local target).
    pub references: &'a [(String, CitationKind, String)],
    /// `plan_id` values of plans prepared in this investigation.
    pub plans: &'a [String],
}

/// The problems of a submission and, when its text could be checked, the answer to show if
/// the repairs run out.
pub(crate) type Rejection = (Vec<Issue>, Option<Box<Answer>>);
