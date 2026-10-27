//! Applications, member processes and service activity for the desktop.
use super::{
    labels::{read_name, readable_name},
    process_name, Measure, Readable,
};
use pcagent_shared::{Evidence, Metric, ProcessCoverage, ProcessInstance, Status};
use serde::Serialize;

/// A Windows service hosted by one of an application's processes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ServiceView {
    pub name: String,
    pub display_name: String,
    pub state: String,
}
/// One process of an application group, so a group is never mistaken for a single process.
#[derive(Debug, Clone, Serialize)]
pub struct MemberView {
    pub entity_ref: String,
    /// The executable name: the tooltip of `readable_name`, and the only name a question may
    /// carry (C5).
    pub name: String,
    /// What the window shows when known: the executable's own description ("Google Chrome").
    /// Local display only, never sent anywhere.
    pub readable_name: Option<String>,
    pub pid: u32,
    pub cpu: Measure,
    pub working_set: Measure,
    pub services: Vec<ServiceView>,
    /// No executable name was read: `name` is the English fallback the window replaces.
    pub unnamed: bool,
    pub browser: pcagent_telemetry::browser::BrowserClassification,
}
/// Closed types and measured sums only. Captions/tabs are returned by a separate local command.
#[derive(Debug, Clone, Serialize)]
pub struct BrowserTypeView {
    pub process_type: pcagent_telemetry::browser::BrowserProcessType,
    pub process_count: usize,
    pub classified_count: usize,
    pub working_set: Measure,
    pub cpu: Measure,
}
/// An application group: its measurements are sums over its members during the window.
#[derive(Debug, Clone, Serialize)]
pub struct ApplicationView {
    pub entity_ref: String,
    /// Local UI identity across captures; never a model selection reference.
    pub local_key: String,
    /// The executable the group is named after, as for `MemberView::name`.
    pub name: String,
    /// The readable name of the member the group is named after, as for `MemberView`.
    pub readable_name: Option<String>,
    pub process_count: usize,
    pub members_truncated: bool,
    pub cpu: Measure,
    pub working_set: Measure,
    pub private_commit: Measure,
    pub working_set_increase: Measure,
    pub io_read: Measure,
    pub io_write: Measure,
    pub gpu: Measure,
    pub services: Vec<ServiceView>,
    /// Members among the returned processes; others were ranked lower and are counted only.
    pub members: Vec<MemberView>,
    /// No application name was read: `name` is the English fallback the window replaces.
    pub unnamed: bool,
    pub browser_types: Vec<BrowserTypeView>,
}
#[derive(Debug, Clone, Serialize)]
pub struct ActivityView {
    pub selection_source_id: Option<String>,
    pub collected_at_unix_ms: u64,
    pub window_ms: u64,
    pub status: Status,
    pub applications: Vec<ApplicationView>,
    pub applications_total: usize,
    pub coverage: ProcessCoverage,
    pub services_status: Status,
    pub gpu_status: Status,
    pub limitations: Vec<String>,
}

pub(super) fn evidence_measure(evidence: &[Evidence], entity: &str, metric: Metric) -> Measure {
    Measure::of(
        evidence
            .iter()
            .find(|e| e.entity_ref == entity && e.metric == metric),
    )
}

pub(super) fn services(list: &[pcagent_telemetry::ServiceProcess]) -> Vec<ServiceView> {
    list.iter()
        .map(|s| ServiceView {
            name: s.service_name.clone(),
            display_name: s.display_name.clone(),
            state: s.state.clone(),
        })
        .collect()
}

/// The member an application group is named after, as the collector names it: the earliest
/// instance (creation time, then PID) of the group's executable among its listed members.
/// `instance` resolves a member reference, including members ranked too low to be returned.
pub(super) fn naming_member(
    app: &pcagent_telemetry::ApplicationActivity,
    instance: &impl Fn(&str) -> Option<ProcessInstance>,
) -> Option<ProcessInstance> {
    let name = app.name.as_deref()?;
    app.process_refs
        .iter()
        .filter_map(|reference| instance(reference))
        .filter(|p| p.executable_name.as_deref() == Some(name))
        .min_by_key(|p| (p.creation_time_filetime, p.pid))
}

