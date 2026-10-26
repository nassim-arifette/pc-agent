//! Schemas for reading locally approved helper measurements.
use crate::{Param, ParamKind, ToolClass, ToolSpec};

const TASKS: &[&str] = &[
    "system_caches",
    "mft_inventory",
    "shadow_storage",
    "disk_reliability",
    "process_images",
    "battery_energy",
    "performance_trace",
];
pub const GET_APPROVED_INSPECTION: ToolSpec = ToolSpec {
    name: "get_approved_elevated_inspection", class: ToolClass::Inspection,
    description: "Read the already measured result of one read-only Windows helper task the person approved locally with administrator rights. Tasks: system_caches (sizes of Windows.old, downloaded Windows updates, the Delivery Optimization cache and restore points); mft_inventory (largest folders and files of a whole NTFS volume); shadow_storage (space shadow copies and restore points use and reserve); disk_reliability (each disk's temperature, wear, powered-on hours, read and write errors); process_images (full paths of protected processes); battery_energy (energy each app used on battery); performance_trace (short performance trace). Call it when the question needs one of these. It never launches UAC, collects fresh data, changes files or grants approval. Without an approved result it returns coverage needs_admin: the app then offers that check to the person; do not ask them to run tools themselves. Cite results as captured measurements, not current state.",
    params: &[Param { name: "task", kind: ParamKind::OneOf(TASKS), required: true,
        description: "Fixed read-only inspection whose host-approved cached result is requested." }],
};
pub const TOOLS: &[ToolSpec] = &[GET_APPROVED_INSPECTION];

// Variants for `capabilities::VARIANTS`: only the tasks a host offers on this PC, each with its
// canonical clause. Process images and the performance trace are never offered.
macro_rules! system_caches {
    () => {
        "system_caches (sizes of Windows.old, downloaded Windows updates, the Delivery Optimization cache and restore points)"
    };
}
macro_rules! mft_inventory {
    () => {
        "mft_inventory (largest folders and files of a whole NTFS volume)"
    };
}
macro_rules! shadow_storage {
    () => {
        "shadow_storage (space shadow copies and restore points use and reserve)"
    };
}
macro_rules! disk_reliability {
    () => {
        "disk_reliability (each disk's temperature, wear, powered-on hours, read and write errors)"
    };
}
macro_rules! battery_energy {
    () => {
        "battery_energy (energy each app used on battery)"
    };
}
/// The canonical description with the clauses of the given tasks only.
macro_rules! approved_description {
    ($first:expr $(, $task:expr)*) => {
        concat!(
            "Read the already measured result of one read-only Windows helper task the person approved locally with administrator rights. Tasks: ",
            $first, $("; ", $task,)*
            ". Call it when the question needs one of these. It never launches UAC, collects fresh data, changes files or grants approval. Without an approved result it returns coverage needs_admin: the app then offers that check to the person; do not ask them to run tools themselves. Cite results as captured measurements, not current state."
        )
    };
}
const fn task_param(tasks: &'static [&'static str]) -> Param {
    Param {
        name: "task",
        kind: ParamKind::OneOf(tasks),
        required: true,
        description: "Fixed read-only inspection whose host-approved cached result is requested.",
    }
}
const OFFERED_TASK: Param = task_param(&[
    "system_caches",
    "mft_inventory",
    "shadow_storage",
    "disk_reliability",
    "battery_energy",
]);
const TASK_WITHOUT_BATTERY: Param = task_param(&[
    "system_caches",
    "mft_inventory",
    "shadow_storage",
    "disk_reliability",
]);
const TASK_WITHOUT_NTFS: Param = task_param(&[
    "system_caches",
    "shadow_storage",
    "disk_reliability",
    "battery_energy",
]);
const CORE_TASK: Param = task_param(&["system_caches", "shadow_storage", "disk_reliability"]);
/// Every task a host offers: a PC with a battery and a fixed NTFS volume.
pub(crate) const OFFERED_TASKS: ToolSpec = ToolSpec {
    description: approved_description!(
        system_caches!(),
        mft_inventory!(),
        shadow_storage!(),
        disk_reliability!(),
        battery_energy!()
    ),
    params: &[OFFERED_TASK],
    ..GET_APPROVED_INSPECTION
};
pub(crate) const TASKS_WITHOUT_BATTERY: ToolSpec = ToolSpec {
    description: approved_description!(
        system_caches!(),
        mft_inventory!(),
        shadow_storage!(),
        disk_reliability!()
    ),
    params: &[TASK_WITHOUT_BATTERY],
    ..GET_APPROVED_INSPECTION
};
pub(crate) const TASKS_WITHOUT_NTFS: ToolSpec = ToolSpec {
    description: approved_description!(
        system_caches!(),
        shadow_storage!(),
        disk_reliability!(),
        battery_energy!()
    ),
    params: &[TASK_WITHOUT_NTFS],
    ..GET_APPROVED_INSPECTION
};
pub(crate) const CORE_TASKS: ToolSpec = ToolSpec {
    description: approved_description!(system_caches!(), shadow_storage!(), disk_reliability!()),
    params: &[CORE_TASK],
    ..GET_APPROVED_INSPECTION
};
