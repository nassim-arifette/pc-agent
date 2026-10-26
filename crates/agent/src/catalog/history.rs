//! Schemas for persisted history and bounded recordings.
pub const MAX_HISTORY_POINTS: usize = 120;
pub const MAX_RECORDING_POINTS: usize = 30;
pub const MAX_PROCESS_PEAKS: usize = 10;

use crate::{Param, ParamKind, ToolClass, ToolSpec};

pub const METRICS: &[&str] = &[
    "physical_total_bytes",
    "physical_available_bytes",
    "system_commit_bytes",
    "system_commit_limit_bytes",
    "cpu_busy_basis_points",
    "cpu_busiest_core_basis_points",
    "disk_read_latency_microseconds",
    "disk_write_latency_microseconds",
    "disk_idle_basis_points",
    "disk_queue_length",
    "hard_fault_pages_per_second_x100",
    "gpu_busiest_engine_basis_points",
    "process_working_set_bytes",
    "process_private_commit_bytes",
    "process_cpu_basis_points",
    "process_io_read_bytes_per_second",
    "process_io_write_bytes_per_second",
    "process_io_other_bytes_per_second",
    "process_gpu_basis_points",
];
const WINDOW: Param = Param {
    name: "window_seconds",
    description: "History window, up to seven days.",
    kind: ParamKind::Integer {
        min: 1,
        max: 604_800,
    },
    required: true,
};
const END: Param = Param {
    name: "end_unix_ms",
    description:
        "Optional end of a past window as Unix milliseconds; defaults to now. Never a future time.",
    kind: ParamKind::Integer {
        min: 1,
        max: i64::MAX as u64,
    },
    required: false,
};
pub const TOOLS: &[ToolSpec] = &[
    ToolSpec { name:"get_metric_history", class:ToolClass::Inspection,
        description:"Read persisted local samples, including previous app sessions. Seven-day retention; one-minute averages/minima/maxima after 24 hours. At most 120 response points retain weighted averages, extrema and coverage. Gaps and missing process rows remain unknown. Use end_unix_ms to investigate a past time.",
        params:&[Param {name:"metric",description:"Recorded metric.",kind:ParamKind::OneOf(METRICS),required:true},WINDOW,END,
            Param {name:"entity_ref",description:"Opaque process reference from history/events, or system (default).",kind:ParamKind::OpaqueRef {prefix:""},required:false}] },
    ToolSpec { name:"get_activity_events",class:ToolClass::Inspection,
        description:"Measured CPU, memory and disk latency peaks in local history, limited to 30 recent events with explicit omissions. Contributor means observed resource use, not proven cause; normal recovery is not an incident.",params:&[WINDOW,END] },
    ToolSpec { name:"record_activity",class:ToolClass::Inspection,
        description:"Measure the problem now at one-second cadence for 30–120 seconds. Returns markers, at most 120 system history points, gaps, 10 process CPU peaks, up to 10 separate peaks each for process GPU/read/write activity, and at most 128 battery points across 16 opaque batteries. Activity is limited to sampled processes and indicates correlation, not per-app energy. Battery evolution uses only continuous coherent discharge intervals. Can be cancelled by stopping the turn. Reads only; changes no system setting.",
        params:&[Param{name:"duration_seconds",description:"Recording duration, default 30 seconds.",kind:ParamKind::Integer{min:30,max:120},required:false}] },
];
