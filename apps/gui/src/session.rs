//! Observable wrappers for an investigation shown live: the model and the toolbox report each
//! real step when it starts and ends, and the model refuses the next request once the person
//! asks to stop. Nothing here changes what the agent may do.
use crate::view::{self, error_code};
use pcagent_agent::{
    draft_text, AgentError, Args, Model, Pricing, Reply, ToolChoice, ToolOutput, ToolSpec, Toolbox,
    SUBMIT_ANSWER,
};
use pcagent_shared::{ProcessInstance, Status};
use serde::Serialize;
use serde_json::Value;
use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};

/// Drafts are sent to the window at most this often.
const DRAFT_EVERY: Duration = Duration::from_millis(80);

/// One real step of an investigation, in the order it happened.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Step {
    SelectedEvidenceLoaded {
        intent: pcagent_agent::selection::SelectionIntent,
        measured_at_unix_ms: Option<u64>,
    },
    ModelChanged {
        change: pcagent_agent::routing::ModelChange,
    },
    SnapshotStarted,
    SnapshotFinished {
        duration_ms: u64,
        measured: usize,
    },
    ModelStarted {
        request: usize,
    },
    ModelFinished {
        request: usize,
        duration_ms: u64,
        prompt_tokens: Option<u64>,
        completion_tokens: Option<u64>,
        /// `ok`, or the error code of a failed or rejected reply.
        outcome: String,
    },
    /// The answer as far as the model has written it. Shown while it streams, never checked:
    /// the reviewed answer replaces it when the run ends.
    AnswerDraft {
        request: usize,
        text: String,
    },
    ToolStarted {
        tool: String,
    },
    ToolFinished {
        tool: String,
        status: Status,
        duration_ms: u64,
        /// The exact query a web lookup sent (C6).
        #[serde(skip_serializing_if = "Option::is_none")]
        query: Option<String>,
    },
}

pub struct ObservedModel<M, F> {
    inner: M,
    stop: Arc<AtomicBool>,
    on_step: F,
    requests: usize,
}
impl<M, F> ObservedModel<M, F> {
    pub fn new(inner: M, stop: Arc<AtomicBool>, on_step: F) -> Self {
        Self {
            inner,
            stop,
            on_step,
            requests: 0,
        }
    }
}
impl<M: Model, F: FnMut(Step)> Model for ObservedModel<M, F> {
    fn complete(
        &mut self,
        messages: &[Value],
        tools: &Value,
        choice: ToolChoice,
        timeout: Duration,
    ) -> Result<Reply, AgentError> {
        // A request already sent cannot be recalled, but its eventual answer and tool calls
        // must not complete a stopped turn. Preserve any billed usage when it returns.
        if self.stop.load(Ordering::SeqCst) {
            return Err(AgentError::Cancelled);
        }
        self.requests += 1;
        let request = self.requests;
        (self.on_step)(Step::ModelStarted { request });
        let started = Instant::now();
        let on_step = &mut self.on_step;
        let stop = &self.stop;
        let mut shown = String::new();
        let mut last = None::<Instant>;
        let mut on_call = |name: &str, arguments: &str| {
            if stop.load(Ordering::SeqCst)
                || name != SUBMIT_ANSWER
                || last.is_some_and(|at| at.elapsed() < DRAFT_EVERY)
            {
                return;
            }
            let Some(text) = draft_text(arguments) else {
                return;
            };
            if text.trim().is_empty() || text == shown {
                return;
            }
            last = Some(Instant::now());
            shown.clone_from(&text);
            on_step(Step::AnswerDraft { request, text });
        };
        let mut result =
            self.inner
                .complete_streaming(messages, tools, choice, timeout, &mut on_call);
        if self.stop.load(Ordering::SeqCst) {
            match &mut result {
                Ok(reply) => {
                    reply.calls.clear();
                    reply.content = None;
                    reply.rejected = Some(AgentError::Cancelled);
                }
                Err(error) => *error = AgentError::Cancelled,
            }
        }
        let (prompt_tokens, completion_tokens, outcome) = match &result {
            Ok(reply) => (
                reply.prompt_tokens,
                reply.completion_tokens,
                reply
                    .rejected
                    .as_ref()
                    .map_or_else(|| "ok".to_owned(), error_code),
            ),
            Err(e) => (None, None, error_code(e)),
        };
        (self.on_step)(Step::ModelFinished {
            request,
            duration_ms: started.elapsed().as_millis() as u64,
            prompt_tokens,
            completion_tokens,
            outcome,
        });
        result
    }
    fn pricing(&self) -> Option<Pricing> {
        self.inner.pricing()
    }
    fn model_id(&self) -> Option<&str> {
        self.inner.model_id()
    }
    fn routing_state(&self) -> Option<pcagent_agent::routing::RoutingState> {
        self.inner.routing_state()
    }
    fn try_escalate(
        &mut self,
        reason: pcagent_agent::routing::EscalationReason,
        requests: usize,
    ) -> Option<pcagent_agent::routing::ModelChange> {
        if self.stop.load(Ordering::SeqCst) {
            return None;
        }
        let change = self.inner.try_escalate(reason, requests)?;
        (self.on_step)(Step::ModelChanged {
            change: change.clone(),
        });
        Some(change)
    }
}

