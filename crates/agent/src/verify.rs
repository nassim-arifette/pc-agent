//! Before/after verification of an executed action (spec 18). Computed locally from repeated
//! system samples; the model may explain the result but never computes it.
use pcagent_actions::{ActionKind, ActionResult, ItemOutcome};
use pcagent_diagnostics::{compare, Comparison};
use pcagent_shared::{Evidence, Metric, Snapshot, Status, SCHEMA_VERSION};
use serde::Serialize;

pub const VERIFIED_METRICS: &[Metric] = &[
    Metric::PhysicalAvailableBytes,
    Metric::SystemCommitBytes,
    Metric::CpuBusyBasisPoints,
];
/// System samples taken on each side of an action.
pub const VERIFY_SAMPLES: usize = 3;

#[derive(Debug, Clone, Serialize)]
pub struct VolumeChange {
    pub free_before_bytes: Option<u64>,
    pub free_after_bytes: Option<u64>,
    /// Free-space gain; `None` when unknown or when free space decreased.
    pub gained_bytes: Option<u64>,
    /// Bytes the adapter reports having removed.
    pub reported_removed_bytes: Option<u64>,
    pub status: Status,
}
#[derive(Debug, Clone, Serialize)]
pub struct ActionVerification {
    pub schema_version: u32,
    pub plan_id: String,
    pub action_kind: ActionKind,
    pub before_ref: Option<String>,
    pub after_ref: Option<String>,
    pub comparisons: Vec<Comparison>,
    pub volume: Option<VolumeChange>,
    pub limitations: Vec<&'static str>,
}

fn evidence(samples: &[Snapshot]) -> Vec<Evidence> {
    samples
        .iter()
        .flat_map(|s| s.evidence.iter().cloned())
        .collect()
}
pub fn reported_removed_bytes(result: &ActionResult) -> Option<u64> {
    result
        .items
        .iter()
        .try_fold(0_u64, |sum, item| match item.outcome {
            ItemOutcome::Completed {
                bytes_removed: Some(b),
                ..
            }
            | ItemOutcome::Partial {
                bytes_removed: Some(b),
                ..
            } => sum.checked_add(b),
            _ => None,
        })
}
/// `volume` is (free before, free after) for actions that change disk usage.
pub fn verify(
    result: &ActionResult,
    before: &[Snapshot],
    after: &[Snapshot],
    volume: Option<(Option<u64>, Option<u64>)>,
) -> ActionVerification {
    let (b, a) = (evidence(before), evidence(after));
    let comparisons = VERIFIED_METRICS
        .iter()
        .filter(|_| result.action_kind == ActionKind::RequestNormalClose)
        .map(|&m| compare(m, "system", &b, &a, VERIFY_SAMPLES))
        .collect();
    let volume = volume.map(|(free_before, free_after)| {
        let gained_bytes = free_before
            .zip(free_after)
            .and_then(|(b, a)| a.checked_sub(b));
        VolumeChange {
            free_before_bytes: free_before,
            free_after_bytes: free_after,
            gained_bytes,
            reported_removed_bytes: reported_removed_bytes(result),
            status: if free_before.is_some() && free_after.is_some() {
                Status::Ok
            } else {
                Status::Unsupported
            },
        }
    });
    let mut limitations =
        vec!["Short windows around one action; background activity can dominate the difference."];
    if matches!(
        result.action_kind,
        ActionKind::OpenInExplorer | ActionKind::OpenSettingsPage
    ) {
        limitations
            .push("Opening Explorer or Settings is not expected to change resource use by itself.");
    }
    if volume.is_some() {
        limitations.push("Free-space changes include writes by other programs during the window.");
    }
    ActionVerification {
        schema_version: SCHEMA_VERSION,
        plan_id: result.plan_id.clone(),
        action_kind: result.action_kind,
        before_ref: result.measurement.before_ref.clone(),
        after_ref: result.measurement.after_ref.clone(),
        comparisons,
        volume,
        limitations,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pcagent_actions::{ItemResult, MeasurementHooks, OverallOutcome};
    use pcagent_diagnostics::Outcome;
    use pcagent_shared::{Origin, Window};

    fn sample(at: u64, available: u64) -> Snapshot {
        let mut s = Snapshot::empty("verify");
        s.origin = Origin::SyntheticFixture;
        let window = Window {
            started_at_unix_ms: at - 1000,
            finished_at_unix_ms: at,
            duration_ms: 1000,
        };
        let mut e = Evidence::measured(
            Metric::PhysicalAvailableBytes,
            "system",
            available,
            "fixture",
            window,
        );
        e.origin = Origin::SyntheticFixture;
        s.evidence.push(e);
        s
    }
    fn result(removed: Option<u64>) -> ActionResult {
        ActionResult {
            schema_version: 1,
            plan_id: "plan-1".into(),
            fingerprint: "f".into(),
            action_kind: ActionKind::RunCleanupAdapter,
            overall: OverallOutcome::Completed,
            journal_status: Status::Ok,
            items: vec![ItemResult {
                item_index: 0,
                target_ref: "synthetic_demo_cache".into(),
                outcome: ItemOutcome::Completed {
                    detail: None,
                    bytes_removed: removed,
                },
            }],
            measurement: MeasurementHooks {
                before_ref: Some("before".into()),
                operation_started_at_unix_ms: 3500,
                operation_finished_at_unix_ms: 3600,
                after_ref: Some("after".into()),
                after_recorded_at_unix_ms: Some(9000),
            },
        }
    }

    #[test]
    fn verdicts_come_from_local_samples_and_unknown_stays_unknown() {
        let before: Vec<_> = (1..=3).map(|i| sample(i * 1000, 100 + i)).collect();
        let after: Vec<_> = (5..=7).map(|i| sample(i * 1000, 500 + i)).collect();
        let mut close = result(Some(64));
        close.action_kind = ActionKind::RequestNormalClose;
        let v = verify(&close, &before, &after, Some((Some(1000), Some(1064))));
        let available = &v.comparisons[0];
        assert_eq!(available.metric, Metric::PhysicalAvailableBytes);
        assert_eq!(available.outcome, Outcome::Improved);
        assert_eq!(v.comparisons[2].outcome, Outcome::NeedsMoreMeasurement);
        let volume = v.volume.unwrap();
        assert_eq!(
            (volume.gained_bytes, volume.reported_removed_bytes),
            (Some(64), Some(64))
        );

        let v = verify(&result(None), &before, &after, Some((Some(1000), None)));
        let volume = v.volume.unwrap();
        assert_eq!(volume.status, Status::Unsupported);
        assert_eq!(
            (volume.gained_bytes, volume.reported_removed_bytes),
            (None, None)
        );
        let shrunk = verify(
            &result(None),
            &before,
            &after,
            Some((Some(1000), Some(900))),
        );
        assert_eq!(shrunk.volume.unwrap().gained_bytes, None);

        let single = verify(&close, &before[2..], &after[..1], None);
        assert_eq!(single.comparisons[0].outcome, Outcome::NeedsMoreMeasurement);
        // Only limitations that apply to this action and measurement are listed.
        assert_eq!(single.limitations.len(), 1);
        assert!(!shrunk.limitations.iter().any(|l| l.contains("Settings")));
        assert!(shrunk.limitations.iter().any(|l| l.contains("Free-space")));
    }
}
