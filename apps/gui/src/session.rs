//! Observable wrappers for an investigation shown live: the model and the toolbox report each
//! real step when it starts and ends, and the model refuses the next request once the person
//! asks to stop. Nothing here changes what the agent may do.
use crate::view::error_code;
use pcagent_agent::{
    draft_text, AgentError, Args, Model, Pricing, Reply, ToolChoice, ToolOutput, ToolSpec, Toolbox,
    SUBMIT_ANSWER,
};
use pcagent_shared::Status;
use serde::Serialize;
use serde_json::Value;
use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};

/// Drafts are sent to the window at most this often.
const DRAFT_EVERY: Duration = Duration::from_millis(80);

/// One real step of an investigation, in the order it happened.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Step {
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
}

pub struct ObservedTools<T, F> {
    inner: T,
    on_start: F,
}
impl<T, F> ObservedTools<T, F> {
    pub fn new(inner: T, on_start: F) -> Self {
        Self { inner, on_start }
    }
    pub fn into_inner(self) -> T {
        self.inner
    }
}
impl<T: Toolbox, F: FnMut(&str)> Toolbox for ObservedTools<T, F> {
    fn specs(&self) -> &[ToolSpec] {
        self.inner.specs()
    }
    fn sharing(&self) -> pcagent_agent::Sharing {
        self.inner.sharing()
    }
    fn run(&mut self, spec: &ToolSpec, args: &Args) -> ToolOutput {
        (self.on_start)(spec.name);
        self.inner.run(spec, args)
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
            let mut tools =
                ObservedTools::new(SnapshotTools::new(|_, _| snapshot()), |name: &str| {
                    started.push(name.to_owned())
                });
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
                        json!({"text": "Votre PC ralentit parce que la mémoire est pleine."})
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