/// The processes a turn has seen so far: its initial capture, the selected evidence and each tool
/// output, as the investigation keeps them (`local_processes`). The streamed draft is named from
/// them exactly as the reviewed answer will be (`view::localize`), so it does not change when the
/// run ends.
#[derive(Default)]
pub struct SeenProcesses(Mutex<Vec<ProcessInstance>>);
impl SeenProcesses {
    /// Keeps the first instance seen under each reference, like the investigation.
    pub fn add(&self, processes: &[ProcessInstance]) {
        let mut seen = self.0.lock().unwrap_or_else(|p| p.into_inner());
        for p in processes {
            if !seen.iter().any(|known| known.entity_ref == p.entity_ref) {
                seen.push(p.clone());
            }
        }
    }
    /// A draft as the window shows it.
    pub fn localize(&self, text: &str, readable: view::Readable<'_>) -> String {
        let seen = self.0.lock().unwrap_or_else(|p| p.into_inner());
        view::localize(text, &seen, readable)
    }
}

pub struct ObservedTools<T, F, G> {
    inner: T,
    on_start: F,
    on_output: G,
}
impl<T, F, G> ObservedTools<T, F, G> {
    /// `on_start` gets each tool's name as it starts, `on_output` its output once it returns.
    pub fn new(inner: T, on_start: F, on_output: G) -> Self {
        Self {
            inner,
            on_start,
            on_output,
        }
    }
    pub fn into_inner(self) -> T {
        self.inner
    }
}
impl<T: Toolbox, F: FnMut(&str), G: FnMut(&ToolOutput)> Toolbox for ObservedTools<T, F, G> {
    fn specs(&self) -> &[ToolSpec] {
        self.inner.specs()
    }
    fn sharing(&self) -> pcagent_agent::Sharing {
        self.inner.sharing()
    }
    fn run(&mut self, spec: &ToolSpec, args: &Args) -> ToolOutput {
        (self.on_start)(spec.name);
        let output = self.inner.run(spec, args);
        (self.on_output)(&output);
        output
    }
    fn definitions(&self) -> Value {
        self.inner.definitions()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pcagent_agent::{investigate, FunctionCall, SnapshotTools, ToolCall};
    use pcagent_diagnostics::analyze;
    use pcagent_shared::{Evidence, Metric, Snapshot, Window};
    use serde_json::json;
    use std::collections::VecDeque;

    struct Script(VecDeque<Reply>);
    impl Model for Script {
        fn complete(
            &mut self,
            _: &[Value],
            _: &Value,
            _: ToolChoice,
            _: Duration,
        ) -> Result<Reply, AgentError> {
            self.0.pop_front().ok_or(AgentError::Network)
        }
    }
    fn snapshot() -> Snapshot {
        let mut s = Snapshot::empty("fixture");
        s.evidence.push(Evidence::measured(
            Metric::CpuBusyBasisPoints,
            "system",
            500,
            "fixture",
            Window::instant(),
        ));
        s
    }

    #[test]
    fn routing_changes_are_reported_without_resetting_request_numbers_and_stop_has_priority() {
        use pcagent_agent::routing::{EscalationReason, ModelChange, RoutingMode, RoutingState};
        struct RoutedFixture {
            id: String,
            change: Option<ModelChange>,
            escalation_calls: usize,
        }
        impl Model for RoutedFixture {
            fn complete(
                &mut self,
                _: &[Value],
                _: &Value,
                _: ToolChoice,
                _: Duration,
            ) -> Result<Reply, AgentError> {
                Ok(Reply::default())
            }
            fn model_id(&self) -> Option<&str> {
                Some(&self.id)
            }
            fn routing_state(&self) -> Option<RoutingState> {
                Some(RoutingState {
                    mode: RoutingMode::Automatic,
                    current_model_id: Some(self.id.clone()),
                    calibration_reviewed_at_unix_ms: Some(1000),
                    change: self.change.clone(),
                    last_request_error: None,
                })
            }
            fn try_escalate(
                &mut self,
                reason: EscalationReason,
                requests: usize,
            ) -> Option<ModelChange> {
                self.escalation_calls += 1;
                if self.change.is_some() {
                    return None;
                }
                let change = ModelChange {
                    from_model_id: self.id.clone(),
                    to_model_id: "nvidia/ultra-fixture".into(),
                    reason,
                    after_requests: requests,
                };
                self.id = change.to_model_id.clone();
                self.change = Some(change.clone());
                Some(change)
            }
        }
        let fixture = || RoutedFixture {
            id: "nvidia/super-fixture".into(),
            change: None,
            escalation_calls: 0,
        };
        let stop = Arc::new(AtomicBool::new(false));
        let mut steps = Vec::new();
        {
            let mut model =
                ObservedModel::new(fixture(), Arc::clone(&stop), |step| steps.push(step));
            model
                .complete(
                    &[],
                    &json!([]),
                    ToolChoice::Required,
                    Duration::from_secs(1),
                )
                .unwrap();
            let change = model
                .try_escalate(EscalationReason::LowConfidence, 1)
                .unwrap();
            assert_eq!(model.model_id(), Some("nvidia/ultra-fixture"));
            assert_eq!(model.routing_state().unwrap().change, Some(change));
            model
                .complete(
                    &[],
                    &json!([]),
                    ToolChoice::Required,
                    Duration::from_secs(1),
                )
                .unwrap();
            assert!(model
                .try_escalate(EscalationReason::LowConfidence, 2)
                .is_none());
        }
        assert!(matches!(steps[0], Step::ModelStarted { request: 1 }));
        assert!(matches!(&steps[2], Step::ModelChanged { change } if change.after_requests == 1));
        assert!(matches!(steps[3], Step::ModelStarted { request: 2 }));
        stop.store(true, Ordering::SeqCst);
        let mut model = ObservedModel::new(fixture(), stop, |_| {
            panic!("A stopped model cannot emit a request or routing event")
        });
        assert!(model
            .try_escalate(EscalationReason::LowConfidence, 0)
            .is_none());
        assert_eq!(model.inner.escalation_calls, 0);
        assert_eq!(
            model
                .complete(
                    &[],
                    &json!([]),
                    ToolChoice::Required,
                    Duration::from_secs(1)
                )
                .unwrap_err(),
            AgentError::Cancelled
        );
    }
    fn tool_call() -> Reply {
        Reply {
            calls: vec![ToolCall {
                id: "a".into(),
                kind: "function".into(),
                function: FunctionCall {
                    name: "get_system_snapshot".into(),
                    arguments: "{}".into(),
                },
            }],
            prompt_tokens: Some(4_700),
            completion_tokens: Some(80),
            ..Reply::default()
        }
    }

    #[test]
    fn every_request_and_tool_is_reported_in_order() {
        let s = snapshot();
        let done = Reply {
            calls: vec![ToolCall {
                id: "answer".into(),
                kind: "function".into(),
                function: FunctionCall {
                    name: pcagent_agent::SUBMIT_ANSWER.into(),
                    arguments: json!({
                        "answer": format!("See [F{}].", analyze(&s).len() + 1),
                        "confidence": "low",
                        "category": "other"
                    })
                    .to_string(),
                },
            }],
            prompt_tokens: Some(5_000),
            completion_tokens: Some(20),
            ..Reply::default()
        };
        let mut steps = Vec::new();
        let mut started = Vec::new();
        {
            let stop = Arc::new(AtomicBool::new(false));
            let mut model =
                ObservedModel::new(Script(VecDeque::from([tool_call(), done])), stop, |step| {
                    steps.push(step)
                });
            let mut tools = ObservedTools::new(
                SnapshotTools::new(|_, _| snapshot()),
                |name: &str| started.push(name.to_owned()),
                |_: &ToolOutput| {},
            );
            let report = investigate(&mut model, "test", &s, &mut tools, |_| {});
            assert!(report.error.is_none(), "{:?}", report.error);
        }
        assert_eq!(started, ["get_system_snapshot"]);
        let kinds: Vec<String> = steps
            .iter()
            .map(|s| match s {
                Step::ModelStarted { request } => format!("start {request}"),
                Step::ModelFinished {
                    request,
                    prompt_tokens,
                    outcome,
                    ..
                } => format!("end {request} {prompt_tokens:?} {outcome}"),
                other => format!("{other:?}"),
            })
            .collect();
        assert_eq!(
            kinds,
            [
                "start 1",
                "end 1 Some(4700) ok",
                "start 2",
                "end 2 Some(5000) ok"
            ]
        );
    }

    /// Streams each call's arguments in three pieces before returning the reply.
    struct Streaming(VecDeque<Reply>);
    impl Model for Streaming {
        fn complete(
            &mut self,
            m: &[Value],
            t: &Value,
            c: ToolChoice,
            d: Duration,
        ) -> Result<Reply, AgentError> {
            self.complete_streaming(m, t, c, d, &mut |_, _| {})
        }
        fn complete_streaming(
            &mut self,
            _: &[Value],
            _: &Value,
            _: ToolChoice,
            _: Duration,
            on_call: &mut dyn FnMut(&str, &str),
        ) -> Result<Reply, AgentError> {
            let reply = self.0.pop_front().ok_or(AgentError::Network)?;
            for call in &reply.calls {
                let args = &call.function.arguments;
                for end in [args.len() / 3, 2 * args.len() / 3, args.len()] {
                    on_call(&call.function.name, &args[..end]);
                }
            }
            Ok(reply)
        }
    }

    #[test]
    fn the_answer_streams_as_drafts_but_tool_calls_do_not() {
        let answer = Reply {
            calls: vec![ToolCall {
                id: "answer".into(),
                kind: "function".into(),
                function: FunctionCall {
                    name: SUBMIT_ANSWER.into(),
                    arguments:
                        json!({"answer": "Votre PC ralentit parce que la mémoire est pleine.", "confidence": "high", "category": "memory"})
                            .to_string(),
                },
            }],
            ..Reply::default()
        };
        let mut steps = Vec::new();
        let mut model = ObservedModel::new(
            Streaming(VecDeque::from([tool_call(), answer])),
            Arc::new(AtomicBool::new(false)),
            |step| steps.push(step),
        );
        for _ in 0..2 {
            model
                .complete(
                    &[],
                    &json!([]),
                    ToolChoice::Required,
                    Duration::from_secs(1),
                )
                .unwrap();
        }
        let drafts: Vec<(usize, String)> = steps
            .iter()
            .filter_map(|s| match s {
                Step::AnswerDraft { request, text } => Some((*request, text.clone())),
                _ => None,
            })
            .collect();
        // Only the answer's request drafts; the first piece is shown at once, later ones are
        // throttled, and a draft is always a prefix of the text.
        assert!(!drafts.is_empty());
        assert!(drafts.iter().all(|(request, text)| *request == 2
            && "Votre PC ralentit parce que la mémoire est pleine.".starts_with(text.as_str())));
    }

    #[test]
    fn stopping_during_the_final_reply_discards_drafts_and_answer_but_keeps_usage() {
        struct StopDuringReply {
            replies: VecDeque<Reply>,
            stop: Arc<AtomicBool>,
        }
        impl Model for StopDuringReply {
            fn complete(
                &mut self,
                m: &[Value],
                t: &Value,
                c: ToolChoice,
                d: Duration,
            ) -> Result<Reply, AgentError> {
                self.complete_streaming(m, t, c, d, &mut |_, _| {})
            }
            fn complete_streaming(
                &mut self,
                _: &[Value],
                _: &Value,
                _: ToolChoice,
                _: Duration,
                on_call: &mut dyn FnMut(&str, &str),
            ) -> Result<Reply, AgentError> {
                let reply = self.replies.pop_front().ok_or(AgentError::Network)?;
                for call in &reply.calls {
                    if call.function.name == SUBMIT_ANSWER {
                        self.stop.store(true, Ordering::SeqCst);
                        on_call(&call.function.name, &call.function.arguments);
                    }
                }
                Ok(reply)
            }
        }
        let done = Reply {
            calls: vec![ToolCall {
                id: "answer".into(),
                kind: "function".into(),
                function: FunctionCall {
                    name: SUBMIT_ANSWER.into(),
                    arguments: json!({"answer":"The CPU is at 5 % [O1].", "confidence":"high",
                        "category":"cpu", "action_refs":[], "follow_ups":[]})
                    .to_string(),
                },
            }],
            prompt_tokens: Some(5_000),
            completion_tokens: Some(20),
            ..Reply::default()
        };
        let stop = Arc::new(AtomicBool::new(false));
        let mut steps = Vec::new();
        let mut model = ObservedModel::new(
            StopDuringReply {
                replies: VecDeque::from([tool_call(), done]),
                stop: stop.clone(),
            },
            stop,
            |step| steps.push(step),
        );
        let mut tools = SnapshotTools::new(|_, _| snapshot());
        let report = investigate(&mut model, "test", &snapshot(), &mut tools, |_| {});
        assert_eq!(report.error, Some(AgentError::Cancelled));
        assert!(report.answer.is_none());
        assert_eq!(report.stats.prompt_tokens, Some(9_700));
        assert_eq!(report.stats.completion_tokens, Some(100));
        assert!(!steps
            .iter()
            .any(|step| matches!(step, Step::AnswerDraft { .. })));
        assert!(
            matches!(steps.last(), Some(Step::ModelFinished { outcome, .. }) if outcome == "cancelled")
        );
    }

    #[test]
    fn drafts_are_named_from_the_processes_the_turn_has_seen_like_its_answer() {
        use std::collections::BTreeSet;
        let measured =
            || {
                let mut s = snapshot();
                s.processes
                    .extend([("proc-0000000a", 40), ("proc-0000000b", 41)].map(
                        |(reference, pid)| ProcessInstance {
                            entity_ref: reference.into(),
                            pid,
                            creation_time_filetime: 7,
                            boot_id: None,
                            executable_name: Some("browser.exe".into()),
                        },
                    ));
                s
            };
        let names = |pid: u32, _: u64| matches!(pid, 40 | 41).then(|| "Example Browser".to_owned());
        let text = "browser.exe (proc-0000000a) is the busiest program [O1].";
        let answer = Reply {
            calls: vec![ToolCall {
                id: "answer".into(),
                kind: "function".into(),
                function: FunctionCall {
                    name: SUBMIT_ANSWER.into(),
                    arguments: json!({"answer":text, "confidence":"high", "category":"cpu",
                        "action_refs":[], "follow_ups":[]})
                    .to_string(),
                },
            }],
            ..Reply::default()
        };
        let seen = SeenProcesses::default();
        let initial = snapshot();
        seen.add(&initial.processes);
        // Before any tool output, a draft has nothing to name.
        assert_eq!(seen.localize(text, &names), text);
        let report = {
            let mut model = ObservedModel::new(
                Script(VecDeque::from([tool_call(), answer])),
                Arc::new(AtomicBool::new(false)),
                |_| {},
            );
            let mut tools = ObservedTools::new(
                SnapshotTools::new(|_, _| measured()),
                |_: &str| {},
                |output: &ToolOutput| seen.add(&output.processes),
            );
            investigate(&mut model, "test", &initial, &mut tools, |_| {})
        };
        // The tool's processes were recorded as the investigation recorded them, so the last
        // draft reads exactly as the reviewed answer will.
        let refs = |list: &[ProcessInstance]| -> BTreeSet<String> {
            list.iter().map(|p| p.entity_ref.clone()).collect()
        };
        assert_eq!(refs(&seen.0.lock().unwrap()), refs(&report.local_processes));
        assert_eq!(
            seen.localize(text, &names),
            "Example Browser is the busiest program [O1]."
        );
        assert_eq!(
            seen.localize(text, &names),
            view::localize(text, &report.local_processes, &names)
        );
    }

    #[test]
    fn stopping_refuses_the_next_request_without_sending_it() {
        let stop = Arc::new(AtomicBool::new(false));
        let mut steps = 0;
        {
            let mut model = ObservedModel::new(
                Script(VecDeque::from([tool_call(), tool_call()])),
                stop.clone(),
                |_| steps += 1,
            );
            let s = snapshot();
            let mut tools = SnapshotTools::new(|_, _| {
                // The person presses Stop while the first tool runs.
                stop.store(true, Ordering::SeqCst);
                snapshot()
            });
            let report = investigate(&mut model, "test", &s, &mut tools, |_| {});
            assert_eq!(report.error, Some(AgentError::Cancelled));
            assert_eq!(report.stats.inference_calls, 2);
        }
        // Only the first request was sent: one start and one end.
        assert_eq!(steps, 2);
    }
}
