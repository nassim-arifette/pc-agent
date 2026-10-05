//! Resolve the citations next to a claim before looking for its measured quantities.
//! Conversation prose is useful context, but cannot certify a measurement.
use super::{claims, is_reference, Context, Dim, Issue, IssueKind, Known};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};

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
        // Several processes of one program named once ("une instance de Firefox"): the claim is
        // about one of them. Different programs keep strict attribution.
        let instances: Vec<BTreeSet<String>> = if same_program(ctx.received, &mentioned) {
            mentioned
                .iter()
                .map(|s| BTreeSet::from([s.clone()]))
                .collect()
        } else {
            Vec::new()
        };
        for claim in claims {
            let references = &groups
                .iter()
                .find(|(at, _)| *at >= claim.end)
                .unwrap_or_else(|| groups.last().expect("non-empty groups"))
                .1;
            let supported = |any_wording: bool| {
                std::iter::once(&mentioned)
                    .chain(&instances)
                    .any(|subjects| {
                        let query = Query {
                            passage,
                            subjects,
                            topic: topic(passage, claim.dim),
                            any_wording,
                        };
                        known(ctx, references, &query).supports(&claim)
                    })
            };
            if supported(false) {
                continue;
            }
            // A value of an earlier measurement cited as if it were current: the number is right,
            // the sentence must say when it was measured.
            let kind = if supported(true) {
                IssueKind::HistoricalWording
            } else {
                IssueKind::UnsupportedNumber
            };
            issues.push(Issue::new(kind, claim.excerpt));
        }
    }
}

/// Whether every reference was named through the same executable name, like several firefox.exe.
fn same_program(received: &[Value], references: &BTreeSet<String>) -> bool {
    fn collect(value: &Value, names: &mut BTreeMap<String, String>) {
        match value {
            Value::Object(map) => {
                for key in ["process_names", "shared_names"] {
                    if let Some(found) = map.get(key).and_then(Value::as_object) {
                        for (reference, name) in found {
                            if let Some(name) = name.as_str() {
                                names.insert(reference.clone(), name.to_lowercase());
                            }
                        }
                    }
                }
                map.values().for_each(|child| collect(child, names));
            }
            Value::Array(items) => items.iter().for_each(|child| collect(child, names)),
            _ => {}
        }
    }
    if references.len() < 2 {
        return false;
    }
    let mut names = BTreeMap::new();
    received.iter().for_each(|value| collect(value, &mut names));
    let programs: BTreeSet<_> = references.iter().map(|r| names.get(r)).collect();
    programs.len() == 1 && programs.iter().all(Option::is_some)
}

/// Prices and versions come from the web (C6): each must appear in the snippet of a `[W…]` source
/// cited in its passage, a price to the written precision and a version exactly. A price is never
/// a measurement of this PC.
pub(super) fn check_web(text: &str, ctx: &Context<'_>, issues: &mut Vec<Issue>) {
    for passage in passages(text) {
        let prices = prices(passage);
        let versions = versions(passage);
        if prices.is_empty() && versions.is_empty() {
            continue;
        }
        let web: Vec<&str> = reference_groups(passage)
            .into_iter()
            .flat_map(|(_, refs)| refs)
            .filter(|r| r.starts_with('W'))
            .collect();
        let snippets: Vec<&str> = ctx
            .received
            .iter()
            .flat_map(|payload| payload["sources"].as_array().into_iter().flatten())
            .filter(|s| s["source_id"].as_str().is_some_and(|id| web.contains(&id)))
            .filter_map(|s| s["snippet"].as_str())
            .collect();
        for (excerpt, amounts) in prices {
            let found = snippets.iter().any(|snippet| {
                let known = amount_values(snippet);
                amounts
                    .iter()
                    .any(|a| known.iter().any(|k| (a - k).abs() < 0.005))
            });
            if !found {
                let kind = if web.is_empty() {
                    IssueKind::MissingCitation
                } else {
                    IssueKind::UnsupportedNumber
                };
                issues.push(Issue::new(kind, excerpt));
            }
        }
        for version in versions {
            if !web.is_empty() && !snippets.iter().any(|snippet| snippet.contains(&version)) {
                issues.push(Issue::new(IssueKind::UnsupportedNumber, version));
            }
        }
    }
}

const CURRENCIES: [&str; 7] = ["€", "$", "£", "eur", "usd", "gbp", "euros"];