pub fn application_key(
    app: &pcagent_telemetry::ApplicationActivity,
    instance: &impl Fn(&str) -> Option<ProcessInstance>,
) -> String {
    naming_member(app, instance).map_or_else(|| app.entity_ref.clone(), |p| p.entity_ref)
}

/// Application groups with their members and services, for the local Live PC view. `instance`
/// resolves member references (the live collector's registry); `readable` gives readable names.
pub fn activity(
    a: &pcagent_telemetry::ProcessActivity,
    instance: impl Fn(&str) -> Option<ProcessInstance>,
    readable: Readable<'_>,
) -> ActivityView {
    let m = |entity: &str, metric| evidence_measure(&a.evidence, entity, metric);
    let applications = a
        .applications
        .iter()
        .map(|app| {
            let members: Vec<MemberView> = a
                .processes
                .iter()
                .filter(|p| p.application_ref == app.entity_ref)
                .map(|p| MemberView {
                    browser: p.browser,
                    entity_ref: p.instance.entity_ref.clone(),
                    name: process_name(&p.instance),
                    readable_name: readable_name(&p.instance, readable),
                    pid: p.instance.pid,
                    cpu: m(&p.instance.entity_ref, Metric::ProcessCpuBasisPoints),
                    working_set: m(&p.instance.entity_ref, Metric::ProcessWorkingSetBytes),
                    services: services(&p.services),
                    unnamed: read_name(&p.instance).is_none(),
                })
                .collect();
            let mut all_services: Vec<ServiceView> =
                members.iter().flat_map(|p| p.services.clone()).collect();
            all_services.sort_by(|x, y| x.name.cmp(&y.name));
            all_services.dedup_by(|x, y| x.name == y.name);
            let named = app.name.as_deref().map(str::trim).filter(|n| !n.is_empty());
            ApplicationView {
                browser_types: app
                    .browser_types
                    .iter()
                    .map(|t| BrowserTypeView {
                        process_type: t.process_type,
                        process_count: t.process_count,
                        classified_count: t.classified_count,
                        working_set: m(&t.entity_ref, Metric::ProcessWorkingSetBytes),
                        cpu: m(&t.entity_ref, Metric::ProcessCpuBasisPoints),
                    })
                    .collect(),
                entity_ref: app.entity_ref.clone(),
                local_key: application_key(app, &instance),
                unnamed: named.is_none(),
                name: app
                    .name
                    .as_deref()
                    .map(str::trim)
                    .filter(|n| !n.is_empty())
                    .unwrap_or("Unnamed application")
                    .to_owned(),
                readable_name: naming_member(app, &instance)
                    .and_then(|p| readable_name(&p, readable)),
                process_count: app.process_count,
                members_truncated: app.members_truncated || members.len() < app.process_count,
                cpu: m(&app.entity_ref, Metric::ProcessCpuBasisPoints),
                working_set: m(&app.entity_ref, Metric::ProcessWorkingSetBytes),
                private_commit: m(&app.entity_ref, Metric::ProcessPrivateCommitBytes),
                working_set_increase: m(&app.entity_ref, Metric::ProcessWorkingSetIncreaseBytes),
                io_read: m(&app.entity_ref, Metric::ProcessIoReadBytesPerSecond),
                io_write: m(&app.entity_ref, Metric::ProcessIoWriteBytesPerSecond),
                gpu: m(&app.entity_ref, Metric::ProcessGpuBasisPoints),
                services: all_services,
                members,
            }
        })
        .collect();
    ActivityView {
        selection_source_id: None,
        collected_at_unix_ms: a.window.finished_at_unix_ms,
        window_ms: a.window.duration_ms,
        status: a.status,
        applications,
        applications_total: a.coverage.applications_total,
        coverage: a.coverage.processes.clone(),
        services_status: a.coverage.services_status,
        gpu_status: a.coverage.gpu_status,
        limitations: a.limitations.clone(),
    }
}
