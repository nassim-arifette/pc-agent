//! Conversation answers, citations, coverage notes and administrative hints.
use super::{
    finding,
    labels::{fact_phrases, local_phrase},
    usage, FindingView, ProcessLabels, Readable, Usage,
};
use pcagent_agent::{
    AgentError, Answer, Category, CitationKind, Confidence, ExecutionStatus, ExtensionRequest,
    Investigation, IssueKind, SensorCoverage,
};
use pcagent_diagnostics::{Classification, Support};
use pcagent_shared::{Phrase, ProcessInstance, Status};
use serde::Serialize;
use serde_json::Value;

#[derive(Debug, Clone, Serialize)]
pub struct ObservationView {
    pub tool: String,
    pub status: Status,
    pub facts: Vec<String>,
    pub limitations: Vec<String>,
}
#[derive(Debug, Clone, Serialize)]
pub struct ErrorView {
    pub code: String,
    pub message: String,
}
#[derive(Debug, Clone, Serialize)]
pub struct CitationView {
    /// As written in the answer: `O2`, `F1`.
    pub reference: String,
    pub kind: CitationKind,
    /// The tool that made the observation, or the finding's label.
    pub title: String,
    /// A finding's classification, so the window can name it in its own language.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub classification: Option<Classification>,
    pub facts: Vec<String>,
    /// Each fact as a closed key with its values, in the same order (see `FindingView`); empty
    /// for web sources and for sources cited from an earlier turn, which keep their stored text
    /// (the window finds their wording among that turn's sources).
    pub fact_phrases: Vec<Option<Phrase>>,
    /// A finding a fresher measurement has since replaced: it describes an earlier moment.
    pub earlier: bool,
    pub measured_at_unix_ms: Option<u64>,
    /// A web source (C6): where it comes from and when it was read.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub web: Option<WebSourceView>,
}
#[derive(Debug, Clone, Serialize)]
pub struct WebSourceView {
    pub domain: String,
    /// Opened only by the `open_web_source` command, which reads it from the stored turn.
    pub url: String,
    pub published_date: Option<String>,
    pub searched_at_unix_ms: Option<u64>,
    /// The exact query that found it.
    pub query: Option<String>,
}
#[derive(Debug, Clone, Serialize)]
pub struct IssueView {
    pub kind: IssueKind,
    pub excerpt: String,
}
/// The model's answer. `text` is untrusted: the page renders it as text, never as markup.
#[derive(Debug, Clone, Serialize)]
pub struct AnswerView {
    pub text: String,
    pub confidence: Confidence,
    pub category: Category,
    pub citations: Vec<CitationView>,
    pub follow_ups: Vec<String>,
    pub action_refs: Vec<String>,
    pub unverified: Vec<IssueView>,
    pub repairs: usize,
}
#[derive(Debug, Clone, Serialize)]
pub struct Report {
    /// Original model context and measured sources, before local display substitutions.
    pub context: pcagent_agent::PastTurn,
    pub question: String,
    pub mode: String,
    pub execution: ExecutionStatus,
    pub coverage: SensorCoverage,
    pub support: Support,
    pub error: Option<ErrorView>,
    /// Absent for a local analysis or an investigation that ended without an answer.
    pub answer: Option<AnswerView>,
    pub selected: FindingView,
    pub alternatives: Vec<FindingView>,
    /// Observations the model cited, or every usable one when the investigation stopped early.
    pub observations: Vec<ObservationView>,
    pub tool_events: Vec<pcagent_agent::ToolEvent>,
    pub limitations: Vec<String>,
    /// Each limitation as a closed key when the window knows its wording ([`note_phrase`]), in
    /// the same order; `None` keeps the English text.
    pub limitation_phrases: Vec<Option<Phrase>>,
    /// Deeper checks (closed `ElevatedTask` kinds) whose reading this turn lacked, in display
    /// order. A local rule: the model has no authority here, and nothing runs without the
    /// person's click and the Windows prompt.
    pub admin_hints: Vec<String>,
    /// Absent for a local analysis, which makes no provider request.
    pub usage: Option<Usage>,
    pub continuation: Option<ExtensionRequest>,
}

