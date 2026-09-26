//! Declarative cleanup adapters (spec 11.1). Only validated adapters are executable.
use crate::path::NormalizedPath;
use crate::protect::KnownFolders;
use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Base {
    /// `%LOCALAPPDATA%` (default layout `<drive>:\Users\<user>\AppData\Local`).
    LocalAppData,
    /// `%USERPROFILE%` (default layout `<drive>:\Users\<user>`).
    UserProfile,
}

/// Lower-case components relative to `base`; `*` matches exactly one component.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct TargetPattern {
    pub base: Base,
    pub relative: &'static [&'static str],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionMode {
    RecommendOnly,
    DeleteContents,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RecoveryStatus {
    RecycleBin,
    QuarantineSameVolume,
    /// Deleted permanently; the owner can regenerate it, but not restore it identically.
    RegenerableNotRestorable,
    PermanentNoUndo,
    NotApplicable,
}
impl RecoveryStatus {
    /// Space stays allocated on the same volume until the bin or quarantine is emptied.
    pub fn frees_space_immediately(self) -> bool {
        matches!(self, Self::RegenerableNotRestorable | Self::PermanentNoUndo)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Adapter {
    pub id: &'static str,
    pub owner: &'static str,
    pub synthetic: bool,
    pub targets: &'static [TargetPattern],
    pub recognized_conditions: &'static [&'static str],
    pub preconditions: &'static [&'static str],
    pub exclusions: &'static [&'static str],
    pub execution_mode: ExecutionMode,
    pub recovery: RecoveryStatus,
    pub supports_recycle: bool,
    pub regeneration: &'static str,
    pub side_effects: &'static [&'static str],
    pub tests: &'static [&'static str],
    pub validated: bool,
}
impl Adapter {
    pub fn is_executable(&self) -> bool {
        self.validated && self.execution_mode != ExecutionMode::RecommendOnly
    }
    fn target_len(&self, path: &NormalizedPath, known: &KnownFolders) -> Option<usize> {
        let keys = path.keys();
        self.targets
            .iter()
            .filter_map(|t| {
                let rel = |offset: usize| {
                    let end = offset.checked_add(t.relative.len())?;
                    (keys.len() >= end
                        && t.relative
                            .iter()
                            .zip(&keys[offset..end])
                            .all(|(p, k)| *p == "*" || p == k))
                    .then_some(end)
                };
                let pattern: &[&str] = match t.base {
                    Base::LocalAppData => &["users", "*", "appdata", "local"],
                    Base::UserProfile => &["users", "*"],
                };
                let default_layout = path.drive().is_some()
                    && keys.len() >= pattern.len()
                    && pattern.iter().zip(keys).all(|(p, k)| *p == "*" || p == k);
                let roots = match t.base {
                    Base::LocalAppData => &known.local_app_data,
                    Base::UserProfile => &known.user_profiles,
                };
                let from_default = default_layout.then(|| rel(pattern.len())).flatten();
                let from_env = roots
                    .iter()
                    .filter(|r| path.starts_with(r))
                    .find_map(|r| rel(r.keys().len()));
                from_default.or(from_env)
            })
            .max()
    }
}

const SYNTHETIC_CACHE_DIR: &[&str] = &["pcagent", "synthetic-demo-cache"];
pub const SYNTHETIC_CACHE_ID: &str = "pcagent.synthetic_demo_cache";
pub const SYNTHETIC_CACHE_MARKER: &str = "PCAGENT_SYNTHETIC_CACHE";

const BROWSER_CONDITIONS: &[&str] = &[
    "Browser closed",
    "Prefer the browser's own 'Clear browsing data' setting",
];

