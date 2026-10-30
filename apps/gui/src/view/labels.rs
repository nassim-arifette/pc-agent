//! Local process naming, reference substitution and finding presentation.
use pcagent_diagnostics::{Classification, Finding, Support};
use pcagent_shared::{Phrase, ProcessInstance, Quantity};
use serde::Serialize;

/// The executable name read for this process, if one was.
pub(super) fn read_name(p: &ProcessInstance) -> Option<&str> {
    p.executable_name
        .as_deref()
        .map(str::trim)
        .filter(|n| !n.is_empty())
}
pub fn process_name(p: &ProcessInstance) -> String {
    read_name(p).unwrap_or("Unnamed process").to_owned()
}

/// Readable program names by process instance (PID, creation time): the description or product
/// name of the executable's version resource, as Task Manager shows them ("Google Chrome"), read
/// on a worker thread (`pcagent_telemetry::ProgramNames`). They are for this window only: never
/// part of a model payload, a preview, a question or the conversation context (C5, note of 26
/// October), which keep executable names or opaque references.
pub type Readable<'a> = &'a dyn Fn(u32, u64) -> Option<String>;

/// For views built without readable names (a turn that failed before measuring anything).
pub fn no_readable_names(_pid: u32, _created: u64) -> Option<String> {
    None
}

/// The readable name the window shows for a process, when one is known and says more than its
/// executable name.
pub(super) fn readable_name(p: &ProcessInstance, readable: Readable<'_>) -> Option<String> {
    let name = readable(p.pid, p.creation_time_filetime)?;
    let name = name.trim();
    let same = p
        .executable_name
        .as_deref()
        .is_some_and(|exe| exe.trim().eq_ignore_ascii_case(name));
    (!name.is_empty() && !same).then(|| name.to_owned())
}

/// How the window names a process: its readable name, else its executable name.
pub(super) fn display_name(p: &ProcessInstance, readable: Readable<'_>) -> String {
    readable_name(p, readable).unwrap_or_else(|| process_name(p))
}

#[derive(Debug, Clone, Serialize)]
pub struct FindingView {
    pub classification: Classification,
    pub label: &'static str,
    pub support: Support,
    pub support_reason: String,
    pub summary: String,
    pub facts: Vec<String>,
    pub hypothesis: Option<String>,
    pub counter_evidence: Vec<String>,
    pub missing: Vec<String>,
    pub next_step: String,
    pub limitations: Vec<String>,
    /// The displayed sentences as closed keys with their measured values, for the window's
    /// language: the summary, each fact and each limitation in the same order, the next step.
    /// `None` where only the English text exists; reports saved before have none at all.
    pub summary_phrase: Option<Phrase>,
    pub fact_phrases: Vec<Option<Phrase>>,
    pub next_step_phrase: Option<Phrase>,
    pub limitation_phrases: Vec<Option<Phrase>>,
    pub support_reason_phrase: Option<Phrase>,
    pub hypothesis_phrase: Option<Phrase>,
    pub counter_phrases: Vec<Option<Phrase>>,
    pub missing_phrases: Vec<Option<Phrase>>,
}
/// Whether `reference` stands at byte `at` of `text` as a whole word.
pub(super) fn reference_at(text: &str, at: usize, reference: &str) -> bool {
    text[at..].starts_with(reference)
        && text[..at]
            .chars()
            .next_back()
            .is_none_or(|c| !(c.is_alphanumeric() || c == '-' || c == '_'))
        && text[at + reference.len()..]
            .chars()
            .next()
            .is_none_or(|c| !c.is_alphanumeric())
}
/// A phrase as this window shows it: a process reference among its values becomes that process,
/// named like `localize` names it in the text beside it (its readable name, "Google Chrome",
/// when one is known). Display only: payloads keep executable names or opaque references.
pub(super) fn local_phrase(
    phrase: &Phrase,
    processes: &[ProcessInstance],
    readable: Readable<'_>,
) -> Phrase {
    let mut phrase = phrase.clone();
    for value in phrase.values.values_mut() {
        let Quantity::Entity(reference) = value else {
            continue;
        };
        if let Some(p) = processes.iter().find(|p| p.entity_ref == *reference) {
            *value = Quantity::Process {
                name: readable_name(p, readable).or_else(|| read_name(p).map(str::to_owned)),
                pid: p.pid,
            };
        }
    }
    phrase
}
/// The phrases of a finding's facts, in their order.
pub(super) fn fact_phrases(
    f: &Finding,
    processes: &[ProcessInstance],
    readable: Readable<'_>,
) -> Vec<Option<Phrase>> {
    f.facts
        .iter()
        .map(|fact| {
            fact.phrase
                .as_ref()
                .map(|p| local_phrase(p, processes, readable))
        })
        .collect()
}

pub(super) fn cites(text: &str, reference: &str) -> bool {
    text.match_indices(reference)
        .any(|(at, _)| reference_at(text, at, reference))
}

/// One process cited in a text: its reference, its executable name as the model may have written
/// it before the reference, and how the window names it there.
pub(super) struct Label {
    reference: String,
    written: String,
    label: String,
}

