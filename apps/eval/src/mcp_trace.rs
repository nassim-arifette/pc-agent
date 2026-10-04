//! Bound retained MCP calls separately from the transport's request-count limit.
use pcagent_agent::{
    replay::{RecordedCall, Recorder},
    Args, ToolOutput, ToolSpec, Toolbox,
};
use pcagent_shared::Status;
use serde_json::{json, Value};

const MAX_CALL_BYTES: usize = 16 * 1024 * 1024;

pub struct TraceTools<T> {
    recorder: Recorder<T>,
    remaining: usize,
    complete: bool,
}
impl<T> TraceTools<T> {
    pub fn new(inner: T) -> Self {
        Self {
            recorder: Recorder::new(inner),
            remaining: MAX_CALL_BYTES,
            complete: true,
        }
    }
    pub fn into_parts(self) -> (T, Vec<RecordedCall>, bool) {
        let (tools, calls) = self.recorder.into_parts();
        (tools, calls, self.complete)
    }
}
impl<T: Toolbox> Toolbox for TraceTools<T> {
    fn specs(&self) -> &[ToolSpec] {
        self.recorder.specs()
    }
    fn sharing(&self) -> pcagent_agent::Sharing {
        self.recorder.sharing()
    }
    fn definitions(&self) -> Value {
        self.recorder.definitions()
    }
    fn run(&mut self, spec: &ToolSpec, args: &Args) -> ToolOutput {
        if self.complete {
            let output = self.recorder.run(spec, args);
            let size = serde_json::to_vec(self.recorder.calls.last().expect("recorded call"))
                .expect("serializable call")
                .len();
            if size <= self.remaining {
                self.remaining -= size;
                return output;
            }
            self.recorder.calls.pop();
            self.complete = false;
        }
        ToolOutput {
            status: Status::Unsupported,
            inspection_complete: false,
            cloud: json!({"status":"unsupported", "reason":"Local MCP recording budget exhausted; end this session. No further tools will run."}),
            evidence: Vec::new(),
            findings: Vec::new(),
            processes: Vec::new(),
            storage_items: Vec::new(),
            limitations: vec![
                "MCP recording is incomplete: its retained-call byte budget was exhausted.".into(),
            ],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pcagent_agent::{SnapshotTools, WORKSTATION_TOOLS};
    use pcagent_shared::Snapshot;

    #[test]
    fn an_exhausted_trace_stops_collecting_and_is_explicitly_incomplete() {
        let collected = std::cell::Cell::new(0);
        let inner = SnapshotTools::new(|_, _| {
            collected.set(collected.get() + 1);
            Snapshot::empty("test")
        });
        let mut tools = TraceTools::new(inner);
        tools.remaining = 0;
        let spec = WORKSTATION_TOOLS
            .iter()
            .find(|spec| spec.name == "get_system_snapshot")
            .unwrap();
        let args = spec.validate("{}").unwrap();
        for _ in 0..2 {
            let output = tools.run(spec, &args);
            assert_eq!(output.status, Status::Unsupported);
            assert!(!output.inspection_complete);
        }
        let (_, calls, complete) = tools.into_parts();
        assert_eq!(collected.get(), 1);
        assert!(calls.is_empty());
        assert!(!complete);
    }
}
