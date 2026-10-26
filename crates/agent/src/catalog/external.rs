//! Closed schemas for locally composed web lookups.
use crate::{Param, ParamKind, ToolClass, ToolSpec};

const DEVICE_REF: Param = Param {
    name: "device_ref",
    description: "A device reference returned by get_drivers in this turn.",
    kind: ParamKind::OpaqueRef { prefix: "device-" },
    required: true,
};

pub const LOOKUP_DRIVER_NOTES: ToolSpec = ToolSpec {
    name: "lookup_driver_notes",
    class: ToolClass::External,
    description: "Search the driver maker's website for notes on one installed driver: latest version, open and fixed issues, security bulletins. The query is composed locally from the device's provider, model and installed version (call get_drivers first). Returns web snippets cited [W1], [W2]…",
    params: &[DEVICE_REF],
};
pub const LOOKUP_KNOWN_ISSUE: ToolSpec = ToolSpec {
    name: "lookup_known_issue",
    class: ToolClass::External,
    description: "Search Microsoft's documentation for a known issue: give either kb (an update number like KB5034441) or code (a stop or error code like 0x0000009F), as measured by get_recent_events, get_system_state or get_drivers. Returns web snippets cited [W1], [W2]…",
    params: &[
        Param {
            name: "kb",
            description: "A Windows update number from a measurement, like KB5034441.",
            kind: ParamKind::OpaqueRef { prefix: "KB" },
            required: false,
        },
        Param {
            name: "code",
            description: "A stop or error code from a measurement, like 0x0000009F.",
            kind: ParamKind::OpaqueRef { prefix: "0x" },
            required: false,
        },
    ],
};
pub const LOOKUP_UPGRADE_PRICE: ToolSpec = ToolSpec {
    name: "lookup_upgrade_price",
    class: ToolClass::External,
    description: "Search shops of the user's country for the price of a compatible memory module, composed locally from the measured memory type, form factor, speed and module size. Returns dated web snippets cited [W1], [W2]…",
    params: &[Param {
        name: "component",
        description: "The part to price.",
        kind: ParamKind::OneOf(&["memory"]),
        required: true,
    }],
};
pub const LOOKUP_PC_SPECS: ToolSpec = ToolSpec {
    name: "lookup_pc_specs",
    class: ToolClass::External,
    description: "Search the PC maker's website for this model's specification sheet: maximum memory, soldered memory, free slots. The query is composed locally from the measured maker and model. Returns web snippets cited [W1], [W2]…",
    params: &[],
};
pub const TOOLS: &[ToolSpec] = &[
    LOOKUP_DRIVER_NOTES,
    LOOKUP_KNOWN_ISSUE,
    LOOKUP_UPGRADE_PRICE,
    LOOKUP_PC_SPECS,
];

pub fn is_external(tool: &str) -> bool {
    TOOLS.iter().any(|spec| spec.name == tool)
}
