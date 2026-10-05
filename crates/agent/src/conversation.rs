//! Bounded conversation memory. Only retained local observations can support old numerical
//! claims; displayed prose never becomes evidence. Sharing is reapplied on every turn.
use crate::{
    initial_message_with_privacy, shared_name, CitationKind, Investigation, PrivacyContext, Sharing,
};
use pcagent_shared::Snapshot;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeMap;

/// Maximum serialized bytes of earlier turns, including retained evidence (about 3,000 tokens).
pub const MAX_PAST_CHARS: usize = 12_000;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PastObservation {
    pub reference: String,
    pub kind: CitationKind,
    pub measured_at_unix_ms: u64,
    pub tool: String,
    pub facts: Vec<String>,
    /// The minimized source, not the model's description of it.
    pub payload: Value,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PastTurn {
    pub question: String,
    pub answer: String,
    #[serde(default)]
    pub observations: Vec<PastObservation>,
    /// Basenames only; local metadata for reapplying the sharing choice, never sent wholesale.
    #[serde(default)]
    pub program_names: BTreeMap<String, String>,
    /// All permitted names in prior tool results, including files, services and applications.
    /// Kept locally so switching to anonymized mode removes them from previous prose too.
    #[serde(default)]
    pub shared_names: BTreeMap<String, String>,
}

impl PastTurn {
    /// Keep the model's original text and its measured sources, before GUI localization.
    pub fn from_report(question: &str, report: &Investigation) -> Self {
        let mut turn = Self {
            question: question.to_owned(),
            answer: report
                .answer
                .as_ref()
                .map_or_else(|| report.selected.summary.clone(), |a| a.text.clone()),
            program_names: report
                .local_processes
                .iter()
                .filter_map(|p| {
                    Some((
                        p.entity_ref.clone(),
                        shared_name(p.executable_name.as_deref()?)?,
                    ))
                })
                .collect(),
            ..Self::default()
        };
        for (reference, name) in &report.conversation_names {
            turn.shared_names
                .entry(reference.clone())
                .or_insert_with(|| name.clone());
        }
        for observation in &report.observations {
            crate::privacy::collect_shared_names(&observation.observation, &mut turn.shared_names);
        }
        if let Some(answer) = &report.answer {
            for citation in &answer.citations {
                if let Some(old) = report
                    .historical_observations
                    .iter()
                    .find(|o| o.reference == citation.target)
                {
                    turn.observations.push(old.clone());
                    continue;
                }
                match citation.kind {
                    CitationKind::Observation => {
                        if let Some(o) = report
                            .observations
                            .iter()
                            .chain(std::iter::once(&report.initial_observation))
                            .find(|o| o.observation_id == citation.target)
                        {
                            turn.observations.push(PastObservation {
                                reference: citation.reference.clone(),
                                kind: citation.kind,
                                measured_at_unix_ms: o.measured_at_unix_ms,
                                tool: o.tool.clone(),
                                facts: local_facts(&o.facts, &turn.program_names),
                                payload: o.observation.clone(),
                            });
                        }
                    }
                    // Web snippets stay with the turn that searched; a follow-up searches again.
                    CitationKind::Web => {}
                    CitationKind::Finding => {
                        if let Some(f) = report
                            .findings
                            .iter()
                            .chain(&report.historical_findings)
                            .find(|f| f.id == citation.target)
                        {
                            let facts: Vec<_> =
                                f.facts.iter().map(|fact| fact.text.clone()).collect();
                            let at = report
                                .evidence
                                .iter()
                                .filter(|e| f.evidence_refs.contains(&e.id))
                                .map(|e| e.window.finished_at_unix_ms)
                                .max()
                                .unwrap_or(0);
                            turn.observations.push(PastObservation {
                                reference: citation.reference.clone(), kind: citation.kind,
                                measured_at_unix_ms: at, tool: f.classification.label().into(),
                                payload: json!({"findings":[{"id":citation.reference,"facts":facts}]}),
                                facts: local_facts(&facts, &turn.program_names),
                            });
                        }
                    }
                }
            }
        }
        turn
    }
}

/// The exact data message for both the consent preview and the first model request.
pub fn conversation_message(
    past: &[PastTurn],
    question: &str,
    initial: &Snapshot,
    sharing: Sharing,
) -> Value {
    conversation_message_with_privacy(past, question, initial, sharing, &PrivacyContext::local())
}

/// The exact first data message, using metadata frozen by the local consent ticket.
pub fn conversation_message_with_privacy(
    past: &[PastTurn],
    question: &str,
    initial: &Snapshot,
    sharing: Sharing,
    privacy: &PrivacyContext,
) -> Value {
    prepare(
        initial_message_with_privacy(question, initial, sharing, privacy),
        past,
        sharing,
        privacy,
    )
    .0
}

pub(crate) fn recent(past: &[PastTurn]) -> &[PastTurn] {
    let mut used = 0;
    let mut start = past.len();
    for (i, turn) in past.iter().enumerate().rev() {
        used += serde_json::to_vec(turn).map_or(MAX_PAST_CHARS + 1, |v| v.len());
        if used > MAX_PAST_CHARS {
            break;
        }
        start = i;
    }
    &past[start..]
}

pub(crate) fn prepare(
    mut first: Value,
    past: &[PastTurn],
    sharing: Sharing,
    privacy: &PrivacyContext,
) -> (Value, Vec<PastObservation>) {
    let now = first["initial_observation"]["previewed_at_unix_ms"]
        .as_u64()
        .or_else(|| first["initial_observation"]["measured_at_unix_ms"].as_u64())
        .unwrap_or(0);
    let mut turns = Vec::new();
    let mut retained = Vec::new();
    let mut known_names: BTreeMap<String, String> = past
        .iter()
        .flat_map(|turn| {
            turn.program_names
                .iter()
                .chain(&turn.shared_names)
                .map(|(key, name)| (key.clone(), name.clone()))
        })
        .collect();
    for turn in past {
        for observation in &turn.observations {
            crate::privacy::collect_shared_names(&observation.payload, &mut known_names);
        }
    }
    for turn in recent(past) {
        let mut answer = turn.answer.clone();
        let mut sources = Vec::new();
        // Replace all aliases in one pass; old aliases must not collide with new ones.
        let mut aliases = BTreeMap::new();
        for source in &turn.observations {
            let prefix = match source.kind {
                CitationKind::Observation => 'O',
                CitationKind::Finding => 'F',
                CitationKind::Web => 'W',
            };
            let reference = format!("{prefix}{}", 10_000 + retained.len());
            aliases.insert(source.reference.clone(), reference.clone());
            let mut source = source.clone();
            source.reference = reference.clone();
            let payload = &mut source.payload;
            if source.kind == CitationKind::Observation {
                payload["observation_id"] = json!(reference);
            } else if let Some(findings) = payload["findings"].as_array_mut() {
                for finding in findings {
                    finding["id"] = json!(reference);
                }
            }
            payload["historical"] = json!(true);
            payload["measured_at_unix_ms"] = json!(source.measured_at_unix_ms);
            if source.measured_at_unix_ms > 0 && now >= source.measured_at_unix_ms {
                payload["age_ms"] = json!(now - source.measured_at_unix_ms);
            }
            // Context metadata such as action plans and finding IDs is not reusable evidence.
            if let Some(map) = payload.as_object_mut() {
                for key in ["current_finding_ids", "citable_observation_ids", "plan"] {
                    map.remove(key);
                }
                map.remove("process_names");
                if sharing == Sharing::Complete && !turn.program_names.is_empty() {
                    map.insert("process_names".into(), json!(turn.program_names));
                }
            }
            if sharing == Sharing::Anonymized {
                crate::privacy::anonymize(payload, &known_names);
            }
            privacy.sanitize_value(payload);
            sources.push(payload.clone());
            retained.push(source);
        }
        answer = remap_citations(&answer, &aliases);
        let mut question = turn.question.clone();
        if sharing == Sharing::Anonymized {
            question = crate::privacy::anonymize_text(&question, &known_names);
            answer = crate::privacy::anonymize_text(&answer, &known_names);
        }
        turns.push(json!({"question":question,"answer":answer,"observations":sources}));
    }
    if !turns.is_empty() {
        first["previous_turns"] = json!(turns);
    }
    if sharing == Sharing::Anonymized {
        crate::privacy::anonymize(&mut first, &known_names);
    }
    privacy.sanitize_value(&mut first);
    (first, retained)
}

fn local_facts(facts: &[String], names: &BTreeMap<String, String>) -> Vec<String> {
    let mut entries: Vec<_> = names.iter().collect();
    entries.sort_by_key(|(reference, _)| std::cmp::Reverse(reference.len()));
    facts
        .iter()
        .map(|fact| {
            entries
                .iter()
                .fold(fact.clone(), |text, (reference, name)| {
                    text.replace(reference.as_str(), name)
                })
        })
        .collect()
}

fn remap_citations(text: &str, aliases: &BTreeMap<String, String>) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(open) = rest.find('[') {
        out.push_str(&rest[..open]);
        let after = &rest[open + 1..];
        let Some(close) = after.find(']') else {
            out.push_str(&rest[open..]);
            return out;
        };
        let group = &after[..close];
        let tokens: Vec<_> = group
            .split([',', ';', ' '])
            .filter(|t| !t.is_empty())
            .collect();
        if !tokens.is_empty() && tokens.iter().all(|t| crate::answer::is_reference(t)) {
            let mapped: Vec<_> = tokens
                .into_iter()
                .filter_map(|t| aliases.get(t).map(String::as_str))
                .collect();
            if !mapped.is_empty() {
                out.push_str(&format!("[{}]", mapped.join(", ")));
            } else {
                out.truncate(out.trim_end_matches(' ').len());
            }
        } else {
            out.push('[');
            out.push_str(group);
            out.push(']');
        }
        rest = &after[close + 1..];
    }
    out.push_str(rest);
    out
}
