//! One optional, evidence-calibrated escalation inside the existing investigation budget.
//! No default model or trigger is inferred from a model name. Automatic routing requires a
//! locally reviewed campaign of real paired Nemotron runs; synthetic fixtures are tests only.
use crate::{cost, validate_model_id, AgentError, Model, Pricing, Reply, ToolChoice, MAX_ROUNDS};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{collections::BTreeSet, time::Duration};

pub const MAX_CALIBRATION_BYTES: usize = 64 * 1024;
pub const MAX_CALIBRATION_CASES: usize = 128;
pub const CALIBRATION_SCHEMA_VERSION: u32 = 2;
pub const REPLAY_HARNESS_VERSION: &str = "pcagent-eval-replay-v2";

/// Identity captured by the producer, never filled in while loading an older trace.
/// The global registry permits the same calibration to be consumed by CLI/GUI turns
/// with different subsets of tools. The trace retains its own replay-registry hash too.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CalibrationRuntime {
    pub registered_tool_registry_sha256: String,
    pub analyst_sha256: String,
    pub harness_version: String,
}
impl CalibrationRuntime {
    pub fn current() -> Self {
        static CURRENT: std::sync::LazyLock<CalibrationRuntime> =
            std::sync::LazyLock::new(CalibrationRuntime::compute);
        CURRENT.clone()
    }
    fn compute() -> Self {
        // The fixed canonical catalog: a turn's capability filtering never reaches it.
        let mut specs = crate::Workstation::tool_specs();
        specs.extend([
            crate::PREPARE_ACTION_PLAN,
            crate::selection::GET_SELECTED_EVIDENCE,
        ]);
        let registry = registry_sha256(&specs, crate::answer::definition());
        // Bind controller, prompt, budgets, C1 and context behavior conservatively. LF
        // normalization makes this identity independent of Git's checkout line endings.
        let mut analyst = Sha256::new();
        for source in [
            include_str!("lib.rs"),
            include_str!("routing.rs"),
            include_str!("answer.rs"),
            include_str!("answer/grounding.rs"),
            include_str!("budget.rs"),
            include_str!("conversation.rs"),
            include_str!("tool_context.rs"),
            include_str!("nebius.rs"),
            include_str!("privacy.rs"),
            include_str!("tools.rs"),
            include_str!("report.rs"),
            include_str!("replay.rs"),
            include_str!("cost.rs"),
            include_str!("workstation.rs"),
            include_str!("system_tools.rs"),
            include_str!("activity_tools.rs"),
            include_str!("history_tools.rs"),
            include_str!("elevated_tools.rs"),
            include_str!("selection.rs"),
        ] {
            let normalized = source.replace("\r\n", "\n");
            analyst.update((normalized.len() as u64).to_le_bytes());
            analyst.update(normalized.as_bytes());
        }
        Self {
            registered_tool_registry_sha256: registry,
            analyst_sha256: format!("{:x}", analyst.finalize()),
            harness_version: REPLAY_HARNESS_VERSION.into(),
        }
    }
}

