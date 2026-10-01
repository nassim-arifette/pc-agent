//! Resolve the citations next to a claim before looking for its measured quantities.
//! Conversation prose is useful context, but cannot certify a measurement.
use super::{claims, is_reference, Context, Dim, Issue, IssueKind, Known};
use serde_json::{json, Value};
use std::collections::BTreeSet;

pub(super) fn check_numbers(text: &str, ctx: &Context<'_>, issues: &mut Vec<Issue>) {
    for passage in passages(text) {
        let claims = claims(passage);
        if claims.is_empty() {
            continue;
        }
        let groups = reference_groups(passage);
        if groups.is_empty() {
            issues.push(Issue::new(IssueKind::MissingCitation, passage.trim()));
            continue;
        }
        let mentioned = subjects(ctx.received, passage);
        for claim in claims {
            let references = &groups
                .iter()
                .find(|(at, _)| *at >= claim.end)
                .unwrap_or_else(|| groups.last().expect("non-empty groups"))
                .1;
            let query = Query {
                passage,
                subjects: &mentioned,
                topic: topic(passage, claim.dim),
            };
            let mut values = Vec::new();
            let mut facts = Vec::new();
            for reference in references {
                if !ctx.references.iter().any(|(r, _, _)| r == reference) {
                    continue;
                }
                for received in ctx.received {
                    collect_source(received, reference, &query, &mut values, &mut facts);
                }
            }
            let mut known = Known::from_received(&values);
            // Findings are generated locally from measured facts. Do not parse their hypotheses,
            // missing-data notes or recommendations as numerical evidence.
            for fact in facts {
                if query.subjects.len() > 1
                    || (!query.subjects.is_empty()
                        && subjects(ctx.received, &fact) != *query.subjects)
                {
                    continue;
                }
                for claim in super::claims(&fact) {
                    if query.topic != Topic::Any && topic(&fact, claim.dim) != query.topic {
                        continue;
                    }
                    known.values.extend(
                        claim
                            .values
                            .into_iter()
                            .map(|(value, _)| (claim.dim, value)),
                    );
                }
            }
            if !known.supports(&claim) {
                issues.push(Issue::new(IssueKind::UnsupportedNumber, claim.excerpt));
            }
        }
    }
}

/// A source must be in the same sentence or list item as its claims. Decimal separators and
/// dots within executable names do not end a passage.
fn passages(text: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut brackets: usize = 0;
    for (at, c) in text.char_indices() {
        let next = text[at + c.len_utf8()..].chars().next();
        if c == '[' {
            brackets += 1;
        }
        if c == ']' {
            brackets = brackets.saturating_sub(1);
        }
        if brackets == 0
            && (matches!(c, '\n' | ';' | '!' | '?')
                || (c == '.' && next.is_none_or(char::is_whitespace)))
        {
            out.push(&text[start..at + c.len_utf8()]);
            start = at + c.len_utf8();
        }
    }
    out.push(&text[start..]);
    out
}

fn reference_groups(mut text: &str) -> Vec<(usize, Vec<&str>)> {
    let mut out = Vec::new();
    let mut offset = 0;
    while let Some(open) = text.find('[') {
        let after = &text[open + 1..];
        let Some(close) = after.find(']') else { break };
        let tokens: Vec<_> = after[..close]
            .split([',', ';', ' '])
            .filter(|s| !s.is_empty())
            .collect();
        if !tokens.is_empty() && tokens.iter().all(|s| is_reference(s)) {
            out.push((offset + open, tokens));
        }
        offset += open + 1 + close + 1;
        text = &after[close + 1..];
    }
    out
}

struct Query<'a> {
    passage: &'a str,
    subjects: &'a BTreeSet<String>,
    topic: Topic,
}

fn collect_source(
    value: &Value,
    reference: &str,
    query: &Query<'_>,
    values: &mut Vec<Value>,
    facts: &mut Vec<String>,
) {
    let Some(map) = value.as_object() else { return };
    if value["historical"] == true && !describes_history(query.passage) {
        return;
    }
    if map.get("observation_id").and_then(Value::as_str) == Some(reference) {
        values.push(for_entity(value, query));
    }
    if let Some(initial) = map.get("initial_observation") {
        collect_source(initial, reference, query, values, facts);
    }
    if let Some(findings) = map.get("findings").and_then(Value::as_array) {
        for finding in findings.iter().filter(|f| f["id"] == reference) {
            if let Some(measured) = finding["facts"].as_array() {
                facts.extend(measured.iter().filter_map(Value::as_str).map(str::to_owned));
            }
        }
    }
    if let Some(turns) = map.get("previous_turns").and_then(Value::as_array) {
        for turn in turns {
            if let Some(observations) = turn["observations"].as_array() {
                for source in observations {
                    collect_source(source, reference, query, values, facts);
                }
            }
        }
    }
}