/// How this window names the processes that texts shown together (an answer, a finding, a source)
/// cite by opaque reference (`proc-1a2b3c4d`): each by its readable name ("Google Chrome"), else
/// its executable name, with " (PID n)" only when two different processes cited there would
/// otherwise read the same. Display only: the model's own text, kept for the conversation
/// context, never changes.
pub struct ProcessLabels {
    /// Longest reference first, so a reference never matches inside a longer one.
    labels: Vec<Label>,
}
impl ProcessLabels {
    pub fn new(texts: &[&str], processes: &[ProcessInstance], readable: Readable<'_>) -> Self {
        let cited: Vec<(&ProcessInstance, String)> = processes
            .iter()
            .filter(|p| {
                !p.entity_ref.is_empty() && texts.iter().any(|text| cites(text, &p.entity_ref))
            })
            .map(|p| (p, display_name(p, readable)))
            .collect();
        let mut labels: Vec<Label> = cited
            .iter()
            .map(|(p, name)| {
                let shared = cited.iter().any(|(other, other_name)| {
                    other.entity_ref != p.entity_ref
                        && other_name.to_lowercase() == name.to_lowercase()
                });
                Label {
                    reference: p.entity_ref.clone(),
                    written: process_name(p),
                    label: if shared {
                        format!("{name} (PID {})", p.pid)
                    } else {
                        name.clone()
                    },
                }
            })
            .collect();
        labels.sort_by_key(|l| std::cmp::Reverse(l.reference.len()));
        Self { labels }
    }
    /// The text with each cited reference replaced by its label, in one pass: a label is never
    /// read again, so a program whose name looks like a reference cannot stand for another. A
    /// model that wrote the executable name before the reference ("chrome.exe (proc-1a2b3c4d)",
    /// as it does when several processes share a name) gets the label once.
    pub fn apply(&self, text: &str) -> String {
        if self.labels.is_empty() {
            return text.to_owned();
        }
        let mut out = String::with_capacity(text.len());
        // Where the last inserted label ends: what precedes it is never rewritten.
        let mut fixed = 0;
        let mut at = 0;
        'scan: while at < text.len() {
            for label in &self.labels {
                if !reference_at(text, at, &label.reference) {
                    continue;
                }
                let end = at + label.reference.len();
                let written = format!("{} (", label.written);
                let start = out.len().saturating_sub(written.len());
                let named = text[end..].starts_with(')')
                    && start >= fixed
                    && out.is_char_boundary(start)
                    && out[start..].eq_ignore_ascii_case(&written);
                if named {
                    out.truncate(start);
                }
                out.push_str(&label.label);
                fixed = out.len();
                at = if named { end + 1 } else { end };
                continue 'scan;
            }
            let c = text[at..]
                .chars()
                .next()
                .expect("at is a character boundary");
            out.push(c);
            at += c.len_utf8();
        }
        out
    }
    pub(super) fn all(&self, texts: &[String]) -> Vec<String> {
        texts.iter().map(|text| self.apply(text)).collect()
    }
}

/// One text as the window shows it (`ProcessLabels`). The streamed draft goes through this same
/// function as the reviewed answer's text, so the answer does not change when the run ends.
pub fn localize(text: &str, processes: &[ProcessInstance], readable: Readable<'_>) -> String {
    ProcessLabels::new(&[text], processes, readable).apply(text)
}

pub fn finding(f: &Finding, processes: &[ProcessInstance], readable: Readable<'_>) -> FindingView {
    let mut texts = vec![
        f.support_reason.as_str(),
        f.summary.as_str(),
        f.next_step.as_str(),
    ];
    texts.extend(f.facts.iter().map(|fact| fact.text.as_str()));
    texts.extend(f.hypothesis.as_deref());
    texts.extend(
        [&f.counter_evidence, &f.missing, &f.limitations]
            .into_iter()
            .flatten()
            .map(String::as_str),
    );
    // A finding reads as one block: a process keeps one label across its texts.
    let labels = ProcessLabels::new(&texts, processes, readable);
    FindingView {
        classification: f.classification,
        label: f.classification.label(),
        support: f.support,
        support_reason: labels.apply(&f.support_reason),
        summary: labels.apply(&f.summary),
        facts: f
            .facts
            .iter()
            .map(|fact| labels.apply(&fact.text))
            .collect(),
        hypothesis: f.hypothesis.as_deref().map(|text| labels.apply(text)),
        counter_evidence: labels.all(&f.counter_evidence),
        missing: labels.all(&f.missing),
        next_step: labels.apply(&f.next_step),
        limitations: labels.all(&f.limitations),
        summary_phrase: pcagent_diagnostics::phrase(&f.summary),
        fact_phrases: fact_phrases(f, processes, readable),
        next_step_phrase: pcagent_diagnostics::phrase(&f.next_step),
        limitation_phrases: f
            .limitations
            .iter()
            .map(|l| pcagent_diagnostics::phrase(l))
            .collect(),
        support_reason_phrase: pcagent_diagnostics::phrase(&f.support_reason),
        hypothesis_phrase: f
            .hypothesis
            .as_deref()
            .and_then(pcagent_diagnostics::phrase),
        counter_phrases: f
            .counter_evidence
            .iter()
            .enumerate()
            .map(|(index, _)| {
                f.counter_phrases
                    .get(index)
                    .map(|p| local_phrase(p, processes, readable))
            })
            .collect(),
        missing_phrases: f
            .missing
            .iter()
            .map(|text| pcagent_diagnostics::phrase(text))
            .collect(),
    }
}
