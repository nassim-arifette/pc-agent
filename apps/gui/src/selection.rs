//! Bounded local sources and exact, single-use consent for already measured selections.
use crate::failure::{self, Code, Failure};
use crate::model_routing::RoutingReview;
use pcagent_agent::{
    selection::{
        FrozenSelection, SelectionIntent, SelectionRef, SelectionSource, MAX_SOURCE_BYTES,
    },
    PastTurn, PrivacyContext, Sharing,
};
use pcagent_shared::{new_id, unix_ms};
use serde::Serialize;
use serde_json::Value;
use std::{
    collections::{BTreeSet, VecDeque},
    sync::Arc,
    time::{Duration, Instant},
};

pub const MAX_SELECTION_AGE: Duration = Duration::from_secs(300);
/// Dashboard 12/min + process activity 6/min + battery 6/min = 24 sources/min: a reviewed
/// preview keeps its source about 4 min.
const MAX_SOURCES: usize = 96;
const MAX_RETAINED_BYTES: usize = 16 * 1024 * 1024;
const MAX_TICKETS: usize = 16;
const MAX_PAST_BYTES: usize = 512 * 1024;

struct SourceEntry {
    id: String,
    source: Arc<SelectionSource>,
    expires_at: Instant,
    bytes: usize,
}

struct Ticket {
    prepared: PreparedSelection,
    expires_at: Instant,
}

/// Never serialize this object: it contains local source identities and private host metadata.
pub struct PreparedSelection {
    pub reference: SelectionRef,
    pub selection: FrozenSelection,
    pub question: String,
    pub conversation_id: Option<String>,
    pub past: Vec<PastTurn>,
    pub sharing: Sharing,
    pub file_names: bool,
    pub privacy: PrivacyContext,
    message: Value,
    pub context_revision: u64,
    pub routing: Option<RoutingReview>,
    id: String,
}

#[derive(Serialize)]
pub struct SelectionDescriptor {
    pub reference: SelectionRef,
    pub measured_at_unix_ms: Option<u64>,
    pub historical: bool,
}

#[derive(Serialize)]
pub struct SelectionPreview {
    pub preview_id: String,
    pub message: Value,
    pub selected_evidence: Value,
    pub intent: SelectionIntent,
    pub measured_at_unix_ms: Option<u64>,
    pub age_ms: Option<u64>,
    pub context_revision: u64,
    pub file_name_preview: Value,
}

pub struct ReviewedSelection<'a> {
    pub preview_id: &'a str,
    pub reference: &'a SelectionRef,
    pub intent: SelectionIntent,
    pub question: Option<&'a str>,
    pub conversation_id: Option<&'a str>,
    pub past: &'a [PastTurn],
    pub sharing: Sharing,
    pub file_names: bool,
    /// Increment when local model/routing configuration changes; no credential is included.
    pub context_revision: u64,
}

#[derive(Default)]
pub struct SelectionRegistry {
    sources: VecDeque<SourceEntry>,
    tickets: VecDeque<Ticket>,
    history_revision: u64,
    history_clears: usize,
}

impl SelectionRegistry {
    fn expire(&mut self) {
        let now = Instant::now();
        let oldest_alert = unix_ms().saturating_sub(604_800_000);
        self.sources.retain(|source| source.expires_at > now && !matches!(
            &*source.source, SelectionSource::WatchAlert(alert) if alert.started_at_unix_ms < oldest_alert
        ));
        let sources: BTreeSet<_> = self
            .sources
            .iter()
            .map(|source| source.id.as_str())
            .collect();
        self.tickets.retain(|ticket| {
            ticket.expires_at > now
                && sources.contains(ticket.prepared.reference.source_id.as_str())
        });
    }

    /// Called by existing local measurement/read paths, never by a model tool.
    pub fn publish(&mut self, source: SelectionSource) -> Result<String, String> {
        self.expire();
        if source.is_history() && self.history_clears != 0 {
            return Err("Local history is being cleared.".into());
        }
        let bytes = source.retained_bytes();
        if bytes > MAX_SOURCE_BYTES || source.targets().is_empty() {
            return Err(
                "The local selection source is empty or exceeds its retention bound.".into(),
            );
        }
        while self.sources.len() >= MAX_SOURCES
            || self
                .sources
                .iter()
                .map(|source| source.bytes)
                .sum::<usize>()
                .saturating_add(bytes)
                > MAX_RETAINED_BYTES
        {
            self.sources.pop_front();
        }
        let id = new_id("selection-source");
        self.sources.push_back(SourceEntry {
            id: id.clone(),
            source: Arc::new(source),
            expires_at: Instant::now() + MAX_SELECTION_AGE,
            bytes,
        });
        self.expire();
        Ok(id)
    }

