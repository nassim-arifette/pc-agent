//! Closed schemas for standard-user system inspections.
use crate::{Param, ParamKind, ToolClass, ToolSpec};
use pcagent_inspect::QUERY_NAMES;

const LIMIT: Param = Param {
    name: "limit",
    description: "Maximum returned entries (default 100).",
    kind: ParamKind::Integer { min: 1, max: 500 },
    required: false,
};
pub const TOOLS:&[ToolSpec]=&[
    ToolSpec{name:"get_startup_programs",class:ToolClass::Inspection,description:"Read Run/RunOnce, startup folders, StartupApproved state and logon scheduled tasks. Missing approval states stay unknown; commands and task actions are never read.",params:&[]},
    ToolSpec{name:"get_services",class:ToolClass::Inspection,description:"Read visible Windows services, state, startup type, opaque shared-process groups and recent SCM failure counts. Coverage says which protected services or configuration fields are unavailable.",params:&[LIMIT]},
    ToolSpec{name:"get_recent_events",class:ToolClass::Inspection,description:"Count selected crash, hang, hardware, disk, update and resource exhaustion events; return bounded latest occurrences. No event messages, user IDs, computer names or file contents.",params:&[
        Param{name:"days",description:"Trailing calendar duration in days (default 7, maximum 30).",kind:ParamKind::Integer{min:1,max:30},required:false},
        Param{name:"limit",description:"Maximum recent occurrence timestamps (default 50).",kind:ParamKind::Integer{min:1,max:200},required:false}]},
    ToolSpec{name:"get_system_state",class:ToolClass::Inspection,description:"Read uptime, Windows build, pending-reboot markers, fast-startup configuration, installed servicing hotfixes and pagefile usage. Missing dates and incomplete update coverage are explicit.",params:&[]},
    ToolSpec{name:"get_power_and_thermal",class:ToolClass::Inspection,description:"Read battery design/full/remaining capacities, estimated wear, optional cycles, signed charge/discharge power and separate Windows/current-rate runtime estimates; AC/saver, active plan, CPU frequency/limits and supported thermal/GPU sensors. Unknown values/units stay null; batteries stay separate. Application activity in recordings shows correlations, not measured per-app watts.",params:&[]},
    ToolSpec{name:"get_installed_apps",class:ToolClass::Inspection,description:"Read Uninstall registrations across HKCU/HKLM and registry views: publisher, version, installation date and estimated size. Estimates are not measured disk use; Store/portable app coverage is explicit.",params:&[LIMIT]},
    ToolSpec{name:"get_hardware_inventory",class:ToolClass::Inspection,description:"Read CPU, Windows build, RAM capacity and SMBIOS memory slots, volume space, disk bus/rotational properties and graphics driver versions. No serial numbers or hardware identifiers.",params:&[]},
    ToolSpec{name:"get_drivers",class:ToolClass::Inspection,description:"Read installed display, network, storage, system, audio and Bluetooth drivers and Windows Update offers. The cache is used by default. Version comparisons are unknown unless the offer matches one device unambiguously. Never recommend a downgrade or install a driver.",params:&[
        Param{name:"limit",description:"Maximum installed devices (default 64).",kind:ParamKind::Integer{min:1,max:100},required:false},
        Param{name:"windows_update",description:"none skips offers; offline reads the local update cache (default); online explicitly contacts Microsoft and may time out.",kind:ParamKind::OneOf(&["none","offline","online"]),required:false}]},
    ToolSpec{name:"get_cleanup_opportunities",class:ToolClass::Inspection,description:"Bounded metadata-only inventory of recycle bin, temporary/browser/shader/development caches, WSL/Docker disks, old large downloads, Windows update/rollback files and system-managed files. Categories report review/regenerable/keep, unknown sizes and partial lower bounds. Proposes no deletion and grants no permission.",params:&[]},
    ToolSpec{name:"query_system",class:ToolClass::Inspection,description:"Execute one fixed allowlisted read-only WMI, registry or event-log query. Cannot accept commands, WQL, paths, usernames, remote hosts or arbitrary registry keys.",params:&[
        Param{name:"query",description:"Named fixed inspection.",kind:ParamKind::OneOf(QUERY_NAMES),required:true},
        Param{name:"limit",description:"Maximum WMI rows or event occurrences (default 50).",kind:ParamKind::Integer{min:1,max:100},required:false}]},
];
/// `get_power_and_thermal` on a PC where Windows lists no system battery
/// (`capabilities::VARIANTS`).
pub(crate) const POWER_WITHOUT_BATTERY: ToolSpec = ToolSpec {
    name: "get_power_and_thermal",
    class: ToolClass::Inspection,
    description: "Read AC/saver state, active plan, CPU frequency/limits and supported thermal/GPU sensors. Windows lists no system battery on this PC, so there is no battery capacity, wear, charge or runtime to read. Unknown values/units stay null.",
    params: &[],
};
