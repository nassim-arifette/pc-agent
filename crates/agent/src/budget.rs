use crate::{AgentError, Investigation, MAX_DURATION, MAX_ROUNDS, MAX_TOOLS};
use serde::Serialize;
use std::time::{Duration, Instant};

/// A finite context bound even if the user repeatedly requests more investigation.
pub const MAX_EXTENSIONS: usize = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BudgetLimit {
    Rounds,
    Tools,
    Duration,
}

#[derive(Debug, Clone, Serialize)]
pub struct ExtensionRequest {
    pub reason: BudgetLimit,
    pub available: bool,
    pub additional_rounds: usize,
    pub additional_tool_calls: usize,
    pub additional_seconds: u64,
}

pub(crate) enum Checkpoint {
    Inference,
    Tools(usize),
    Time,
}

pub(crate) struct Budget {
    started: Instant,
    human_wait: Duration,
    extensions: usize,
}

impl Budget {
    pub(crate) fn new() -> Self {
        Self {
            started: Instant::now(),
            human_wait: Duration::ZERO,
            extensions: 0,
        }
    }
    pub(crate) fn remaining(&self) -> Duration {
        (MAX_DURATION * (1 + self.extensions as u32))
            .saturating_sub(self.started.elapsed().saturating_sub(self.human_wait))
    }
    fn reason(&self, report: &Investigation, checkpoint: &Checkpoint) -> Option<BudgetLimit> {
        if self.remaining().is_zero() {
            return Some(BudgetLimit::Duration);
        }
        match checkpoint {
            Checkpoint::Inference
                if report.stats.inference_calls >= MAX_ROUNDS * (1 + self.extensions) =>
            {
                Some(BudgetLimit::Rounds)
            }
            Checkpoint::Tools(n)
                if *n > MAX_TOOLS
                    || *n
                        > (MAX_TOOLS * (1 + self.extensions))
                            .saturating_sub(report.stats.tool_calls) =>
            {
                Some(BudgetLimit::Tools)
            }
            _ => None,
        }
    }
    /// Whether this checkpoint is over budget, without asking the user for more.
    pub(crate) fn exhausted(&self, report: &Investigation, checkpoint: &Checkpoint) -> bool {
        self.reason(report, checkpoint).is_some()
    }
    /// The callback is local UI consent, never a model tool or a provider instruction.
    pub(crate) fn ensure(
        &mut self,
        report: &mut Investigation,
        checkpoint: Checkpoint,
        on_extension: &mut impl FnMut(&Investigation, &ExtensionRequest) -> bool,
    ) -> bool {
        while let Some(reason) = self.reason(report, &checkpoint) {
            let available = self.extensions < MAX_EXTENSIONS
                && !matches!(checkpoint, Checkpoint::Tools(n) if n > MAX_TOOLS);
            let request = ExtensionRequest {
                reason,
                available,
                additional_rounds: MAX_ROUNDS,
                additional_tool_calls: MAX_TOOLS,
                additional_seconds: MAX_DURATION.as_secs(),
            };
            report.continuation = Some(request.clone());
            let partial = report.clone().partial(AgentError::Budget);
            let paused = Instant::now();
            let accepted = available && on_extension(&partial, &request);
            self.human_wait += paused.elapsed();
            if !accepted {
                return false;
            }
            self.extensions += 1;
            report.stats.extensions = self.extensions;
            report.continuation = None;
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{local_report, ExecutionStatus};
    use pcagent_shared::Snapshot;

    #[test]
    fn elapsed_budget_is_reported_before_another_request_and_extends_only_on_consent() {
        for accept in [false, true] {
            let mut budget = Budget::new();
            budget.started -= MAX_DURATION;
            let mut report = local_report(&Snapshot::empty("fixture"));
            let mut prompts = 0;
            assert_eq!(
                budget.ensure(&mut report, Checkpoint::Time, &mut |partial, request| {
                    prompts += 1;
                    assert_eq!(request.reason, BudgetLimit::Duration);
                    assert_eq!(partial.execution_status, ExecutionStatus::BudgetExhausted);
                    accept
                }),
                accept
            );
            assert_eq!(prompts, 1);
            assert_eq!(report.stats.extensions, usize::from(accept));
            assert_eq!(budget.remaining().is_zero(), !accept);
        }
    }
}
