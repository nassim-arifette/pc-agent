//! Locally inferred descriptions. These hints neither establish ownership nor grant actions.
use crate::{CandidateInput, CleanupClass, PersonalKind, Policy, Preferences, RecoveryStatus};
use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct ApplicationHint {
    pub name: &'static str,
    pub basis: &'static str,
    pub installed_verified: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct RecoveryOptions {
    pub adapter_method: Option<RecoveryStatus>,
    pub regeneration: Option<&'static str>,
    pub recycle_bin: &'static str,
    pub backup: &'static str,
    pub restoration_guaranteed: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct ItemContext {
    pub role: CleanupClass,
    pub folder_role: CleanupClass,
    pub personal_kind: Option<PersonalKind>,
    pub basis: &'static str,
    pub application: Option<ApplicationHint>,
    pub recovery: RecoveryOptions,
    pub conditions: Vec<&'static str>,
}

impl Policy {
    /// Pure rule evaluation on local names; returned text is from the trusted rule registry.
    pub fn item_context(&self, input: &CandidateInput, prefs: &Preferences) -> ItemContext {
        let classification = self.classify(input, prefs);
        let adapter = classification
            .adapter_id
            .and_then(|id| self.adapters.get(id));
        let folder_role = if input.is_dir {
            classification.class
        } else if let Some((parent, _)) = input.path.rsplit_once(['/', '\\']) {
            self.classify(
                &CandidateInput {
                    path: parent.into(),
                    is_dir: true,
                    marker_verified: false,
                    ..input.clone()
                },
                prefs,
            )
            .class
        } else {
            CleanupClass::Unclassified
        };
        ItemContext {
            role: classification.class,
            folder_role,
            personal_kind: classification.personal_kind,
            basis: if adapter.is_some() {
                "registered_path_pattern"
            } else if classification.class == CleanupClass::Unclassified {
                "unknown"
            } else {
                "local_path_rule_not_content_validation"
            },
            application: adapter.map(|a| ApplicationHint {
                name: a.owner,
                basis: "registered_path_pattern",
                installed_verified: false,
            }),
            recovery: RecoveryOptions {
                adapter_method: adapter.map(|a| a.recovery),
                regeneration: adapter.map(|a| a.regeneration),
                recycle_bin: "not_verified_for_this_item_or_volume",
                backup: "not_checked",
                restoration_guaranteed: false,
            },
            conditions: classification.conditions,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Attributes;
    use pcagent_shared::Origin;
    fn input(path: &str) -> CandidateInput {
        CandidateInput {
            item_ref: "file-1".into(),
            path: path.into(),
            is_dir: false,
            logical_bytes: 20,
            allocated_bytes: Some(20),
            modified_unix_ms: None,
            attributes: Attributes::default(),
            origin: Origin::Live,
            marker_verified: false,
        }
    }
    #[test]
    fn context_keeps_hints_conditional_and_unknown_recovery_explicit() {
        let policy = Policy::default();
        let prefs = Preferences::default();
        let cache = policy.item_context(
            &input(r"C:\Users\alice\AppData\Local\Google\Chrome\User Data\Default\Cache\blob"),
            &prefs,
        );
        assert!(cache.application.is_some());
        assert!(!cache.application.unwrap().installed_verified);
        assert!(cache.recovery.regeneration.is_some());
        assert!(!cache.recovery.restoration_guaranteed);
        let document = policy.item_context(&input(r"C:\Users\alice\Documents\taxes.pdf"), &prefs);
        assert_eq!(document.role, CleanupClass::PersonalData);
        assert_eq!(document.personal_kind, Some(PersonalKind::Document));
        assert!(document.application.is_none());
        assert!(document.recovery.adapter_method.is_none());
        assert_eq!(document.recovery.backup, "not_checked");
        let build = policy.item_context(&input(r"C:\work\project\target\a.bin"), &prefs);
        assert_eq!(build.folder_role, CleanupClass::DevBuildCache);
        assert!(!build.conditions.is_empty());
    }
}