/// Amounts written with a currency, like `89,99 €`, `$129` or `1 299 EUR`: the excerpt and the
/// values it can mean (a comma can separate decimals or thousands).
fn prices(passage: &str) -> Vec<(String, Vec<f64>)> {
    let lower = passage.to_lowercase();
    let chars: Vec<(usize, char)> = lower.char_indices().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        if !chars[i].1.is_ascii_digit() {
            i += 1;
            continue;
        }
        let start = chars[i].0;
        let mut j = i;
        while j < chars.len()
            && (chars[j].1.is_ascii_digit()
                || matches!(chars[j].1, ',' | '.' | ' ' | '\u{a0}' | '\u{202f}')
                    && chars.get(j + 1).is_some_and(|(_, c)| c.is_ascii_digit()))
        {
            j += 1;
        }
        let end = chars.get(j).map_or(lower.len(), |(at, _)| *at);
        let number = &lower[start..end];
        let before = lower[..start].trim_end();
        let after = lower[end..].trim_start();
        let currency = CURRENCIES
            .iter()
            .any(|c| after.starts_with(c) || before.ends_with(c));
        if currency {
            out.push((passage[start..end].trim().to_owned(), amount_values(number)));
        }
        i = j.max(i + 1);
    }
    out
}

/// Every value the numbers of a text can mean, for amounts.
fn amount_values(text: &str) -> Vec<f64> {
    let mut values = Vec::new();
    for token in text.split(|c: char| !(c.is_ascii_digit() || matches!(c, ',' | '.'))) {
        let token = token.trim_matches([',', '.']);
        if token.is_empty() {
            continue;
        }
        let plain = token.replace(',', "");
        if let Ok(v) = plain.parse::<f64>() {
            values.push(v);
        }
        if let Ok(v) = token.replace('.', "").replace(',', ".").parse::<f64>() {
            values.push(v);
        }
    }
    // Thousands written with spaces, like "1 299", read as one amount.
    let joined: String = text
        .chars()
        .filter(|c| !matches!(c, ' ' | '\u{a0}' | '\u{202f}'))
        .collect();
    if joined != text {
        values.extend(amount_values(&joined));
    }
    values
}

/// Version numbers like `32.0.15.6636` or `version 566.36`: two dots or more, or one dot right
/// after the word version.
fn versions(passage: &str) -> Vec<String> {
    let mut out = Vec::new();
    let words: Vec<&str> = passage
        .split(|c: char| c.is_whitespace() || matches!(c, '(' | ')' | ',' | ';' | ':' | '['))
        .filter(|w| !w.is_empty())
        .collect();
    for (i, word) in words.iter().enumerate() {
        let token = word.trim_end_matches(['.', ']']);
        let numeric = token
            .split('.')
            .all(|part| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit()));
        let dots = token.matches('.').count();
        let named = i > 0
            && matches!(
                words[i - 1].to_lowercase().as_str(),
                "version" | "v" | "pilote" | "driver"
            );
        if numeric && (dots >= 2 || (dots == 1 && named)) {
            out.push(token.to_owned());
        }
    }
    out
}

/// The values the cited references hold for this claim's subjects and topic.
fn known(ctx: &Context<'_>, references: &[&str], query: &Query<'_>) -> Known {
    let mut values = Vec::new();
    let mut facts = Vec::new();
    for reference in references {
        if !ctx.references.iter().any(|(r, _, _)| r == reference) {
            continue;
        }
        for received in ctx.received {
            collect_source(received, reference, query, &mut values, &mut facts);
        }
    }
    let mut known = Known::from_received(&values, !matches!(query.topic, Topic::Battery(_)));
    // Findings are generated locally from measured facts. Do not parse their hypotheses,
    // missing-data notes or recommendations as numerical evidence.
    for fact in facts {
        if query.subjects.len() > 1
            || (!query.subjects.is_empty() && subjects(ctx.received, &fact) != *query.subjects)
        {
            continue;
        }
        for fact_claim in super::claims(&fact) {
            if query.topic != Topic::Any && topic(&fact, fact_claim.dim) != query.topic {
                continue;
            }
            known.values.extend(
                fact_claim
                    .values
                    .into_iter()
                    .map(|(value, _)| (fact_claim.dim, value)),
            );
        }
    }
    known
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
    /// Read earlier measurements whatever the wording, to tell a dating problem from a wrong value.
    any_wording: bool,
}