pub const BUILTIN_ADAPTERS: &[Adapter] = &[
    Adapter {
        id: SYNTHETIC_CACHE_ID,
        owner: "pcagent (synthetic demo data)",
        synthetic: true,
        targets: &[TargetPattern { base: Base::LocalAppData, relative: SYNTHETIC_CACHE_DIR }],
        recognized_conditions: &[
            "Directory created by the pcagent synthetic demo generator",
            "Contains the PCAGENT_SYNTHETIC_CACHE marker file",
        ],
        preconditions: &[
            "Marker file present and unchanged",
            "No reparse points below the directory",
            "Target identity revalidated immediately before execution",
        ],
        exclusions: &["The marker file itself", "Anything outside the synthetic-demo-cache directory"],
        execution_mode: ExecutionMode::DeleteContents,
        recovery: RecoveryStatus::RegenerableNotRestorable,
        supports_recycle: true,
        regeneration: "Recreated on demand by the pcagent demo generator; contents are synthetic filler without user value.",
        side_effects: &["Removes synthetic filler data only; this is demonstration data, not a real application cache."],
        tests: &[
            "adapters::tests::synthetic_cache_is_the_only_executable_adapter",
            "plan::tests::synthetic_cache_reaches_goal",
        ],
        validated: true,
    },
    Adapter {
        id: "chrome.cache",
        owner: "Google Chrome",
        synthetic: false,
        targets: &[TargetPattern { base: Base::LocalAppData, relative: &["google", "chrome", "user data", "*", "cache"] }],
        recognized_conditions: &["Chromium profile cache directory layout"],
        preconditions: BROWSER_CONDITIONS,
        exclusions: &["Profile data outside the Cache directory (history, passwords, extensions)"],
        execution_mode: ExecutionMode::RecommendOnly,
        recovery: RecoveryStatus::RegenerableNotRestorable,
        supports_recycle: false,
        regeneration: "Refilled by the browser while browsing; first page loads may be slower.",
        side_effects: &["Slower first loads", "Some sites may need to re-download assets"],
        tests: &["adapters::tests::known_caches_are_recommendation_only"],
        validated: false,
    },
    Adapter {
        id: "edge.cache",
        owner: "Microsoft Edge",
        synthetic: false,
        targets: &[TargetPattern { base: Base::LocalAppData, relative: &["microsoft", "edge", "user data", "*", "cache"] }],
        recognized_conditions: &["Chromium profile cache directory layout"],
        preconditions: BROWSER_CONDITIONS,
        exclusions: &["Profile data outside the Cache directory (history, passwords, extensions)"],
        execution_mode: ExecutionMode::RecommendOnly,
        recovery: RecoveryStatus::RegenerableNotRestorable,
        supports_recycle: false,
        regeneration: "Refilled by the browser while browsing; first page loads may be slower.",
        side_effects: &["Slower first loads"],
        tests: &["adapters::tests::known_caches_are_recommendation_only"],
        validated: false,
    },
    Adapter {
        id: "npm.cache",
        owner: "npm",
        synthetic: false,
        targets: &[TargetPattern { base: Base::LocalAppData, relative: &["npm-cache"] }],
        recognized_conditions: &["Default npm cache location on Windows"],
        preconditions: &["No npm install running", "Prefer `npm cache clean --force` or `npm cache verify`"],
        exclusions: &["Custom cache locations configured in .npmrc"],
        execution_mode: ExecutionMode::RecommendOnly,
        recovery: RecoveryStatus::RegenerableNotRestorable,
        supports_recycle: false,
        regeneration: "Packages are downloaded again on the next install; requires network access.",
        side_effects: &["Offline installs no longer possible until re-downloaded"],
        tests: &["adapters::tests::known_caches_are_recommendation_only"],
        validated: false,
    },
    Adapter {
        id: "cargo.registry_cache",
        owner: "Cargo (Rust)",
        synthetic: false,
        targets: &[TargetPattern { base: Base::UserProfile, relative: &[".cargo", "registry", "cache"] }],
        recognized_conditions: &["Default CARGO_HOME layout"],
        preconditions: &["No cargo build running"],
        exclusions: &["Custom CARGO_HOME", "Crates vendored inside projects"],
        execution_mode: ExecutionMode::RecommendOnly,
        recovery: RecoveryStatus::RegenerableNotRestorable,
        supports_recycle: false,
        regeneration: "Crates are downloaded again on the next build; requires network access.",
        side_effects: &["Offline builds no longer possible until re-downloaded"],
        tests: &["adapters::tests::known_caches_are_recommendation_only"],
        validated: false,
    },
    Adapter {
        id: "pip.cache",
        owner: "pip (Python)",
        synthetic: false,
        targets: &[TargetPattern { base: Base::LocalAppData, relative: &["pip", "cache"] }],
        recognized_conditions: &["Default pip cache location on Windows"],
        preconditions: &["No pip install running", "Prefer `pip cache purge`"],
        exclusions: &["Custom PIP_CACHE_DIR"],
        execution_mode: ExecutionMode::RecommendOnly,
        recovery: RecoveryStatus::RegenerableNotRestorable,
        supports_recycle: false,
        regeneration: "Wheels are downloaded or rebuilt on the next install.",
        side_effects: &["Slower next install"],
        tests: &["adapters::tests::known_caches_are_recommendation_only"],
        validated: false,
    },
];

