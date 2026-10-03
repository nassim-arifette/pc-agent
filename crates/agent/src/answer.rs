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
use serde_json::{json, Value};
mod grounding;

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

/// The arguments of `submit_answer`, exactly as the model sent them.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Submission {
    answer: String,
    confidence: Confidence,
    category: Category,
    #[serde(default)]
    action_refs: Vec<String>,
    #[serde(default)]
    follow_ups: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CitationKind {
    Observation,
    Finding,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Citation {
    /// As written in the answer: `O2`, `F1`.
    pub reference: String,
    pub kind: CitationKind,
    /// The local observation ID or finding ID it resolves to.
    pub target: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum IssueKind {
    UnknownCitation,
    MissingCitation,
    UnsupportedNumber,
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
    /// What the model must change, sent with the repair request.
    fn fix(self) -> &'static str {
        match self {
            Self::UnknownCitation => "Cite only references listed in valid_references, written like [O2] or [F1].",
            Self::MissingCitation => "Cite the observation each number comes from right after it, like [O2].",
            Self::UnsupportedNumber => "The citation does not support this value for the named entity, even rounded or as a simple ratio. Cite the matching measurement immediately after the claim, or remove the number. Questions and previous answers are not measurements.",
            Self::UnseenName => "No tool result contains this name. Do not guess program names; use the names or references the tools returned.",
            Self::Command => "Do not give commands to run. Describe the step in words, or propose an action with prepare_action_plan when it is offered.",
            Self::Link => "Remove links and images.",
            Self::Html => "Remove HTML; write plain text and short lists.",
            Self::UnknownAction => "action_refs may only contain plan_id values returned by prepare_action_plan in this investigation.",
            Self::TooLong => "Shorten the answer to at most 6000 characters.",
            Self::Empty => "Write the answer.",
            Self::Malformed => "The arguments do not follow the submit_answer schema: answer, confidence and category are required, and no other field is allowed.",
            Self::NoInspection => "Call at least one inspection tool before answering.",
            Self::NotAlone => "Call submit_answer alone, in a turn without other tool calls.",
            Self::NoToolCall => "Answer by calling submit_answer; do not write the answer as a plain message.",
        }
    }
    /// A problem of the text itself: the answer can still be shown, marked as unverified.
    fn in_text(self) -> bool {
        !matches!(
            self,
            Self::Malformed | Self::NoInspection | Self::NotAlone | Self::NoToolCall | Self::Empty
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
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

pub(crate) fn definition() -> Value {
    json!({"type":"function","function":{"name":SUBMIT_ANSWER,
        "description":"Finish the investigation with your answer. Call it alone, after at least one inspection.",
        "parameters":{"type":"object","additionalProperties":false,
            "required":["answer","confidence","category"],
            "properties":{
                "answer":{"type":"string","maxLength":MAX_ANSWER_CHARS,
                    "description":"The answer in the language of the question: the direct answer first, then what the user can do, then what supports it and what remains uncertain. Plain prose and short lists; cite the source of every number right after it, like [O2] or [F1]."},
                "confidence":{"type":"string","enum":["high","medium","low"],
                    "description":"How well the measurements support the answer."},
                "category":{"type":"string","enum":CATEGORIES,
                    "description":"Main topic of the answer."},
                "action_refs":{"type":"array","maxItems":MAX_ACTION_REFS,"items":{"type":"string"},
                    "description":"plan_id values returned by prepare_action_plan in this investigation that the answer recommends."},
                "follow_ups":{"type":"array","maxItems":MAX_FOLLOW_UPS,
                    "items":{"type":"string","maxLength":MAX_FOLLOW_UP_CHARS},
                    "description":"Up to three short follow-up questions or checks the user could ask next, in the language of the question."}}}}})
}

/// The tool reply that sends an answer back for correction. When a citation is wrong, it lists
/// the references the answer may use.
pub(crate) fn repair_request(
    issues: &[Issue],
    references: &[(String, CitationKind, String)],
) -> Value {
    let problems: Vec<Value> = issues
        .iter()
        .map(|i| json!({"kind":i.kind,"excerpt":i.excerpt,"fix":i.kind.fix()}))
        .collect();
    let mut request = json!({"accepted":false,"problems":problems,
        "next":"Correct only these problems and call submit_answer again with the complete answer."});
    if issues.iter().any(|i| {
        matches!(
            i.kind,
            IssueKind::UnknownCitation | IssueKind::MissingCitation
        )
    }) {
        request["valid_references"] = references.iter().map(|(r, _, _)| r.as_str()).collect();
    }
    request
}

/// The problems of a submission and, when its text could be checked, the answer to show if
/// the repairs run out.
pub(crate) type Rejection = (Vec<Issue>, Option<Box<Answer>>);

/// Parses and checks one submission. `Ok` is an answer without problems.
pub(crate) fn review(arguments: &str, ctx: &Context<'_>) -> Result<Answer, Rejection> {
    let Ok(submission) = serde_json::from_str::<Submission>(arguments) else {
        return Err((vec![Issue::new(IssueKind::Malformed, "")], None));
    };
    let mut issues = Vec::new();
    let mut text = submission.answer.trim().to_owned();
    if text.is_empty() {
        return Err((vec![Issue::new(IssueKind::Empty, "")], None));
    }
    if text.chars().count() > MAX_ANSWER_CHARS {
        issues.push(Issue::new(IssueKind::TooLong, ""));
        text = text.chars().take(MAX_ANSWER_CHARS).collect::<String>() + "…";
    }
    let citations = check_citations(&text, ctx, &mut issues);
    grounding::check_numbers(&text, ctx, &mut issues);
    check_names(&text, ctx, &mut issues);
    check_markup(&text, &mut issues);
    let mut action_refs: Vec<String> = Vec::new();
    for reference in submission.action_refs {
        if !ctx.plans.contains(&reference) {
            issues.push(Issue::new(IssueKind::UnknownAction, reference));
        } else if !action_refs.contains(&reference) && action_refs.len() < MAX_ACTION_REFS {
            action_refs.push(reference);
        }
    }
    let follow_ups = submission
        .follow_ups
        .iter()
        .map(|f| f.trim())
        .filter(|f| {
            let mut probe = Vec::new();
            check_markup(f, &mut probe);
            !f.is_empty() && probe.is_empty()
        })
        .map(|f| {
            if f.chars().count() > MAX_FOLLOW_UP_CHARS {
                f.chars().take(MAX_FOLLOW_UP_CHARS).collect::<String>() + "…"
            } else {
                f.to_owned()
            }
        })
        .take(MAX_FOLLOW_UPS)
        .collect();
    let answer = Answer {
        text,
        confidence: submission.confidence,
        category: submission.category,
        citations,
        action_refs,
        follow_ups,
        unverified: Vec::new(),
        repairs: 0,
    };
    if issues.is_empty() {
        Ok(answer)
    } else {
        let shown = Answer {
            unverified: issues
                .iter()
                .filter(|i| i.kind.in_text())
                .cloned()
                .collect(),
            ..answer
        };
        Err((issues, Some(Box::new(shown))))
    }
}

/// `[O2]`, `[F1]` or `[O2, F1]`; any other bracketed text is ordinary text.
fn check_citations(text: &str, ctx: &Context<'_>, issues: &mut Vec<Issue>) -> Vec<Citation> {
    let mut citations: Vec<Citation> = Vec::new();
    let mut rest = text;
    while let Some(open) = rest.find('[') {
        let after = &rest[open + 1..];
        let Some(close) = after.find(']') else { break };
        let inside = &after[..close];
        rest = &after[close + 1..];
        let tokens: Vec<&str> = inside
            .split([',', ';', ' '])
            .filter(|t| !t.is_empty())
            .collect();
        if tokens.is_empty() || !tokens.iter().all(|t| is_reference(t)) {
            continue;
        }
        for token in tokens {
            match ctx.references.iter().find(|(r, _, _)| r == token) {
                Some((reference, kind, target)) => {
                    if !citations.iter().any(|c| &c.reference == reference) {
                        citations.push(Citation {
                            reference: reference.clone(),
                            kind: *kind,
                            target: target.clone(),
                        });
                    }
                }
                None => issues.push(Issue::new(IssueKind::UnknownCitation, format!("[{token}]"))),
            }
        }
    }
    citations
}

pub(super) fn is_reference(token: &str) -> bool {
    let mut chars = token.chars();
    matches!(chars.next(), Some('O' | 'F'))
        && (2..=6).contains(&token.len())
        && chars.all(|c| c.is_ascii_digit())
}

/// Executable names must appear in what the model received: it cannot know others.
fn check_names(text: &str, ctx: &Context<'_>, issues: &mut Vec<Issue>) {
    let mut names = std::collections::BTreeMap::new();
    for payload in ctx.received {
        crate::privacy::collect_shared_names(payload, &mut names);
    }
    let mut from = 0;
    while let Some(found) = find_ci(&text[from..], ".exe") {
        let dot = from + found;
        let end = dot + 4;
        let start = text[..dot]
            .char_indices()
            .rev()
            .take_while(|(_, c)| c.is_alphanumeric() || matches!(c, '_' | '-' | '.' | '+'))
            .last()
            .map_or(dot, |(i, _)| i);
        let boundary = text[end..]
            .chars()
            .next()
            .is_none_or(|c| !c.is_alphanumeric());
        let name = &text[start..end];
        if start < dot
            && boundary
            && !names.values().any(|known| {
                known.eq_ignore_ascii_case(name)
                    || text[..end].to_lowercase().ends_with(&known.to_lowercase())
            })
        {
            issues.push(Issue::new(IssueKind::UnseenName, name));
        }
        from = end;
    }
}

/// Byte offset of an ASCII needle, ignoring ASCII case. An ASCII match always starts on a
/// character boundary, so the offset can slice the original text.
fn find_ci(text: &str, needle: &str) -> Option<usize> {
    let (hay, needle) = (text.as_bytes(), needle.as_bytes());
    (0..=hay.len().checked_sub(needle.len())?)
        .find(|&i| hay[i..i + needle.len()].eq_ignore_ascii_case(needle))
}

/// Words that start a command line; an inline code span beginning with one is a command.
const COMMANDS: [&str; 40] = [
    "del",
    "erase",
    "rd",
    "rmdir",
    "rm",
    "format",
    "diskpart",
    "reg",
    "bcdedit",
    "sc",
    "net",
    "netsh",
    "taskkill",
    "wmic",
    "powershell",
    "pwsh",
    "cmd",
    "dism",
    "sfc",
    "chkdsk",
    "cleanmgr",
    "cipher",
    "takeown",
    "icacls",
    "attrib",
    "shutdown",
    "schtasks",
    "winget",
    "choco",
    "curl",
    "wget",
    "iwr",
    "irm",
    "iex",
    "msiexec",
    "vssadmin",
    "powercfg",
    "bootrec",
    "fsutil",
    "mklink",
];
const COMMAND_VERBS: [&str; 12] = [
    "remove-",
    "stop-",
    "set-",
    "new-",
    "invoke-",
    "start-",
    "clear-",
    "disable-",
    "enable-",
    "uninstall-",
    "install-",
    "restart-",
];

fn check_markup(text: &str, issues: &mut Vec<Issue>) {
    if let Some(at) = text.find("```") {
        issues.push(Issue::new(IssueKind::Command, excerpt(text, at, 60)));
    } else {
        let mut parts = text.split('`');
        parts.next();
        while let (Some(code), Some(_)) = (parts.next(), parts.clone().next()) {
            parts.next();
            let first = code.split_whitespace().next().unwrap_or("").to_lowercase();
            let first = first.trim_start_matches(".\\").trim_end_matches(".exe");
            if COMMANDS.contains(&first) || COMMAND_VERBS.iter().any(|v| first.starts_with(v)) {
                issues.push(Issue::new(IssueKind::Command, code));
            }
        }
    }
    for marker in ["](", "![", "http://", "https://", "www."] {
        if let Some(at) = find_ci(text, marker) {
            issues.push(Issue::new(IssueKind::Link, excerpt(text, at, 60)));
            break;
        }
    }
    let bytes = text.as_bytes();
    if let Some(at) = (0..bytes.len().saturating_sub(1)).find(|&i| {
        bytes[i] == b'<'
            && (bytes[i + 1].is_ascii_alphabetic() || matches!(bytes[i + 1], b'/' | b'!'))
    }) {
        issues.push(Issue::new(IssueKind::Html, excerpt(text, at, 40)));
    }
}

fn excerpt(text: &str, at: usize, max: usize) -> String {
    text[at..].chars().take(max).collect()
}

/// A physical dimension, in its canonical unit: bytes, percent, milliseconds, °C, MHz, watts, or
/// events per second.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Dim {
    Bytes,
    Percent,
    Millis,
    Celsius,
    Megahertz,
    Watts,
    PerSecond,
    BytesPerSecond,
    Count,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Bound {
    Exact,
    About,
    AtMost,
    AtLeast,
}

/// A number with a unit written in a text.
#[derive(Debug, Clone)]
struct Claim {
    excerpt: String,
    end: usize,
    dim: Dim,
    /// Candidate canonical values with their tolerance: a separator or a byte unit can be read
    /// in two ways (1,234 / 1.234; GB as 10^9 or 2^30).
    values: Vec<(f64, f64)>,
    bound: Bound,
}

/// Relative slack for a number written as measured, and for one introduced by "about".
const EXACT_SHARE: f64 = 0.05;
const ABOUT_SHARE: f64 = 0.15;

/// A quantity found in what the model received.
#[derive(Debug, Clone, Copy)]
struct Quantity {
    dim: Dim,
    value: f64,
    /// A total others can be a share of.
    total: bool,
    /// Installed or physical memory: any memory amount can be stated as a share of it.
    memory_total: bool,
    /// Quantities of one object, measured entity or text: shares are only taken within it.
    group: usize,
}

/// Every quantity the model received, plus the simple derivations an answer may state: the
/// share of a total, its complement and what remains of a total.
struct Known {
    values: Vec<(Dim, f64)>,
}
impl Known {
    fn from_received(received: &[Value]) -> Self {
        let mut found: Vec<Quantity> = Vec::new();
        let mut groups = 0;
        for value in received {
            walk(value, None, 0, &mut groups, &mut found);
        }
        let mut values: Vec<(Dim, f64)> = found.iter().map(|q| (q.dim, q.value)).collect();
        for total in found.iter().filter(|q| q.total && q.value > 0.0) {
            for part in &found {
                let related = part.group == total.group || total.memory_total;
                if part.dim != total.dim || part.value > total.value || !related {
                    continue;
                }
                let share = part.value / total.value * 100.0;
                values.push((Dim::Percent, share));
                values.push((total.dim, total.value - part.value));
            }
        }
        let complements: Vec<(Dim, f64)> = values
            .iter()
            .filter(|(d, v)| *d == Dim::Percent && *v <= 100.0)
            .map(|(_, v)| (Dim::Percent, 100.0 - v))
            .collect();
        values.extend(complements);
        Self { values }
    }
    fn supports(&self, claim: &Claim) -> bool {
        self.values
            .iter()
            .filter(|(dim, _)| *dim == claim.dim)
            .any(|(_, known)| {
                claim.values.iter().any(|(value, tolerance)| {
                    let slack = |share: f64| tolerance.max(share * known.abs()).max(1e-9);
                    match claim.bound {
                        Bound::Exact => (value - known).abs() <= slack(EXACT_SHARE),
                        Bound::About => (value - known).abs() <= slack(ABOUT_SHARE),
                        Bound::AtMost => *known <= value + slack(EXACT_SHARE),
                        Bound::AtLeast => *known >= value - slack(EXACT_SHARE),
                    }
                })
            })
    }
}

/// Collects quantities from a payload. The unit of a number comes from:
/// - its key (`…_bytes`, `…_basis_points`, …);
/// - the unit map of a measurements object;
/// - the `metric` field of a history;
/// - the key of a map whose values are all numbers (`windows_ms`).
///
/// Prose is not measured evidence. Only the facts of a locally computed finding are parsed
/// separately by the source resolver.
fn walk(
    value: &Value,
    unit: Option<&str>,
    group: usize,
    groups: &mut usize,
    out: &mut Vec<Quantity>,
) {
    let mut push = |dim: Dim, value: f64, name: &str, group: usize| {
        let memory = name.contains("physical_total") || name.contains("installed_memory");
        out.push(Quantity {
            dim,
            value,
            total: is_total(name),
            memory_total: memory && is_total(name),
            group,
        });
    };
    match value {
        Value::Object(map) => {
            if map
                .get("measurement_metadata_available")
                .and_then(Value::as_bool)
                == Some(false)
            {
                return;
            }
            if map.contains_key("value")
                && map.get("status").and_then(Value::as_str).is_some_and(|s| {
                    s != "ok"
                        && !(s == "partial"
                            && map
                                .get("measured_samples")
                                .and_then(Value::as_u64)
                                .is_some_and(|count| count > 0))
                })
            {
                return;
            }
            *groups += 1;
            let own = *groups;
            if let (Some(Value::Object(values)), Some(Value::Object(units))) =
                (map.get("values"), map.get("units"))
            {
                for metrics in values.values().filter_map(Value::as_object) {
                    *groups += 1;
                    for (metric, n) in metrics {
                        let unit = units.get(metric).and_then(Value::as_str).unwrap_or(metric);
                        if let (Some(n), Some((dim, factor))) = (n.as_f64(), unit_of(unit)) {
                            push(dim, n * factor, metric, *groups);
                        }
                    }
                }
            }
            let metric = map.get("metric").and_then(Value::as_str);
            let event_unit = if map.contains_key("peak")
                && map.contains_key("started_at_unix_ms")
                && map.contains_key("ended_at_unix_ms")
                && map.contains_key("metric")
            {
                map.get("unit").and_then(Value::as_str)
            } else {
                None
            };
            // C4 inspection records have a closed, typed schema. Their explicit unit applies
            // only to the numeric value of a usable measurement, never to descriptive prose.
            let record_unit = if map.contains_key("entity_ref")
                && map.contains_key("observation_id")
                && map.get("status").and_then(Value::as_str) == Some("ok")
            {
                map.get("unit").and_then(Value::as_str)
            } else {
                None
            };
            let inherit_all = unit.is_some() && map.values().all(Value::is_number);
            for (key, child) in map {
                // A wall-clock timestamp is not a latency or measurement duration.
                if key.ends_with("_unix_ms") {
                    continue;
                }
                if key == "values" && map.contains_key("units") {
                    continue;
                }
                let hint = if key == "value" {
                    record_unit.or(unit)
                } else if key == "peak" {
                    event_unit
                } else if inherit_all || (matches!(key.as_str(), "min" | "max") && unit.is_some()) {
                    unit
                } else if unit_of(key).is_some() {
                    Some(key.as_str())
                } else if key == "points" {
                    metric
                } else {
                    None
                };
                walk(child, hint, own, groups, out);
            }
        }
        Value::Array(items) => {
            for item in items {
                walk(item, unit, group, groups, out);
            }
        }
        Value::Number(n) => {
            if let (Some(n), Some(name)) = (n.as_f64(), unit) {
                if let Some((dim, factor)) = unit_of(name) {
                    push(dim, n * factor, name, group);
                }
            }
        }
        _ => {}
    }
}

/// The dimension and factor to the canonical unit of a payload unit or key name.
fn unit_of(name: &str) -> Option<(Dim, f64)> {
    let n = name.to_ascii_lowercase();
    let is = |unit: &str| n == unit || n.ends_with(&format!("_{unit}"));
    if n.contains("unix_ms") {
        None
    } else if n == "count" {
        Some((Dim::Count, 1.0))
    } else if is("bytes_per_second") {
        Some((Dim::BytesPerSecond, 1.0))
    } else if is("bytes") {
        Some((Dim::Bytes, 1.0))
    } else if n.starts_with("basis_points") || is("basis_points") {
        Some((Dim::Percent, 0.01))
    } else if is("microseconds") {
        Some((Dim::Millis, 0.001))
    } else if is("milliseconds") || is("ms") {
        Some((Dim::Millis, 1.0))
    } else if is("seconds") {
        Some((Dim::Millis, 1_000.0))
    } else if is("celsius") {
        Some((Dim::Celsius, 1.0))
    } else if is("megahertz") {
        Some((Dim::Megahertz, 1.0))
    } else if is("milliwatts") {
        Some((Dim::Watts, 0.001))
    } else if n.contains("per_second_x100") {
        Some((Dim::PerSecond, 0.01))
    } else if is("percent") {
        Some((Dim::Percent, 1.0))
    } else {
        None
    }
}

fn is_total(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    ["total", "limit", "capacity", "installed"]
        .iter()
        .any(|w| n.contains(w))
}

const THOUSANDS_SPACES: [char; 4] = [' ', '\u{a0}', '\u{202f}', '\u{2009}'];

/// Numbers followed by a known unit. Numbers inside identifiers (`F1`, `x86`, `proc-1a2b`) and
/// numbers without a unit (versions, counts, dates) are not claims.
fn claims(text: &str) -> Vec<Claim> {
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let (start, c) = chars[i];
        if !c.is_ascii_digit() {
            i += 1;
            continue;
        }
        let prev = i.checked_sub(1).map(|p| chars[p].1);
        let joined = prev.is_some_and(|p| p.is_alphanumeric() || p == '_')
            || (prev == Some('-') && i >= 2 && chars[i - 2].1.is_alphanumeric());
        // Parse digit groups and their separators.
        let mut groups = vec![String::new()];
        let mut seps: Vec<char> = Vec::new();
        let mut j = i;
        while j < chars.len() {
            let ch = chars[j].1;
            if ch.is_ascii_digit() {
                groups.last_mut().expect("one group").push(ch);
                j += 1;
                continue;
            }
            let next_digits = chars[j + 1..]
                .iter()
                .take_while(|(_, c)| c.is_ascii_digit())
                .count();
            let after = chars.get(j + 1 + next_digits).map(|(_, c)| *c);
            let thousands_space = THOUSANDS_SPACES.contains(&ch)
                && next_digits == 3
                && after.is_none_or(|c| !c.is_ascii_digit());
            if (matches!(ch, '.' | ',') && next_digits > 0) || thousands_space {
                seps.push(ch);
                groups.push(String::new());
                j += 1;
                continue;
            }
            break;
        }
        // An identifier continues after the number (`4060ti`, `v2beta`): not a quantity.
        let glued = chars
            .get(j)
            .is_some_and(|(_, c)| c.is_alphanumeric() && !unit_start(text, chars[j].0));
        if joined || glued {
            i = j.max(i + 1);
            while i < chars.len() && (chars[i].1.is_alphanumeric() || chars[i].1 == '_') {
                i += 1;
            }
            continue;
        }
        let end_number = chars.get(j).map_or(text.len(), |(p, _)| *p);
        let readings = readings(&groups, &seps);
        // The unit: after optional spaces.
        let mut k = j;
        while k < chars.len() && THOUSANDS_SPACES.contains(&chars[k].1) && k < j + 2 {
            k += 1;
        }
        let unit_from = chars.get(k).map_or(text.len(), |(p, _)| *p);
        let unit_text: String = text[unit_from..]
            .chars()
            .take_while(|c| c.is_alphabetic() || matches!(c, '%' | '°' | '/' | 'µ'))
            .collect();
        let unit_end = unit_from + unit_text.len();
        let per_second = unit_text.to_lowercase().ends_with("/s")
            || text[unit_end..].to_lowercase().starts_with(" par seconde")
            || text[unit_end..].to_lowercase().starts_with(" per second");
        if let Some((dim, factors)) = unit(&unit_text, per_second) {
            let bound = bound_before(&text[..start]);
            let values = readings
                .iter()
                .flat_map(|(value, decimals)| {
                    factors
                        .iter()
                        .map(move |f| (value * f, 0.5 * 10f64.powi(-(*decimals as i32)) * f))
                })
                .collect();
            out.push(Claim {
                excerpt: text[start..unit_end.max(end_number)].trim().to_owned(),
                end: unit_end.max(end_number),
                dim,
                values,
                bound,
            });
            i = k.max(j) + unit_text.chars().count();
        } else {
            i = j.max(i + 1);
        }
    }
    out
}

/// Whether a unit starts at this byte offset, so that `3Go` still reads as a quantity.
fn unit_start(text: &str, at: usize) -> bool {
    let word: String = text[at..]
        .chars()
        .take_while(|c| c.is_alphabetic() || matches!(c, '%' | '°' | '/' | 'µ'))
        .collect();
    unit(&word, word.to_lowercase().ends_with("/s")).is_some()
}

/// Possible values of a written number with their count of decimals.
fn readings(groups: &[String], seps: &[char]) -> Vec<(f64, usize)> {
    let digits = |gs: &[String]| gs.concat();
    let parse = |s: &str| s.parse::<f64>().ok();
    let Some(&last) = seps.last() else {
        return parse(&groups[0]).map(|v| vec![(v, 0)]).unwrap_or_default();
    };
    let integer = parse(&digits(groups)).map(|v| (v, 0));
    if THOUSANDS_SPACES.contains(&last) {
        return integer.into_iter().collect();
    }
    let tail = groups.last().expect("a group per separator");
    let head = &groups[..groups.len() - 1];
    let decimal = parse(&format!("{}.{}", digits(head), tail)).map(|v| (v, tail.len()));
    let same = seps.iter().filter(|&&s| s == last).count();
    // "1,234" or "1.234": thousands or three decimals. "1,5" or "12.25": decimals. "1.234.567":
    // thousands.
    let grouped =
        tail.len() == 3 && head.iter().skip(1).all(|g| g.len() == 3) && head[0].len() <= 3;
    match (grouped, same > 1) {
        (true, true) => integer.into_iter().collect(),
        (true, false) => integer.into_iter().chain(decimal).collect(),
        (false, _) => decimal.into_iter().collect(),
    }
}

const BINARY: [f64; 5] = [
    1.0,
    1024.0,
    1_048_576.0,
    1_073_741_824.0,
    1_099_511_627_776.0,
];
const DECIMAL: [f64; 5] = [1.0, 1e3, 1e6, 1e9, 1e12];

/// The dimension of a written unit and its factors to the canonical unit.
fn unit(written: &str, per_second: bool) -> Option<(Dim, Vec<f64>)> {
    let lower = written.to_lowercase();
    let base = lower.trim_end_matches("/s");
    let bytes = |power: usize| vec![DECIMAL[power], BINARY[power]];
    let byte_unit = match base {
        "o" | "b" | "octet" | "octets" | "byte" | "bytes" => Some(vec![1.0]),
        "ko" | "kb" | "kio" | "kib" => Some(bytes(1)),
        "mo" | "mb" | "mio" | "mib" => Some(bytes(2)),
        "go" | "gb" | "gio" | "gib" => Some(bytes(3)),
        "to" | "tb" | "tio" | "tib" => Some(bytes(4)),
        _ => None,
    };
    if per_second {
        return match (byte_unit, base) {
            (Some(factors), _) => Some((Dim::BytesPerSecond, factors)),
            (None, "" | "page" | "pages" | "e" | "fautes" | "faults") => {
                Some((Dim::PerSecond, vec![1.0]))
            }
            _ => None,
        };
    }
    if let Some(factors) = byte_unit {
        return Some((Dim::Bytes, factors));
    }
    let one = |dim: Dim, factor: f64| Some((dim, vec![factor]));
    match lower.as_str() {
        "%" => one(Dim::Percent, 1.0),
        "ms" => one(Dim::Millis, 1.0),
        "µs" | "us" => one(Dim::Millis, 0.001),
        "s" | "sec" | "seconde" | "secondes" | "second" | "seconds" => one(Dim::Millis, 1_000.0),
        "min" | "minute" | "minutes" => one(Dim::Millis, 60_000.0),
        "°c" | "°" | "degrés" | "degres" | "degrees" => one(Dim::Celsius, 1.0),
        "mhz" => one(Dim::Megahertz, 1.0),
        "ghz" => one(Dim::Megahertz, 1_000.0),
        "w" | "watt" | "watts" => one(Dim::Watts, 1.0),
        "mw" => one(Dim::Watts, 0.001),
        "processes" | "processus" | "services" | "applications" | "apps" | "programs"
        | "programmes" | "slots" | "emplacements" | "modules" | "files" | "fichiers"
        | "folders" | "dossiers" | "entries" | "entrées" => one(Dim::Count, 1.0),
        _ => None,
    }
}

fn ends_with_word(before: &str, words: &[&str]) -> bool {
    let lower = before.trim_end().to_lowercase();
    words.iter().any(|w| {
        lower.ends_with(w)
            && lower[..lower.len() - w.len()]
                .chars()
                .next_back()
                .is_none_or(|c| !c.is_alphanumeric())
    })
}

fn bound_before(before: &str) -> Bound {
    const AT_MOST: [&str; 14] = [
        "moins de",
        "less than",
        "under",
        "below",
        "au plus",
        "at most",
        "inférieur à",
        "inférieure à",
        "sous",
        "<",
        "≤",
        "jusqu'à",
        "up to",
        "max",
    ];
    const AT_LEAST: [&str; 14] = [
        "plus de",
        "more than",
        "over",
        "above",
        "au moins",
        "at least",
        "supérieur à",
        "supérieure à",
        ">",
        "≥",
        "dépasse",
        "exceeds",
        "beyond",
        "au-delà de",
    ];
    const ABOUT: [&str; 14] = [
        "environ",
        "about",
        "around",
        "approximately",
        "approx.",
        "~",
        "≈",
        "près de",
        "roughly",
        "nearly",
        "presque",
        "almost",
        "autour de",
        "de l'ordre de",
    ];
    if ends_with_word(before, &AT_MOST) {
        Bound::AtMost
    } else if ends_with_word(before, &AT_LEAST) {
        Bound::AtLeast
    } else if ends_with_word(before, &ABOUT) {
        Bound::About
    } else {
        Bound::Exact
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn received() -> Vec<Value> {
        vec![
            json!({"question":"Pourquoi mon PC rame ? J'ai 8 Go de RAM.",
                "initial_observation":{"observation_id":"O0","measurements":{
                    "values":{"system":{"physical_total_bytes":17_179_869_184_u64,
                        "physical_available_bytes":1_653_562_163_u64,
                        "cpu_busy_basis_points":1_740,
                        "disk_read_latency_microseconds":1_300},
                        "proc-1a2b3c4d":{"process_working_set_bytes":2_480_000_000_u64}},
                    "units":{"physical_total_bytes":"bytes","physical_available_bytes":"bytes",
                        "cpu_busy_basis_points":"basis_points_of_total_cpu",
                        "disk_read_latency_microseconds":"microseconds",
                        "process_working_set_bytes":"bytes"}}},
                "process_names":{"proc-1a2b3c4d":"chrome.exe"},
                "findings":[{"id":"F1","facts":["Hard-fault page reads: 12.4 pages/s."]}]}),
            json!({"observation_id":"O1","metric":"cpu_busy_basis_points",
                "points":[{"value":500,"status":"ok"},{"value":900,"status":"ok"}],
                "observed_interval_ms":2000,
                "installed_memory_bytes":{"value":17_179_869_184_u64,"status":"ok"},
                "volume":{"free_bytes":64_000_000_000_u64,"total_bytes":512_000_000_000_u64},
                "items":[{"name":"chrome.exe"}]}),
        ]
    }
    fn refs() -> Vec<(String, CitationKind, String)> {
        vec![
            (
                "O0".into(),
                CitationKind::Observation,
                "initial-real".into(),
            ),
            ("O1".into(), CitationKind::Observation, "obs-real".into()),
            ("F1".into(), CitationKind::Finding, "finding-real".into()),
        ]
    }
    fn submit(text: &str) -> String {
        json!({"answer":text,"confidence":"medium","category":"memory"}).to_string()
    }
    fn check(text: &str) -> Result<Answer, Rejection> {
        let received = received();
        let references = refs();
        let plans = vec!["plan-1".to_owned()];
        review(
            &submit(text),
            &Context {
                received: &received,
                references: &references,
                plans: &plans,
            },
        )
    }
    fn kinds(text: &str) -> Vec<IssueKind> {
        match check(text) {
            Ok(_) => Vec::new(),
            Err((issues, _)) => issues.into_iter().map(|i| i.kind).collect(),
        }
    }

    #[test]
    fn grounding_a_value_from_another_observation_does_not_support_a_claim() {
        let received = vec![
            json!({"observation_id":"O1", "measurements":{
                "values":{"proc-a":{"process_working_set_bytes":2_000_000_000_u64}},
                "units":{"process_working_set_bytes":"bytes"}}}),
            json!({"observation_id":"O2", "measurements":{
                "values":{"proc-b":{"process_working_set_bytes":500_000_000}},
                "units":{"process_working_set_bytes":"bytes"}}}),
        ];
        let references = vec![
            ("O1".into(), CitationKind::Observation, "first".into()),
            ("O2".into(), CitationKind::Observation, "second".into()),
        ];
        let result = review(
            &submit("proc-b utilise 2 Go [O2]."),
            &Context {
                received: &received,
                references: &references,
                plans: &[],
            },
        );
        assert!(
            matches!(result, Err((ref issues, _)) if issues.iter().any(|i| i.kind == IssueKind::UnsupportedNumber)),
            "a citation cannot borrow a value from a different observation: {result:?}"
        );
    }

    #[test]
    fn grounding_named_processes_and_metrics_cannot_borrow_other_values() {
        let received = vec![json!({"observation_id":"O1", "measurements":{
            "values":{"proc-a":{"process_working_set_bytes":2_000_000_000_u64},
                "proc-b":{"process_working_set_bytes":500_000_000},
                "system":{"cpu_busy_basis_points":1_700},
                "gpu-nvidia-0":{"gpu_utilization_basis_points":7_200}},
            "units":{"process_working_set_bytes":"bytes", "cpu_busy_basis_points":"basis_points", "gpu_utilization_basis_points":"basis_points"}},
            "process_names":{"proc-a":"alpha.exe", "proc-b":"beta.exe"}})];
        let references = refs();
        let context = Context {
            received: &received,
            references: &references,
            plans: &[],
        };
        for text in ["beta.exe uses 2 GB [O1].", "Le CPU est occupé à 72 % [O1]."] {
            assert!(
                matches!(review(&submit(text), &context), Err((ref issues, _)) if issues.iter().any(|i| i.kind == IssueKind::UnsupportedNumber)),
                "{text}"
            );
        }
        for text in [
            "beta.exe uses 500 MB [O1].",
            "Le CPU est occupé à 17 % [O1].",
            "Le GPU est occupé à 72 % [O1].",
        ] {
            assert!(review(&submit(text), &context).is_ok(), "{text}");
        }
    }

    #[test]
    fn grounding_finding_facts_keep_their_process_and_metric() {
        let received = vec![json!({"findings":[{"id":"F1", "facts":[
            "CPU busy: 17 %.", "GPU utilization: 72 %.",
            "alpha.exe used 2 GB.", "beta.exe used 500 MB."
        ]}], "process_names":{"proc-a":"alpha.exe", "proc-b":"beta.exe"}})];
        let references = refs();
        let context = Context {
            received: &received,
            references: &references,
            plans: &[],
        };
        for text in ["Le CPU est à 72 % [F1].", "beta.exe uses 2 GB [F1]."] {
            assert!(
                matches!(review(&submit(text), &context), Err((ref issues, _)) if issues.iter().any(|i| i.kind == IssueKind::UnsupportedNumber)),
                "{text}"
            );
        }
        for text in ["Le CPU est à 17 % [F1].", "beta.exe uses 500 MB [F1]."] {
            assert!(review(&submit(text), &context).is_ok(), "{text}");
        }
    }

    #[test]
    fn grounding_question_text_is_not_a_measurement() {
        let mut received = received();
        received[0]["question"] = json!("Est-ce que le PC utilise 99 Go ?");
        let references = refs();
        let result = review(
            &submit("Le PC utilise 99 Go [O1]."),
            &Context {
                received: &received,
                references: &references,
                plans: &[],
            },
        );
        assert!(
            matches!(result, Err((ref issues, _)) if issues.iter().any(|i| i.kind == IssueKind::UnsupportedNumber)),
            "a question cannot establish a measured value: {result:?}"
        );
    }

    #[test]
    fn grounding_a_citation_in_another_sentence_is_insufficient() {
        assert!(
            kinds("La mémoire est presque pleine [O1]. Le CPU est à 17,4 %.")
                .contains(&IssueKind::MissingCitation)
        );
    }

    #[test]
    fn measured_numbers_pass_in_either_language_and_unit_convention() {
        for text in [
            "Il reste 1,54 Go de mémoire disponible sur 16 Go [O0].",
            "Only 1.54 GiB of 16 GiB is available [O0].",
            "Available memory: 1.65 GB [O0], about 1.5 GB.",
            "Le processeur est occupé à 17,4 % [O0].",
            "Memory is 90 % in use [O0].",
            "Used memory is 14.46 GiB [O0].",
            "Le disque répond en 1,3 ms [O0], sous 3 ms.",
            "Hard faults: 12.4 pages/s [F1].",
            "CPU use varied between 5 % and 9 % over 2 s [O1].",
            "The largest process uses 2.48 GB [O0], about 14 % of the memory.",
            "C: has 64 GB free of 512 GB [O1], 87.5 % full.",
            "The PC reports 16 Go [O0].",
            "chrome.exe uses 2.3 GiB [O0].",
        ] {
            assert_eq!(kinds(text), Vec::<IssueKind>::new(), "{text}");
        }
    }

    #[test]
    fn invented_numbers_and_missing_citations_are_reported() {
        assert_eq!(
            kinds("Windows needs 32 GB [O1]."),
            [IssueKind::UnsupportedNumber]
        );
        assert_eq!(
            kinds("The CPU runs at 3.2 GHz [O1]."),
            [IssueKind::UnsupportedNumber]
        );
        assert_eq!(
            kinds("La température atteint 85 °C [O1]."),
            [IssueKind::UnsupportedNumber]
        );
        assert_eq!(
            kinds("Memory is 90 % in use."),
            [IssueKind::MissingCitation]
        );
        // Counts, versions and identifiers carry no unit and are not checked.
        assert!(kinds("Windows 11 on an RTX 4060 shows proc-1a2b3c4d [O1].").is_empty());
    }

    #[test]
    fn citations_must_exist_and_follow_the_short_form() {
        assert_eq!(kinds("See [O9]."), [IssueKind::UnknownCitation]);
        assert_eq!(kinds("See [O1, F7]."), [IssueKind::UnknownCitation]);
        let answer = check("Mémoire [F1] et processus [O1] ; voir [la note].").unwrap();
        assert_eq!(
            answer
                .citations
                .iter()
                .map(|c| c.target.as_str())
                .collect::<Vec<_>>(),
            ["finding-real", "obs-real"]
        );
    }

    #[test]
    fn names_must_have_been_received_and_markup_is_refused() {
        assert_eq!(kinds("Close steam.exe [O1]."), [IssueKind::UnseenName]);
        assert_eq!(kinds("Run `sfc /scannow` [O1]."), [IssueKind::Command]);
        assert_eq!(
            kinds("Run `Remove-Item C:\\temp` [O1]."),
            [IssueKind::Command]
        );
        assert!(kinds("The key `Run` lists startup programs [O1].").is_empty());
        assert_eq!(kinds("```\ndel /s\n``` [O1]"), [IssueKind::Command]);
        assert_eq!(kinds("See [docs](https://x.test) [O1]."), [IssueKind::Link]);
        assert_eq!(kinds("<b>Low memory</b> [O1]."), [IssueKind::Html]);
        assert!(kinds("Latency stays < 3 ms [O0].").is_empty());
    }

    #[test]
    fn schema_and_action_references_are_enforced() {
        let received = received();
        let references = refs();
        let plans = vec!["plan-1".to_owned()];
        let ctx = Context {
            received: &received,
            references: &references,
            plans: &plans,
        };
        let malformed = review(
            r#"{"answer":"x","confidence":"sure","category":"memory"}"#,
            &ctx,
        );
        assert!(matches!(malformed, Err((ref i, None)) if i[0].kind == IssueKind::Malformed));
        let extra = review(
            r#"{"answer":"x","confidence":"low","category":"memory","verdict":"ok"}"#,
            &ctx,
        );
        assert!(extra.is_err());
        let args =
            json!({"answer":"Free space [O1].","confidence":"low","category":"storage_space",
            "action_refs":["plan-1","plan-9"],
            "follow_ups":["  Et le disque D: ?  ","See https://x.test",""]})
            .to_string();
        let Err((issues, Some(shown))) = review(&args, &ctx) else {
            panic!("an unknown plan is a problem");
        };
        assert_eq!(issues[0].kind, IssueKind::UnknownAction);
        assert_eq!(shown.action_refs, ["plan-1"]);
        assert_eq!(shown.follow_ups, ["Et le disque D: ?"]);
        assert_eq!(shown.unverified.len(), 1);
    }

    #[test]
    fn repair_requests_explain_each_problem() {
        let request = repair_request(
            &[Issue::new(IssueKind::UnsupportedNumber, "32 GB")],
            &refs(),
        );
        assert_eq!(request["accepted"], false);
        assert_eq!(request["problems"][0]["kind"], "unsupported_number");
        assert_eq!(request["problems"][0]["excerpt"], "32 GB");
        assert!(request["problems"][0]["fix"]
            .as_str()
            .unwrap()
            .contains("citation"));
        assert!(request.get("valid_references").is_none());
        let request = repair_request(&[Issue::new(IssueKind::UnknownCitation, "[O9]")], &refs());
        assert_eq!(request["valid_references"], json!(["O0", "O1", "F1"]));
    }

    #[test]
    fn written_numbers_read_like_a_person_writes_them() {
        let values = |text: &str| -> Vec<f64> {
            claims(text)
                .into_iter()
                .flat_map(|c| c.values.into_iter().map(|(v, _)| v))
                .collect()
        };
        assert_eq!(values("1 234 ms"), [1_234.0]);
        assert_eq!(values("1\u{202f}234 ms"), [1_234.0]);
        assert_eq!(values("1,5 ms"), [1.5]);
        assert_eq!(values("1,234 ms"), [1_234.0, 1.234]);
        assert_eq!(values("12.25 %"), [12.25]);
        assert_eq!(values("3Go"), [3e9, 3.0 * 1_073_741_824.0]);
        assert!(values("x86 and F1 and 2026 and version 2.1").is_empty());
        assert_eq!(claims("moins de 3 ms")[0].bound, Bound::AtMost);
        assert_eq!(claims("plus de 2 Go")[0].bound, Bound::AtLeast);
        assert_eq!(claims("environ 80 %")[0].bound, Bound::About);
    }

    #[test]
    fn activity_rates_growth_and_gpu_use_the_matching_measured_dimension() {
        let received = vec![
            json!({"observation_id":"O1", "process_names":{"proc-a":"browser.exe"},
            "measurements":{"values":{"proc-a":{
                "process_working_set_bytes":2_000_000_000_u64,
                "process_working_set_increase_bytes":100_000_000,
                "process_working_set_decrease_bytes":20_000_000,
                "process_io_read_bytes_per_second":5_000_000,
                "process_gpu_basis_points":4200,
                "page_reads_per_second_x100":1_700_000_000_u64}},
            "units":{"process_working_set_bytes":"bytes", "process_working_set_increase_bytes":"bytes",
                "process_working_set_decrease_bytes":"bytes", "process_io_read_bytes_per_second":"bytes_per_second",
                "process_gpu_basis_points":"basis_points_of_engine_time", "page_reads_per_second_x100":"pages_per_second_x100"}}}),
        ];
        let references = refs();
        let ctx = Context {
            received: &received,
            references: &references,
            plans: &[],
        };
        for text in [
            "browser.exe disk I/O: 5 MB/s [O1].",
            "browser.exe memory increased by 100 MB [O1].",
            "browser.exe memory decreased by 20 MB [O1].",
            "browser.exe GPU activity: 42 % [O1].",
        ] {
            assert!(
                review(&submit(text), &ctx).is_ok(),
                "valid activity claim rejected: {text}"
            );
        }
        for text in [
            "browser.exe disk I/O: 17 MB/s [O1].",
            "browser.exe memory increased by 2 GB [O1].",
            "browser.exe memory decreased by 100 MB [O1].",
            "browser.exe memory: 100 MB [O1].",
        ] {
            assert!(
                review(&submit(text), &ctx).is_err(),
                "unmeasured activity claim accepted: {text}"
            );
        }
    }

    #[test]
    fn cpu_and_gpu_sensors_cannot_borrow_each_others_values_or_firmware_temperatures() {
        let readings = [
            ("cpu_frequency_megahertz", 3600, "megahertz"),
            ("cpu_temperature_celsius", 45, "degrees_celsius"),
            ("cpu_power_milliwatts", 65_000, "milliwatts"),
            ("gpu_sm_clock_megahertz", 2100, "megahertz"),
            ("gpu_temperature_celsius", 80, "degrees_celsius"),
            ("gpu_power_milliwatts", 120_000, "milliwatts"),
            ("thermal_zone_temperature_celsius", 65, "degrees_celsius"),
        ];
        let observations: Vec<_> = readings.iter().map(|(metric, value, unit)| json!({
            "observation_id":format!("measurement-{metric}"), "entity_ref":"system", "metric":metric,
            "value":value, "unit":unit, "status":"ok", "source":"fixture",
        })).collect();
        let received = vec![json!({"observation_id":"O1", "observations":observations})];
        let references = refs();
        let ctx = Context {
            received: &received,
            references: &references,
            plans: &[],
        };
        for text in [
            "The CPU frequency is 3.6 GHz [O1].",
            "Le processeur est à 45 °C [O1].",
            "Processor power: 65 W [O1].",
            "GPU frequency: 2100 MHz [O1].",
            "La carte graphique est à 80 °C [O1].",
            "GPU power: 120 W [O1].",
            "The firmware thermal zone reports 65 °C [O1].",
        ] {
            assert!(
                review(&submit(text), &ctx).is_ok(),
                "measured sensor claim rejected: {text}"
            );
        }
        for text in [
            "CPU frequency: 2.1 GHz [O1].",
            "CPU temperature: 80 °C [O1].",
            "Le processeur est à 65 °C [O1].",
            "Processor power: 120 W [O1].",
            "GPU frequency: 3.6 GHz [O1].",
            "GPU temperature: 45 °C [O1].",
            "La carte graphique est à 65 °C [O1].",
            "GPU power: 65 W [O1].",
            "CPU and GPU temperature: 80 °C [O1].",
        ] {
            let (issues, _) = review(&submit(text), &ctx).unwrap_err();
            assert!(
                issues
                    .iter()
                    .any(|issue| issue.kind == IssueKind::UnsupportedNumber),
                "unrelated sensor supplied the claim: {text}"
            );
        }
    }

    #[test]
    fn c4_rows_require_usable_values_matching_the_named_entity() {
        let row = |entity: &str, value: u64, status: &str| {
            json!({
                "observation_id":"measurement-1", "entity_ref":entity, "metric":"estimated_bytes",
                "value":value, "unit":"bytes", "status":status, "source":"fixture", "measured_at_unix_ms":1,
            })
        };
        let received = vec![
            json!({"observation_id":"O1", "shared_names":{"app-a":"Editor", "app-b":"Player"},
            "observations":[row("app-a", 2_000_000_000, "ok"), row("app-b", 8_000_000_000, "denied")],
            "volume":{"free_bytes":12_000_000_000_u64}}),
        ];
        let references = refs();
        let ctx = Context {
            received: &received,
            references: &references,
            plans: &[],
        };
        assert!(review(&submit("Editor uses 2 GB [O1]."), &ctx).is_ok());
        for text in [
            "Player uses 8 GB [O1].",
            "Player uses 2 GB [O1].",
            "Editor uses 12 GB [O1].",
        ] {
            assert!(
                review(&submit(text), &ctx).is_err(),
                "unsupported C4 claim accepted: {text}"
            );
        }
        let received = vec![json!({"observation_id":"O1", "observations":[{
            "observation_id":"measurement-date", "entity_ref":"system", "metric":"last_update", "value":1234,
            "unit":"unix_ms", "status":"ok", "source":"fixture"}]})];
        let ctx = Context {
            received: &received,
            references: &references,
            plans: &[],
        };
        assert!(review(&submit("The delay was 1234 ms [O1]."), &ctx).is_err());
    }

    #[test]
    fn application_name_uses_its_aggregate_and_explicit_process_refs_take_priority() {
        let received = vec![json!({"observation_id":"O1",
            "process_names":{"proc-a":"browser.exe", "proc-b":"browser.exe"},
            "shared_names":{"app-a":"browser.exe"},
            "applications":[{"entity_ref":"app-a", "process_refs":["proc-a","proc-b"]}],
            "measurements":{"values":{"proc-a":{"process_working_set_bytes":1_000_000_000_u64},
                "proc-b":{"process_working_set_bytes":2_000_000_000_u64},
                "app-a":{"process_working_set_bytes":3_000_000_000_u64}},
                "units":{"process_working_set_bytes":"bytes"}}})];
        let references = refs();
        let ctx = Context {
            received: &received,
            references: &references,
            plans: &[],
        };
        for text in [
            "browser.exe uses 3 GB [O1].",
            "browser.exe process proc-a uses 1 GB [O1].",
        ] {
            assert!(
                review(&submit(text), &ctx).is_ok(),
                "aggregate scoping rejected: {text}"
            );
        }
        assert!(review(&submit("browser.exe uses 1 GB [O1]."), &ctx).is_err());
    }

    #[test]
    fn persisted_history_extrema_and_event_peaks_retain_units_and_entity_scope() {
        let received = vec![
            json!({"observation_id":"O1", "historical":true, "metric":"process_cpu_basis_points", "entity_ref":"proc-a",
            "process_names":{"proc-a":"browser.exe", "proc-b":"other.exe"},
            "points":[{"value":5000,"min":1700,"max":8700,"status":"partial","measured_samples":3,"expected_samples":5}],
            "events":[{"metric":"cpu_busy_basis_points","unit":"basis_points","entity_ref":"system",
                "peak":9900,"started_at_unix_ms":1000,"ended_at_unix_ms":5000,
                "contributor_ref":"proc-b","contributor_value":3600,
                "contributor_metric":"process_cpu_basis_points","contributor_unit":"basis_points_of_machine_time"}]}),
        ];
        let references = refs();
        let ctx = Context {
            received: &received,
            references: &references,
            plans: &[],
        };
        for text in [
            "During recording browser.exe CPU peak: 87 % [O1].",
            "Recorded browser.exe CPU average: 50 % [O1].",
            "At 14:00 browser.exe CPU peak: 87 % [O1].",
            "À 14 h browser.exe : pic CPU de 87 % [O1].",
            "System CPU peak during recording: 99 % [O1].",
            "During recording other.exe CPU: 36 % [O1].",
        ] {
            assert!(
                review(&submit(text), &ctx).is_ok(),
                "measured history rejected: {text}"
            );
        }
        for text in [
            "During recording browser.exe CPU peak: 99 % [O1].",
            "During recording other.exe CPU peak: 99 % [O1].",
            "During recording other.exe CPU peak: 87 % [O1].",
            "During recording browser.exe GPU peak: 87 % [O1].",
            "browser.exe CPU average now: 50 % [O1].",
            "During recording browser.exe CPU is still 50 % [O1].",
        ] {
            assert!(
                review(&submit(text), &ctx).is_err(),
                "history borrowed unrelated measurement: {text}"
            );
        }
    }

    #[test]
    fn historical_event_values_without_measurement_metadata_remain_unverified() {
        let mut received = vec![json!({"observation_id":"O1","historical":true,
            "process_names":{"proc-b":"other.exe"},
            "events":[{"metric":"cpu_busy_basis_points","unit":"basis_points","entity_ref":"system",
                "peak":9900,"started_at_unix_ms":1000,"ended_at_unix_ms":5000,
                "contributor_ref":"proc-b","contributor_value":3600,
                "contributor_metric":"process_cpu_basis_points","contributor_unit":"basis_points",
                "measurement_metadata_available":false,"peak_observation":null,"contributor_observation":null}]})];
        let references = refs();
        for text in [
            "System CPU peak during recording: 99 % [O1].",
            "During recording other.exe CPU: 36 % [O1].",
        ] {
            let context = Context {
                received: &received,
                references: &references,
                plans: &[],
            };
            assert!(review(&submit(text), &context).is_err(), "{text}");
        }
        received[0]["events"][0]["measurement_metadata_available"] = json!(true);
        received[0]["events"][0]["peak_observation"] = json!({"observation_id":"event-peak","entity_ref":"system",
            "metric":"cpu_busy_basis_points","unit":"basis_points","value":9900,"status":"ok"});
        received[0]["events"][0]["contributor_observation"] = json!({"observation_id":"event-contributor","entity_ref":"proc-b",
            "metric":"process_cpu_basis_points","unit":"basis_points","value":3600,"status":"ok"});
        for text in [
            "System CPU peak during recording: 99 % [O1].",
            "During recording other.exe CPU: 36 % [O1].",
        ] {
            let context = Context {
                received: &received,
                references: &references,
                plans: &[],
            };
            assert!(review(&submit(text), &context).is_ok(), "{text}");
        }
        let context = Context {
            received: &received,
            references: &references,
            plans: &[],
        };
        assert!(review(
            &submit("During recording other.exe CPU: 99 % [O1]."),
            &context
        )
        .is_err());
    }
}
