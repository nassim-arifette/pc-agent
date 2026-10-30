//! Optional next-measure planner. Existing tool validation, evidence and final review still apply.
use crate::{AgentError, FunctionCall, Model, Pricing, Reply, ToolCall, ToolChoice};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Duration,
};

pub const MODEL_ID: &str = "local/nemotron-nano-4b-specialist-v0.1.0";
pub const BASE_MODEL: &str = "nvidia/Llama-3.1-Nemotron-Nano-4B-v1.1";
pub const BASE_REVISION: &str = "d552708a9d575fa8d4a690b988fd870d65279f98";
pub const METRICS: &[&str] = &[
    "cpu_busy_basis_points",
    "cpu_busiest_core_basis_points",
    "physical_available_bytes",
    "disk_read_latency_microseconds",
    "hard_fault_pages_per_second_x100",
];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Call {
    pub tool: String,
    pub arguments: Value,
}

pub fn calls() -> Vec<Call> {
    let mut values = vec![
        json!({"tool":"get_system_snapshot","arguments":{}}),
        json!({"tool":"record_activity","arguments":{"duration_seconds":30}}),
    ];
    for sort_by in ["cpu", "private_commit", "io_read", "io_write"] {
        values.push(json!({"tool":"get_processes","arguments":{"limit":5,"sort_by":sort_by}}));
    }
    for metric in METRICS {
        values.push(
            json!({"tool":"get_metric_history","arguments":{"metric":metric,"window_seconds":60}}),
        );
    }
    values
        .into_iter()
        .map(|v| serde_json::from_value(v).expect("static call"))
        .collect()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Decision {
    pub decision: String,
    pub hypotheses: Vec<String>,
    pub evidence_refs: Vec<String>,
    pub missing: Vec<String>,
    #[serde(deserialize_with = "required_call")]
    pub next_call: Option<Call>,
    pub premature_conclusions: Vec<String>,
}

fn required_call<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<Call>, D::Error> {
    Option::deserialize(deserializer)
}

fn closed(values: &[String], choices: &[&str]) -> bool {
    values
        .iter()
        .enumerate()
        .all(|(i, value)| choices.contains(&value.as_str()) && !values[..i].contains(value))
}

impl Decision {
    pub fn validate(&self, state: &Value, tools: &Value) -> bool {
        let refs: Vec<&str> = state["observations"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|o| o["id"].as_str())
            .collect();
        ["collect", "ask", "summarize", "handoff"].contains(&self.decision.as_str())
            && !self.hypotheses.is_empty()
            && closed(
                &self.hypotheses,
                &[
                    "cpu_contention",
                    "memory_pressure",
                    "disk_contention",
                    "none_observed",
                    "unknown",
                ],
            )
            && closed(&self.evidence_refs, &refs)
            && closed(
                &self.missing,
                &[
                    "current_measurements",
                    "persistence",
                    "sensor_coverage",
                    "symptom_timing",
                    "correlated_paging_and_io",
                    "process_contributors",
                ],
            )
            && closed(
                &self.premature_conclusions,
                &["hardware_failure", "memory_leak", "upgrade_required"],
            )
            && (self.decision == "collect") == self.next_call.is_some()
            && self.next_call.as_ref().is_none_or(|call| {
                calls().contains(call)
                    && !state["completed_calls"]
                        .as_array()
                        .is_some_and(|done| done.contains(&json!(call)))
                    && tools.as_array().is_some_and(|tools| {
                        tools
                            .iter()
                            .any(|tool| tool["function"]["name"] == call.tool)
                    })
            })
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Event {
    Started,
    Decided {
        decision: Decision,
        duration_ms: u64,
        device: String,
    },
    Handoff {
        reason: String,
    },
}

pub struct Planned {
    pub decision: Decision,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub duration_ms: u64,
    pub device: String,
}

pub trait Backend {
    fn decide(
        &mut self,
        state: &Value,
        timeout: Duration,
        stop: &Arc<AtomicBool>,
    ) -> Result<Planned, &'static str>;
}

/// This wrapper returns only ordinary tool calls. The existing controller executes them.
pub struct Specialist<M, S, F> {
    analyst: M,
    backend: Option<S>,
    on_event: F,
    stop: Arc<AtomicBool>,
    local_reply: bool,
    requests: u64,
    handoff_context: Option<Value>,
}

impl<M: Model, S: Backend, F: FnMut(Event)> Specialist<M, S, F> {
    pub fn new(analyst: M, backend: Option<S>, on_event: F) -> Self {
        Self {
            analyst,
            backend,
            on_event,
            stop: Arc::new(AtomicBool::new(false)),
            local_reply: false,
            requests: 0,
            handoff_context: None,
        }
    }

    fn plan(
        &mut self,
        messages: &[Value],
        tools: &Value,
        choice: ToolChoice,
        timeout: Duration,
    ) -> Result<Option<Reply>, AgentError> {
        if self.stop.load(Ordering::SeqCst) {
            return Err(AgentError::Cancelled);
        }
        let Some(backend) = self.backend.as_mut() else {
            return Ok(None);
        };
        if choice == ToolChoice::SubmitAnswer {
            self.handoff("answer_review");
            return Ok(None);
        }
        let Some(state) = state_from_messages(messages) else {
            self.handoff("unsupported_context");
            return Ok(None);
        };
        (self.on_event)(Event::Started);
        let planned = backend.decide(&state, timeout, &self.stop);
        if self.stop.load(Ordering::SeqCst) {
            return Err(AgentError::Cancelled);
        }
        let planned = match planned {
            Ok(planned) if planned.decision.validate(&state, tools) => planned,
            Ok(_) => {
                self.handoff("invalid_decision");
                return Ok(None);
            }
            Err(reason) => {
                self.handoff(reason);
                return Ok(None);
            }
        };
        (self.on_event)(Event::Decided {
            decision: planned.decision.clone(),
            duration_ms: planned.duration_ms,
            device: planned.device,
        });
        let Some(call) = planned.decision.next_call else {
            self.handoff_context = Some(json!({"decision": planned.decision.decision,
                "evidence_refs": planned.decision.evidence_refs}));
            self.handoff(&planned.decision.decision);
            return Ok(None);
        };
        self.local_reply = true;
        self.requests += 1;
        Ok(Some(Reply {
            calls: vec![ToolCall {
                id: format!("specialist-{}", self.requests),
                kind: "function".into(),
                function: FunctionCall {
                    name: call.tool,
                    arguments: call.arguments.to_string(),
                },
            }],
            prompt_tokens: Some(planned.input_tokens),
            completion_tokens: Some(planned.output_tokens),
            total_tokens: Some(planned.input_tokens + planned.output_tokens),
            ..Reply::default()
        }))
    }

    fn handoff(&mut self, reason: &str) {
        self.backend = None;
        self.local_reply = false;
        (self.on_event)(Event::Handoff {
            reason: reason.into(),
        });
    }
}

impl<M: Model, S: Backend, F: FnMut(Event)> Model for Specialist<M, S, F> {
    fn complete(
        &mut self,
        messages: &[Value],
        tools: &Value,
        choice: ToolChoice,
        timeout: Duration,
    ) -> Result<Reply, AgentError> {
        self.complete_streaming(messages, tools, choice, timeout, &mut |_, _| {})
    }
    fn complete_streaming(
        &mut self,
        messages: &[Value],
        tools: &Value,
        choice: ToolChoice,
        timeout: Duration,
        on_call: &mut dyn FnMut(&str, &str),
    ) -> Result<Reply, AgentError> {
        let started = std::time::Instant::now();
        if let Some(reply) = self.plan(messages, tools, choice, timeout)? {
            return Ok(reply);
        }
        self.local_reply = false;
        let remaining = timeout.saturating_sub(started.elapsed());
        if remaining.is_zero() {
            return Err(AgentError::Timeout);
        }
        let mut context = messages.to_vec();
        if let Some(handoff) = &self.handoff_context {
            context.push(json!({"role":"system", "content": format!(
                "An experimental local next-measure planner handed off: {}. Its output is not evidence or a diagnosis. Use only the real tool observations and original citations. If it requested clarification, ask the user; never assume an answer or symptom overlap. All original verification and sharing rules apply.", handoff)}));
        }
        self.analyst
            .complete_streaming(&context, tools, choice, remaining, on_call)
    }
    fn interrupt(&mut self, stop: Arc<AtomicBool>) {
        self.stop = stop.clone();
        self.analyst.interrupt(stop);
    }
    fn model_id(&self) -> Option<&str> {
        if self.local_reply {
            Some(MODEL_ID)
        } else {
            self.analyst.model_id()
        }
    }
    fn pricing(&self) -> Option<Pricing> {
        if self.local_reply {
            Some(Pricing {
                model_id: MODEL_ID.into(),
                prompt_usd_per_token: "0".into(),
                completion_usd_per_token: "0".into(),
                source: "local execution; no provider billing; device costs excluded".into(),
                read_at_unix_ms: pcagent_shared::unix_ms(),
            })
        } else {
            self.analyst.pricing()
        }
    }
    fn routing_state(&self) -> Option<crate::routing::RoutingState> {
        self.analyst.routing_state()
    }
    fn try_escalate(
        &mut self,
        reason: crate::routing::EscalationReason,
        requests: usize,
    ) -> Option<crate::routing::ModelChange> {
        if self.backend.is_some() {
            None
        } else {
            self.analyst
                .try_escalate(reason, requests.saturating_sub(self.requests as usize))
        }
    }
}

pub fn state_from_messages(messages: &[Value]) -> Option<Value> {
    let (start, question) = messages.iter().enumerate().rev().find_map(|(i, message)| {
        if message["role"] != "user" {
            return None;
        }
        let value: Value = serde_json::from_str(message["content"].as_str()?).ok()?;
        value.get("initial_observation")?;
        Some((i, value["question"].as_str()?.to_owned()))
    })?;
    let mut calls = BTreeMap::<String, Call>::new();
    let mut observations = Vec::new();
    let mut completed = Vec::new();
    for message in &messages[start + 1..] {
        if message["role"] == "assistant" {
            for call in message["tool_calls"].as_array().into_iter().flatten() {
                let arguments =
                    serde_json::from_str(call["function"]["arguments"].as_str()?).ok()?;
                calls.insert(
                    call["id"].as_str()?.into(),
                    Call {
                        tool: call["function"]["name"].as_str()?.into(),
                        arguments,
                    },
                );
            }
        } else if message["role"] == "tool" {
            let call = calls.get(message["tool_call_id"].as_str()?)?;
            let output: Value = serde_json::from_str(message["content"].as_str()?).ok()?;
            let citation = output["observation_id"].as_str()?;
            observations.push(json!({"id":citation,"tool":call.tool,"arguments":call.arguments,"output":project(&output)}));
            completed.push(json!(call));
        }
    }
    Some(
        json!({"symptom":question,"symptom_active_now":null,"observations":observations,"completed_calls":completed,
        "data_origin":"live PC measurements; symptom timing unknown; reduced read-only projection"}),
    )
}

fn project(value: &Value) -> Value {
    let mut out = serde_json::Map::new();
    for key in [
        "schema_version",
        "status",
        "origin",
        "measured_at_unix_ms",
        "window",
        "window_ms",
        "limitations",
        "inspection_complete",
        "metric",
        "unit",
        "entity_ref",
        "from_unix_ms",
        "to_unix_ms",
        "coverage_fraction",
        "gaps",
        "sources",
        "same_result_as",
    ] {
        if let Some(value) = value.get(key) {
            out.insert(key.into(), value.clone());
        }
    }
    if let Some(measurements) = value.get("measurements") {
        let mut measured =
            json!({"values":{},"units":{},"not_measured":measurements["not_measured"]});
        if let Some(entities) = measurements["values"].as_object() {
            for (entity, values) in entities {
                let values: serde_json::Map<_, _> = values
                    .as_object()
                    .into_iter()
                    .flatten()
                    .filter(|(key, _)| {
                        METRICS.contains(&key.as_str())
                            || [
                                "physical_total_bytes",
                                "disk_write_latency_microseconds",
                                "process_private_commit_bytes",
                                "process_cpu_basis_points",
                                "process_io_read_bytes_per_second",
                                "process_io_write_bytes_per_second",
                            ]
                            .contains(&key.as_str())
                    })
                    .map(|(key, v)| {
                        measured["units"][key] = measurements["units"][key].clone();
                        (key.clone(), v.clone())
                    })
                    .collect();
                if !values.is_empty() {
                    measured["values"][entity] = json!(values);
                }
            }
        }
        out.insert("measurements".into(), measured);
    }
    for key in ["points", "histories"] {
        if let Some(points) = value[key].as_array() {
            if key == "histories" {
                out.insert(
                    key.into(),
                    Value::Array(points.iter().map(project).collect()),
                );
            } else {
                let selected: Vec<_> = if points.len() <= 6 {
                    points.clone()
                } else {
                    (0..6)
                        .map(|i| points[i * (points.len() - 1) / 5].clone())
                        .collect()
                };
                out.insert(
                    "omitted_points".into(),
                    json!(points.len() - selected.len()),
                );
                out.insert(key.into(), json!(selected));
            }
        }
    }
    Value::Object(out)
}