pub fn error_code(e: &AgentError) -> String {
    match serde_json::to_value(e) {
        Ok(Value::String(code)) => code,
        Ok(Value::Object(map)) => map.keys().next().cloned().unwrap_or_default(),
        _ => "error".to_owned(),
    }
}
pub(super) fn answer(r: &Investigation, a: &Answer, readable: Readable<'_>) -> AnswerView {
    let processes = &r.local_processes;
    // The text is labelled on its own, exactly as its streamed draft was; quoted passages keep
    // its labels.
    let text = ProcessLabels::new(&[&a.text], processes, readable);
    let citations = a
        .citations
        .iter()
        .filter_map(|c| {
            if let Some(old) = r
                .historical_observations
                .iter()
                .find(|o| o.reference == c.target)
            {
                return Some(CitationView {
                    reference: c.reference.clone(),
                    kind: c.kind,
                    title: old.tool.clone(),
                    // An earlier turn stored a finding under its English heading.
                    classification: Classification::ALL
                        .into_iter()
                        .find(|f| old.kind == CitationKind::Finding && f.label() == old.tool),
                    facts: old.facts.clone(),
                    fact_phrases: Vec::new(),
                    earlier: true,
                    measured_at_unix_ms: (old.measured_at_unix_ms > 0)
                        .then_some(old.measured_at_unix_ms),
                    web: None,
                });
            }
            if c.kind == CitationKind::Web {
                let (observation_id, id) = c.target.split_once('#')?;
                let o = r
                    .observations
                    .iter()
                    .find(|o| o.observation_id == observation_id)?;
                let source = o.observation["sources"]
                    .as_array()?
                    .iter()
                    .find(|s| s["source_id"] == id)?;
                let text = |key: &str| source[key].as_str().map(str::to_owned);
                return Some(CitationView {
                    reference: c.reference.clone(),
                    kind: c.kind,
                    title: text("title").unwrap_or_default(),
                    classification: None,
                    facts: text("snippet").into_iter().collect(),
                    fact_phrases: Vec::new(),
                    earlier: false,
                    measured_at_unix_ms: None,
                    web: Some(WebSourceView {
                        domain: text("domain").unwrap_or_default(),
                        url: text("url")?,
                        published_date: text("published_date"),
                        searched_at_unix_ms: o.observation["searched_at_unix_ms"].as_u64(),
                        query: o.observation["query"].as_str().map(str::to_owned),
                    }),
                });
            }
            let (title, classification, facts, phrases, earlier) = match c.kind {
                CitationKind::Observation => {
                    let o = r
                        .observations
                        .iter()
                        .chain(std::iter::once(&r.initial_observation))
                        .find(|o| o.observation_id == c.target)?;
                    (
                        o.tool.clone(),
                        None,
                        local_block(&o.facts, processes, readable),
                        o.fact_phrases
                            .iter()
                            .map(|p| p.as_ref().map(|p| local_phrase(p, processes)))
                            .collect(),
                        false,
                    )
                }
                CitationKind::Finding => {
                    let earlier = r.historical_findings.iter().any(|f| f.id == c.target);
                    let f = r
                        .findings
                        .iter()
                        .chain(&r.historical_findings)
                        .find(|f| f.id == c.target)?;
                    let facts: Vec<String> = std::iter::once(f.summary.clone())
                        .chain(f.facts.iter().map(|fact| fact.text.clone()))
                        .collect();
                    let phrases = std::iter::once(pcagent_diagnostics::phrase(&f.summary))
                        .chain(fact_phrases(f, processes))
                        .collect();
                    (
                        f.classification.label().to_owned(),
                        Some(f.classification),
                        local_block(&facts, processes, readable),
                        phrases,
                        earlier,
                    )
                }
                CitationKind::Web => return None,
            };
            Some(CitationView {
                reference: c.reference.clone(),
                kind: c.kind,
                title,
                classification,
                facts,
                fact_phrases: phrases,
                earlier,
                measured_at_unix_ms: r
                    .observations
                    .iter()
                    .chain(std::iter::once(&r.initial_observation))
                    .find(|o| o.observation_id == c.target)
                    .map(|o| o.measured_at_unix_ms)
                    .filter(|at| *at > 0),
                web: None,
            })
        })
        .collect();
    AnswerView {
        text: text.apply(&a.text),
        confidence: a.confidence,
        category: a.category,
        citations,
        // Sent as the next question when clicked: they keep the model's own references.
        follow_ups: a.follow_ups.clone(),
        action_refs: a.action_refs.clone(),
        unverified: a
            .unverified
            .iter()
            .map(|i| IssueView {
                kind: i.kind,
                excerpt: text.apply(&i.excerpt),
            })
            .collect(),
        repairs: a.repairs,
    }
}
/// Texts shown together (the facts of one source), with one label per process.
pub(super) fn local_block(
    texts: &[String],
    processes: &[ProcessInstance],
    readable: Readable<'_>,
) -> Vec<String> {
    let all: Vec<&str> = texts.iter().map(String::as_str).collect();
    ProcessLabels::new(&all, processes, readable).all(texts)
}
/// The deeper checks the chat offers under an answer; the Storage view lists the same ones.
pub(super) const OFFERED_CHECKS: [&str; 4] = [
    "system_caches",
    "shadow_storage",
    "disk_reliability",
    "battery_energy",
];
/// Cleanup categories the helper's `system_caches` task measures when the user cannot.
pub(super) const SYSTEM_CACHE_CATEGORIES: [&str; 3] = [
    "previous_windows",
    "windows_update_cache",
    "delivery_optimization",
];
/// Which offered checks a turn lacked: the model asked for an approved helper result that did
/// not exist, or a local inspection reported `needs_admin` on a scope one of the checks covers.
pub(super) fn admin_hints(r: &Investigation) -> Vec<String> {
    let mut hints: Vec<&str> = Vec::new();
    for o in &r.observations {
        let Some(coverage) = o.observation["coverage"].as_array() else {
            continue;
        };
        let denied = |scope: fn(&str) -> bool| {
            coverage
                .iter()
                .any(|c| c["status"] == "needs_admin" && c["scope"].as_str().is_none_or(scope))
        };
        let hint = match o.tool.as_str() {
            "get_approved_elevated_inspection" if denied(|_| true) => {
                o.observation["task"].as_str()
            }
            "get_cleanup_opportunities" if denied(|s| SYSTEM_CACHE_CATEGORIES.contains(&s)) => {
                Some("system_caches")
            }
            _ => None,
        };
        if let Some(hint) = hint.filter(|h| OFFERED_CHECKS.contains(h)) {
            if !hints.contains(&hint) {
                hints.push(hint);
            }
        }
    }
    hints.sort_by_key(|h| OFFERED_CHECKS.iter().position(|o| o == h));
    hints.into_iter().map(str::to_owned).collect()
}
/// What a local Explain adds to the turn's limitations: no model was asked.
pub const LOCAL_EVIDENCE_NOTE: &str = "Local evidence only: no model explanation was requested.";
/// The limitations most turns carry, as their producers word them (the initial snapshot and the
/// live activity in `crates/telemetry`, the local history in `crates/store` and
/// `crates/agent/src/projections/history.rs`), and the local Explain note, with the closed key the window
/// words them by. Any other limitation keeps its English text; `notes_match_their_producers`
/// checks these texts against their sources.
pub(super) const NOTES: [(&str, &str); 27] = [
    ("note.processes_system_list", "Every process comes from one system list (NtQuerySystemInformation) without opening any, so services and other users' processes are measured without administrator rights."),
    ("note.processes_unknown", "Process enumeration failed; coverage is unknown."),
    ("note.processes_one_by_one", "The system process list was unavailable: processes were opened one by one, and those of services or other users may be missing."),
    ("note.working_sets", "Working sets include shared pages and must not be summed as exclusive physical RAM."),
    ("note.process_identities", "Process identities are session-scoped (PID plus creation time)."),
    ("note.process_rates", "Process CPU and I/O are rates over the measurement window (about one second), or since this collector's previous reading of the same instance."),
    ("note.boot_id", "boot_id is an estimate (GetSystemTimeAsFileTime minus GetTickCount64, rounded to one minute), fixed per collector; clock changes can shift it between sessions."),
    ("note.short_sample", "Short sample: no sustained contention, memory leak or causal diagnosis can be established."),
    ("note.recording_gpu", "Recorded process GPU uses the existing PDH window for the retained process list. First windows, missing engines and identities that cannot be revalidated remain unknown; occupancy is not per-application energy."),
    ("note.activity_window", "CPU and I/O are window averages; memory is measured at the end. Rates and memory changes use PID plus creation time, never a PID alone."),
    ("note.activity_groups", "Application groups are an image-and-parent heuristic within one Windows session, not verified product identities. Executable basenames can coincide. Generic shells and service hosts form separate boundaries; all members are aggregated before the returned lists are capped."),
    ("note.activity_references", "Application references are scoped to this observation; process references remain stable for PID plus creation time in the collector session."),
    ("note.activity_working_sets", "Working-set sums include shared pages; they are not exclusive physical RAM. Memory increase/decrease is observed net change, not evidence of a leak."),
    ("note.activity_gpu", "GPU is the busiest observed physical engine. Application GPU sums members on each physical engine before taking the maximum; missing or newly created counter instances are counted in coverage. No GPU instance means unknown, never zero."),
    ("note.activity_exited", "Exited processes are missing from end-of-window attribution, and newly started or reused instances have no full-window rate. Short windows cannot establish sustained contention."),
    ("note.activity_ranking", "Rows are ranked by CPU, then I/O throughput, then working set; service and raw process metadata remain local until privacy projection."),
    ("note.activity_no_baseline", "The initial process list was unavailable; memory is measured but rates and changes are unknown."),
    ("note.activity_no_final_list", "The final system process list could not be read."),
    ("note.services_visible", "SCM enumeration covers Win32 services visible to this account; services with a denied SERVICE_QUERY_STATUS right may be omitted by Windows. Kernel drivers are excluded."),
    ("note.services_pids", "Service PIDs are observations, not execution authority; starting and stopping services have no reliable PID attribution."),
    ("note.services_stalled", "SCM pagination made no progress; remaining services are unavailable."),
    ("note.services_bound", "Service enumeration reached the page bound."),
    ("note.history_samples", "Persisted local samples; values are not interpolated. After 24 hours, values are duration-weighted minute averages with measured minima and maxima. Instantaneous gauges use the sampling interval as averaging weight, not as a measured window."),
    ("note.history_top_processes", "Only the top 20 processes at each sample are retained; absence of a process is unknown, not zero."),
    ("note.history_timestamps", "Observation timestamps and accumulated measured duration are preserved separately from the nominal sampling cadence. A recording's first rate may cover the preceding background interval. Minute buckets overlapping the requested range are returned in full."),
    ("note.history_events", "Event boundaries are the first and last anomalous samples, not a measured continuous duration. Peak and contributor observations retain their own measured windows; derived values identify their original operands. A contributor is observed resource use, not a proven cause."),
    ("note.local_evidence", LOCAL_EVIDENCE_NOTE),
];
/// "Process list is ranked by {sort}; unknown values sort last.", one key per process sort.
pub(super) const RANKED: (&str, &str) =
    ("Process list is ranked by ", "; unknown values sort last.");
