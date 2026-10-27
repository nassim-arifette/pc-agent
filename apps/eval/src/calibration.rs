//! Offline compilation of explicit human annotations bound to actual replay artifacts.
//! Declared live origin is checked, not inferred from a model name or a successful transport.
use crate::{files, fingerprint, Result};
use pcagent_agent::{
    estimate_cost,
    replay::{RecordedCall, RecordedOutput, RecordedRun, ReplayTools, Scenario, SCENARIO_VERSION},
    routing::{
        Calibration, CalibrationCase, CalibrationOrigin, CalibrationRuntime, EscalationReason,
        RoutingMode, RoutingSettings, CALIBRATION_SCHEMA_VERSION, MAX_CALIBRATION_BYTES,
        MAX_CALIBRATION_CASES, REPLAY_HARNESS_VERSION,
    },
    Pricing, Sharing, Toolbox, MAX_QUESTION_BYTES, MAX_REPAIRS, MAX_ROUNDS,
};
use pcagent_shared::{unix_ms, Origin};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
};

const MAX_ANNOTATION_BYTES: usize = 256 * 1024;
const CATALOG_SOURCE: &str = "https://api.tokenfactory.nebius.com/v1/models?verbose=true";
const PICO_PER_USD: u128 = 1_000_000_000_000;

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
enum HumanSource {
    Human,
}
#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
enum QualityScale {
    #[serde(rename = "routing_human_0_to_4")]
    RoutingHuman0To4,
}
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct HumanReview {
    source: HumanSource,
    quality_scale: QualityScale,
    human_reviewed: bool,
    trigger: EscalationReason,
    primary_quality: u8,
    ultra_quality: u8,
    primary_critical_error: bool,
    ultra_critical_error: bool,
    rationale: String,
}
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct AnnotatedPair {
    scenario: PathBuf,
    primary_result: PathBuf,
    ultra_result: PathBuf,
    scenario_sha256: String,
    tool_registry_sha256: String,
    question_sha256: String,
    sharing: Sharing,
    primary_trace_sha256: String,
    ultra_trace_sha256: String,
    review: HumanReview,
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Annotations {
    schema_version: u32,
    super_model_id: String,
    ultra_model_id: String,
    reviewed_at_unix_ms: u64,
    maximum_extra_cost_usd: String,
    pairs: Vec<AnnotatedPair>,
}
#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum ReplayMode {
    NebiusReplay,
}
/// Import only the result envelope this harness actually emits. Raw reports remain local.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReplayResult {
    scenario_sha256: String,
    tool_registry_sha256: String,
    scenario_origin: Origin,
    captured_at_unix_ms: u64,
    harness_version: String,
    runtime: CalibrationRuntime,
    schema_version: u32,
    scenario: String,
    question: String,
    sharing: Sharing,
    mode: ReplayMode,
    run: RecordedRun,
    calls: Vec<RecordedCall>,
    replay_misses: Vec<Value>,
    served_captures: usize,
    reused_calls: usize,
    unused_captures: usize,
    duration_ms: u64,
    replay_complete: bool,
}
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
struct Usage {
    prompt_tokens: Option<u64>,
    completion_tokens: Option<u64>,
    reasoning_tokens: Option<u64>,
}
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct Price {
    model_id: String,
    prompt_usd_per_token: String,
    completion_usd_per_token: String,
    source: String,
    read_at_unix_ms: u64,
}
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct RecordedCost {
    usd: String,
    prompt_tokens: u64,
    completion_tokens: u64,
    pricing: Price,
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ModelUsage {
    request: usize,
    model_id: Option<String>,
    usage: Usage,
    cost: Option<RecordedCost>,
    #[serde(default, rename = "duration_ms")]
    _duration_ms: Option<u64>,
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Stats {
    inference_calls: usize,
    tool_calls: usize,
    inspection_calls: usize,
    total_tokens: Option<u64>,
    prompt_tokens: Option<u64>,
    completion_tokens: Option<u64>,
    reasoning_tokens: Option<u64>,
    requests: Vec<Usage>,
    cost: Option<RecordedCost>,
    // Old real single-model replay traces predate per-model usage. Their full dated
    // aggregate price remains checkable; absent new fields are not fabricated requests.
    #[serde(default)]
    model_requests: Vec<ModelUsage>,
    #[serde(default)]
    total_cost_usd: Option<String>,
    #[serde(default)]
    routing: Option<Value>,
    extensions: usize,
    #[serde(default)]
    repair_requests: Option<Vec<pcagent_agent::Repair>>,
    #[serde(default)]
    known_prompt_tokens: Option<u64>,
    #[serde(default)]
    known_completion_tokens: Option<u64>,
    #[serde(default)]
    unknown_requests: Option<usize>,
    #[serde(default)]
    cost_lower_bound: Option<RecordedCost>,
    #[serde(default)]
    web_usage: Option<pcagent_agent::web_usage::WebUsage>,
}
struct Measurements {
    requests: usize,
    duration_ms: u64,
    started_at_unix_ms: u64,
    cost_usd: String,
}
#[derive(Debug, Serialize)]
pub(crate) struct Provenance {
    schema_version: u32,
    runtime: CalibrationRuntime,
    annotations_sha256: String,
    calibration_sha256: String,
    compiler_version: &'static str,
    cost_basis: &'static str,
    evidence_basis: &'static str,
    pairs: Vec<PairProvenance>,
}
#[derive(Debug, Serialize)]
struct PairProvenance {
    scenario_file_sha256: String,
    scenario_sha256: String,
    tool_registry_sha256: String,
    question_sha256: String,
    sharing: Sharing,
    origin: Origin,
    primary_trace_sha256: String,
    ultra_trace_sha256: String,
    primary_requests: usize,
    ultra_requests: usize,
    primary_duration_ms: u64,
    ultra_duration_ms: u64,
    primary_started_at_unix_ms: u64,
    ultra_started_at_unix_ms: u64,
    primary_cost_usd: String,
    ultra_cost_usd: String,
}

fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn valid_hash(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}
fn same_hash(left: &str, right: &str) -> bool {
    valid_hash(left) && valid_hash(right) && left.eq_ignore_ascii_case(right)
}
fn path(base: &Path, value: &Path) -> Result<PathBuf> {
    if value.as_os_str().is_empty() || value.as_os_str().len() > 4096 {
        return Err("invalid_calibration_input_path".into());
    }
    Ok(if value.is_absolute() {
        value.to_owned()
    } else {
        base.join(value)
    })
}
fn decimal_pico(value: &str) -> Option<u128> {
    if value.len() > 128 {
        return None;
    }
    let (whole, fraction) = value.split_once('.').unwrap_or((value, ""));
    if whole.is_empty()
        || !whole.bytes().all(|byte| byte.is_ascii_digit())
        || fraction.len() > 12
        || !fraction.bytes().all(|byte| byte.is_ascii_digit())
        || (value.contains('.') && fraction.is_empty())
    {
        return None;
    }
    let whole = whole.parse::<u128>().ok()?.checked_mul(PICO_PER_USD)?;
    let fraction = if fraction.is_empty() {
        0
    } else {
        fraction
            .parse::<u128>()
            .ok()?
            .checked_mul(10_u128.pow(12 - fraction.len() as u32))?
    };
    whole.checked_add(fraction)
}
fn format_pico(value: u128) -> String {
    let whole = value / PICO_PER_USD;
    let fraction = value % PICO_PER_USD;
    if fraction == 0 {
        whole.to_string()
    } else {
        format!(
            "{whole}.{}",
            format!("{fraction:012}").trim_end_matches('0')
        )
    }
}
fn verified_cost(cost: &RecordedCost, model: &str, end_unix_ms: u64) -> Result<u128> {
    if cost.pricing.model_id != model
        || cost.pricing.source != CATALOG_SOURCE
        || cost.pricing.read_at_unix_ms == 0
        || cost.pricing.read_at_unix_ms > end_unix_ms
    {
        return Err("calibration_requires_dated_nebius_catalog_prices".into());
    }
    let pricing = Pricing {
        model_id: cost.pricing.model_id.clone(),
        prompt_usd_per_token: cost.pricing.prompt_usd_per_token.clone(),
        completion_usd_per_token: cost.pricing.completion_usd_per_token.clone(),
        source: cost.pricing.source.clone(),
        read_at_unix_ms: cost.pricing.read_at_unix_ms,
    };
    let expected = estimate_cost(cost.prompt_tokens, cost.completion_tokens, &pricing)
        .ok_or("calibration_cost_overflow_or_invalid_price")?;
    let measured = decimal_pico(&cost.usd).ok_or("calibration_cost_unknown_or_invalid")?;
    if decimal_pico(&expected.usd) != Some(measured) {
        return Err("calibration_cost_does_not_match_measured_tokens".into());
    }
    Ok(measured)
}
fn measurements(result: &ReplayResult, model: &str, primary: bool) -> Result<Measurements> {
    let run = &result.run;
    let end = run
        .started_at_unix_ms
        .checked_add(run.duration_ms)
        .ok_or("calibration_duration_overflow")?;
    if run.model != model
        || run.started_at_unix_ms < result.captured_at_unix_ms
        || run.duration_ms == 0
        || result.duration_ms < run.duration_ms
        || run.harness_version != result.harness_version
        || result.harness_version.is_empty()
        || result.harness_version.len() > 64
        || run.report["schema_version"] != pcagent_shared::SCHEMA_VERSION
        || run.report["mode"] != "nebius"
        || !matches!(
            run.report["execution_status"].as_str(),
            Some("completed" | "failed" | "budget_exhausted")
        )
    {
        return Err("calibration_requires_real_single_model_run_measurements".into());
    }
    let stats: Stats = serde_json::from_value(run.report["stats"].clone())
        .map_err(|_| "calibration_requires_complete_request_stats")?;
    if stats.unknown_requests.is_some_and(|count| count != 0) || stats.cost_lower_bound.is_some() {
        return Err("calibration_usage_unknown".into());
    }
    if stats.web_usage.as_ref().is_some_and(|web| {
        !web.known_credits.is_finite()
            || web.known_credits < 0.0
            || web.cache_hits > web.lookups
            || web.unknown_requests > web.lookups.saturating_sub(web.cache_hits)
    }) {
        return Err("calibration_invalid_web_usage".into());
    }
    if stats.inference_calls == 0
        || stats.inference_calls > MAX_ROUNDS
        || (primary && stats.inference_calls >= MAX_ROUNDS)
        || stats.requests.len() != stats.inference_calls
        || stats.tool_calls != result.calls.len()
        || stats.inspection_calls == 0
        || stats.inspection_calls > stats.tool_calls
        || stats.extensions != 0
        || stats.repair_requests.as_ref().is_some_and(|repairs| {
            repairs
                .iter()
                .any(|repair| repair.request == 0 || repair.request > stats.inference_calls)
        })
        || stats
            .routing
            .as_ref()
            .is_some_and(|state| !state["change"].is_null())
    {
        return Err("calibration_request_counts_or_budget_mismatch".into());
    }
    let (mut prompt, mut completion, mut reasoning) = (0_u64, 0_u64, Some(0_u64));
    for usage in &stats.requests {
        let p = usage
            .prompt_tokens
            .filter(|p| *p > 0)
            .ok_or("calibration_usage_unknown")?;
        let c = usage.completion_tokens.ok_or("calibration_usage_unknown")?;
        if usage.reasoning_tokens.is_some_and(|r| r > c) {
            return Err("calibration_reasoning_count_mismatch".into());
        }
        prompt = prompt.checked_add(p).ok_or("calibration_usage_overflow")?;
        completion = completion
            .checked_add(c)
            .ok_or("calibration_usage_overflow")?;
        reasoning = match (reasoning, usage.reasoning_tokens) {
            (Some(a), Some(b)) => Some(a.checked_add(b).ok_or("calibration_usage_overflow")?),
            _ => None,
        };
    }
    if stats.prompt_tokens != Some(prompt)
        || stats.completion_tokens != Some(completion)
        || stats.total_tokens != prompt.checked_add(completion)
        || stats.reasoning_tokens != reasoning
        || stats
            .known_prompt_tokens
            .is_some_and(|known| known != prompt)
        || stats
            .known_completion_tokens
            .is_some_and(|known| known != completion)
    {
        return Err("calibration_usage_totals_mismatch".into());
    }
    let aggregate = stats
        .cost
        .as_ref()
        .map(|cost| {
            if cost.prompt_tokens != prompt || cost.completion_tokens != completion {
                return Err("calibration_cost_usage_mismatch".into());
            }
            verified_cost(cost, model, end)
        })
        .transpose()?;
    let amount = if stats.model_requests.is_empty() {
        aggregate.ok_or("calibration_cost_unknown")?
    } else {
        if stats.model_requests.len() != stats.inference_calls {
            return Err("calibration_model_request_counts_mismatch".into());
        }
        let mut sum = 0_u128;
        for (index, request) in stats.model_requests.iter().enumerate() {
            if request.request != index + 1
                || request.model_id.as_deref() != Some(model)
                || request.usage != stats.requests[index]
            {
                return Err("calibration_mixed_or_misidentified_requests".into());
            }
            let cost = request
                .cost
                .as_ref()
                .ok_or("calibration_request_cost_unknown")?;
            if Some(cost.prompt_tokens) != request.usage.prompt_tokens
                || Some(cost.completion_tokens) != request.usage.completion_tokens
            {
                return Err("calibration_request_cost_usage_mismatch".into());
            }
            sum = sum
                .checked_add(verified_cost(cost, model, end)?)
                .ok_or("calibration_cost_overflow")?;
        }
        if stats.total_cost_usd.as_deref().and_then(decimal_pico) != Some(sum)
            || aggregate.is_some_and(|amount| amount != sum)
        {
            return Err("calibration_total_request_cost_mismatch".into());
        }
        sum
    };
    if stats
        .total_cost_usd
        .as_deref()
        .is_some_and(|total| decimal_pico(total) != Some(amount))
    {
        return Err("calibration_total_cost_mismatch".into());
    }
    Ok(Measurements {
        requests: stats.inference_calls,
        duration_ms: run.duration_ms,
        started_at_unix_ms: run.started_at_unix_ms,
        cost_usd: format_pico(amount),
    })
}
fn validate_trigger(result: &ReplayResult, review: &HumanReview) -> Result<()> {
    let answer = &result.run.report["answer"];
    let supported = match review.trigger {
        EscalationReason::LowConfidence => {
            answer["confidence"] == "low" && answer["category"] != "insufficient_evidence"
        }
        EscalationReason::InsufficientEvidence => answer["category"] == "insufficient_evidence",
        EscalationReason::VerificationFailure => {
            answer["unverified"]
                .as_array()
                .is_some_and(|items| !items.is_empty())
                && answer["repairs"]
                    .as_u64()
                    .is_some_and(|repairs| repairs >= MAX_REPAIRS as u64)
        }
        // This is a reviewed candidate threshold; its observed count is checked independently.
        EscalationReason::PrimaryRequestLimit => true,
    };
    if !supported {
        return Err("calibration_trigger_not_supported_by_primary_trace".into());
    }
    Ok(())
}
fn validate_replay(
    result: &ReplayResult,
    scenario: &Scenario,
    pair: &AnnotatedPair,
    runtime: &CalibrationRuntime,
) -> Result<()> {
    if &result.runtime != runtime
        || result.run.runtime.as_ref() != Some(runtime)
        || result.harness_version != REPLAY_HARNESS_VERSION
        || result.run.harness_version != REPLAY_HARNESS_VERSION
    {
        return Err("calibration_requires_identical_current_execution_runtime".into());
    }
    if result.schema_version != SCENARIO_VERSION
        || result.scenario_origin != Origin::Live
        || result.captured_at_unix_ms != scenario.captured_at_unix_ms
        || result.scenario != scenario.name
        || result.question.trim().is_empty()
        || result.question.len() > MAX_QUESTION_BYTES
        || result.sharing != pair.sharing
        || !same_hash(&result.scenario_sha256, &pair.scenario_sha256)
        || !same_hash(&result.tool_registry_sha256, &pair.tool_registry_sha256)
        || !same_hash(&hash(result.question.as_bytes()), &pair.question_sha256)
        || !result.replay_complete
        || !result.replay_misses.is_empty()
    {
        return Err("calibration_requires_identical_complete_live_replays".into());
    }
    match result.mode {
        ReplayMode::NebiusReplay => {}
    }
    let mut replay = ReplayTools::new(scenario, pair.sharing)?;
    if !same_hash(
        &fingerprint(&replay.definitions())?,
        &pair.tool_registry_sha256,
    ) {
        return Err("calibration_registry_does_not_match_frozen_scenario".into());
    }
    for call in &result.calls {
        let spec = replay
            .specs()
            .iter()
            .find(|spec| spec.name == call.tool)
            .copied()
            .ok_or("calibration_unknown_recorded_tool")?;
        let args = spec
            .validate(&call.arguments.to_string())
            .map_err(|_| "calibration_invalid_recorded_arguments")?;
        let output = replay.run(&spec, &args);
        if !replay.misses.is_empty()
            || serde_json::to_value(RecordedOutput::from(&output))?
                != serde_json::to_value(&call.output)?
            || call.recorded_at_unix_ms.is_none_or(|time| {
                time < result.run.started_at_unix_ms
                    || result
                        .run
                        .started_at_unix_ms
                        .checked_add(result.run.duration_ms)
                        .is_none_or(|end| time > end)
            })
            || call.duration_ms.is_none()
        {
            return Err("calibration_tool_trace_does_not_match_frozen_capture".into());
        }
    }
    if result.served_captures != replay.served_count()
        || result.reused_calls != replay.reused_calls
        || result.unused_captures != scenario.calls.len() - replay.served_count()
    {
        return Err("calibration_replay_accounting_mismatch".into());
    }
    if result.run.report["evidence"]
        .as_array()
        .is_none_or(|items| items.iter().any(|item| item["origin"] != "live"))
    {
        return Err("calibration_report_contains_synthetic_evidence".into());
    }
    Ok(())
}
fn validate_review(review: &HumanReview) -> Result<()> {
    match (review.source, review.quality_scale) {
        (HumanSource::Human, QualityScale::RoutingHuman0To4) => {}
    }
    if !review.human_reviewed
        || review.primary_quality > 4
        || review.ultra_quality > 4
        || review.rationale.trim().is_empty()
        || review.rationale.len() > 2048
    {
        return Err("calibration_requires_explicit_human_labels_and_rationale".into());
    }
    Ok(())
}

pub(crate) fn compile(annotation_path: &Path) -> Result<(Calibration, Provenance)> {
    let annotation_bytes = files::read_bytes_limited(annotation_path, MAX_ANNOTATION_BYTES as u64)?;
    let annotations: Annotations = serde_json::from_slice(&annotation_bytes)
        .map_err(|_| "invalid_calibration_annotation_schema")?;
    compile_annotations(
        annotations,
        annotation_path.parent().unwrap_or(Path::new(".")),
        hash(&annotation_bytes),
    )
}
fn compile_annotations(
    annotations: Annotations,
    base: &Path,
    annotation_hash: String,
) -> Result<(Calibration, Provenance)> {
    if annotations.schema_version != 1
        || annotations.reviewed_at_unix_ms == 0
        || annotations.reviewed_at_unix_ms > unix_ms()
        || annotations.pairs.is_empty()
        || annotations.pairs.len() > MAX_CALIBRATION_CASES
    {
        return Err("invalid_calibration_campaign_metadata".into());
    }
    let settings = RoutingSettings {
        mode: RoutingMode::Automatic,
        super_model_id: Some(annotations.super_model_id.clone()),
        ultra_model_id: Some(annotations.ultra_model_id.clone()),
    };
    settings.validate()?;
    if annotations
        .super_model_id
        .eq_ignore_ascii_case(&annotations.ultra_model_id)
    {
        return Err("calibration_requires_distinct_super_and_ultra_models".into());
    }
    let runtime = CalibrationRuntime::current();
    let mut cases = Vec::new();
    let mut provenance_pairs = Vec::new();
    let mut captures = BTreeSet::new();
    let mut traces = BTreeSet::new();
    for pair in &annotations.pairs {
        validate_review(&pair.review)?;
        if !captures.insert(pair.scenario_sha256.to_ascii_lowercase())
            || !traces.insert(pair.primary_trace_sha256.to_ascii_lowercase())
            || !traces.insert(pair.ultra_trace_sha256.to_ascii_lowercase())
        {
            return Err("calibration_requires_independent_captures_and_traces".into());
        }
        let scenario_bytes = files::read_bytes(&path(base, &pair.scenario)?)?;
        let scenario: Scenario =
            serde_json::from_slice(&scenario_bytes).map_err(|_| "invalid_calibration_scenario")?;
        scenario.validate()?;
        if scenario.initial.origin != Origin::Live
            || !scenario.initial.has_observations()
            || scenario.captured_at_unix_ms == 0
            || scenario
                .initial
                .evidence
                .iter()
                .any(|evidence| evidence.origin != Origin::Live)
            || scenario.calls.iter().any(|call| {
                call.recorded_at_unix_ms.is_none()
                    || call.duration_ms.is_none()
                    || call
                        .output
                        .evidence
                        .iter()
                        .any(|evidence| evidence.origin != Origin::Live)
            })
            || !same_hash(&fingerprint(&scenario)?, &pair.scenario_sha256)
        {
            return Err("calibration_requires_a_real_frozen_capture".into());
        }
        let primary_bytes = files::read_bytes(&path(base, &pair.primary_result)?)?;
        let ultra_bytes = files::read_bytes(&path(base, &pair.ultra_result)?)?;
        if !same_hash(&hash(&primary_bytes), &pair.primary_trace_sha256)
            || !same_hash(&hash(&ultra_bytes), &pair.ultra_trace_sha256)
        {
            return Err("calibration_trace_changed_since_human_review".into());
        }
        let primary: ReplayResult = serde_json::from_slice(&primary_bytes)
            .map_err(|_| "invalid_primary_calibration_replay")?;
        let ultra: ReplayResult =
            serde_json::from_slice(&ultra_bytes).map_err(|_| "invalid_ultra_calibration_replay")?;
        validate_replay(&primary, &scenario, pair, &runtime)?;
        validate_replay(&ultra, &scenario, pair, &runtime)?;
        if primary.question != ultra.question || primary.harness_version != ultra.harness_version {
            return Err("calibration_pair_question_or_harness_mismatch".into());
        }
        validate_trigger(&primary, &pair.review)?;
        let primary = measurements(&primary, &annotations.super_model_id, true)?;
        let ultra = measurements(&ultra, &annotations.ultra_model_id, false)?;
        if primary
            .started_at_unix_ms
            .checked_add(primary.duration_ms)
            .is_none_or(|end| end > annotations.reviewed_at_unix_ms)
            || ultra
                .started_at_unix_ms
                .checked_add(ultra.duration_ms)
                .is_none_or(|end| end > annotations.reviewed_at_unix_ms)
        {
            return Err("calibration_review_must_follow_both_runs".into());
        }
        cases.push(CalibrationCase {
            capture_sha256: pair.scenario_sha256.to_ascii_lowercase(),
            primary_trace_sha256: pair.primary_trace_sha256.to_ascii_lowercase(),
            ultra_trace_sha256: pair.ultra_trace_sha256.to_ascii_lowercase(),
            origin: CalibrationOrigin::Live,
            human_reviewed: true,
            trigger: pair.review.trigger,
            primary_requests: primary.requests,
            primary_quality: pair.review.primary_quality,
            ultra_quality: pair.review.ultra_quality,
            primary_critical_error: pair.review.primary_critical_error,
            ultra_critical_error: pair.review.ultra_critical_error,
            primary_cost_usd: primary.cost_usd.clone(),
            ultra_cost_usd: ultra.cost_usd.clone(),
            primary_duration_ms: primary.duration_ms,
            ultra_duration_ms: ultra.duration_ms,
        });
        provenance_pairs.push(PairProvenance {
            scenario_file_sha256: hash(&scenario_bytes),
            scenario_sha256: pair.scenario_sha256.to_ascii_lowercase(),
            tool_registry_sha256: pair.tool_registry_sha256.to_ascii_lowercase(),
            question_sha256: pair.question_sha256.to_ascii_lowercase(),
            sharing: pair.sharing,
            origin: Origin::Live,
            primary_trace_sha256: pair.primary_trace_sha256.to_ascii_lowercase(),
            ultra_trace_sha256: pair.ultra_trace_sha256.to_ascii_lowercase(),
            primary_requests: primary.requests,
            ultra_requests: ultra.requests,
            primary_duration_ms: primary.duration_ms,
            ultra_duration_ms: ultra.duration_ms,
            primary_started_at_unix_ms: primary.started_at_unix_ms,
            ultra_started_at_unix_ms: ultra.started_at_unix_ms,
            primary_cost_usd: primary.cost_usd,
            ultra_cost_usd: ultra.cost_usd,
        });
    }
    let calibration = Calibration {
        schema_version: CALIBRATION_SCHEMA_VERSION,
        runtime: runtime.clone(),
        super_model_id: annotations.super_model_id,
        ultra_model_id: annotations.ultra_model_id,
        reviewed_at_unix_ms: annotations.reviewed_at_unix_ms,
        maximum_extra_cost_usd: annotations.maximum_extra_cost_usd,
        cases,
    };
    calibration.rules(&settings)?;
    let bytes = serde_json::to_vec(&calibration)?;
    if serde_json::to_vec_pretty(&calibration)?.len() + 1 > MAX_CALIBRATION_BYTES {
        return Err("calibration_too_large".into());
    }
    let provenance = Provenance {
        schema_version: 2,
        runtime,
        annotations_sha256: annotation_hash,
        calibration_sha256: hash(&bytes),
        compiler_version: env!("CARGO_PKG_VERSION"),
        cost_basis: "measured_usage_times_dated_nebius_catalog_prices_not_invoice",
        evidence_basis: "independent_single_model_replays_of_identical_frozen_live_capture",
        pairs: provenance_pairs,
    };
    Ok((calibration, provenance))
}

#[cfg(test)]
mod tests {
    use super::*;
    use pcagent_agent::ToolOutput;
    use pcagent_shared::{Evidence, Metric, Snapshot, Window};
    use serde_json::json;
    use std::fs;

    struct Fixture {
        directory: PathBuf,
        annotation: Value,
    }
    impl Fixture {
        // Synthetic parser fixtures deliberately declare Live solely to exercise the acceptance
        // branch. They are ephemeral tests, never a campaign, calibration asset or quality claim.
        fn new() -> Self {
            let directory =
                std::env::temp_dir().join(pcagent_shared::new_id("calibration-unit-test"));
            fs::create_dir(&directory).unwrap();
            let mut pairs = Vec::new();
            for index in 0..3 {
                let mut initial = Snapshot::empty("synthetic-unit-test");
                initial.evidence.push(Evidence::measured(
                    Metric::CpuBusyBasisPoints,
                    "system",
                    100 + index,
                    "synthetic unit fixture, no hardware measured",
                    Window::instant(),
                ));
                let mut scenario = Scenario::new(
                    format!("synthetic-case-{index}"),
                    Some("private fixture question".into()),
                    initial,
                );
                scenario.captured_at_unix_ms = 100;
                scenario.calls.push(RecordedCall {
                    tool: "get_system_snapshot".into(),
                    arguments: json!({}),
                    output: RecordedOutput::from(&ToolOutput::from_snapshot(&scenario.initial)),
                    recorded_at_unix_ms: Some(101),
                    duration_ms: Some(1),
                });
                let scenario_name = format!("scenario-{index}.json");
                fs::write(
                    directory.join(&scenario_name),
                    serde_json::to_vec(&scenario).unwrap(),
                )
                .unwrap();
                let primary = result(&scenario, "nvidia/nemotron-super-unit-fixture", "low");
                let ultra = result(&scenario, "nvidia/nemotron-ultra-unit-fixture", "high");
                let primary_name = format!("primary-{index}.json");
                let ultra_name = format!("ultra-{index}.json");
                let primary_bytes = serde_json::to_vec(&primary).unwrap();
                let ultra_bytes = serde_json::to_vec(&ultra).unwrap();
                fs::write(directory.join(&primary_name), &primary_bytes).unwrap();
                fs::write(directory.join(&ultra_name), &ultra_bytes).unwrap();
                pairs.push(json!({
                    "scenario": scenario_name, "primary_result": primary_name, "ultra_result": ultra_name,
                    "scenario_sha256": fingerprint(&scenario).unwrap(),
                    "tool_registry_sha256": primary["tool_registry_sha256"],
                    "question_sha256": hash(b"private fixture question"), "sharing": "anonymized",
                    "primary_trace_sha256": hash(&primary_bytes), "ultra_trace_sha256": hash(&ultra_bytes),
                    "review": {"source":"human", "quality_scale":"routing_human_0_to_4",
                        "human_reviewed":true, "trigger":"low_confidence", "primary_quality":1,
                        "ultra_quality":3, "primary_critical_error":false, "ultra_critical_error":false,
                        "rationale":"Synthetic annotation fixture; this is not a real human rating."}
                }));
            }
            let annotation = json!({"schema_version":1,
                "super_model_id":"nvidia/nemotron-super-unit-fixture",
                "ultra_model_id":"nvidia/nemotron-ultra-unit-fixture",
                "reviewed_at_unix_ms":1000,"maximum_extra_cost_usd":"0.01", "pairs":pairs});
            Self {
                directory,
                annotation,
            }
        }
        fn compile(&self) -> Result<(Calibration, Provenance)> {
            let annotations = serde_json::from_value(self.annotation.clone())
                .map_err(|_| "invalid_calibration_annotation_schema")?;
            compile_annotations(annotations, &self.directory, fingerprint(&self.annotation)?)
        }
        fn replace_result(
            &mut self,
            primary: bool,
            mutate: impl FnOnce(&mut Value),
            reapprove: bool,
        ) {
            let field = if primary {
                "primary_result"
            } else {
                "ultra_result"
            };
            let hash_field = if primary {
                "primary_trace_sha256"
            } else {
                "ultra_trace_sha256"
            };
            let path = self
                .directory
                .join(self.annotation["pairs"][0][field].as_str().unwrap());
            let mut value: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
            mutate(&mut value);
            let bytes = serde_json::to_vec(&value).unwrap();
            fs::write(path, &bytes).unwrap();
            if reapprove {
                self.annotation["pairs"][0][hash_field] = hash(&bytes).into();
            }
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.directory);
        }
    }
    fn result(scenario: &Scenario, model: &str, confidence: &str) -> Value {
        let replay = ReplayTools::new(scenario, Sharing::Anonymized).unwrap();
        let price = Pricing {
            model_id: model.into(),
            prompt_usd_per_token: "0.000001".into(),
            completion_usd_per_token: "0.000002".into(),
            source: CATALOG_SOURCE.into(),
            read_at_unix_ms: 201,
        };
        let cost = estimate_cost(200, 20, &price).unwrap();
        let request_cost = estimate_cost(100, 10, &price).unwrap();
        let usage = pcagent_agent::RequestUsage {
            prompt_tokens: Some(100),
            completion_tokens: Some(10),
            reasoning_tokens: Some(0),
        };
        let mut calls = scenario.calls.clone();
        calls[0].recorded_at_unix_ms = Some(205);
        let model_requests: Vec<_> = (1..=2)
            .map(|request| pcagent_agent::ModelRequestUsage {
                request,
                model_id: Some(model.into()),
                usage: usage.clone(),
                cost: Some(request_cost.clone()),
                duration_ms: 4,
            })
            .collect();
        let mut stats = pcagent_agent::local_report(&scenario.initial).stats;
        stats.inference_calls = 2;
        stats.tool_calls = 1;
        stats.inspection_calls = 1;
        stats.total_tokens = Some(220);
        stats.prompt_tokens = Some(200);
        stats.completion_tokens = Some(20);
        stats.reasoning_tokens = Some(0);
        stats.requests = vec![usage.clone(), usage];
        stats.cost = Some(cost);
        stats.model_requests = model_requests;
        stats.total_cost_usd = Some("0.00024".into());
        stats.known_prompt_tokens = 200;
        stats.known_completion_tokens = 20;
        stats.repair_requests.push(pcagent_agent::Repair {
            request: 1,
            issues: vec![pcagent_agent::Issue {
                kind: pcagent_agent::IssueKind::MissingCitation,
                excerpt: "Synthetic fixture repair".into(),
            }],
            output_limit: false,
        });
        let answer = json!({"text":"Synthetic fixture answer", "confidence":confidence,
            "category":"cpu", "unverified":[],"repairs":0});
        let report = json!({"schema_version":pcagent_shared::SCHEMA_VERSION,"mode":"nebius",
            "execution_status":"completed", "error":null, "evidence":scenario.initial.evidence,
            "answer":answer,"stats":stats});
        let runtime = CalibrationRuntime::current();
        let run = json!({"model":model,"harness_version":REPLAY_HARNESS_VERSION,"runtime":runtime,
            "started_at_unix_ms":200,"duration_ms":10,"report":report});
        json!({
            "schema_version":SCENARIO_VERSION,"scenario":scenario.name,
            "scenario_sha256":fingerprint(scenario).unwrap(),
            "tool_registry_sha256":fingerprint(&replay.definitions()).unwrap(),
            "scenario_origin":"live","captured_at_unix_ms":scenario.captured_at_unix_ms,
            "harness_version":REPLAY_HARNESS_VERSION,"runtime":runtime,"question":"private fixture question",
            "sharing":"anonymized","mode":"nebius_replay","calls":calls,"replay_misses":[],
            "served_captures":1,"reused_calls":0,"unused_captures":0,"duration_ms":15,"replay_complete":true,
            "run":run
        })
    }
    #[test]
    fn measured_fields_and_human_labels_are_preserved_without_raw_text_in_outputs() {
        let fixture = Fixture::new();
        let (calibration, provenance) = fixture.compile().unwrap();
        assert_eq!(calibration.schema_version, CALIBRATION_SCHEMA_VERSION);
        assert_eq!(calibration.runtime, CalibrationRuntime::current());
        assert_eq!(provenance.runtime, calibration.runtime);
        assert_eq!(calibration.cases.len(), 3);
        assert_eq!(calibration.cases[0].primary_cost_usd, "0.00024");
        assert_eq!(calibration.cases[0].primary_requests, 2);
        assert_eq!(calibration.cases[0].primary_duration_ms, 10);
        assert_eq!(calibration.cases[0].primary_quality, 1);
        assert_eq!(provenance.pairs[0].ultra_requests, 2);
        let serialized = serde_json::to_string(&(calibration, provenance)).unwrap();
        assert!(!serialized.contains("private fixture question"));
        assert!(!serialized.contains("Synthetic fixture answer"));
        assert!(!serialized.contains("Synthetic annotation"));
        assert!(!serialized.contains(fixture.directory.to_str().unwrap()));
    }
    #[test]
    fn synthetic_incomplete_mismatched_and_modified_traces_are_refused() {
        for mutation in 0..9 {
            let mut fixture = Fixture::new();
            fixture.replace_result(
                false,
                |result| match mutation {
                    0 => result["scenario_origin"] = "synthetic_fixture".into(),
                    1 => result["question"] = "different question".into(),
                    2 => result["sharing"] = "complete".into(),
                    3 => result["replay_complete"] = false.into(),
                    4 => result["replay_misses"] = json!([{}]),
                    5 => result["calls"][0]["output"]["evidence"][0]["value"] = 999.into(),
                    6 => result["served_captures"] = 0.into(),
                    7 => result["tool_registry_sha256"] = "a".repeat(64).into(),
                    _ => result["mode"] = "offline_replay".into(),
                },
                true,
            );
            assert!(fixture.compile().is_err(), "mutation {mutation}");
        }
        let mut fixture = Fixture::new();
        fixture.replace_result(
            false,
            |result| result["run"]["report"]["answer"]["text"] = "edited answer".into(),
            false,
        );
        assert!(fixture.compile().is_err());
    }
    #[test]
    fn absent_old_divergent_and_extended_execution_identities_are_refused() {
        for mutation in 0..14 {
            let mut fixture = Fixture::new();
            fixture.replace_result(
                false,
                |result| match mutation {
                    0 => {
                        result.as_object_mut().unwrap().remove("runtime");
                    }
                    1 => {
                        result["run"].as_object_mut().unwrap().remove("runtime");
                    }
                    2 => result["runtime"] = Value::Null,
                    3 => result["run"]["runtime"] = Value::Null,
                    4 => {
                        result["runtime"]["registered_tool_registry_sha256"] =
                            "a".repeat(64).into();
                    }
                    5 => result["run"]["runtime"]["analyst_sha256"] = "b".repeat(64).into(),
                    6 => {
                        for pointer in ["/runtime", "/run/runtime"] {
                            result.pointer_mut(pointer).unwrap()
                                ["registered_tool_registry_sha256"] = "a".repeat(64).into();
                        }
                    }
                    7 => {
                        for pointer in ["/runtime", "/run/runtime"] {
                            result.pointer_mut(pointer).unwrap()["analyst_sha256"] =
                                "b".repeat(64).into();
                        }
                    }
                    8 => {
                        for pointer in ["/runtime", "/run/runtime"] {
                            result.pointer_mut(pointer).unwrap()["harness_version"] =
                                "pcagent-eval-replay-v1".into();
                        }
                    }
                    9 => result["harness_version"] = "old-harness".into(),
                    10 => result["run"]["harness_version"] = "old-harness".into(),
                    11 => {
                        result["harness_version"] = "old-harness".into();
                        result["run"]["harness_version"] = "old-harness".into();
                    }
                    12 => result["runtime"]["extension"] = true.into(),
                    _ => result["run"]["runtime"]["extension"] = true.into(),
                },
                true,
            );
            assert!(fixture.compile().is_err(), "mutation {mutation}");
        }
        let mut fixture = Fixture::new();
        for primary in [true, false] {
            fixture.replace_result(
                primary,
                |result| {
                    for pointer in ["/runtime", "/run/runtime"] {
                        result.pointer_mut(pointer).unwrap()["harness_version"] =
                            "pcagent-eval-replay-v1".into();
                    }
                    result["harness_version"] = "pcagent-eval-replay-v1".into();
                    result["run"]["harness_version"] = "pcagent-eval-replay-v1".into();
                },
                true,
            );
        }
        assert!(fixture.compile().is_err());
    }
    #[test]
    fn full_production_stats_and_older_optional_field_absence_are_accepted() {
        let mut fixture = Fixture::new();
        assert!(fixture.compile().is_ok());
        for primary in [true, false] {
            fixture.replace_result(
                primary,
                |result| {
                    let stats = result["run"]["report"]["stats"].as_object_mut().unwrap();
                    for field in [
                        "repair_requests",
                        "known_prompt_tokens",
                        "known_completion_tokens",
                        "unknown_requests",
                        "cost_lower_bound",
                        "web_usage",
                    ] {
                        assert!(stats.remove(field).is_some(), "production field {field}");
                    }
                    for request in stats["model_requests"].as_array_mut().unwrap() {
                        assert!(request
                            .as_object_mut()
                            .unwrap()
                            .remove("duration_ms")
                            .is_some());
                    }
                },
                true,
            );
        }
        assert!(fixture.compile().is_ok());
    }

    #[test]
    fn auxiliary_stats_stay_typed_and_cannot_hide_unknown_usage() {
        for mutation in 0..10 {
            let mut fixture = Fixture::new();
            fixture.replace_result(
                true,
                |result| {
                    let stats = &mut result["run"]["report"]["stats"];
                    match mutation {
                        0 => stats["unknown_requests"] = 1.into(),
                        1 => stats["known_prompt_tokens"] = 201.into(),
                        2 => stats["known_completion_tokens"] = 21.into(),
                        3 => stats["cost_lower_bound"] = stats["cost"].clone(),
                        4 => stats["model_requests"][0]["duration_ms"] = "unknown".into(),
                        5 => stats["repair_requests"][0]["request"] = 0.into(),
                        6 => stats["repair_requests"][0]["issues"][0]["kind"] = "invented".into(),
                        7 => stats["web_usage"]["known_credits"] = (-1).into(),
                        8 => stats["web_usage"]["extra"] = true.into(),
                        _ => stats["undeclared_stats"] = true.into(),
                    }
                },
                true,
            );
            assert!(fixture.compile().is_err(), "mutation {mutation}");
        }
    }

    #[test]
    fn unknown_costs_wrong_products_mixed_models_and_invented_usage_are_refused() {
        for mutation in 0..8 {
            let mut fixture = Fixture::new();
            fixture.replace_result(
                false,
                |result| {
                    let stats = &mut result["run"]["report"]["stats"];
                    match mutation {
                        0 => stats["requests"][0]["prompt_tokens"] = Value::Null,
                        1 => stats["cost"]["usd"] = "0".into(),
                        2 => stats["cost"]["pricing"]["source"] = "guessed price".into(),
                        3 => stats["model_requests"][0]["cost"] = Value::Null,
                        4 => {
                            stats["model_requests"][0]["model_id"] = "nvidia/nemotron-other".into()
                        }
                        5 => stats["inference_calls"] = 3.into(),
                        6 => stats["model_requests"][0]["request"] = 2.into(),
                        _ => stats["total_cost_usd"] = "NaN".into(),
                    }
                },
                true,
            );
            assert!(fixture.compile().is_err(), "mutation {mutation}");
        }
        for invalid in [
            "NaN",
            "1e-9",
            "0.",
            "-1",
            "0.0000000000001",
            "340282366920938463463374607431768211455",
        ] {
            assert_eq!(decimal_pico(invalid), None);
        }
        assert_eq!(decimal_pico("0"), Some(0));
        assert_eq!(decimal_pico("0.000000000001"), Some(1));
    }
    #[test]
    fn no_grade_conversion_annotation_inference_or_unsupported_rules_are_allowed() {
        for mutation in 0..7 {
            let mut fixture = Fixture::new();
            match mutation {
                0 => fixture.annotation["pairs"][0]["review"]["human_reviewed"] = false.into(),
                1 => fixture.annotation["pairs"][0]["review"]["source"] = "llm".into(),
                2 => fixture.annotation["pairs"][0]["review"]["primary_quality"] = 10.into(),
                3 => fixture.annotation["pairs"][0]["review"]["rationale"] = " ".into(),
                4 => {
                    fixture.annotation["pairs"][0]["review"]["trigger"] =
                        "verification_failure".into()
                }
                5 => {
                    fixture.annotation["pairs"].as_array_mut().unwrap().pop();
                }
                _ => fixture.annotation["unexpected"] = true.into(),
            }
            assert!(fixture.compile().is_err(), "mutation {mutation}");
        }
        let mut fixture = Fixture::new();
        let duplicate = fixture.annotation["pairs"][0].clone();
        fixture.annotation["pairs"][1] = duplicate;
        assert!(fixture.compile().is_err());
        let mut fixture = Fixture::new();
        fixture.annotation["pairs"][0]["review"]["ultra_quality"] = 0.into();
        assert!(fixture.compile().is_err());
    }
}
