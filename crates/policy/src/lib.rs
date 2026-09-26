//! Deterministic cleanup policy: Windows path normalization, protected locations, user
//! preferences, cleanup classification, declarative adapters and goal-oriented plans.
//! Pure logic modelling Windows paths; nothing here touches the filesystem or executes actions.
#![forbid(unsafe_code)]
pub mod adapters;
pub mod classify;
pub mod path;
pub mod plan;
pub mod prefs;
pub mod protect;

pub use adapters::{Adapter, AdapterRegistry, ExecutionMode, RecoveryStatus};
pub use classify::{Attributes, CandidateInput, Classification, CleanupClass, PersonalKind};
pub use path::{NormalizedPath, PathRefusal};
pub use plan::{
    CandidateView, CleanupCandidatesView, CleanupPlan, GoalStatus, Method, PlanConstraints,
    PlanLine, PlanView,
};
pub use prefs::{Permissions, Preferences, PreferencesView, PreservedApp};
pub use protect::{
    check_target, ForbidReason, KnownFolders, OfficialTool, ProtectedKind, RecommendReason,
    Relation, Verdict,
};

/// Policy context: known folder locations plus the adapter registry.
#[derive(Debug, Clone, Default)]
pub struct Policy {
    pub known: KnownFolders,
    pub adapters: AdapterRegistry,
}
impl Policy {
    pub fn new(known: KnownFolders, adapters: AdapterRegistry) -> Self {
        Self { known, adapters }
    }
    /// Built-in adapters and known folders from the process environment.
    pub fn from_env() -> Self {
        Self::new(KnownFolders::from_env(), AdapterRegistry::builtin())
    }
    pub fn check_target(&self, path: &NormalizedPath, prefs: &Preferences) -> Verdict {
        self.known.check_target(path, prefs)
    }
    /// `get_user_preferences` tool output.
    pub fn user_preferences_view(&self, prefs: &Preferences) -> PreferencesView {
        prefs.view()
    }
}