pub(super) const RANKED_KEYS: [(&str, &str); 5] = [
    ("working_set", "note.ranked_working_set"),
    ("private_commit", "note.ranked_private_commit"),
    ("cpu", "note.ranked_cpu"),
    ("io_read", "note.ranked_io_read"),
    ("io_write", "note.ranked_io_write"),
];
/// A limitation of a turn as a closed key, when the window knows how to word it: the notes
/// above, a finding's fixed sentences ([`pcagent_diagnostics::phrase`]), or `None`.
pub(super) fn note_phrase(text: &str) -> Option<Phrase> {
    if let Some((key, _)) = NOTES.iter().find(|(_, note)| *note == text) {
        return Some(Phrase::new(key));
    }
    if let Some(label) = text
        .strip_prefix(RANKED.0)
        .and_then(|rest| rest.strip_suffix(RANKED.1))
    {
        return RANKED_KEYS
            .iter()
            .find(|(code, _)| {
                pcagent_telemetry::ProcessSort::parse(code).is_some_and(|s| s.label() == label)
            })
            .map(|(_, key)| Phrase::new(key));
    }
    pcagent_diagnostics::phrase(text)
}
/// The finished turn as the window shows it. Process references in its texts become readable
/// names (`ProcessLabels`); `context` keeps the model's own text for the next turns.
pub fn report(r: &Investigation, question: &str, readable: Readable<'_>) -> Report {
    let processes = &r.local_processes;
    let observations = r
        .observations
        .iter()
        .filter(|o| {
            r.observation_refs.contains(&o.observation_id) || (r.error.is_some() && o.citable)
        })
        .map(|o| ObservationView {
            tool: o.tool.clone(),
            status: o.status,
            facts: local_block(&o.facts, processes, readable),
            limitations: o.limitations.clone(),
        })
        .collect();
    Report {
        context: pcagent_agent::PastTurn::from_report(question, r),
        question: question.to_owned(),
        mode: r.mode.clone(),
        execution: r.execution_status,
        coverage: r.coverage.clone(),
        support: r.diagnostic_support,
        error: r.error.as_ref().map(|e| ErrorView {
            code: error_code(e),
            message: e.to_string(),
        }),
        answer: r.answer.as_ref().map(|a| answer(r, a, readable)),
        selected: finding(&r.selected, processes, readable),
        alternatives: r
            .alternatives
            .iter()
            .map(|f| finding(f, processes, readable))
            .collect(),
        observations,
        tool_events: r.tool_events.clone(),
        limitations: r.limitations.clone(),
        limitation_phrases: r.limitations.iter().map(|l| note_phrase(l)).collect(),
        admin_hints: admin_hints(r),
        usage: (r.mode != "local_rules").then(|| usage(&r.stats)),
        continuation: r.continuation.clone(),
    }
}