/// The identity of a tool registry: its definitions with their classes, sorted by name, then the
/// answer schema.
pub(crate) fn registry_sha256(specs: &[crate::ToolSpec], answer: Value) -> String {
    let mut specs = specs.to_vec();
    specs.sort_by_key(|spec| spec.name);
    let mut definitions: Vec<_> = specs
        .iter()
        .map(|spec| {
            let class = match spec.class {
                crate::ToolClass::Inspection => "inspection",
                crate::ToolClass::Context => "context",
                crate::ToolClass::Proposal => "proposal",
                crate::ToolClass::External => "external",
            };
            serde_json::json!({"class":class,"definition":spec.definition()})
        })
        .collect();
    definitions.push(answer);
    let registry = serde_json::to_vec(&definitions).expect("static tool definitions");
    format!("{:x}", Sha256::digest(registry))
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RoutingMode {
    #[default]
    Configured,
    AlwaysSuper,
    AlwaysUltra,
    Automatic,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoutingSettings {
    pub mode: RoutingMode,
    pub super_model_id: Option<String>,
    pub ultra_model_id: Option<String>,
}
impl RoutingSettings {
    pub fn validate(&self) -> Result<(), AgentError> {
        if self.mode == RoutingMode::Automatic
            && self.super_model_id.is_some()
            && self.super_model_id == self.ultra_model_id
        {
            return Err(AgentError::InvalidModel);
        }
        for (id, family) in [
            (&self.super_model_id, "super"),
            (&self.ultra_model_id, "ultra"),
        ] {
            if let Some(id) = id {
                validate_model_id(id)?;
                if !id.to_ascii_lowercase().contains(family) {
                    return Err(AgentError::InvalidModel);
                }
            }
        }
        if (matches!(self.mode, RoutingMode::AlwaysSuper | RoutingMode::Automatic)
            && self.super_model_id.is_none())
            || (matches!(self.mode, RoutingMode::AlwaysUltra | RoutingMode::Automatic)
                && self.ultra_model_id.is_none())
        {
            return Err(AgentError::MissingModel);
        }
        Ok(())
    }

    pub fn initial_model<'a>(&'a self, configured: &'a str) -> Result<&'a str, AgentError> {
        self.validate()?;
        Ok(match self.mode {
            RoutingMode::Configured => configured,
            RoutingMode::AlwaysSuper | RoutingMode::Automatic => {
                self.super_model_id.as_deref().unwrap()
            }
            RoutingMode::AlwaysUltra => self.ultra_model_id.as_deref().unwrap(),
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EscalationReason {
    LowConfidence,
    InsufficientEvidence,
    VerificationFailure,
    PrimaryRequestLimit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CalibrationOrigin {
    Live,
    Synthetic,
}

/// Human labels and measured costs for two runs of the same frozen capture. No question,
/// answer text, path, name or personal identifier belongs to a routing calibration file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CalibrationCase {
    pub capture_sha256: String,
    pub primary_trace_sha256: String,
    pub ultra_trace_sha256: String,
    pub origin: CalibrationOrigin,
    pub human_reviewed: bool,
    pub trigger: EscalationReason,
    pub primary_requests: usize,
    pub primary_quality: u8,
    pub ultra_quality: u8,
    pub primary_critical_error: bool,
    pub ultra_critical_error: bool,
    pub primary_cost_usd: String,
    pub ultra_cost_usd: String,
    pub primary_duration_ms: u64,
    pub ultra_duration_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Calibration {
    pub schema_version: u32,
    pub runtime: CalibrationRuntime,
    pub super_model_id: String,
    pub ultra_model_id: String,
    pub reviewed_at_unix_ms: u64,
    /// Maximum additional measured inference cost allowed by the campaign reviewer.
    pub maximum_extra_cost_usd: String,
    pub cases: Vec<CalibrationCase>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CalibratedRule {
    pub reason: EscalationReason,
    pub cases: usize,
    pub recoveries: usize,
    pub primary_request_limit: Option<usize>,
}

impl Calibration {
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, &'static str> {
        if bytes.len() > MAX_CALIBRATION_BYTES {
            return Err("calibration_too_large");
        }
        serde_json::from_slice(bytes).map_err(|_| "invalid_calibration_schema")
    }

    /// A rule needs at least three independent live captures, two human-rated recoveries,
    /// no quality regression and no critical Ultra error. These acceptance bounds do not
    /// claim campaign quality: absent real labels, no rule is enabled.
    pub fn rules(&self, settings: &RoutingSettings) -> Result<Vec<CalibratedRule>, &'static str> {
        settings.validate().map_err(|_| "invalid_models")?;
        if self.schema_version != CALIBRATION_SCHEMA_VERSION
            || self.reviewed_at_unix_ms == 0
            || settings.super_model_id.as_deref() != Some(self.super_model_id.as_str())
            || settings.ultra_model_id.as_deref() != Some(self.ultra_model_id.as_str())
            || self.cases.is_empty()
            || self.cases.len() > MAX_CALIBRATION_CASES
        {
            return Err("calibration_model_or_version_mismatch");
        }
        if self.runtime != CalibrationRuntime::current() {
            return Err("calibration_runtime_mismatch");
        }
        let cap = cost::parse_usd_pico(&self.maximum_extra_cost_usd)
            .filter(|v| *v > 0)
            .ok_or("invalid_cost_cap")?;
        let mut captures = BTreeSet::new();
        let mut traces = BTreeSet::new();
        for case in &self.cases {
            let hash = |s: &str| s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit());
            if !hash(&case.capture_sha256)
                || !hash(&case.primary_trace_sha256)
                || !hash(&case.ultra_trace_sha256)
                || !captures.insert(case.capture_sha256.to_ascii_lowercase())
                || !traces.insert(case.primary_trace_sha256.to_ascii_lowercase())
                || !traces.insert(case.ultra_trace_sha256.to_ascii_lowercase())
                || case.origin != CalibrationOrigin::Live
                || !case.human_reviewed
                || case.primary_quality > 4
                || case.ultra_quality > 4
                || !(1..MAX_ROUNDS).contains(&case.primary_requests)
                || case.primary_duration_ms == 0
                || case.ultra_duration_ms == 0
                || cost::parse_usd_pico(&case.primary_cost_usd).is_none()
                || cost::parse_usd_pico(&case.ultra_cost_usd).is_none()
            {
                return Err("calibration_requires_independent_reviewed_live_runs");
            }
        }
        let mut rules = Vec::new();
        for reason in [
            EscalationReason::LowConfidence,
            EscalationReason::InsufficientEvidence,
            EscalationReason::VerificationFailure,
            EscalationReason::PrimaryRequestLimit,
        ] {
            let cases: Vec<_> = self.cases.iter().filter(|c| c.trigger == reason).collect();
            let recoveries = cases
                .iter()
                .filter(|c| {
                    c.ultra_quality > c.primary_quality
                        || (c.primary_critical_error && !c.ultra_critical_error)
                })
                .count();
            let acceptable = cases.len() >= 3
                && recoveries >= 2
                && cases.iter().all(|c| {
                    !c.ultra_critical_error
                        && c.ultra_quality >= c.primary_quality
                        && cost::parse_usd_pico(&c.ultra_cost_usd).is_some_and(|v| v <= cap)
                });
            if acceptable {
                let limit = (reason == EscalationReason::PrimaryRequestLimit)
                    .then(|| cases.iter().map(|c| c.primary_requests).max().unwrap());
                if limit.is_some_and(|v| v >= MAX_ROUNDS - 1) {
                    continue;
                }
                rules.push(CalibratedRule {
                    reason,
                    cases: cases.len(),
                    recoveries,
                    primary_request_limit: limit,
                });
            }
        }
        if rules.is_empty() {
            return Err("no_supported_escalation_rule");
        }
        Ok(rules)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ModelChange {
    pub from_model_id: String,
    pub to_model_id: String,
    pub reason: EscalationReason,
    pub after_requests: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RoutingState {
    pub mode: RoutingMode,
    pub current_model_id: Option<String>,
    pub calibration_reviewed_at_unix_ms: Option<u64>,
    pub change: Option<ModelChange>,
    pub last_request_error: Option<AgentError>,
}

/// The controller retains messages, observations, aliases, repair and tool budgets. This
/// transport changes only the active model, at most once, after a supported local trigger.
pub struct RoutedModel<M> {
    primary: M,
    ultra: Option<M>,
    rules: Vec<CalibratedRule>,
    state: RoutingState,
}
impl<M: Model> RoutedModel<M> {
    pub fn new(
        primary: M,
        ultra: Option<M>,
        settings: &RoutingSettings,
        calibration: Option<&Calibration>,
    ) -> Result<Self, &'static str> {
        settings
            .validate()
            .map_err(|_| "invalid_routing_settings")?;
        let rules = if settings.mode == RoutingMode::Automatic {
            if primary.model_id() != settings.super_model_id.as_deref()
                || ultra.as_ref().and_then(Model::model_id) != settings.ultra_model_id.as_deref()
            {
                return Err("routing_transport_model_mismatch");
            }
            calibration
                .ok_or("automatic_routing_needs_calibration")?
                .rules(settings)?
        } else {
            Vec::new()
        };
        let state = RoutingState {
            mode: settings.mode,
            current_model_id: primary.model_id().map(str::to_owned),
            calibration_reviewed_at_unix_ms: (settings.mode == RoutingMode::Automatic)
                .then(|| calibration.unwrap().reviewed_at_unix_ms),
            change: None,
            last_request_error: None,
        };
        Ok(Self {
            primary,
            ultra,
            rules,
            state,
        })
    }
    fn active(&self) -> &M {
        if self.state.change.is_some() {
            self.ultra.as_ref().unwrap()
        } else {
            &self.primary
        }
    }
    fn active_mut(&mut self) -> &mut M {
        if self.state.change.is_some() {
            self.ultra.as_mut().unwrap()
        } else {
            &mut self.primary
        }
    }
}
impl<M: Model> Model for RoutedModel<M> {
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
        let reply = self
            .active_mut()
            .complete_streaming(messages, tools, choice, timeout, on_call);
        self.state.last_request_error = match &reply {
            Ok(reply) => reply.rejected.clone(),
            Err(error) => Some(error.clone()),
        };
        reply
    }
    fn pricing(&self) -> Option<Pricing> {
        self.active().pricing()
    }
    fn model_id(&self) -> Option<&str> {
        self.active().model_id()
    }
    fn routing_state(&self) -> Option<RoutingState> {
        Some(self.state.clone())
    }
    fn try_escalate(&mut self, reason: EscalationReason, requests: usize) -> Option<ModelChange> {
        if self.state.change.is_some()
            || self.state.last_request_error == Some(AgentError::Cancelled)
            || requests >= MAX_ROUNDS
            || !self.rules.iter().any(|r| {
                r.reason == reason
                    && r.primary_request_limit
                        .is_none_or(|limit| requests >= limit)
            })
        {
            return None;
        }
        let change = ModelChange {
            from_model_id: self.primary.model_id()?.into(),
            to_model_id: self.ultra.as_ref()?.model_id()?.into(),
            reason,
            after_requests: requests,
        };
        self.state.current_model_id = Some(change.to_model_id.clone());
        self.state.change = Some(change.clone());
        Some(change)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn settings() -> RoutingSettings {
        RoutingSettings {
            mode: RoutingMode::Automatic,
            super_model_id: Some("nvidia/nemotron-super-fixture".into()),
            ultra_model_id: Some("nvidia/nemotron-ultra-fixture".into()),
        }
    }
    // Entirely synthetic unit input. It is never saved or presented as a real campaign.
    fn fixture() -> Calibration {
        Calibration {
            schema_version: CALIBRATION_SCHEMA_VERSION,
            runtime: CalibrationRuntime::current(),
            super_model_id: settings().super_model_id.unwrap(),
            ultra_model_id: settings().ultra_model_id.unwrap(),
            reviewed_at_unix_ms: 42,
            maximum_extra_cost_usd: "0.02".into(),
            cases: (1..=3)
                .map(|i| CalibrationCase {
                    capture_sha256: format!("{i:064x}"),
                    primary_trace_sha256: format!("{:064x}", i + 10),
                    ultra_trace_sha256: format!("{:064x}", i + 20),
                    origin: CalibrationOrigin::Live,
                    human_reviewed: true,
                    trigger: EscalationReason::LowConfidence,
                    primary_requests: 2,
                    primary_quality: 1,
                    ultra_quality: 3,
                    primary_critical_error: false,
                    ultra_critical_error: false,
                    primary_cost_usd: "0.001".into(),
                    ultra_cost_usd: "0.01".into(),
                    primary_duration_ms: 10,
                    ultra_duration_ms: 20,
                })
                .collect(),
        }
    }
    #[test]
    fn automatic_requires_real_reviewed_independent_compatible_cases() {
        assert_eq!(fixture().rules(&settings()).unwrap()[0].recoveries, 3);
        for mutate in [0, 1, 2, 3, 4] {
            let mut c = fixture();
            match mutate {
                0 => c.cases[0].origin = CalibrationOrigin::Synthetic,
                1 => c.cases[0].human_reviewed = false,
                2 => c.cases[0].capture_sha256 = c.cases[1].capture_sha256.clone(),
                3 => c.ultra_model_id = "nvidia/nemotron-other".into(),
                _ => c.cases[0].ultra_cost_usd = "NaN".into(),
            }
            assert!(c.rules(&settings()).is_err());
        }
        let mut c = fixture();
        c.cases[0].ultra_critical_error = true;
        assert!(c.rules(&settings()).is_err());
        let mut c = fixture();
        c.cases[0].ultra_quality = 0;
        assert!(c.rules(&settings()).is_err());
        let mut c = fixture();
        c.cases[0].ultra_cost_usd = "1".into();
        assert!(c.rules(&settings()).is_err());
    }
    #[test]
    fn schemas_and_roles_are_closed_and_no_model_is_chosen_by_default() {
        assert_eq!(
            RoutingSettings::default()
                .initial_model("nvidia/nemotron-configured")
                .unwrap(),
            "nvidia/nemotron-configured"
        );
        assert!(serde_json::from_str::<RoutingSettings>(
            r#"{"mode":"configured","api_key":"secret"}"#
        )
        .is_err());
        let mut value = serde_json::to_value(fixture()).unwrap();
        value["command"] = Value::Bool(true);
        assert!(Calibration::from_bytes(&serde_json::to_vec(&value).unwrap()).is_err());
        assert!(Calibration::from_bytes(&vec![b' '; MAX_CALIBRATION_BYTES + 1]).is_err());
        let mut s = settings();
        s.ultra_model_id = Some("anthropic/sonnet".into());
        assert!(s.validate().is_err());
    }

    #[test]
    fn older_or_mismatched_runtime_cannot_enable_automatic_routing() {
        let current = fixture();
        assert!(current.rules(&settings()).is_ok());
        for field in [
            "registered_tool_registry_sha256",
            "analyst_sha256",
            "harness_version",
        ] {
            let mut value = serde_json::to_value(&current).unwrap();
            value["runtime"][field] = Value::String("old-runtime".into());
            let stale = Calibration::from_bytes(&serde_json::to_vec(&value).unwrap()).unwrap();
            assert_eq!(
                stale.rules(&settings()).unwrap_err(),
                "calibration_runtime_mismatch"
            );
        }
        let mut value = serde_json::to_value(&current).unwrap();
        value.as_object_mut().unwrap().remove("runtime");
        assert!(Calibration::from_bytes(&serde_json::to_vec(&value).unwrap()).is_err());
        let mut old = current;
        old.schema_version = 1;
        assert!(old.rules(&settings()).is_err());
    }
}
