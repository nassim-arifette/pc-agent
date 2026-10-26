//! Schema for a fresh process-activity measurement.
use crate::{Param, ParamKind, ToolClass, ToolSpec};

pub const TOOLS: &[ToolSpec] = &[ToolSpec {
    name: "get_process_activity",
    class: ToolClass::Inspection,
    description: "Measure CPU, memory increase/decrease, I/O throughput and observed GPU activity over one fresh window. Return processes and application groups, ranked by CPU then I/O and memory, with service attribution and explicit coverage. Growth is an observation, not proof of a leak. No commands, paths or process IDs.",
    params: &[
        Param { name: "window_seconds", description: "Fresh two-sample measurement duration in seconds (default 3).", kind: ParamKind::Integer { min: 1, max: 30 }, required: false },
        Param { name: "limit", description: "Maximum returned processes and applications, after measuring all instances (default 10).", kind: ParamKind::Integer { min: 1, max: 50 }, required: false },
    ],
}];
