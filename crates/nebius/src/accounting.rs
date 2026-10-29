//! Eval-only full-stream estimates. Never substitute these for a cancelled product stream.
use crate::{body_error, Usage, MAX_STREAM_BYTES};
use pcagent_agent::{estimate_cost, AgentError, Cost, Pricing};
use serde::{Deserialize, Serialize};
use std::{
    io::{BufRead, BufReader, Read, Take},
    sync::atomic::{AtomicBool, Ordering},
    time::Instant,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DrainedUsage {
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub total_tokens: u64,
    pub reasoning_tokens: Option<u64>,
}

impl DrainedUsage {
    pub(crate) fn validated(usage: Usage) -> Result<Self, AgentError> {
        let invalid = AgentError::InvalidResponse;
        let prompt_tokens = usage.prompt_tokens.ok_or_else(|| invalid.clone())?;
        let completion_tokens = usage.completion_tokens.ok_or_else(|| invalid.clone())?;
        let total_tokens = usage.total_tokens.ok_or_else(|| invalid.clone())?;
        let reasoning_tokens = usage
            .completion_tokens_details
            .and_then(|d| d.reasoning_tokens);
        if prompt_tokens.checked_add(completion_tokens) != Some(total_tokens)
            || reasoning_tokens.is_some_and(|n| n > completion_tokens)
        {
            return Err(invalid);
        }
        Ok(Self {
            prompt_tokens,
            completion_tokens,
            total_tokens,
            reasoning_tokens,
        })
    }
}

#[derive(Debug, Serialize)]
pub struct RequestAccounting {
    pub request: usize,
    pub model_id: String,
    pub answer_completed_ms: Option<u64>,
    pub stream_total_ms: u64,
    pub stream_complete: bool,
    pub usage: Option<DrainedUsage>,
    pub full_stream_cost: Option<Cost>,
    pub accounting_error: Option<AgentError>,
    #[serde(skip)]
    pub(crate) started: Instant,
    #[serde(skip)]
    pub(crate) deadline: Instant,
}

impl RequestAccounting {
    pub(crate) fn new(request: usize, model_id: String) -> Self {
        let started = Instant::now();
        Self {
            request,
            model_id,
            answer_completed_ms: None,
            stream_total_ms: 0,
            stream_complete: false,
            usage: None,
            full_stream_cost: None,
            accounting_error: None,
            started,
            deadline: started + pcagent_agent::REQUEST_TIMEOUT,
        }
    }

    pub(crate) fn record(&mut self, usage: Result<Option<DrainedUsage>, AgentError>) {
        match usage {
            Ok(usage) => {
                self.stream_complete = true;
                self.usage = usage;
            }
            Err(error) => self.accounting_error = Some(error),
        }
    }

    pub(crate) fn finish(&mut self, pricing: Option<&Pricing>) {
        self.stream_total_ms = self.started.elapsed().as_millis() as u64;
        self.full_stream_cost = self.usage.as_ref().and_then(|usage| {
            let pricing = pricing.filter(|p| p.model_id == self.model_id)?;
            estimate_cost(usage.prompt_tokens, usage.completion_tokens, pricing)
        });
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UsagePolicy {
    FullStreamDrain,
}

#[derive(Debug, Serialize)]
pub struct EvalAccounting<'a> {
    pub usage_policy: UsagePolicy,
    pub requests: &'a [RequestAccounting],
    pub full_stream_cost: Option<Cost>,
}

impl<'a> EvalAccounting<'a> {
    pub(crate) fn new(requests: &'a [RequestAccounting]) -> Self {
        let total = || {
            let pricing = &requests.first()?.full_stream_cost.as_ref()?.pricing;
            let (mut prompt, mut completion) = (0_u64, 0_u64);
            for request in requests {
                let cost = request.full_stream_cost.as_ref()?;
                if &cost.pricing != pricing {
                    return None;
                }
                prompt = prompt.checked_add(cost.prompt_tokens)?;
                completion = completion.checked_add(cost.completion_tokens)?;
            }
            estimate_cost(prompt, completion, pricing)
        };
        Self {
            usage_policy: UsagePolicy::FullStreamDrain,
            requests,
            full_stream_cost: total(),
        }
    }
}

#[derive(Deserialize)]
struct UsageChunk {
    usage: Option<Usage>,
}

/// Ownership closes the response on completion, errors, early exit, or client drop.
pub(crate) struct PendingUsage<R: Read> {
    pub(crate) reader: BufReader<Take<R>>,
    pub(crate) read: usize,
    pub(crate) first_usage: Option<Usage>,
    pub(crate) deadline: Instant,
}

impl<R: Read> PendingUsage<R> {
    pub(crate) fn drain(
        mut self,
        interrupt: Option<&AtomicBool>,
    ) -> Result<Option<DrainedUsage>, AgentError> {
        drain_tail(
            &mut self.reader,
            self.read,
            self.first_usage,
            interrupt,
            self.deadline,
        )
    }
}

/// Choice deltas are deliberately ignored, not deserialized into executable calls or callbacks.
pub(crate) fn drain_tail(
    reader: &mut impl BufRead,
    mut read: usize,
    first_usage: Option<Usage>,
    interrupt: Option<&AtomicBool>,
    deadline: Instant,
) -> Result<Option<DrainedUsage>, AgentError> {
    let mut usage = first_usage.map(DrainedUsage::validated).transpose()?;
    let mut line = String::new();
    loop {
        if interrupt.is_some_and(|flag| flag.load(Ordering::SeqCst)) {
            return Err(AgentError::Cancelled);
        }
        if Instant::now() >= deadline {
            return Err(AgentError::Timeout);
        }
        line.clear();
        let n = reader.read_line(&mut line).map_err(|e| body_error(&e))?;
        if interrupt.is_some_and(|flag| flag.load(Ordering::SeqCst)) {
            return Err(AgentError::Cancelled);
        }
        if Instant::now() >= deadline {
            return Err(AgentError::Timeout);
        }
        if n == 0 {
            return Err(AgentError::IncompleteResponse);
        }
        read += n;
        if read > MAX_STREAM_BYTES {
            return Err(AgentError::InvalidResponse);
        }
        let Some(data) = line.trim_end().strip_prefix("data:") else {
            continue;
        };
        let data = data.trim_start();
        if data == "[DONE]" {
            return Ok(usage);
        }
        let chunk: UsageChunk =
            serde_json::from_str(data).map_err(|_| AgentError::InvalidResponse)?;
        if let Some(reported) = chunk.usage {
            let reported = DrainedUsage::validated(reported)?;
            if usage.as_ref().is_some_and(|prior| prior != &reported) {
                return Err(AgentError::InvalidResponse);
            }
            usage = Some(reported);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{parse_reply, read_stream, read_stream_observed};
    use serde_json::{json, Value};
    use std::io::{self, Read};
    use std::time::Duration;

    fn answer() -> String {
        format!(
            "data: {}\n\n",
            json!({"choices":[{"delta":{"tool_calls":[{"index":0,"id":"first",
                "function":{"name":"submit_answer","arguments":"{\"text\":\"original\"}"}}]}}]})
        )
    }

    fn usage() -> String {
        format!(
            "data: {}\n\n",
            json!({"choices":[],"usage":{"prompt_tokens":20,"completion_tokens":30,
                "total_tokens":50,"completion_tokens_details":{"reasoning_tokens":10}}})
        )
    }

    fn signature(reply: pcagent_agent::Reply) -> Value {
        json!({"calls":reply.calls,"content":reply.content,"rejected":reply.rejected,
            "prompt":reply.prompt_tokens,"completion":reply.completion_tokens,
            "reasoning":reply.reasoning_tokens,"total":reply.total_tokens})
    }

    fn compare(reader: impl Read) -> RequestAccounting {
        let mut fast_calls = Vec::new();
        let fast = signature(parse_reply(
            read_stream(
                answer().as_bytes(),
                &mut |n, a| fast_calls.push((n.to_owned(), a.to_owned())),
                None,
            )
            .unwrap(),
        ));
        let mut drained_calls = Vec::new();
        let mut accounting = RequestAccounting::new(1, "nvidia/Nemotron-fixture".into());
        let drained = signature(parse_reply(
            read_stream_observed(
                reader,
                &mut |n, a| drained_calls.push((n.to_owned(), a.to_owned())),
                None,
                Some(&mut accounting),
            )
            .unwrap(),
        ));
        accounting.finish(None);
        assert_eq!(drained, fast);
        assert_eq!(fast_calls, drained_calls);
        assert!(accounting.answer_completed_ms.is_some());
        assert!(accounting.stream_total_ms >= accounting.answer_completed_ms.unwrap());
        accounting
    }

    #[test]
    fn duplicate_and_tool_bearing_tail_never_reprocesses_calls_or_callbacks() {
        let body = format!(
            "{}data: {}\n\ndata: {}\n\n{}data: [DONE]\n\n",
            answer(),
            json!({"choices":[{"delta":{"tool_calls":[{"index":0,"id":"duplicate",
                "function":{"name":"submit_answer","arguments":"{\"text\":\"replacement\"}"}},
                {"index":1,"id":"tail-tool","function":{"name":"get_processes","arguments":"{}"}}]}}]}),
            json!({"choices":"malformed choice shape is not parsed after freeze"}),
            usage()
        );
        let accounting = compare(body.as_bytes());
        assert!(accounting.stream_complete);
        assert_eq!(accounting.usage.unwrap().completion_tokens, 30);
        assert_eq!(accounting.accounting_error, None);
    }

    #[test]
    fn malformed_missing_conflicting_or_invalid_tail_usage_stays_unknown() {
        let invalid = [
            json!({"prompt_tokens":20,"completion_tokens":30,"total_tokens":51}),
            json!({"prompt_tokens":20,"completion_tokens":30,"total_tokens":50,
                "completion_tokens_details":{"reasoning_tokens":31}}),
            json!({"prompt_tokens":20,"completion_tokens":null,"total_tokens":50}),
            json!({"prompt_tokens":20,"completion_tokens":-1,"total_tokens":19}),
            json!({"prompt_tokens":u64::MAX,"completion_tokens":1,"total_tokens":0}),
            json!({"prompt_tokens":"20","completion_tokens":30,"total_tokens":50}),
        ];
        let mut tails = vec![
            "data: {broken\n\ndata: [DONE]\n\n".to_owned(),
            String::new(),
            usage(), // EOF without [DONE] is not a fully accounted stream.
            format!("{}data: {{\"usage\":{{\"prompt_tokens\":20,\"completion_tokens\":31,\"total_tokens\":51}}}}\n\ndata: [DONE]\n\n", usage()),
        ];
        tails.extend(
            invalid
                .into_iter()
                .map(|u| format!("data: {}\n\ndata: [DONE]\n\n", json!({"usage":u}))),
        );
        for tail in tails {
            let accounting = compare(format!("{}{tail}", answer()).as_bytes());
            assert!(accounting.usage.is_none());
            assert!(accounting.full_stream_cost.is_none());
            assert!(accounting.accounting_error.is_some());
        }
        let missing = compare(format!("{}data: [DONE]\n\n", answer()).as_bytes());
        assert!(missing.stream_complete);
        assert!(missing.usage.is_none());
        assert!(missing.full_stream_cost.is_none());
    }

    struct TailFailure<'a> {
        prefix: &'a [u8],
        kind: io::ErrorKind,
    }
    impl Read for TailFailure<'_> {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            if self.prefix.is_empty() {
                return Err(io::Error::from(self.kind));
            }
            self.prefix.read(buf)
        }
    }

    #[test]
    fn tail_errors_preserve_frozen_answer_but_pre_answer_errors_still_reject() {
        for (kind, error) in [
            (io::ErrorKind::TimedOut, AgentError::Timeout),
            (io::ErrorKind::ConnectionReset, AgentError::Network),
        ] {
            let prefix = answer();
            let accounting = compare(TailFailure {
                prefix: prefix.as_bytes(),
                kind,
            });
            assert_eq!(accounting.accounting_error, Some(error.clone()));
            assert!(accounting.usage.is_none());
            let result = read_stream_observed(
                TailFailure { prefix: &[], kind },
                &mut |_, _| {},
                None,
                Some(&mut RequestAccounting::new(
                    1,
                    "nvidia/Nemotron-fixture".into(),
                )),
            );
            assert_eq!(result.err(), Some(error));
        }
        let oversized = format!("{}{}", answer(), "x".repeat(MAX_STREAM_BYTES));
        assert_eq!(
            compare(oversized.as_bytes()).accounting_error,
            Some(AgentError::InvalidResponse)
        );
    }

    #[test]
    fn stop_before_answer_rejects_and_stop_after_answer_only_stops_accounting() {
        let flag = AtomicBool::new(false);
        let mut accounting = RequestAccounting::new(1, "nvidia/Nemotron-fixture".into());
        let envelope = read_stream_observed(
            format!("{}{}", answer(), usage()).as_bytes(),
            &mut |_, _| flag.store(true, Ordering::SeqCst),
            Some(&flag),
            Some(&mut accounting),
        )
        .unwrap();
        assert_eq!(parse_reply(envelope).calls.len(), 1);
        assert_eq!(accounting.accounting_error, Some(AgentError::Cancelled));
        assert!(accounting.usage.is_none());
        let before = read_stream_observed(answer().as_bytes(), &mut |_, _| {}, Some(&flag), None);
        assert_eq!(before.err(), Some(AgentError::Cancelled));
    }

    struct SlowTail {
        prefix: io::Cursor<Vec<u8>>,
        tail: io::Cursor<Vec<u8>>,
        waited: bool,
    }
    impl Read for SlowTail {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            let n = self.prefix.read(buf)?;
            if n != 0 {
                return Ok(n);
            }
            if !self.waited {
                std::thread::sleep(Duration::from_millis(25));
                self.waited = true;
            }
            self.tail.read(buf)
        }
    }

    #[test]
    fn answer_completion_is_measured_before_drain_elapsed_time() {
        let accounting = compare(SlowTail {
            prefix: io::Cursor::new(answer().into_bytes()),
            tail: io::Cursor::new(format!("{}data: [DONE]\n\n", usage()).into_bytes()),
            waited: false,
        });
        assert!(accounting.stream_total_ms - accounting.answer_completed_ms.unwrap() >= 20);
    }

    #[test]
    fn catalog_costs_are_separate_and_missing_requests_never_become_zero() {
        let mut request = compare(format!("{}{}data: [DONE]\n\n", answer(), usage()).as_bytes());
        let pricing = Pricing {
            model_id: request.model_id.clone(),
            prompt_usd_per_token: "0.000001".into(),
            completion_usd_per_token: "0.000002".into(),
            source: "https://api.tokenfactory.nebius.com/v1/models?verbose=true".into(),
            read_at_unix_ms: 1,
        };
        request.finish(Some(&pricing));
        assert_eq!(request.full_stream_cost.as_ref().unwrap().usd, "0.00008");
        let known = [request];
        assert_eq!(
            EvalAccounting::new(&known).full_stream_cost.unwrap().usd,
            "0.00008"
        );
        assert!(EvalAccounting::new(&[]).full_stream_cost.is_none());
        let missing = RequestAccounting::new(2, pricing.model_id.clone());
        let partial = [known.into_iter().next().unwrap(), missing];
        assert!(EvalAccounting::new(&partial).full_stream_cost.is_none());
    }

    #[test]
    fn normal_tool_streams_keep_product_usage_and_opt_in_is_never_default() {
        let mut client = crate::Nebius::new(crate::Config {
            api_key: "not-a-live-key".into(),
            model_id: "nvidia/Nemotron-fixture".into(),
            reasoning: crate::Reasoning::default(),
        })
        .unwrap();
        assert!(client.eval_accounting().is_none());
        client.enable_eval_usage_drain();
        assert!(client.eval_accounting().unwrap().requests.is_empty());
        let body = format!(
            "data: {}\n\n{}data: [DONE]\n\n",
            json!({"choices":[{"delta":{"tool_calls":[{"index":0,"id":"inspection",
                "function":{"name":"get_processes","arguments":"{}"}}]},"finish_reason":"tool_calls"}]}),
            usage()
        );
        let mut accounting = RequestAccounting::new(1, "nvidia/Nemotron-fixture".into());
        let fast = signature(parse_reply(
            read_stream(body.as_bytes(), &mut |_, _| {}, None).unwrap(),
        ));
        let drain = signature(parse_reply(
            read_stream_observed(body.as_bytes(), &mut |_, _| {}, None, Some(&mut accounting))
                .unwrap(),
        ));
        assert_eq!(fast, drain);
        assert_eq!(fast["prompt"], 20);
        assert_eq!(accounting.answer_completed_ms, None);
        assert_eq!(accounting.usage.unwrap().prompt_tokens, 20);
    }

    struct TurnReader {
        prefix: io::Cursor<Vec<u8>>,
        tail: io::Cursor<Vec<u8>>,
        investigation_done: std::sync::Arc<AtomicBool>,
        tail_reads: std::sync::Arc<std::sync::atomic::AtomicUsize>,
        drops: std::sync::Arc<std::sync::atomic::AtomicUsize>,
        fail: bool,
    }
    impl Read for TurnReader {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            let n = self.prefix.read(buf)?;
            if n != 0 {
                return Ok(n);
            }
            assert!(
                self.investigation_done.load(Ordering::SeqCst),
                "drain must not run inside the agent's budget"
            );
            self.tail_reads.fetch_add(1, Ordering::SeqCst);
            if self.fail {
                return Err(io::Error::from(io::ErrorKind::TimedOut));
            }
            self.tail.read(buf)
        }
    }
    impl Drop for TurnReader {
        fn drop(&mut self) {
            self.drops.fetch_add(1, Ordering::SeqCst);
        }
    }

    #[derive(Default)]
    struct TurnModel {
        drain: bool,
        fail_tail: bool,
        bad_prefix: bool,
        replies: std::collections::VecDeque<String>,
        requests: Vec<RequestAccounting>,
        pending: Vec<(usize, PendingUsage<TurnReader>)>,
        investigation_done: std::sync::Arc<AtomicBool>,
        tail_reads: std::sync::Arc<std::sync::atomic::AtomicUsize>,
        drops: std::sync::Arc<std::sync::atomic::AtomicUsize>,
        callbacks: usize,
    }
    impl pcagent_agent::Model for TurnModel {
        fn complete(
            &mut self,
            _: &[Value],
            _: &Value,
            _: pcagent_agent::ToolChoice,
            timeout: Duration,
        ) -> Result<pcagent_agent::Reply, AgentError> {
            let prefix = self.replies.pop_front().expect("unexpected inference");
            let mut accounting =
                RequestAccounting::new(self.requests.len() + 1, "nvidia/Nemotron-fixture".into());
            accounting.deadline = accounting.started + timeout;
            let reader = TurnReader {
                prefix: io::Cursor::new(prefix.into_bytes()),
                tail: io::Cursor::new(
                    format!("{}{}data: [DONE]\n\n", answer(), usage()).into_bytes(),
                ),
                investigation_done: self.investigation_done.clone(),
                tail_reads: self.tail_reads.clone(),
                drops: self.drops.clone(),
                fail: self.fail_tail,
            };
            let result = crate::read_stream_deferred(
                reader,
                &mut |_, _| self.callbacks += 1,
                None,
                self.drain.then_some(&mut accounting),
            );
            accounting.finish(None);
            let index = self.requests.len();
            self.requests.push(accounting);
            result.map(|(envelope, pending)| {
                if let Some(pending) = pending {
                    self.pending.push((index, pending));
                }
                parse_reply(envelope)
            })
        }
    }
    impl TurnModel {
        fn run(&mut self) -> pcagent_agent::Investigation {
            fn call(id: &str, name: &str, args: Value) -> String {
                format!(
                    "data: {}\n\n",
                    json!({"choices":[{"delta":{"tool_calls":[{
                    "index":0,"id":id,"function":{"name":name,"arguments":args.to_string()}
                }]},"finish_reason":"tool_calls"}]})
                )
            }
            let submission = |id, text| {
                call(
                    id,
                    "submit_answer",
                    json!({
                        "answer":text,"confidence":"medium","category":"other"
                    }),
                )
            };
            self.replies = [
                format!(
                    "{}{}data: [DONE]\n\n",
                    call("inspect", "get_system_snapshot", json!({})),
                    usage()
                ),
                if self.bad_prefix {
                    "data: {bad\n\n".into()
                } else {
                    submission("repair-needed", "See [O999].")
                },
                submission("accepted", "See [O1]."),
            ]
            .into();
            let mut snapshot = pcagent_shared::Snapshot::empty("synthetic-deferred-accounting");
            snapshot.origin = pcagent_shared::Origin::SyntheticFixture;
            snapshot.evidence.push(pcagent_shared::Evidence::measured(
                pcagent_shared::Metric::PhysicalTotalBytes,
                "system",
                8_589_934_592,
                "synthetic-deferred-accounting",
                pcagent_shared::Window::instant(),
            ));
            let mut tool_calls = 0;
            let mut tools = pcagent_agent::SnapshotTools::new(|_, _| {
                tool_calls += 1;
                snapshot.clone()
            });
            let report = pcagent_agent::investigate(
                self,
                "Describe the captured situation.",
                &snapshot,
                &mut tools,
                |_| {},
            );
            assert_eq!(tool_calls, 1);
            assert_eq!(self.tail_reads.load(Ordering::SeqCst), 0);
            self.investigation_done.store(true, Ordering::SeqCst);
            report
        }

        fn finish(&mut self) {
            for (index, pending) in std::mem::take(&mut self.pending) {
                self.requests[index].record(pending.drain(None));
                self.requests[index].finish(None);
            }
        }
    }

    #[test]
    fn full_loop_repairs_and_budget_checks_finish_before_any_accounting_tail() {
        for bad_prefix in [false, true] {
            let mut fast = TurnModel {
                bad_prefix,
                ..TurnModel::default()
            };
            let fast_report = fast.run();
            for fail_tail in [false, true] {
                let mut drained = TurnModel {
                    drain: true,
                    fail_tail,
                    bad_prefix,
                    ..TurnModel::default()
                };
                let report = drained.run();
                assert_eq!(report.error, fast_report.error);
                assert_eq!(report.execution_status, fast_report.execution_status);
                assert!(report.continuation.is_none());
                if bad_prefix {
                    assert_eq!(report.error, Some(AgentError::InvalidResponse));
                    assert!(report.answer.is_none());
                } else {
                    assert_eq!(report.error, None);
                    let answer = report.answer.as_ref().unwrap();
                    assert_eq!(answer.text, fast_report.answer.as_ref().unwrap().text);
                    assert_eq!(answer.repairs, 1);
                    assert!(answer.unverified.is_empty());
                    assert_eq!(report.stats.inference_calls, 3);
                    assert_eq!(report.stats.unknown_requests, 2);
                    assert!(report.stats.total_cost_usd.is_none());
                }
                assert_eq!(drained.callbacks, fast.callbacks);
                let frozen = serde_json::to_value(&report).unwrap();
                let callbacks = drained.callbacks;
                drained.finish();
                assert!(drained.pending.is_empty());
                assert_eq!(
                    drained.drops.load(Ordering::SeqCst),
                    report.stats.inference_calls
                );
                assert_eq!(serde_json::to_value(&report).unwrap(), frozen);
                assert_eq!(drained.callbacks, callbacks);
                if !bad_prefix {
                    for request in &drained.requests[1..] {
                        assert_eq!(
                            request.accounting_error,
                            fail_tail.then_some(AgentError::Timeout)
                        );
                        assert_eq!(request.usage.is_some(), !fail_tail);
                    }
                }
            }
        }
    }

    #[test]
    fn deferred_deadline_is_absolute_and_readers_close_even_without_finishing() {
        let mut model = TurnModel {
            drain: true,
            ..TurnModel::default()
        };
        model.run();
        for (_, pending) in &mut model.pending {
            pending.deadline = Instant::now();
        }
        model.finish();
        assert_eq!(model.tail_reads.load(Ordering::SeqCst), 0);
        assert_eq!(model.drops.load(Ordering::SeqCst), 3);
        for request in &model.requests[1..] {
            assert_eq!(request.accounting_error, Some(AgentError::Timeout));
            assert!(request.usage.is_none());
        }

        let mut abandoned = TurnModel {
            drain: true,
            ..TurnModel::default()
        };
        abandoned.run();
        let drops = abandoned.drops.clone();
        drop(abandoned);
        assert_eq!(drops.load(Ordering::SeqCst), 3);
    }
}