fn accepts_record(record: &Value, query: &Query<'_>) -> bool {
    if record["metric"].as_str().is_some_and(|metric| {
        metric.ends_with("_energy_estimated") || metric == "battery_energy_estimated_total"
    }) {
        let words: Vec<_> = query
            .passage
            .split(|c: char| !c.is_alphabetic())
            .map(str::to_lowercase)
            .collect();
        let has = |list: &[&str]| words.iter().any(|word| list.contains(&word.as_str()));
        if !has(&[
            "estimated",
            "estimate",
            "estimates",
            "estimée",
            "estimé",
            "estimation",
            "estimations",
        ]) || !has(&["energy", "énergie", "consumption", "consommation", "e3"])
            || has(&[
                "capacity",
                "capacité",
                "capacite",
                "autonomy",
                "autonomie",
                "runtime",
                "power",
                "puissance",
            ])
        {
            return false;
        }
    }
    record["metric"]
        .as_str()
        .is_some_and(|metric| query.topic.accepts(metric))
        && (query.subjects.is_empty()
            || record["entity_ref"]
                .as_str()
                .is_some_and(|entity| query.subjects.contains(entity)))
}

/// Recording results have their own quantities. A capacity input is not the measured loss,
/// and a native instantaneous rate is not the average computed over a recorded segment.
fn accepts_recording_metric(metric: &str, query: &Query<'_>) -> bool {
    if !metric.starts_with("battery_") || !query.topic.accepts(metric) {
        return false;
    }
    let words: Vec<_> = query
        .passage
        .split(|c: char| !c.is_alphabetic())
        .map(str::to_lowercase)
        .collect();
    let has = |terms: &[&str]| words.iter().any(|word| terms.contains(&word.as_str()));
    if has(&[
        "loss",
        "lost",
        "decrease",
        "decreased",
        "drop",
        "baisse",
        "perte",
        "perdu",
        "perdue",
        "diminution",
        "diminué",
    ]) {
        return metric == "battery_measured_capacity_decrease_mwh";
    }
    if has(&["runtime", "autonomy", "autonomie", "endurance"]) {
        if has(&["windows"]) {
            return metric == "battery_estimated_runtime_windows_seconds";
        }
        if has(&["recording", "recorded", "enregistrement"])
            && has(&["from", "based", "depuis", "calculée", "calculé"])
        {
            return metric == "battery_estimated_runtime_from_recording_seconds";
        }
        return matches!(
            metric,
            "battery_estimated_runtime_windows_seconds"
                | "battery_estimated_runtime_at_current_rate_seconds"
                | "battery_estimated_runtime_from_recording_seconds"
        );
    }
    if has(&["average", "mean", "moyen", "moyenne"]) {
        return metric == "battery_average_power_rate_mw";
    }
    true
}

/// Only typed values certify battery quantities. Dates, cadence, segment counts and windows
/// remain in the received payload for provenance; they are not runtime measurements. Likewise,
/// derivation inputs remain inspectable there without becoming interchangeable result values.
fn recording_battery_values(value: &Value, query: &Query<'_>) -> Value {
    if !query.any_wording && !describes_history(query.passage) {
        return json!({});
    }
    let Some(batteries) = value["batteries"].as_array() else {
        return json!({});
    };
    let mut records = Vec::new();
    for battery in batteries {
        let Some(reference) = battery["battery_ref"].as_str() else {
            continue;
        };
        if !query.subjects.is_empty() && !query.subjects.contains(reference) {
            continue;
        }
        for collection in ["points", "evolution"] {
            let Some(groups) = battery[collection].as_array() else {
                continue;
            };
            for group in groups {
                let Some(observations) = group["observations"].as_array() else {
                    continue;
                };
                for observation in observations {
                    if observation["entity_ref"].as_str() != Some(reference)
                        || !accepts_record(observation, query)
                        || observation["metric"]
                            .as_str()
                            .is_none_or(|metric| !accepts_recording_metric(metric, query))
                    {
                        continue;
                    }
                    records.push(json!({
                        "observation_id": observation["observation_id"],
                        "entity_ref": observation["entity_ref"],
                        "metric": observation["metric"],
                        "value": observation["value"],
                        "unit": observation["unit"],
                        "status": observation["status"],
                    }));
                }
            }
        }
    }
    json!({"observations": records})
}

fn remove_derivation_inputs(value: &mut Value) {
    match value {
        Value::Object(map) => {
            map.remove("inputs");
            for child in map.values_mut() {
                remove_derivation_inputs(child);
            }
        }
        Value::Array(items) => items.iter_mut().for_each(remove_derivation_inputs),
        _ => {}
    }
}

