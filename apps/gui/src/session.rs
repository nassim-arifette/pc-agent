//! Observable wrappers for an investigation shown live: the model and the toolbox report each
//! real step when it starts and ends, and the model refuses the next request once the person
//! asks to stop. Nothing here changes what the agent may do.
use crate::view::error_code;
use pcagent_agent::{
    AgentError, Args, Model, Pricing, Reply, ToolChoice, ToolOutput, ToolSpec, Toolbox,
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
        // A request already sent cannot be recalled; stopping takes effect at the next one.
        if self.stop.load(Ordering::SeqCst) {
            return Err(AgentError::Cancelled);
        }
        self.requests += 1;
        let request = self.requests;
        (self.on_step)(Step::ModelStarted { request });
        let started = Instant::now();
        let result = self.inner.complete(messages, tools, choice, timeout);
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