    /// Local display references and original dates only; no collector or provider is called.
    pub fn sources(&mut self) -> Vec<SelectionDescriptor> {
        self.expire();
        self.sources
            .iter()
            .rev()
            .flat_map(|source| {
                source
                    .source
                    .targets()
                    .into_iter()
                    .map(|target| SelectionDescriptor {
                        measured_at_unix_ms: source.source.measured_at(&target),
                        historical: source.source.is_history(),
                        reference: SelectionRef {
                            source_id: source.id.clone(),
                            target,
                        },
                    })
            })
            .collect()
    }

    #[allow(clippy::too_many_arguments)]
    pub fn prepare(
        &mut self,
        reference: SelectionRef,
        intent: SelectionIntent,
        question: Option<String>,
        conversation_id: Option<String>,
        past: Vec<PastTurn>,
        sharing: Sharing,
        file_names: bool,
        privacy: PrivacyContext,
        context_revision: u64,
    ) -> Result<SelectionPreview, Failure> {
        self.expire();
        if !reference.validate() {
            return Err("Invalid local selection reference.".into());
        }
        let source = self
            .sources
            .iter()
            .find(|source| source.id == reference.source_id)
            .ok_or_else(|| {
                Failure::new(
                    Code::SelectionExpired,
                    "The selected capture expired or was invalidated. Select the measured item again.",
                )
            })?;
        let question = effective_question(&reference, intent, question.as_deref())?;
        if serde_json::to_vec(&past).map_or(true, |past| past.len() > MAX_PAST_BYTES) {
            return Err(
                "The selected conversation exceeds the local consent retention bound.".into(),
            );
        }
        let selection = FrozenSelection::freeze(
            &source.source,
            &reference.target,
            intent,
            unix_ms(),
            sharing,
            file_names,
            &privacy,
        )
        .map_err(str::to_owned)?;
        let message = selection.preview_message(&past, &question, &privacy);
        let mut file_name_preview = selection.storage_context().sharing_preview();
        privacy.sanitize_value(&mut file_name_preview);
        let preview = SelectionPreview {
            preview_id: new_id("selection-preview"),
            message: message.clone(),
            selected_evidence: selection.output().cloud.clone(),
            intent,
            measured_at_unix_ms: selection.output().cloud["measured_at_unix_ms"].as_u64(),
            age_ms: selection.output().cloud["age_ms"].as_u64(),
            context_revision,
            file_name_preview,
        };
        if self.tickets.len() >= MAX_TICKETS {
            self.tickets.pop_front();
        }
        self.tickets.push_back(Ticket {
            prepared: PreparedSelection {
                reference,
                selection,
                question,
                conversation_id,
                past,
                sharing,
                file_names,
                privacy,
                message,
                context_revision,
                routing: None,
                id: preview.preview_id.clone(),
            },
            expires_at: Instant::now() + MAX_SELECTION_AGE,
        });
        Ok(preview)
    }

    /// Consume exactly the reviewed immutable inputs. A mismatch permits no provider request.
    pub fn take(&mut self, reviewed: ReviewedSelection<'_>) -> Result<PreparedSelection, Failure> {
        self.expire();
        let index = self
            .tickets
            .iter()
            .position(|ticket| ticket.prepared.id == reviewed.preview_id)
            .ok_or_else(|| {
                Failure::new(
                    Code::PreviewRequired,
                    "Open the selected-evidence sharing preview before sending.",
                )
            })?;
        let pending = &self.tickets[index].prepared;
        let question = effective_question(reviewed.reference, reviewed.intent, reviewed.question)?;
        if pending.reference != *reviewed.reference
            || pending.selection.intent() != reviewed.intent
            || pending.question != question
            || pending.conversation_id.as_deref() != reviewed.conversation_id
            || pending.past != reviewed.past
            || pending.sharing != reviewed.sharing
            || pending.file_names != reviewed.file_names
            || pending.context_revision != reviewed.context_revision
        {
            return Err(Failure::new(Code::PreviewChanged, "The selection, question, conversation, sharing or model settings changed. Open the preview again."));
        }
        // Reconstruct only from frozen host data, then compare the exact consented wire
        // message before consuming the ticket. This performs no collection or network I/O.
        if pending
            .selection
            .preview_message(reviewed.past, &question, &pending.privacy)
            != pending.message
        {
            return Err(Failure::new(
                Code::PreviewChanged,
                "The frozen message no longer matches its sharing preview. Open the preview again.",
            ));
        }
        Ok(self
            .tickets
            .remove(index)
            .expect("matching ticket exists")
            .prepared)
    }

