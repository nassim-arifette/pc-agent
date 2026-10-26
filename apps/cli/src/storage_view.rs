//! Storage preparation shared by the cloud preview and the actual investigation.
use pcagent_agent::{PrivacyContext, StorageContext};
use pcagent_policy::{Policy, Preferences};
use pcagent_storage::{ContentOptions, ItemKind, ScanOptions, StorageService};
use serde_json::{json, Value};
use std::time::{Duration, Instant};

pub struct PreparedStorage {
    pub context: StorageContext,
    pub preview: Vec<Value>,
    pub local_items: Vec<pcagent_agent::LocalStorageItem>,
}

/// User-supplied roots only. All preparation shares a 30-second cooperative deadline and
/// content comparisons share a 256 MiB read budget. Metadata reads never imply content consent.
pub fn prepare_with_privacy(
    service: &StorageService,
    policy: &Policy,
    prefs: &Preferences,
    share_names: bool,
    compare_content: bool,
    privacy: &PrivacyContext,
) -> crate::Result<PreparedStorage> {
    let started = Instant::now();
    let remaining = || Duration::from_secs(30).saturating_sub(started.elapsed());
    let mut context = StorageContext::default();
    let mut scopes = service.scopes();
    scopes.sort_by(|a, b| a.scope_id.cmp(&b.scope_id));
    let mut read_budget = ContentOptions::default().max_bytes;
    for scope in &scopes {
        let options = ScanOptions {
            max_entries: 100_000,
            max_duration: remaining().min(Duration::from_secs(15)),
            index_budget_bytes: 64 << 20,
            ..ScanOptions::default()
        };
        service.scan_blocking(&scope.scope_id, &options, &mut |_| {})?;
        let index = service.index(&scope.scope_id)?;
        if share_names {
            context.authorize_useful_names_with_privacy(&index, privacy)?;
        }
        if compare_content {
            let comparison = service.compare_content(
                &scope.scope_id,
                &ContentOptions {
                    max_bytes: read_budget,
                    max_duration: remaining().min(Duration::from_secs(15)),
                    ..ContentOptions::default()
                },
            )?;
            read_budget = read_budget.saturating_sub(comparison.bytes_read);
            context.add_comparison(comparison, &index);
        }
    }
    let mut prepared = PreparedStorage {
        context,
        preview: Vec::new(),
        local_items: Vec::new(),
    };
    for scope in &scopes {
        let overview = service
            .get_storage_overview(&scope.scope_id)?
            .without_paths()
            .redact_names();
        let mut overview = json!(overview);
        overview
            .as_object_mut()
            .expect("overview")
            .remove("redacted_root");
        privacy.sanitize_value(&mut overview);
        prepared
            .preview
            .push(json!({"tool":"get_storage_overview", "payload":overview}));
        for kind in [ItemKind::Files, ItemKind::Dirs] {
            let page = service.get_largest_items(&scope.scope_id, kind, 0, 20)?;
            let (mut cloud, local) = prepared.context.project_page(&page, policy, prefs);
            privacy.sanitize_value(&mut cloud);
            prepared
                .preview
                .push(json!({"tool":"get_largest_items", "payload":cloud}));
            prepared.local_items.extend(local);
        }
    }
    Ok(prepared)
}

impl PreparedStorage {
    pub fn record_uninspected_scopes(&self, report: &mut pcagent_agent::Investigation) {
        // Model-requested observations are newer; preserve their coverage even on failure.
        let inspected: Vec<String> = report
            .observations
            .iter()
            .filter(|o| {
                matches!(
                    o.tool.as_str(),
                    "get_storage_overview" | "get_largest_items" | "get_cleanup_candidates"
                )
            })
            .filter_map(|o| o.observation["scope_id"].as_str().map(str::to_owned))
            .collect();
        for preview in &self.preview {
            if preview["tool"] != "get_storage_overview" {
                continue;
            }
            let cloud = preview["payload"].clone();
            if cloud["scope_id"]
                .as_str()
                .is_some_and(|id| inspected.iter().any(|known| known == id))
            {
                continue;
            }
            let Some(spec) = pcagent_agent::WORKSTATION_TOOLS
                .iter()
                .find(|s| preview["tool"] == s.name)
            else {
                continue;
            };
            let status = cloud
                .get("scan_status")
                .or_else(|| cloud["scan"].get("status"))
                .and_then(|s| serde_json::from_value(s.clone()).ok())
                .unwrap_or(pcagent_shared::Status::Partial);
            report.record_local_observation(
                spec,
                pcagent_agent::ToolOutput {
                    status,
                    inspection_complete: !cloud["scan"].is_null()
                        || cloud["volume"]["free_bytes"].is_u64(),
                    cloud,
                    storage_items: Vec::new(),
                    evidence: Vec::new(),
                    findings: Vec::new(),
                    processes: Vec::new(),
                    limitations: Vec::new(),
                },
            );
        }
    }
    pub fn local_report(&self) -> Value {
        json!({"items":self.local_items, "observations":self.preview, "content_comparisons":self.context.comparisons(),
            "notice":"Bounded preparation of authorized folders; names below are local. Context hints do not authorize deletion."})
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pcagent_agent::{Toolbox, WORKSTATION_TOOLS};
    use pcagent_tools::Workstation;
    use std::{fs, sync::Arc};
    #[test]
    fn preview_and_dispatched_tool_use_the_same_name_allowlist_and_projection() {
        let root = std::env::temp_dir().join(pcagent_shared::new_id("pcagent-preview-projection"));
        fs::create_dir_all(root.join("client-private")).unwrap();
        fs::write(
            root.join("client-private/report-private.pdf"),
            b"private bytes",
        )
        .unwrap();
        let service = Arc::new(StorageService::new());
        let scope = service.authorize(&root).unwrap().scope_id;
        let policy = Policy::default();
        let prefs = Preferences::default();
        let mut privacy = PrivacyContext::local();
        privacy.protect_secret("report-private");
        let prepared =
            prepare_with_privacy(&service, &policy, &prefs, true, false, &privacy).unwrap();
        assert!(!prepared
            .context
            .sharing_preview()
            .to_string()
            .contains("report-private"));
        let expected = prepared
            .preview
            .iter()
            .find(|p| p["payload"]["kind"] == "files")
            .unwrap()["payload"]
            .clone();
        let mut collector = pcagent_telemetry::Collector::new();
        let mut tools = Workstation::new(&mut collector, service.clone(), &policy, &prefs);
        tools.set_storage_context(prepared.context);
        let spec = WORKSTATION_TOOLS
            .iter()
            .find(|s| s.name == "get_largest_items")
            .unwrap();
        let args = spec
            .validate(&json!({"scope_id":scope, "kind":"files", "limit":20}).to_string())
            .unwrap();
        let mut actual = tools.run(spec, &args);
        actual
            .cloud
            .as_object_mut()
            .unwrap()
            .remove("observation_id");
        privacy.sanitize_value(&mut actual.cloud);
        assert_eq!(actual.cloud, expected);
        assert!(!actual.cloud.to_string().contains("report-private"));
        assert!(actual.storage_items[0].path.ends_with("report-private.pdf"));
        assert!(!actual.cloud.to_string().contains("private bytes"));
        drop(tools);
        fs::remove_dir_all(root).unwrap();
    }
}
