//! Closed schemas for locally reviewed action proposals.
use crate::{Param, ParamKind, ToolSpec};

pub const PROPOSABLE_KINDS: &[&str] = &[
    "open_in_explorer",
    "open_settings_page",
    "run_cleanup_adapter",
    "disable_user_startup",
    "request_normal_close",
    "recycle_selected_files",
];
pub const SETTINGS_PAGES: &[&str] = &[
    "storagesense",
    "appsfeatures",
    "startupapps",
    "windowsupdate",
    "windowsupdate-optionalupdates",
    "powersleep",
    "batterysaver-usagedetails",
];
pub const USER_CLEANUP_ADAPTERS: &[&str] = &["user.temp", "nvidia.dxcache", "windows.d3dscache"];

pub const PREPARE_ACTION_PLAN: ToolSpec = ToolSpec {
    name: "prepare_action_plan",
    class: crate::tools::ToolClass::Proposal,
    description: "Propose one action for the user to review locally after the investigation. \
        Nothing runs now; you cannot approve or execute it. open_settings_page needs \
        settings_page. open_in_explorer needs scope_id + item_ref from get_largest_items or \
        get_cleanup_candidates. recycle_selected_files needs a file scope_id + item_ref listed \
        as proposable_actions by those tools. request_normal_close needs process_ref from \
        get_processes. disable_user_startup needs startup_ref from get_startup_programs. \
        run_cleanup_adapter needs adapter_id from get_cleanup_opportunities, or scope_id + \
        item_ref of a validated synthetic demo directory from get_cleanup_candidates. Use \
        only locally issued references; never send a path, command, PID or registry name.",
    params: &[
        ACTION_KIND,
        SCOPE_ID,
        ITEM_REF,
        SETTINGS_PAGE,
        STARTUP_REF,
        ADAPTER_ID,
        PROCESS_REF,
    ],
};
const ACTION_KIND: Param = Param {
    name: "action_kind",
    description: "Action to propose.",
    kind: ParamKind::OneOf(PROPOSABLE_KINDS),
    required: true,
};
const SCOPE_ID: Param = Param {
    name: "scope_id",
    description: "Opaque storage scope of item_ref.",
    kind: ParamKind::OpaqueRef { prefix: "scope-" },
    required: false,
};
const ITEM_REF: Param = Param {
    name: "item_ref",
    description: "Opaque item reference from get_largest_items or get_cleanup_candidates.",
    kind: ParamKind::OpaqueRef { prefix: "scan-" },
    required: false,
};
const SETTINGS_PAGE: Param = Param {
    name: "settings_page",
    description: "Allowlisted Windows Settings page.",
    kind: ParamKind::OneOf(SETTINGS_PAGES),
    required: false,
};
const STARTUP_REF: Param = Param {
    name: "startup_ref",
    description: "Opaque current-user Run target listed by get_startup_programs.",
    kind: ParamKind::OpaqueRef { prefix: "startup-" },
    required: false,
};
const ADAPTER_ID: Param = Param {
    name: "adapter_id",
    description: "Finite adapter listed as proposable by get_cleanup_opportunities.",
    kind: ParamKind::OneOf(USER_CLEANUP_ADAPTERS),
    required: false,
};
const PROCESS_REF: Param = Param {
    name: "process_ref",
    description: "Opaque process instance listed as proposable by get_processes.",
    kind: ParamKind::OpaqueRef { prefix: "proc-" },
    required: false,
};
/// `prepare_action_plan` in a turn without an approved storage scope (`capabilities::VARIANTS`):
/// no action that needs a scanned item, and no storage tool named.
pub(crate) const PREPARE_WITHOUT_STORAGE: ToolSpec = ToolSpec {
    description: "Propose one action for the user to review locally after the investigation. \
        Nothing runs now; you cannot approve or execute it. open_settings_page needs \
        settings_page. request_normal_close needs process_ref from get_processes. \
        disable_user_startup needs startup_ref from get_startup_programs. run_cleanup_adapter \
        needs adapter_id from get_cleanup_opportunities. Use only locally issued references; \
        never send a path, command, PID or registry name.",
    params: &[
        Param {
            kind: ParamKind::OneOf(&[
                "open_settings_page",
                "run_cleanup_adapter",
                "disable_user_startup",
                "request_normal_close",
            ]),
            ..ACTION_KIND
        },
        SETTINGS_PAGE,
        STARTUP_REF,
        ADAPTER_ID,
        PROCESS_REF,
    ],
    ..PREPARE_ACTION_PLAN
};