    /// A new measurement in the same session does not revoke an earlier immutable preview.
    /// A restarted collector does: process references must not cross session boundaries.
    pub fn invalidate_process_session(&mut self, current_session: &str) {
        self.sources.retain(|source| {
            source
                .source
                .process_session()
                .is_none_or(|session| session == current_session)
        });
        self.expire();
    }

    pub fn invalidate_history(&mut self) {
        self.history_revision = self.history_revision.saturating_add(1);
        self.sources.retain(|source| !source.source.is_history());
        self.expire();
    }

    pub fn invalidate_watch(&mut self, watch_id: &str) {
        self.history_revision = self.history_revision.saturating_add(1);
        self.sources.retain(|source| {
            !matches!(&*source.source,
            SelectionSource::WatchAlert(alert) if alert.watch_id == watch_id)
        });
        self.expire();
    }

    /// Revoke every capture of this scope before replacing its scan or authorization.
    pub fn invalidate_storage(&mut self, scope_id: &str) {
        self.sources.retain(|source| {
            source
                .source
                .storage_identity()
                .is_none_or(|(scope, _)| scope != scope_id)
        });
        self.expire();
    }

    pub fn invalidate_tickets(&mut self) {
        self.tickets.clear();
    }

    /// Remove private conversation context without discarding unrelated measured sources.
    pub fn invalidate_conversation(&mut self, conversation_id: &str) {
        self.tickets
            .retain(|ticket| ticket.prepared.conversation_id.as_deref() != Some(conversation_id));
    }

    pub fn history_revision(&self) -> u64 {
        self.history_revision
    }

    pub fn begin_history_clear(&mut self) {
        self.history_clears = self.history_clears.saturating_add(1);
        self.invalidate_history();
    }

    pub fn finish_history_clear(&mut self) {
        self.history_clears = self.history_clears.saturating_sub(1);
        // Reads begun while the database was still being cleared must not publish afterward.
        self.invalidate_history();
    }

    pub fn publish_history(
        &mut self,
        source: SelectionSource,
        revision: u64,
    ) -> Result<String, String> {
        if self.history_revision == u64::MAX || self.history_revision != revision {
            return Err("The local history changed while its capture was being read.".into());
        }
        self.publish(source)
    }

    pub fn bind_routing(
        &mut self,
        preview_id: &str,
        routing: RoutingReview,
    ) -> Result<(), Failure> {
        self.expire();
        let pending = self
            .tickets
            .iter_mut()
            .find(|ticket| ticket.prepared.id == preview_id)
            .ok_or_else(|| Failure::new(Code::PreviewExpired, "The selected preview expired."))?;
        if pending.prepared.routing.is_some() {
            return Err("The selected preview already has a frozen model policy.".into());
        }
        pending.prepared.routing = Some(routing);
        Ok(())
    }

    pub fn choices(&mut self, preview_id: &str) -> Option<(Sharing, bool, RoutingReview)> {
        self.expire();
        let pending = &self
            .tickets
            .iter()
            .find(|ticket| ticket.prepared.id == preview_id)?
            .prepared;
        Some((
            pending.sharing,
            pending.file_names,
            pending.routing.clone()?,
        ))
    }
}

fn effective_question(
    reference: &SelectionRef,
    intent: SelectionIntent,
    question: Option<&str>,
) -> Result<String, Failure> {
    let question = question
        .map(str::to_owned)
        .unwrap_or_else(|| reference.target.default_question(intent));
    failure::check_question(&question)?;
    Ok(question)
}

#[cfg(test)]
mod tests {
    use super::*;
    use pcagent_agent::selection::{Resource, SelectionTarget};
    use pcagent_shared::{Evidence, Metric, Origin, Snapshot, Window};
    use serde_json::json;

    fn source(value: u64) -> SelectionSource {
        let mut snapshot = Snapshot::empty("private-selection-session");
        snapshot.origin = Origin::SyntheticFixture;
        snapshot.evidence.push(Evidence::measured(
            Metric::CpuBusyBasisPoints,
            "system",
            value,
            "fixture",
            Window {
                started_at_unix_ms: 1000,
                finished_at_unix_ms: 2000,
                duration_ms: 1000,
            },
        ));
        SelectionSource::Snapshot(snapshot)
    }

