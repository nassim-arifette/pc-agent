//! Linux descriptions never promise Windows-only measurements or system integrations.
use crate::{Param, ParamKind, ToolSpec};

pub(crate) fn offered(mut spec: ToolSpec, scopes: bool) -> ToolSpec {
    spec.description = match spec.name {
        "get_system_snapshot" => "Collect fresh Linux physical memory, kernel commit accounting, CPU and disk deltas, and available NVML or DRM GPU observations. Missing devices and permissions produce explicit coverage, never invented zero values.",
        "get_startup_programs" => "Read XDG autostart entries and systemd user-unit startup states. Only current-user XDG entries with an exact local fingerprint can be proposed for a startup change. Commands are never shared.",
        "get_services" => "Read visible system and user systemd service states over D-Bus. Missing buses and protected fields report explicit coverage; unit descriptions are not shared.",
        "get_recent_events" => "Read bounded journal timestamps, priorities and sanitized source references. Journal message bodies, user IDs, hostnames and file contents are never read or sent. Standard-user visibility is explicitly partial.",
        "get_system_state" => "Read Linux uptime, load, pressure-stall information and available distribution reboot markers. Unavailable values stay null.",
        "get_power_and_thermal" => "Read available sysfs battery energy or charge, capacity, wear, cycles, signed power and estimated remaining runtime, plus thermal sensors. No battery or unreadable sensors are explicit missing coverage, not zero values.",
        "get_installed_apps" => "Read locally installed packages using dpkg or rpm. Names and versions follow the selected sharing policy. Package installation does not imply application usage or measured disk size.",
        "get_hardware_inventory" => "Read Linux kernel, architecture, usable RAM, volume space, DMI model/vendor and block-device metadata. If installed and authorized, smartctl contributes only allowlisted numeric health data. No serial numbers or device identifiers.",
        "get_drivers" => "Read loaded Linux kernel modules and locally cached apt, dnf or Flatpak update offers. No network refresh, install or downgrade is performed. Missing or stale caches have explicit coverage.",
        "get_cleanup_opportunities" => "Read metadata-only lower bounds for the user's XDG cache and trash. Cache files require exact local review before moving to recoverable trash. Moving to trash does not reclaim disk space. No file contents are shared.",
        "query_system" => "Run one named, fixed Linux inspection. No shell, query language, paths, usernames or arbitrary executable arguments are accepted. Windows-specific query names return unsupported.",
        _ => spec.description,
    };
    if spec.name == "get_drivers" {
        spec.params = &[
            Param { name:"limit", description:"Maximum loaded modules or cached offers.", kind:ParamKind::Integer {min:1,max:100}, required:false },
            Param { name:"windows_update", description:"Compatibility field: none skips cached offers; offline reads local caches only. Linux never refreshes network metadata.", kind:ParamKind::OneOf(&["none","offline"]), required:false },
        ];
    }
    if spec.name == "prepare_action_plan" {
        spec = super::proposals::linux(scopes);
    }
    spec
}