#[derive(Debug, Clone)]
pub struct AdapterRegistry {
    adapters: Vec<Adapter>,
}
impl Default for AdapterRegistry {
    fn default() -> Self {
        Self::builtin()
    }
}
impl AdapterRegistry {
    pub fn builtin() -> Self {
        Self {
            adapters: BUILTIN_ADAPTERS.to_vec(),
        }
    }
    /// Later adapters with an existing id are ignored.
    pub fn from_adapters(adapters: Vec<Adapter>) -> Self {
        let mut unique: Vec<Adapter> = Vec::new();
        for a in adapters {
            if !unique.iter().any(|u| u.id == a.id) {
                unique.push(a);
            }
        }
        Self { adapters: unique }
    }
    pub fn adapters(&self) -> &[Adapter] {
        &self.adapters
    }
    pub fn get(&self, id: &str) -> Option<&Adapter> {
        self.adapters.iter().find(|a| a.id == id)
    }
    /// Most specific adapter whose target contains `path`.
    pub fn find(&self, path: &NormalizedPath, known: &KnownFolders) -> Option<&Adapter> {
        self.adapters
            .iter()
            .filter_map(|a| a.target_len(path, known).map(|len| (len, a)))
            .max_by_key(|(len, _)| *len)
            .map(|(_, a)| a)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn p(s: &str) -> NormalizedPath {
        NormalizedPath::parse(s).unwrap()
    }
    fn find(s: &str) -> Option<&'static str> {
        AdapterRegistry::builtin()
            .find(&p(s), &KnownFolders::default())
            .map(|a| a.id)
    }

    #[test]
    fn synthetic_cache_is_the_only_executable_adapter() {
        let executable: Vec<_> = BUILTIN_ADAPTERS
            .iter()
            .filter(|a| a.is_executable())
            .collect();
        assert_eq!(executable.len(), 1);
        assert_eq!(executable[0].id, SYNTHETIC_CACHE_ID);
        assert!(executable[0].synthetic && executable[0].owner.contains("synthetic"));
        assert_eq!(
            find(r"C:\Users\bob\AppData\Local\pcagent\synthetic-demo-cache\blob-1.bin"),
            Some(SYNTHETIC_CACHE_ID)
        );
        assert_eq!(
            find(r"c:\users\bob\appdata\local\PCAGENT\Synthetic-Demo-Cache"),
            Some(SYNTHETIC_CACHE_ID)
        );
        assert_eq!(find(r"C:\Users\bob\AppData\Local\pcagent"), None);
        assert_eq!(
            find(r"C:\Users\bob\AppData\Local\pcagent\synthetic-demo-cache-other"),
            None
        );
        assert_eq!(
            find(r"C:\Users\bob\Documents\pcagent\synthetic-demo-cache"),
            None
        );
    }
    #[test]
    fn known_caches_are_recommendation_only() {
        for a in BUILTIN_ADAPTERS.iter().filter(|a| !a.synthetic) {
            assert!(!a.validated && !a.is_executable(), "{}", a.id);
            assert_eq!(a.execution_mode, ExecutionMode::RecommendOnly);
            assert!(!a.tests.is_empty() && !a.preconditions.is_empty());
        }
        assert_eq!(
            find(
                r"C:\Users\bob\AppData\Local\Google\Chrome\User Data\Default\Cache\Cache_Data\f_0001"
            ),
            Some("chrome.cache")
        );
        assert_eq!(
            find(r"C:\Users\bob\AppData\Local\Google\Chrome\User Data\Default\History"),
            None
        );
        assert_eq!(
            find(r"C:\Users\bob\.cargo\registry\cache\index"),
            Some("cargo.registry_cache")
        );
        assert_eq!(
            find(r"C:\Users\bob\AppData\Local\npm-cache\_cacache"),
            Some("npm.cache")
        );
    }
    #[test]
    fn relocated_local_app_data_is_recognized() {
        let known =
            KnownFolders::from_lookup(|k| (k == "LOCALAPPDATA").then(|| r"E:\Data\Local".into()));
        let reg = AdapterRegistry::builtin();
        let hit = reg.find(&p(r"E:\Data\Local\pcagent\synthetic-demo-cache\x"), &known);
        assert_eq!(hit.map(|a| a.id), Some(SYNTHETIC_CACHE_ID));
        assert!(reg
            .find(&p(r"E:\Other\pcagent\synthetic-demo-cache\x"), &known)
            .is_none());
    }
    #[test]
    fn registry_ids_are_unique() {
        let mut v = BUILTIN_ADAPTERS.to_vec();
        let mut dup = v[1].clone();
        dup.validated = true;
        dup.id = SYNTHETIC_CACHE_ID;
        v.push(dup);
        let reg = AdapterRegistry::from_adapters(v);
        assert_eq!(reg.adapters().len(), BUILTIN_ADAPTERS.len());
        assert!(reg.get(SYNTHETIC_CACHE_ID).unwrap().synthetic);
    }
}