    fn watch_source(started_at: u64, watch: &str) -> SelectionSource {
        SelectionSource::WatchAlert(serde_json::from_value(json!({
            "id":"alert-fixture","watch_id":watch,
            "rule":{"kind":"resource_above","resource":"cpu","threshold_basis_points":5000,"sustained_seconds":0},
            "started_at_unix_ms":started_at,"last_checked_unix_ms":started_at,
            "ended_at_unix_ms":null,"close_reason":null,
            "observation":{"entity_ref":"system","metric":"cpu_busy_basis_points","value":9300,"unit":"basis_points",
                "source":"fixture","status":"ok","origin":"synthetic_fixture","measured_at_unix_ms":started_at,
                "window":{"started_at_unix_ms":started_at-1000,"finished_at_unix_ms":started_at,"duration_ms":1000},
                "derivation":null,"inputs":[]},
            "extreme_observation":{"entity_ref":"system","metric":"cpu_busy_basis_points","value":9300,"unit":"basis_points",
                "source":"fixture","status":"ok","origin":"synthetic_fixture","measured_at_unix_ms":started_at,
                "window":{"started_at_unix_ms":started_at-1000,"finished_at_unix_ms":started_at,"duration_ms":1000},
                "derivation":null,"inputs":[]},
            "lower_bound":false,"notification":"sent","seen":false
        })).unwrap())
    }

    #[test]
    fn watch_deletion_revokes_its_preview_and_inflight_read_but_preserves_other_sources() {
        let (mut registry, live, _, _) = fixture();
        let revision = registry.history_revision();
        let id = registry
            .publish_history(watch_source(unix_ms(), "watch-a"), revision)
            .unwrap();
        let reference = SelectionRef {
            source_id: id,
            target: SelectionTarget::WatchAlert {
                alert_id: "alert-fixture".into(),
            },
        };
        let preview = registry
            .prepare(
                reference.clone(),
                SelectionIntent::Explain,
                None,
                None,
                vec![],
                Sharing::Anonymized,
                false,
                PrivacyContext::local(),
                1,
            )
            .unwrap();
        registry.invalidate_watch("watch-a");
        assert!(registry
            .sources
            .iter()
            .any(|entry| entry.id == live.source_id));
        assert!(!registry
            .tickets
            .iter()
            .any(|ticket| ticket.prepared.id == preview.preview_id));
        assert!(registry
            .publish_history(watch_source(unix_ms(), "watch-a"), revision)
            .is_err());
        assert!(registry
            .prepare(
                reference,
                SelectionIntent::Explain,
                None,
                None,
                vec![],
                Sharing::Anonymized,
                false,
                PrivacyContext::local(),
                1
            )
            .is_err());
    }

    #[test]
    fn cleared_or_retention_expired_alerts_cannot_be_selected() {
        let mut registry = SelectionRegistry::default();
        registry
            .publish(watch_source(unix_ms() - 604_800_001, "watch-old"))
            .unwrap();
        registry.expire();
        assert!(registry.sources.is_empty());
        registry
            .publish(watch_source(unix_ms(), "watch-new"))
            .unwrap();
        registry.begin_history_clear();
        registry.finish_history_clear();
        assert!(registry.sources.is_empty());
    }

    fn fixture() -> (
        SelectionRegistry,
        SelectionRef,
        SelectionPreview,
        Vec<PastTurn>,
    ) {
        let mut registry = SelectionRegistry::default();
        let source_id = registry.publish(source(2500)).unwrap();
        let reference = SelectionRef {
            source_id,
            target: SelectionTarget::Resource {
                resource: Resource::Cpu,
            },
        };
        let past = vec![PastTurn {
            question: "Avant ?".into(),
            answer: "La collecte était partielle.".into(),
            ..PastTurn::default()
        }];
        let preview = registry
            .prepare(
                reference.clone(),
                SelectionIntent::Explain,
                Some("Pourquoi ?".into()),
                Some("local-conversation".into()),
                past.clone(),
                Sharing::Anonymized,
                false,
                PrivacyContext::local(),
                7,
            )
            .unwrap();
        (registry, reference, preview, past)
    }

