//! Declarative read-only tool registry. Arguments are validated locally against a closed
//! schema before any dispatch; unknown keys, types or out-of-range values are rejected.
use crate::{cloud_observation, AgentError, ToolCall};
use pcagent_diagnostics::{analyze, Finding};
use pcagent_shared::{Evidence, ProcessInstance, Snapshot, Status};
use serde_json::{json, Map, Value};
use std::collections::BTreeMap;

pub const MAX_REF_BYTES: usize = 128;

#[derive(Debug, Clone, Copy)]
pub enum ParamKind {
    Integer {
        min: u64,
        max: u64,
    },
    OneOf(&'static [&'static str]),
    /// A locally issued opaque reference such as `proc-…`; never a path, name or PID.
    OpaqueRef {
        prefix: &'static str,
    },
}
#[derive(Debug, Clone, Copy)]
pub struct Param {
    pub name: &'static str,
    pub description: &'static str,
    pub kind: ParamKind,
    pub required: bool,
}
#[derive(Debug, Clone, Copy)]
pub struct ToolSpec {
    pub name: &'static str,
    pub description: &'static str,
    pub params: &'static [Param],
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArgValue {
    Integer(u64),
    Text(String),
}
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Args(BTreeMap<&'static str, ArgValue>);
impl Args {
    pub fn integer(&self, name: &str) -> Option<u64> {
        match self.0.get(name) {
            Some(ArgValue::Integer(v)) => Some(*v),
            _ => None,
        }
    }
    pub fn text(&self, name: &str) -> Option<&str> {
        match self.0.get(name) {
            Some(ArgValue::Text(v)) => Some(v),
            _ => None,
        }
    }
}

impl ToolSpec {
    pub fn definition(&self) -> Value {
        let mut properties = Map::new();
        for p in self.params {
            let schema = match p.kind {
                ParamKind::Integer { min, max } => {
                    json!({"type":"integer","minimum":min,"maximum":max})
                }
                ParamKind::OneOf(values) => json!({"type":"string","enum":values}),
                ParamKind::OpaqueRef { prefix } => {
                    json!({"type":"string","pattern":format!("^{prefix}[A-Za-z0-9:-]+$"),"maxLength":MAX_REF_BYTES})
                }
            };
            let mut schema = schema;
            schema["description"] = p.description.into();
            properties.insert(p.name.into(), schema);
        }
        let required: Vec<&str> = self
            .params
            .iter()
            .filter(|p| p.required)
            .map(|p| p.name)
            .collect();
        json!({"type":"function","function":{"name":self.name,"description":self.description,
            "parameters":{"type":"object","properties":properties,"required":required,"additionalProperties":false}}})
    }
    pub fn validate(&self, arguments: &str) -> Result<Args, AgentError> {
        let value: Value = serde_json::from_str(arguments).map_err(|_| AgentError::InvalidTool)?;
        let object = value.as_object().ok_or(AgentError::InvalidTool)?;
        if object
            .keys()
            .any(|k| !self.params.iter().any(|p| p.name == k))
        {
            return Err(AgentError::InvalidTool);
        }
        let mut args = Args::default();
        for p in self.params {
            let Some(v) = object.get(p.name) else {
                if p.required {
                    return Err(AgentError::InvalidTool);
                }
                continue;
            };
            let parsed = match p.kind {
                ParamKind::Integer { min, max } => v
                    .as_u64()
                    .filter(|n| (min..=max).contains(n))
                    .map(ArgValue::Integer),
                ParamKind::OneOf(values) => v
                    .as_str()
                    .filter(|s| values.contains(s))
                    .map(|s| ArgValue::Text(s.into())),
                ParamKind::OpaqueRef { prefix } => v
                    .as_str()
                    .filter(|s| {
                        s.len() <= MAX_REF_BYTES
                            && s.strip_prefix(prefix).is_some_and(|rest| {
                                !rest.is_empty()
                                    && rest.bytes().all(|b| {
                                        b.is_ascii_alphanumeric() || b == b'-' || b == b':'
                                    })
                            })
                    })
                    .map(|s| ArgValue::Text(s.into())),
            };
            args.0
                .insert(p.name, parsed.ok_or(AgentError::InvalidTool)?);
        }
        Ok(args)
    }
}

/// Validates the envelope, resolves the tool by name and checks its arguments.
pub fn validate_call<'a>(
    call: &ToolCall,
    specs: &'a [ToolSpec],
) -> Result<(&'a ToolSpec, Args), AgentError> {
    if call.kind != "function"
        || call.id.is_empty()
        || call.id.len() > 256
        || call.id.chars().any(|c| !c.is_ascii_graphic())
    {
        return Err(AgentError::InvalidTool);
    }
    let spec = specs
        .iter()
        .find(|s| s.name == call.function.name)
        .ok_or(AgentError::InvalidTool)?;
    Ok((spec, spec.validate(&call.function.arguments)?))
}

/// Result of a local tool. `cloud` must already be minimized by an allowlist.
#[derive(Debug, Clone)]
pub struct ToolOutput {
    pub status: Status,
    pub cloud: Value,
    pub evidence: Vec<Evidence>,
    pub findings: Vec<Finding>,
    pub processes: Vec<ProcessInstance>,
    pub limitations: Vec<String>,
}
impl ToolOutput {
    pub fn from_snapshot(s: &Snapshot) -> Self {
        Self {
            status: if s.has_observations() {
                Status::Ok
            } else {
                Status::Partial
            },
            cloud: serde_json::to_value(cloud_observation(s)).expect("serializable observation"),
            evidence: s.evidence.clone(),
            findings: analyze(s),
            processes: s.processes.clone(),
            limitations: s.limitations.clone(),
        }
    }
}

/// Local controller side of the tool boundary. Only read-only tools may be registered.
pub trait Toolbox {
    fn specs(&self) -> &[ToolSpec];
    /// Called only with a spec from `specs()` and arguments validated against it.
    fn run(&mut self, spec: &ToolSpec, args: &Args) -> ToolOutput;
    fn definitions(&self) -> Value {
        Value::Array(self.specs().iter().map(ToolSpec::definition).collect())
    }
}

pub const MAX_PROCESSES: u64 = 10;
const SNAPSHOT_TOOLS: [ToolSpec; 2] = [
    ToolSpec {
        name: "get_system_snapshot",
        description: "Collect fresh system memory, aggregate and busiest-core CPU, disk latency, hard-fault and GPU engine-type observations. No changes to the computer.",
        params: &[],
    },
    ToolSpec {
        name: "get_processes",
        description: "Collect readable process memory contributors, represented by opaque session-scoped identities. No paths, names or commands.",
        params: &[Param {
            name: "limit",
            description: "Maximum process instances to return (default 10).",
            kind: ParamKind::Integer { min: 1, max: MAX_PROCESSES },
            required: false,
        }],
    },
];

/// Snapshot-backed tools; `collect(include_processes, limit)` is the real collector.
pub struct SnapshotTools<F: FnMut(bool, usize) -> Snapshot> {
    collect: F,
}
impl<F: FnMut(bool, usize) -> Snapshot> SnapshotTools<F> {
    pub fn new(collect: F) -> Self {
        Self { collect }
    }
}
impl<F: FnMut(bool, usize) -> Snapshot> Toolbox for SnapshotTools<F> {
    fn specs(&self) -> &[ToolSpec] {
        &SNAPSHOT_TOOLS
    }
    fn run(&mut self, spec: &ToolSpec, args: &Args) -> ToolOutput {
        let processes = spec.name == "get_processes";
        let limit = args
            .integer("limit")
            .unwrap_or(MAX_PROCESSES)
            .min(MAX_PROCESSES);
        ToolOutput::from_snapshot(&(self.collect)(processes, limit as usize))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::FunctionCall;
    const SPEC: ToolSpec = ToolSpec {
        name: "t",
        description: "",
        params: &[
            Param {
                name: "n",
                description: "",
                kind: ParamKind::Integer { min: 1, max: 5 },
                required: true,
            },
            Param {
                name: "mode",
                description: "",
                kind: ParamKind::OneOf(&["a", "b"]),
                required: false,
            },
            Param {
                name: "ref",
                description: "",
                kind: ParamKind::OpaqueRef { prefix: "proc-" },
                required: false,
            },
        ],
    };
    #[test]
    fn arguments_follow_a_closed_schema() {
        let a = SPEC
            .validate(r#"{"n":3,"mode":"b","ref":"proc-12ab"}"#)
            .unwrap();
        assert_eq!(a.integer("n"), Some(3));
        assert_eq!(a.text("ref"), Some("proc-12ab"));
        for bad in [
            r#"{}"#,
            r#"{"n":0}"#,
            r#"{"n":6}"#,
            r#"{"n":-1}"#,
            r#"{"n":2.5}"#,
            r#"{"n":"3"}"#,
            r#"{"n":1,"mode":"c"}"#,
            r#"{"n":1,"extra":true}"#,
            r#"{"n":1,"ref":"C:\\Users"}"#,
            r#"{"n":1,"ref":"proc-"}"#,
            r#"{"n":1,"ref":"proc-../x"}"#,
            "null",
            "[]",
        ] {
            assert_eq!(SPEC.validate(bad), Err(AgentError::InvalidTool), "{bad}");
        }
    }
    #[test]
    fn definitions_are_closed_objects() {
        let d = SPEC.definition();
        assert_eq!(d["function"]["parameters"]["additionalProperties"], false);
        assert_eq!(d["function"]["parameters"]["required"], json!(["n"]));
    }
    #[test]
    fn envelope_and_names_are_checked() {
        let call = |id: &str, kind: &str, name: &str| ToolCall {
            id: id.into(),
            kind: kind.into(),
            function: FunctionCall {
                name: name.into(),
                arguments: "{}".into(),
            },
        };
        assert!(validate_call(
            &call("a", "function", "get_system_snapshot"),
            &SNAPSHOT_TOOLS
        )
        .is_ok());
        for c in [
            call("", "function", "get_processes"),
            call("a b", "function", "get_processes"),
            call("a", "code", "get_processes"),
            call("a", "function", "run_shell"),
        ] {
            assert_eq!(
                validate_call(&c, &SNAPSHOT_TOOLS).err(),
                Some(AgentError::InvalidTool)
            );
        }
    }
    #[test]
    fn process_limit_is_forwarded_and_bounded() {
        let mut seen = Vec::new();
        let mut tools = SnapshotTools::new(|p, l| {
            seen.push((p, l));
            Snapshot::empty("s")
        });
        let spec = SNAPSHOT_TOOLS[1];
        tools.run(&spec, &spec.validate(r#"{"limit":3}"#).unwrap());
        tools.run(&spec, &Args::default());
        tools.run(&SNAPSHOT_TOOLS[0], &Args::default());
        assert_eq!(seen, [(true, 3), (true, 10), (false, 10)]);
    }
}