fn describes_history(passage: &str) -> bool {
    let words: Vec<_> = passage
        .split(|c: char| !c.is_alphabetic())
        .map(str::to_lowercase)
        .collect();
    let old = [
        "earlier",
        "previous",
        "previously",
        "before",
        "précédente",
        "précédent",
        "précédemment",
        "ancienne",
        "ancien",
        "auparavant",
    ];
    let current = [
        "now",
        "currently",
        "current",
        "still",
        "maintenant",
        "actuellement",
        "actuel",
        "actuelle",
        "toujours",
    ];
    words.iter().any(|w| old.contains(&w.as_str()))
        && !words.iter().any(|w| current.contains(&w.as_str()))
}

/// A sentence naming a process may only use its metrics. Unknown entities produce no values
/// rather than borrowing another process's numbers.
fn for_entity(value: &Value, query: &Query<'_>) -> Value {
    let mut scoped = value.clone();
    if query.subjects.len() > 1 || query.topic == Topic::Ambiguous {
        return json!({});
    }
    if let Some(metrics) = scoped["measurements"]["values"].as_object_mut() {
        if !query.subjects.is_empty() {
            metrics.retain(|entity, _| query.subjects.contains(entity));
        }
        for measured in metrics.values_mut().filter_map(Value::as_object_mut) {
            measured.retain(|metric, _| query.topic.accepts(metric));
        }
    } else if !query.subjects.is_empty() {
        return json!({});
    }
    // Finding prose must only be used via a finding citation, never through an observation.
    if let Some(map) = scoped.as_object_mut() {
        map.remove("findings");
        map.remove("question");
    }
    scoped
}

fn subjects(received: &[Value], passage: &str) -> BTreeSet<String> {
    let text = passage.to_lowercase();
    let mut found: BTreeSet<String> = text
        .split(|c: char| !c.is_ascii_alphanumeric() && c != '-')
        .filter(|s| s.starts_with("proc-"))
        .map(str::to_owned)
        .collect();
    fn named(value: &Value, text: &str, found: &mut BTreeSet<String>) {
        match value {
            Value::Object(map) => {
                if let Some(names) = map.get("process_names").and_then(Value::as_object) {
                    for (reference, name) in names {
                        if let Some(name) = name.as_str() {
                            let name = name.to_lowercase();
                            let stem = name.strip_suffix(".exe").unwrap_or(&name);
                            if text.contains(&name)
                                || (stem.len() >= 3
                                    && text
                                        .split(|c: char| !c.is_alphanumeric() && c != '_')
                                        .any(|word| word == stem))
                            {
                                found.insert(reference.clone());
                            }
                        }
                    }
                }
                for child in map.values() {
                    named(child, text, found);
                }
            }
            Value::Array(items) => items.iter().for_each(|v| named(v, text, found)),
            _ => {}
        }
    }
    for value in received {
        named(value, &text, &mut found);
    }
    if found.is_empty() && text.contains(".exe") {
        found.insert("unresolved-program".into());
    }
    found
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Topic {
    Any,
    Cpu,
    Gpu,
    Memory,
    Disk,
    Ambiguous,
}
impl Topic {
    fn accepts(self, metric: &str) -> bool {
        match self {
            Self::Any => true,
            Self::Cpu => metric.contains("cpu"),
            Self::Gpu => metric.contains("gpu"),
            Self::Memory => [
                "physical_",
                "memory",
                "working_set",
                "private",
                "commit",
                "page_reads",
            ]
            .iter()
            .any(|s| metric.contains(s)),
            Self::Disk => metric.contains("disk") || metric.contains("io_"),
            Self::Ambiguous => false,
        }
    }
}
fn topic(text: &str, dim: Dim) -> Topic {
    let words: Vec<_> = text
        .split(|c: char| !c.is_alphabetic())
        .map(str::to_lowercase)
        .collect();
    let has = |needles: &[&str]| words.iter().any(|w| needles.contains(&w.as_str()));
    let cpu = has(&["cpu", "processor", "processeur"]) && dim == Dim::Percent;
    let gpu = has(&["gpu", "graphique", "graphics"]);
    if cpu && gpu {
        Topic::Ambiguous
    } else if cpu {
        Topic::Cpu
    } else if gpu {
        Topic::Gpu
    } else if has(&["memory", "mémoire", "ram"]) && matches!(dim, Dim::Bytes | Dim::Percent) {
        Topic::Memory
    } else if has(&["disk", "disque", "latency", "latence"]) {
        Topic::Disk
    } else {
        Topic::Any
    }
}