    fn reviewed<'a>(
        reference: &'a SelectionRef,
        preview: &'a SelectionPreview,
        past: &'a [PastTurn],
    ) -> ReviewedSelection<'a> {
        ReviewedSelection {
            preview_id: &preview.preview_id,
            reference,
            intent: SelectionIntent::Explain,
            question: Some("Pourquoi ?"),
            conversation_id: Some("local-conversation"),
            past,
            sharing: Sharing::Anonymized,
            file_names: false,
            context_revision: 7,
        }
    }

    #[test]
    fn a_new_capture_preserves_the_exact_previous_reviewed_message_and_is_single_use() {
        let (mut registry, reference, preview, past) = fixture();
        let next_id = registry.publish(source(9700)).unwrap();
        assert_ne!(reference.source_id, next_id);
        assert_eq!(registry.sources().len(), 2);
        let prepared = registry
            .take(reviewed(&reference, &preview, &past))
            .unwrap();
        assert_eq!(prepared.message, preview.message);
        assert_eq!(
            prepared.selection.preview_message(
                &prepared.past,
                &prepared.question,
                &prepared.privacy
            ),
            preview.message
        );
        assert_eq!(prepared.selection.output().evidence[0].value, Some(2500));
        assert_eq!(
            prepared.selection.output().evidence[0]
                .window
                .finished_at_unix_ms,
            2000
        );
        assert!(registry
            .take(reviewed(&reference, &preview, &past))
            .is_err());
    }

    #[test]
    fn mismatched_selection_context_cannot_consume_the_consent_ticket() {
        for changed in 0..9 {
            let (mut registry, reference, preview, past) = fixture();
            let mut other_reference = reference.clone();
            other_reference.target = SelectionTarget::Resource {
                resource: Resource::Memory,
            };
            let mut input = reviewed(&reference, &preview, &past);
            match changed {
                0 => input.preview_id = "unknown",
                1 => input.reference = &other_reference,
                2 => input.intent = SelectionIntent::Investigate,
                3 => input.question = Some("Different question"),
                4 => input.conversation_id = None,
                5 => input.past = &[],
                6 => input.sharing = Sharing::Complete,
                7 => input.file_names = true,
                8 => input.context_revision = 8,
                _ => unreachable!(),
            }
            assert!(registry.take(input).is_err(), "case {changed}");
            assert_eq!(
                registry.tickets.len(),
                1,
                "a mismatch must not authorize or consume a turn"
            );
            assert!(registry.take(reviewed(&reference, &preview, &past)).is_ok());
        }
    }

    #[test]
    fn monotonic_expiry_and_source_invalidation_revoke_pending_tickets() {
        for changed in 0..5 {
            let (mut registry, reference, preview, past) = fixture();
            match changed {
                0 => {
                    registry.sources[0].expires_at = Instant::now();
                }
                1 => {
                    registry.tickets[0].expires_at = Instant::now();
                }
                2 => {
                    registry.sources.pop_front();
                }
                3 => registry.invalidate_process_session("new-collector-session"),
                4 => registry.invalidate_tickets(),
                _ => unreachable!(),
            }
            assert!(
                registry
                    .take(reviewed(&reference, &preview, &past))
                    .is_err(),
                "case {changed}"
            );
        }
    }

    #[test]
    fn a_refused_selection_send_says_why_nothing_left() {
        let prepare =
            |registry: &mut SelectionRegistry, reference: &SelectionRef, question: String| {
                registry
                    .prepare(
                        reference.clone(),
                        SelectionIntent::Explain,
                        Some(question),
                        None,
                        vec![],
                        Sharing::Anonymized,
                        false,
                        PrivacyContext::local(),
                        7,
                    )
                    .err()
                    .expect("refused")
                    .code
            };
        let (mut registry, reference, preview, past) = fixture();
        let mut changed = reviewed(&reference, &preview, &past);
        changed.question = Some("Different question");
        assert_eq!(
            registry.take(changed).err().expect("refused").code,
            Code::PreviewChanged
        );
        let mut unknown = reviewed(&reference, &preview, &past);
        unknown.preview_id = "selection-preview-unknown";
        assert_eq!(
            registry.take(unknown).err().expect("refused").code,
            Code::PreviewRequired
        );
        let routing = crate::model_routing::ModelRouting::default().review(None);
        assert_eq!(
            registry
                .bind_routing("selection-preview-unknown", routing)
                .unwrap_err()
                .code,
            Code::PreviewExpired
        );
        assert_eq!(
            prepare(&mut registry, &reference, " ".into()),
            Code::QuestionEmpty
        );
        assert_eq!(
            prepare(&mut registry, &reference, "x".repeat(5000)),
            Code::QuestionTooLong
        );
        registry.sources[0].expires_at = Instant::now();
        assert_eq!(
            prepare(&mut registry, &reference, "Pourquoi ?".into()),
            Code::SelectionExpired
        );
    }

    #[test]
    fn a_changed_frozen_wire_message_cannot_consume_a_ticket() {
        let (mut registry, reference, preview, past) = fixture();
        registry.tickets[0].prepared.message["question"] = json!("Unreviewed replacement question");
        assert!(registry
            .take(reviewed(&reference, &preview, &past))
            .is_err());
        assert_eq!(registry.tickets.len(), 1);
    }

    #[test]
    fn deleting_a_conversation_revokes_its_ticket_even_with_a_previously_read_past() {
        let (mut registry, reference, preview, previously_read_past) = fixture();
        registry.invalidate_conversation("unrelated-conversation");
        assert_eq!(registry.tickets.len(), 1);
        registry.invalidate_conversation("local-conversation");
        assert!(registry
            .take(reviewed(&reference, &preview, &previously_read_past))
            .is_err());
        assert_eq!(
            registry.sources.len(),
            1,
            "independent captures remain available"
        );
        let next = registry
            .prepare(
                reference.clone(),
                SelectionIntent::Explain,
                Some("A new question.".into()),
                None,
                Vec::new(),
                Sharing::Anonymized,
                false,
                PrivacyContext::local(),
                7,
            )
            .unwrap();
        registry.invalidate_tickets();
        assert!(registry
            .take(ReviewedSelection {
                preview_id: &next.preview_id,
                reference: &reference,
                intent: SelectionIntent::Explain,
                question: Some("A new question."),
                conversation_id: None,
                past: &[],
                sharing: Sharing::Anonymized,
                file_names: false,
                context_revision: 7,
            })
            .is_err());
    }

    #[test]
    fn registry_bounds_evict_oldest_captures_and_their_unconsumed_tickets() {
        let (mut registry, reference, preview, past) = fixture();
        for _ in 0..MAX_SOURCES {
            registry.publish(source(1)).unwrap();
        }
        assert_eq!(registry.sources.len(), MAX_SOURCES);
        assert!(registry
            .take(reviewed(&reference, &preview, &past))
            .is_err());
        let mut oversized = Snapshot::empty("private-large-source");
        for _ in 0..2500 {
            let mut evidence = Evidence::measured(
                Metric::CpuBusyBasisPoints,
                "system",
                1,
                "fixture",
                Window::instant(),
            );
            evidence.limitations.push("x".repeat(1024));
            oversized.evidence.push(evidence);
        }
        assert!(registry
            .publish(SelectionSource::Snapshot(oversized))
            .is_err());
        assert_eq!(registry.sources.len(), MAX_SOURCES);
    }

    #[test]
    fn deleted_history_and_recordings_revoke_their_capture_sources() {
        use pcagent_agent::selection::RecordingSource;
        let mut registry = SelectionRegistry::default();
        let recording = || {
            SelectionSource::Recording(RecordingSource { recording: serde_json::from_value(json!({"id":"recording-synthetic","started_at_unix_ms":1000,"ended_at_unix_ms":2000,"duration_seconds":30,"status":"completed","samples":0,"markers":[]})).unwrap(), histories: vec![], cpu_peaks: None, resource_peaks: None, battery: None })
        };
        registry.publish(recording()).unwrap();
        let resource_id = registry.publish(source(20)).unwrap();
        registry.invalidate_history();
        assert_eq!(registry.sources().len(), 1);
        assert_eq!(registry.sources()[0].reference.source_id, resource_id);
        registry.publish(recording()).unwrap();
        registry.invalidate_history();
        assert_eq!(registry.sources().len(), 1);
    }

    #[test]
    fn an_in_flight_history_read_cannot_republish_deleted_evidence() {
        use pcagent_agent::selection::RecordingSource;
        let recording = || {
            SelectionSource::Recording(RecordingSource { recording: serde_json::from_value(json!({"id":"recording-synthetic","started_at_unix_ms":1000,"ended_at_unix_ms":2000,"duration_seconds":30,"status":"completed","samples":0,"markers":[]})).unwrap(), histories: vec![], cpu_peaks: None, resource_peaks: None, battery: None })
        };
        let mut registry = SelectionRegistry::default();
        let before_read = registry.history_revision();
        registry.begin_history_clear();
        assert!(registry.publish_history(recording(), before_read).is_err());
        assert!(registry.sources().is_empty());
        let during_clear = registry.history_revision();
        assert!(registry.publish_history(recording(), during_clear).is_err());
        assert!(registry.publish(recording()).is_err());
        registry.begin_history_clear();
        registry.finish_history_clear();
        assert!(
            registry
                .publish_history(recording(), registry.history_revision())
                .is_err(),
            "a second simultaneous clear still blocks publication"
        );
        registry.finish_history_clear();
        assert!(registry.publish_history(recording(), during_clear).is_err());
        let after_clear = registry.history_revision();
        assert!(registry.publish_history(recording(), after_clear).is_ok());
        assert_eq!(registry.sources().len(), 1);
    }

    #[test]
    fn replacing_a_scan_never_repoints_an_old_ticket_to_a_reused_item_alias() {
        use pcagent_agent::selection::{SelectionTarget, StorageSource};
        let storage = |scan: &str, value: u64| {
            SelectionSource::Storage(StorageSource {
            page: serde_json::from_value(json!({"schema_version":1,"scope_id":"scope-fixture","scan_id":scan,"kind":"files","sort_basis":"allocated_bytes","offset":0,"limit":1,"total":1,"next_offset":null,"scan_status":"ok","limitations":[],"items":[{"item_ref":format!("{scan}:1"),"alias":"file-1","alias_path":"<scope>/file-1","parent_ref":null,"kind":"files","path":null,"redacted_path":"<scope>/file-1","logical_bytes":value,"allocated_bytes":value,"allocation_unknown_files":0,"estimated_reclaim_bytes":null,"files":1,"link_count":1,"sparse":false,"compressed":false,"modified_unix_ms":null,"last_used":"unknown","status":"ok"}]})).unwrap(),
            window: Window { started_at_unix_ms: 1000, finished_at_unix_ms: 2000, duration_ms: 1000 }, origin: Origin::SyntheticFixture, context: pcagent_agent::StorageContext::default(),
        })
        };
        let mut registry = SelectionRegistry::default();
        let source_id = registry.publish(storage("scan-old", 4096)).unwrap();
        let reference = SelectionRef {
            source_id,
            target: SelectionTarget::Storage {
                scope_id: "scope-fixture".into(),
                item_ref: "scan-old:1".into(),
            },
        };
        let preview = registry
            .prepare(
                reference.clone(),
                SelectionIntent::Explain,
                None,
                None,
                vec![],
                Sharing::Anonymized,
                false,
                PrivacyContext::local(),
                0,
            )
            .unwrap();
        registry.invalidate_storage("scope-fixture");
        let next_source = registry.publish(storage("scan-new", 9000)).unwrap();
        assert!(registry
            .take(ReviewedSelection {
                preview_id: &preview.preview_id,
                reference: &reference,
                intent: SelectionIntent::Explain,
                question: None,
                conversation_id: None,
                past: &[],
                sharing: Sharing::Anonymized,
                file_names: false,
                context_revision: 0
            })
            .is_err());
        assert_eq!(registry.sources().len(), 1);
        assert_eq!(registry.sources()[0].reference.source_id, next_source);
        assert_eq!(
            registry.sources()[0].reference.target,
            SelectionTarget::Storage {
                scope_id: "scope-fixture".into(),
                item_ref: "scan-new:1".into()
            }
        );
    }

    #[test]
    fn routing_is_frozen_once_and_source_revocation_removes_its_send_choices() {
        let (mut registry, _, preview, _) = fixture();
        assert!(registry.choices(&preview.preview_id).is_none());
        let policy = crate::model_routing::ModelRouting::default();
        let reviewed = policy.review(Some("nvidia/nemotron-fixture".into()));
        registry
            .bind_routing(&preview.preview_id, reviewed.clone())
            .unwrap();
        assert!(registry
            .bind_routing(&preview.preview_id, reviewed)
            .is_err());
        let (sharing, file_names, review) = registry.choices(&preview.preview_id).unwrap();
        assert_eq!(sharing, Sharing::Anonymized);
        assert!(!file_names);
        assert!(policy.matches(&review, Some("nvidia/nemotron-fixture")));
        assert!(!policy.matches(&review, Some("nvidia/nemotron-external-change")));
        registry.invalidate_process_session("new-collector-session");
        assert!(registry.choices(&preview.preview_id).is_none());
    }

    #[test]
    fn malformed_references_empty_questions_and_oversized_conversations_do_not_create_tickets() {
        let (mut registry, reference, _, _) = fixture();
        registry.invalidate_tickets();
        let mut bad = reference.clone();
        bad.source_id = r"C:\Users\private".into();
        assert!(registry
            .prepare(
                bad,
                SelectionIntent::Explain,
                None,
                None,
                vec![],
                Sharing::Anonymized,
                false,
                PrivacyContext::local(),
                0
            )
            .is_err());
        assert!(registry
            .prepare(
                reference.clone(),
                SelectionIntent::Explain,
                Some(" ".into()),
                None,
                vec![],
                Sharing::Anonymized,
                false,
                PrivacyContext::local(),
                0
            )
            .is_err());
        let past = vec![PastTurn {
            answer: "x".repeat(MAX_PAST_BYTES + 1),
            ..PastTurn::default()
        }];
        assert!(registry
            .prepare(
                reference,
                SelectionIntent::Explain,
                None,
                None,
                past,
                Sharing::Anonymized,
                false,
                PrivacyContext::local(),
                0
            )
            .is_err());
        assert!(registry.tickets.is_empty());
    }

    /// A synthetic live battery read (no device is opened).
    fn battery(charge: Value) -> SelectionSource {
        use pcagent_inspect::{Coverage, CoverageStatus, Inspection, Observation};
        let observation = |metric: &str, value: Value, unit: &str| Observation {
            observation_id: format!("measurement-{metric}"),
            measured_at_unix_ms: 4010,
            window: Window {
                started_at_unix_ms: 4000,
                finished_at_unix_ms: 4010,
                duration_ms: 10,
            },
            entity_ref: "battery-00000000-0000-4000-8000-00000000b001".into(),
            metric: metric.into(),
            status: if value.is_null() {
                CoverageStatus::Unsupported
            } else {
                CoverageStatus::Ok
            },
            value,
            unit: unit.into(),
            source: "fixture".into(),
            origin: Origin::SyntheticFixture,
        };
        SelectionSource::Battery(Inspection {
            status: pcagent_shared::Status::Ok,
            measured_at_unix_ms: 4020,
            window: Window {
                started_at_unix_ms: 3990,
                finished_at_unix_ms: 4020,
                duration_ms: 30,
            },
            origin: Origin::SyntheticFixture,
            observations: vec![
                observation("battery_charge_percent", charge, "percent"),
                observation("battery_charge_state", json!("discharging"), "category"),
            ],
            coverage: vec![Coverage {
                scope: "battery_devices".into(),
                status: CoverageStatus::Ok,
                source: "fixture".into(),
                detail: "Synthetic.".into(),
            }],
            names: Default::default(),
        })
    }

    #[test]
    fn battery_read_source_resolves_the_battery_resource_and_previews_only_battery_observations() {
        let mut registry = SelectionRegistry::default();
        let snapshot_id = registry.publish(source(2500)).unwrap();
        let source_id = registry.publish(battery(json!(78.1))).unwrap();
        let target = SelectionTarget::Resource {
            resource: Resource::Battery,
        };
        let listed = registry.sources();
        let battery_sources: Vec<_> = listed
            .iter()
            .filter(|descriptor| descriptor.reference.source_id == source_id)
            .collect();
        assert_eq!(battery_sources.len(), 1);
        assert_eq!(battery_sources[0].reference.target, target);
        assert_eq!(battery_sources[0].measured_at_unix_ms, Some(4010));
        assert!(!battery_sources[0].historical);
        assert!(listed.iter().all(|descriptor| {
            descriptor.reference.source_id == source_id || descriptor.reference.target != target
        }));
        let reference = SelectionRef { source_id, target };
        let preview = registry
            .prepare(
                reference.clone(),
                SelectionIntent::Explain,
                None,
                None,
                vec![],
                Sharing::Anonymized,
                false,
                PrivacyContext::local(),
                0,
            )
            .unwrap();
        assert_eq!(preview.measured_at_unix_ms, Some(4010));
        let observations = preview.message["initial_observation"]["observations"]
            .as_array()
            .unwrap();
        assert!(observations
            .iter()
            .any(|o| o["metric"] == "battery_charge_percent" && o["value"] == 78.1));
        assert!(observations
            .iter()
            .all(|o| o["metric"].as_str().unwrap().starts_with("battery_")));
        assert!(!preview
            .message
            .to_string()
            .contains("cpu_busy_basis_points"));
        assert_eq!(
            preview.selected_evidence["selection"]["target"],
            json!({"kind":"resource","resource":"battery"})
        );
        assert_eq!(
            preview.selected_evidence["selection"]["fresh_collection"],
            false
        );
        // The snapshot published alongside is a different source, untouched.
        assert!(registry
            .sources()
            .iter()
            .any(|descriptor| descriptor.reference.source_id == snapshot_id));
        // A read without any battery value offers nothing to select.
        let mut empty = battery(Value::Null);
        if let SelectionSource::Battery(read) = &mut empty {
            read.observations
                .retain(|o| o.metric == "battery_charge_percent");
        }
        assert!(registry.publish(empty).is_err());
    }

    #[test]
    fn registry_keeps_96_sources_before_evicting_the_oldest() {
        assert_eq!(MAX_SOURCES, 96);
        let mut registry = SelectionRegistry::default();
        let first = registry.publish(battery(json!(50.0))).unwrap();
        for value in 1..MAX_SOURCES as u64 {
            registry.publish(source(value)).unwrap();
        }
        assert_eq!(registry.sources.len(), 96);
        assert!(registry.sources.iter().any(|source| source.id == first));
        registry.publish(source(9999)).unwrap();
        assert_eq!(registry.sources.len(), 96);
        assert!(registry.sources.iter().all(|source| source.id != first));
    }
}