fn collect_source(
    value: &Value,
    reference: &str,
    query: &Query<'_>,
    values: &mut Vec<Value>,
    facts: &mut Vec<String>,
) {
    let Some(map) = value.as_object() else { return };
    if value["historical"] == true && !query.any_wording && !describes_history(query.passage) {
        return;
    }
    if map.get("observation_id").and_then(Value::as_str) == Some(reference) {
        values.push(for_entity(value, query));
    }
    if let Some(initial) = map.get("initial_observation") {
        collect_source(initial, reference, query, values, facts);
    }
    // A web source's numbers are those of its snippet (C6); they never certify a PC measurement.
    if let Some(sources) = map.get("sources").and_then(Value::as_array) {
        for source in sources.iter().filter(|s| s["source_id"] == reference) {
            if let Some(snippet) = source["snippet"].as_str() {
                facts.push(snippet.to_owned());
            }
        }
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
        "recording",
        "recorded",
        "capture",
        "captured",
        "snapshot",
        "capturé",
        "capturée",
        "mesure",
        "mesures",
        "mesuré",
        "mesurée",
        "mesurés",
        "mesurées",
        "relevé",
        "relevée",
        "relevés",
        "relevées",
        "measured",
        "measurement",
        "measurements",
        "history",
        "historical",
        "enregistrement",
        "enregistré",
        "historique",
        "pendant",
        "during",
        "hier",
        "yesterday",
        "était",
        "étaient",
        "avait",
        "avaient",
        "utilisait",
        "utilisaient",
        "occupait",
        "occupaient",
        "consommait",
        "consommaient",
        "montrait",
        "montraient",
        "restait",
        "restaient",
        "atteignait",
        "atteignaient",
        "was",
        "were",
        "showed",
        "reached",
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
    let lower = passage.to_lowercase();
    let clock_time = lower.char_indices().any(|(at, c)| {
        if !c.is_ascii_digit() {
            return false;
        }
        let before = lower[..at].trim_end();
        if !(before.ends_with(" à") || before == "à" || before.ends_with(" at") || before == "at")
        {
            return false;
        }
        lower[at..]
            .chars()
            .skip_while(char::is_ascii_digit)
            .find(|c| !c.is_ascii_whitespace())
            .is_some_and(|c| matches!(c, ':' | 'h'))
    });
    (clock_time || words.iter().any(|w| old.contains(&w.as_str())))
        && !words.iter().any(|w| current.contains(&w.as_str()))
}

/// A sentence naming an entity may only use its metrics. Unknown entities produce no values
/// rather than borrowing another entity's numbers.
fn for_entity(value: &Value, query: &Query<'_>) -> Value {
    let mut scoped = value.clone();
    if query.subjects.len() > 1 || query.topic == Topic::Ambiguous {
        return json!({});
    }
    let has_records = scoped["observations"].is_array();
    let has_metrics = scoped["measurements"]["values"].is_object();
    let has_history = scoped["points"].is_array();
    let has_histories = scoped["histories"].is_array();
    let has_events = scoped["events"].is_array();
    let has_battery = scoped["battery"]["batteries"].is_array();
    if let Some(metrics) = scoped["measurements"]["values"].as_object_mut() {
        if !query.subjects.is_empty() {
            metrics.retain(|entity, _| query.subjects.contains(entity));
        }
        for measured in metrics.values_mut().filter_map(Value::as_object_mut) {
            measured.retain(|metric, _| query.topic.accepts(metric));
        }
    }
    if let Some(records) = scoped["observations"].as_array_mut() {
        records.retain(|record| accepts_record(record, query));
    }
    if has_history
        && (scoped["metric"]
            .as_str()
            .is_none_or(|metric| !query.topic.accepts(metric))
            || (!query.subjects.is_empty()
                && scoped["entity_ref"]
                    .as_str()
                    .is_none_or(|entity| !query.subjects.contains(entity))))
    {
        scoped["points"] = json!([]);
    }
    if let Some(histories) = scoped["histories"].as_array_mut() {
        for history in histories {
            *history = for_entity(history, query);
        }
    }
    if let Some(events) = scoped["events"].as_array_mut() {
        // A system peak and the process sampled beside it are independent observations.
        // Select the contributor's own typed value only when that process is named; never
        // lend the global peak to a process merely because it is a correlated contributor.
        for event in events.iter_mut() {
            let contributor = event["contributor_ref"].as_str();
            if event.get("measurement_metadata_available").is_some() {
                *event = if contributor.is_some_and(|entity| query.subjects.contains(entity)) {
                    event["contributor_observation"].clone()
                } else {
                    event["peak_observation"].clone()
                };
                continue;
            }
            if contributor.is_some_and(|entity| query.subjects.contains(entity)) {
                *event = match (
                    contributor,
                    event["contributor_metric"].as_str(),
                    event["contributor_unit"].as_str(),
                    event["contributor_value"].as_u64(),
                ) {
                    (Some(entity), Some(metric), Some(unit), Some(value)) => json!({
                        "observation_id":"event-contributor", "entity_ref":entity,
                        "metric":metric, "unit":unit, "value":value, "status":"ok",
                    }),
                    _ => json!({}),
                };
            }
        }
        events.retain(|event| accepts_record(event, query));
    }
    if has_battery {
        scoped["battery"] = recording_battery_values(&scoped["battery"], query);
    }
    remove_derivation_inputs(&mut scoped);
    if !query.subjects.is_empty()
        && !has_metrics
        && !has_records
        && !has_history
        && !has_histories
        && !has_events
        && !has_battery
    {
        return json!({});
    }
    if !query.subjects.is_empty() {
        // A named entity cannot borrow an unrelated volume or system total from the same tool.
        return json!({"measurements":scoped["measurements"],"observations":scoped["observations"],
            "metric":scoped["metric"],"points":scoped["points"],"histories":scoped["histories"],
            "events":scoped["events"],"battery":scoped["battery"]});
    }
    // Finding prose must only be used via a finding citation, never through an observation.
    if let Some(map) = scoped.as_object_mut() {
        map.remove("findings");
        map.remove("question");
    }
    scoped
}

fn subjects(received: &[Value], passage: &str) -> BTreeSet<String> {
    let text = passage
        .to_lowercase()
        .replace(['‐', '‑', '‒', '–', '—', '−'], "-");
    let mut found: BTreeSet<String> = text
        .split(|c: char| !c.is_ascii_alphanumeric() && c != '-')
        .filter(|s| {
            [
                "proc-",
                "app-",
                "service-",
                "startup-",
                "hardware-",
                "cleanup-",
                "dimm-",
                "battery-",
            ]
            .iter()
            .any(|prefix| s.starts_with(prefix))
        })
        .map(str::to_owned)
        .collect();
    // An explicit opaque reference selects the requested entity even if the passage also names
    // the application to which it belongs.
    if !found.is_empty() {
        return found;
    }
    fn named(value: &Value, text: &str, found: &mut BTreeSet<String>) {
        match value {
            Value::Object(map) => {
                for key in ["process_names", "shared_names"] {
                    if let Some(names) = map.get(key).and_then(Value::as_object) {
                        for (reference, name) in names {
                            if let Some(name) = name.as_str() {
                                let name = name.to_lowercase();
                                let stem = name.strip_suffix(".exe").unwrap_or(&name);
                                if crate::privacy::replace_word_ci(text, &name, "") != text
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
    if found.len() > 1 {
        fn application(value: &Value, found: &BTreeSet<String>, candidates: &mut BTreeSet<String>) {
            match value {
                Value::Object(map) => {
                    if let Some(groups) = map.get("applications").and_then(Value::as_array) {
                        for group in groups {
                            let Some(reference) = group["entity_ref"].as_str() else {
                                continue;
                            };
                            let Some(members) = group["process_refs"].as_array() else {
                                continue;
                            };
                            if found.contains(reference)
                                && found.iter().all(|entity| {
                                    entity == reference
                                        || members
                                            .iter()
                                            .any(|member| member.as_str() == Some(entity))
                                })
                            {
                                candidates.insert(reference.to_owned());
                            }
                        }
                    }
                    for child in map.values() {
                        application(child, found, candidates);
                    }
                }
                Value::Array(items) => items
                    .iter()
                    .for_each(|item| application(item, found, candidates)),
                _ => {}
            }
        }
        let mut candidates = BTreeSet::new();
        for value in received {
            application(value, &found, &mut candidates);
        }
        if candidates.len() == 1 {
            found = candidates;
        }
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
    Battery(BatteryRole),
    Memory,
    MemoryIncrease,
    MemoryDecrease,
    Disk,
    Ambiguous,
}
impl Topic {
    fn accepts(self, metric: &str) -> bool {
        match self {
            Self::Any => true,
            Self::Cpu => metric.contains("cpu"),
            Self::Gpu => metric.contains("gpu"),
            Self::Battery(role) => metric.starts_with("battery_") && role.accepts(metric),
            Self::MemoryIncrease => metric.contains("increase_bytes"),
            Self::MemoryDecrease => metric.contains("decrease_bytes"),
            Self::Memory => {
                !metric.contains("increase_bytes")
                    && !metric.contains("decrease_bytes")
                    && [
                        "physical_",
                        "memory",
                        "working_set",
                        "private",
                        "commit",
                        "page_reads",
                    ]
                    .iter()
                    .any(|s| metric.contains(s))
            }
            Self::Disk => metric.contains("disk") || metric.contains("io_"),
            Self::Ambiguous => false,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum BatteryRole {
    Any,
    Design,
    Full,
    Remaining,
    Wear,
    Retention,
    NetPower,
    DischargePower,
    ChargePower,
}
impl BatteryRole {
    fn accepts(self, metric: &str) -> bool {
        match self {
            Self::Any => true,
            Self::Design => metric == "battery_design_capacity_mwh",
            Self::Full => metric == "battery_full_charge_capacity_mwh",
            Self::Remaining => metric == "battery_remaining_capacity_mwh",
            Self::Wear => metric == "battery_estimated_wear_percent",
            Self::Retention => metric == "battery_capacity_retention_percent",
            Self::NetPower => matches!(
                metric,
                "battery_power_rate_mw" | "battery_average_power_rate_mw"
            ),
            Self::DischargePower => metric == "battery_discharge_power_mw",
            Self::ChargePower => metric == "battery_charge_power_mw",
        }
    }
}
fn topic(text: &str, dim: Dim) -> Topic {
    let words: Vec<_> = text
        .split(|c: char| !c.is_alphabetic())
        .map(str::to_lowercase)
        .collect();
    let has = |needles: &[&str]| words.iter().any(|w| needles.contains(&w.as_str()));
    let cpu = has(&["cpu", "processor", "processeur"])
        && matches!(
            dim,
            Dim::Percent | Dim::Celsius | Dim::Megahertz | Dim::Watts
        );
    let gpu = has(&["gpu", "graphique", "graphics"]);
    let battery = has(&[
        "battery",
        "batterie",
        "batteries",
        "batería",
        "baterias",
        "baterías",
        "bateria",
        "akku",
        "batterij",
        "аккумулятор",
        "батарея",
    ]) || ["电池", "バッテリー", "배터리"]
        .iter()
        .any(|name| text.contains(name));
    if (cpu && gpu) || (battery && (cpu || gpu)) {
        Topic::Ambiguous
    } else if battery {
        let role = if dim == Dim::WattHours && has(&["design", "original", "origine", "d'origine"])
        {
            BatteryRole::Design
        } else if dim == Dim::WattHours && has(&["remaining", "restante", "restant"]) {
            BatteryRole::Remaining
        } else if dim == Dim::WattHours && has(&["full", "pleine", "current", "actuelle"]) {
            BatteryRole::Full
        } else if dim == Dim::Percent && has(&["wear", "usure"]) {
            BatteryRole::Wear
        } else if dim == Dim::Percent && has(&["retention", "rétention"]) {
            BatteryRole::Retention
        } else if dim == Dim::Watts && has(&["net", "débit", "rate", "signed", "signé"]) {
            BatteryRole::NetPower
        } else if dim == Dim::Watts
            && has(&[
                "discharge",
                "discharging",
                "décharge",
                "consomme",
                "consommation",
                "consumption",
                "drain",
            ])
        {
            BatteryRole::DischargePower
        } else if dim == Dim::Watts && has(&["charge", "charging", "recharge"]) {
            BatteryRole::ChargePower
        } else {
            BatteryRole::Any
        };
        Topic::Battery(role)
    } else if cpu {
        Topic::Cpu
    } else if gpu {
        Topic::Gpu
    } else if dim == Dim::Bytes
        && has(&[
            "grew",
            "growth",
            "increased",
            "increase",
            "croissance",
            "augmentation",
            "augmenté",
        ])
    {
        Topic::MemoryIncrease
    } else if dim == Dim::Bytes
        && has(&[
            "decreased",
            "decrease",
            "shrank",
            "baisse",
            "diminué",
            "diminution",
        ])
    {
        Topic::MemoryDecrease
    } else if has(&["memory", "mémoire", "ram"]) && matches!(dim, Dim::Bytes | Dim::Percent) {
        Topic::Memory
    } else if has(&["disk", "disque", "latency", "latence"]) {
        Topic::Disk
    } else {
        Topic::Any
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pcagent_inspect::{CoverageStatus, Inspection, Observation};
    use pcagent_shared::{Origin, Status, Window};
    use pcagent_store::Store;
    use std::collections::BTreeMap;

    const BATTERY_A: &str = "battery-00000000-0000-0000-0000-000000000001";
    const BATTERY_B: &str = "battery-00000000-0000-0000-0000-000000000002";

    /// Serialize the real Store response, including native points, derived values and their
    /// endpoint inputs. This also catches changes to the BatteryRecording wire structure.
    fn recorded_batteries() -> Value {
        let store = Store::open_in_memory().unwrap();
        let recording = store.begin_recording(30, 1000).unwrap();
        for at in [1000, 2000] {
            let window = Window {
                started_at_unix_ms: at - 1,
                finished_at_unix_ms: at,
                duration_ms: 1,
            };
            let mut observations = Vec::new();
            for (reference, capacity, rate) in [
                (BATTERY_A, 1000 - (at - 1000) / 500, 15_000),
                (BATTERY_B, 2000 - (at - 1000) / 500, 25_000),
            ] {
                for (metric, value, unit) in [
                    ("battery_kind", json!("system"), "category"),
                    (
                        "battery_capacity_units",
                        json!("milliwatt_hours"),
                        "category",
                    ),
                    ("battery_capacities_consistent", json!(true), "boolean"),
                    ("battery_state_consistent", json!(true), "boolean"),
                    ("battery_charge_state", json!("discharging"), "category"),
                    ("battery_on_ac", json!(false), "boolean"),
                    (
                        "battery_remaining_capacity_mwh",
                        json!(capacity),
                        "milliwatt_hours",
                    ),
                    ("battery_power_rate_mw", json!(-rate), "milliwatts"),
                    ("battery_discharge_power_mw", json!(rate), "milliwatts"),
                    (
                        "battery_estimated_runtime_windows_seconds",
                        json!(71),
                        "seconds",
                    ),
                ] {
                    observations.push(Observation {
                        observation_id: format!("{reference}-{metric}-{at}"),
                        measured_at_unix_ms: at,
                        window: window.clone(),
                        entity_ref: reference.into(),
                        metric: metric.into(),
                        value,
                        unit: unit.into(),
                        status: CoverageStatus::Ok,
                        source: "DeviceIoControl/IOCTL_BATTERY_QUERY_STATUS".into(),
                        origin: Origin::SyntheticFixture,
                    });
                }
            }
            store
                .append_recording_battery_timed(
                    &recording.id,
                    &Inspection {
                        status: Status::Ok,
                        measured_at_unix_ms: at,
                        window,
                        origin: Origin::SyntheticFixture,
                        observations,
                        coverage: Vec::new(),
                        names: BTreeMap::new(),
                    },
                    at,
                    Some(at - 1000),
                )
                .unwrap();
        }
        store
            .finish_recording(&recording.id, 2000, false, 2)
            .unwrap();
        json!({
            "observation_id":"O1", "historical":true,
            "observations":[], "histories":[],
            "battery":store.battery_recording(&recording.id).unwrap().unwrap(),
        })
    }

    fn number_issues(text: &str, received: &Value) -> Vec<Issue> {
        let references = vec![(
            "O1".to_owned(),
            super::super::CitationKind::Observation,
            "recording-fixture".to_owned(),
        )];
        let mut issues = Vec::new();
        check_numbers(
            text,
            &Context {
                received: std::slice::from_ref(received),
                references: &references,
                plans: &[],
            },
            &mut issues,
        );
        issues
    }

    fn unsupported(text: &str, received: &Value) {
        assert!(
            number_issues(text, received)
                .iter()
                .any(|issue| issue.kind == IssueKind::UnsupportedNumber),
            "{text}"
        );
    }

    #[test]
    fn recorded_battery_values_cannot_certify_gpu_power() {
        let received = recorded_batteries();
        unsupported("During recording the GPU used 15 W [O1].", &received);
        unsupported(
            "Pendant l'enregistrement le GPU était à 25 W [O1].",
            &received,
        );
        assert!(
            number_issues("During recording the battery used 15 W [O1].", &received).is_empty()
        );
    }

    #[test]
    fn recorded_batteries_keep_their_entity_even_with_unicode_hyphens() {
        let received = recorded_batteries();
        for reference in [BATTERY_A.to_owned(), BATTERY_A.replace('-', "‑")] {
            let correct = format!("During recording battery {reference} had 1 Wh remaining [O1].");
            assert!(number_issues(&correct, &received).is_empty(), "{correct}");
            unsupported(
                &format!("During recording battery {reference} had 2 Wh remaining [O1]."),
                &received,
            );
            unsupported(
                &format!("During recording battery {reference} discharge power was 25 W [O1]."),
                &received,
            );
        }
        let mut mismatched = received.clone();
        mismatched["battery"]["batteries"][0]["points"][0]["observations"]
            .as_array_mut()
            .unwrap()
            .push(json!({
                "observation_id":"wrong-container", "entity_ref":BATTERY_B,
                "metric":"battery_remaining_capacity_mwh", "value":80_000,
                "unit":"milliwatt_hours", "status":"ok",
            }));
        unsupported(
            &format!("During recording battery {BATTERY_A} had 80 Wh remaining [O1]."),
            &mismatched,
        );
    }

    #[test]
    fn recorded_battery_evidence_requires_historical_language() {
        let mut received = recorded_batteries();
        // The recorded value is right; only its wording claims it is current.
        let dated = |text: &str, received: &Value| {
            let kinds: Vec<_> = number_issues(text, received)
                .into_iter()
                .map(|i| i.kind)
                .collect();
            assert_eq!(kinds, [IssueKind::HistoricalWording], "{text}");
        };
        dated(
            &format!("Battery {BATTERY_A} has 1 Wh remaining now [O1]."),
            &received,
        );
        // The recording structure is historical even if its enclosing flag was omitted.
        received.as_object_mut().unwrap().remove("historical");
        dated(
            &format!("Battery {BATTERY_A} has 1 Wh remaining now [O1]."),
            &received,
        );
        unsupported(
            &format!("Battery {BATTERY_A} has 7 Wh remaining now [O1]."),
            &received,
        );
        assert!(number_issues(
            &format!("Pendant l'enregistrement la batterie {BATTERY_A} avait 1 Wh restante [O1]."),
            &received,
        )
        .is_empty());
    }

    #[test]
    fn recorded_loss_and_runtime_use_results_instead_of_endpoint_inputs() {
        let received = recorded_batteries();
        for text in [
            format!("During recording battery {BATTERY_A} lost 2 mWh [O1]."),
            format!("Battery {BATTERY_A} autonomy estimated from recording: 499 s [O1]."),
            format!("During recording battery {BATTERY_A} average net rate: -7.2 W [O1]."),
        ] {
            assert!(number_issues(&text, &received).is_empty(), "{text}");
        }
        for text in [
            format!("During recording battery {BATTERY_A} lost 1000 mWh [O1]."),
            format!("Battery {BATTERY_A} autonomy estimated from recording: 71 s [O1]."),
            format!("During recording battery {BATTERY_A} average net rate: -15 W [O1]."),
            format!("During recording battery {BATTERY_A} autonomy was 1 ms [O1]."),
        ] {
            unsupported(&text, &received);
        }
        let mut unknown = received.clone();
        let derived = &mut unknown["battery"]["batteries"][0]["evolution"][0]["observations"][0];
        assert_eq!(derived["inputs"][0]["value"], 1000);
        derived["value"] = Value::Null;
        // Even an incomplete producer must not promote its operands to an absent result.
        unsupported(
            &format!("During recording battery {BATTERY_A} lost 1000 mWh [O1]."),
            &unknown,
        );
        unsupported(
            &format!("During recording battery {BATTERY_A} lost 2 mWh [O1]."),
            &unknown,
        );
    }

    #[test]
    fn numeric_projection_keeps_the_original_dated_provenance_intact() {
        let received = recorded_batteries();
        let original = received.clone();
        let text = format!("During recording battery {BATTERY_A} lost 2 mWh [O1].");
        let mentioned = subjects(std::slice::from_ref(&received), &text);
        let scoped = for_entity(
            &received,
            &Query {
                passage: &text,
                subjects: &mentioned,
                topic: topic(&text, Dim::WattHours),
                any_wording: false,
            },
        );
        assert_eq!(
            scoped["battery"]["observations"].as_array().unwrap().len(),
            1
        );
        assert!(scoped["battery"]["observations"][0].get("inputs").is_none());
        assert_eq!(received, original);
        assert_eq!(
            received["battery"]["batteries"][0]["evolution"][0]["observations"][0]["inputs"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
    }
}
