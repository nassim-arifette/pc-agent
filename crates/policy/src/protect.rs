//! Protected Windows locations (spec 10.1) and the path-level verdict.
use crate::classify::CleanupClass;
use crate::path::{NormalizedPath, PathRefusal};
use crate::prefs::Preferences;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProtectedKind {
    DriveRoot,
    WindowsInstallerCache,
    ComponentStore,
    System32,
    WindowsDirectory,
    PreviousWindowsInstallation,
    WindowsUpgradeFiles,
    PagingFile,
    HibernationFile,
    SystemVolumeInformation,
    RecycleBin,
    RecoveryData,
    BootFiles,
    ProgramFiles,
    ProgramData,
    UsersRoot,
    UserProfileRoot,
    AppDataRoot,
    TemporaryFolderRoot,
}
impl ProtectedKind {
    /// Operating-system or installed-software territory, as opposed to user-profile structure.
    pub fn is_system(self) -> bool {
        !matches!(
            self,
            Self::DriveRoot
                | Self::UsersRoot
                | Self::UserProfileRoot
                | Self::AppDataRoot
                | Self::TemporaryFolderRoot
        )
    }
    pub fn official_tool(self) -> Option<OfficialTool> {
        Some(match self {
            Self::WindowsDirectory
            | Self::WindowsInstallerCache
            | Self::ComponentStore
            | Self::System32 => OfficialTool::DiskCleanup,
            Self::PreviousWindowsInstallation | Self::WindowsUpgradeFiles | Self::RecycleBin => {
                OfficialTool::StorageSense
            }
            Self::PagingFile => OfficialTool::VirtualMemorySettings,
            Self::HibernationFile => OfficialTool::PowerOptions,
            Self::SystemVolumeInformation => OfficialTool::SystemProtection,
            Self::ProgramFiles => OfficialTool::InstalledApps,
            _ => return None,
        })
    }
}

/// Official Windows mechanism to suggest instead of a direct deletion.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OfficialTool {
    StorageSense,
    DiskCleanup,
    InstalledApps,
    VirtualMemorySettings,
    PowerOptions,
    SystemProtection,
}
impl OfficialTool {
    pub fn label(self) -> &'static str {
        match self {
            Self::StorageSense => "Settings > System > Storage > Storage Sense / Temporary files",
            Self::DiskCleanup => "Disk Cleanup (cleanmgr), including Windows Update Cleanup",
            Self::InstalledApps => "Settings > Apps > Installed apps (uninstall)",
            Self::VirtualMemorySettings => {
                "System properties > Advanced > Performance > Virtual memory"
            }
            Self::PowerOptions => "Power options / hibernation settings",
            Self::SystemProtection => "System properties > System Protection (restore points)",
        }
    }
    /// Allow-listed `ms-settings:` URI when one exists.
    pub fn settings_uri(self) -> Option<&'static str> {
        match self {
            Self::StorageSense => Some("ms-settings:storagesense"),
            Self::InstalledApps => Some("ms-settings:appsfeatures"),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Relation {
    Target,
    Descendant,
    /// The target contains the protected location; deleting it would delete the location too.
    Ancestor,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "code", rename_all = "snake_case")]
pub enum ForbidReason {
    InvalidPath {
        refusal: PathRefusal,
    },
    NetworkPath,
    Protected {
        kind: ProtectedKind,
        relation: Relation,
    },
    UserExcluded {
        relation: Relation,
    },
    ExcludedCategory {
        class: CleanupClass,
    },
    ReparsePoint,
    CloudPlaceholder,
    SystemAttribute,
}
impl ForbidReason {
    pub fn official_tool(self) -> Option<OfficialTool> {
        match self {
            Self::Protected { kind, .. } => kind.official_tool(),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecommendReason {
    ProgramDataContent,
    TemporaryFolder,
    FolderNameHeuristic,
    AdapterNotValidated,
    PersonalData,
    DiagnosticData,
    Unclassified,
    ContainsNonActionableItems,
    IncompatibleWithConstraints,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "verdict", content = "reason", rename_all = "snake_case")]
pub enum Verdict {
    Allowed,
    RecommendOnly(RecommendReason),
    Forbidden(ForbidReason),
}
impl Verdict {
    pub fn is_allowed(self) -> bool {
        self == Self::Allowed
    }
    pub fn is_forbidden(self) -> bool {
        matches!(self, Self::Forbidden(_))
    }
}

/// Known folder locations. Built-in patterns cover the default layout on every drive; these
/// values add relocated folders. Build them with an injectable lookup for tests.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct KnownFolders {
    pub windows: Vec<NormalizedPath>,
    pub program_files: Vec<NormalizedPath>,
    pub program_data: Vec<NormalizedPath>,
    pub user_profiles: Vec<NormalizedPath>,
    pub roaming_app_data: Vec<NormalizedPath>,
    pub local_app_data: Vec<NormalizedPath>,
    pub temp: Vec<NormalizedPath>,
    /// Environment values that were set but refused (relative, network, ambiguous).
    pub ignored_values: usize,
}
impl KnownFolders {
    pub fn from_env() -> Self {
        Self::from_lookup(|k| std::env::var(k).ok())
    }
    pub fn from_lookup(lookup: impl Fn(&str) -> Option<String>) -> Self {
        let mut known = Self::default();
        let mut ignored = 0usize;
        let mut get = |names: &[&str]| -> Vec<NormalizedPath> {
            let mut out: Vec<NormalizedPath> = Vec::new();
            for value in names.iter().filter_map(|n| lookup(n)) {
                match NormalizedPath::parse(value.trim()) {
                    Ok(p) if !p.is_network() => {
                        if !out.contains(&p) {
                            out.push(p);
                        }
                    }
                    _ => ignored = ignored.saturating_add(1),
                }
            }
            out
        };
        known.windows = get(&["WINDIR", "SystemRoot"]);
        known.program_files = get(&[
            "ProgramFiles",
            "ProgramFiles(x86)",
            "ProgramW6432",
            "ProgramFiles(Arm)",
        ]);
        known.program_data = get(&["ProgramData", "ALLUSERSPROFILE"]);
        known.user_profiles = get(&["USERPROFILE", "PUBLIC"]);
        known.roaming_app_data = get(&["APPDATA"]);
        known.local_app_data = get(&["LOCALAPPDATA"]);
        known.temp = get(&["TEMP", "TMP"]);
        known.ignored_values = ignored;
        known
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Scope {
    Subtree,
    RootOnly,
    RootThenRecommend,
    RecommendSelf,
}
#[derive(Debug, Clone, Copy)]
enum Seg {
    Lit(&'static str),
    Prefix(&'static str),
    Any,
}
impl Seg {
    fn matches(self, key: &str) -> bool {
        match self {
            Seg::Lit(l) => key == l,
            Seg::Prefix(p) => key.starts_with(p),
            Seg::Any => true,
        }
    }
}

use ProtectedKind as K;
use Scope::*;
use Seg::{Any, Lit, Prefix};

const PATTERNS: &[(&[Seg], Scope, ProtectedKind)] = &[
    (&[], RootOnly, K::DriveRoot),
    (
        &[Lit("windows"), Lit("installer")],
        Subtree,
        K::WindowsInstallerCache,
    ),
    (&[Lit("windows"), Lit("winsxs")], Subtree, K::ComponentStore),
    (&[Lit("windows"), Lit("system32")], Subtree, K::System32),
    (&[Lit("windows")], Subtree, K::WindowsDirectory),
    (
        &[Lit("windows.old")],
        Subtree,
        K::PreviousWindowsInstallation,
    ),
    (&[Lit("$windows.~bt")], Subtree, K::WindowsUpgradeFiles),
    (&[Lit("$windows.~ws")], Subtree, K::WindowsUpgradeFiles),
    (&[Lit("pagefile.sys")], Subtree, K::PagingFile),
    (&[Lit("swapfile.sys")], Subtree, K::PagingFile),
    (&[Lit("hiberfil.sys")], Subtree, K::HibernationFile),
    (
        &[Lit("system volume information")],
        Subtree,
        K::SystemVolumeInformation,
    ),
    (&[Lit("$recycle.bin")], Subtree, K::RecycleBin),
    (&[Lit("recycler")], Subtree, K::RecycleBin),
    (&[Lit("recovery")], Subtree, K::RecoveryData),
    (&[Lit("$winreagent")], Subtree, K::RecoveryData),
    (&[Lit("boot")], Subtree, K::BootFiles),
    (&[Lit("bootmgr")], Subtree, K::BootFiles),
    (&[Lit("efi")], Subtree, K::BootFiles),
    (&[Lit("documents and settings")], Subtree, K::UsersRoot),
    (&[Prefix("program files")], Subtree, K::ProgramFiles),
    (&[Lit("programdata")], RootThenRecommend, K::ProgramData),
    (&[Lit("users")], RootOnly, K::UsersRoot),
    (&[Lit("users"), Any], RootOnly, K::UserProfileRoot),
    (
        &[Lit("users"), Any, Lit("appdata")],
        RootOnly,
        K::AppDataRoot,
    ),
    (
        &[Lit("users"), Any, Lit("appdata"), Lit("local")],
        RootOnly,
        K::AppDataRoot,
    ),
    (
        &[Lit("users"), Any, Lit("appdata"), Lit("locallow")],
        RootOnly,
        K::AppDataRoot,
    ),
    (
        &[Lit("users"), Any, Lit("appdata"), Lit("roaming")],
        RootOnly,
        K::AppDataRoot,
    ),
    (
        &[Lit("users"), Any, Lit("appdata"), Lit("local"), Lit("temp")],
        RecommendSelf,
        K::TemporaryFolderRoot,
    ),
];

fn relation(
    target: &[String],
    len: usize,
    seg_match: impl Fn(usize, &str) -> bool,
) -> Option<Relation> {
    let n = target.len().min(len);
    if !(0..n).all(|i| seg_match(i, &target[i])) {
        return None;
    }
    Some(match target.len().cmp(&len) {
        std::cmp::Ordering::Less => Relation::Ancestor,
        std::cmp::Ordering::Equal => Relation::Target,
        std::cmp::Ordering::Greater => Relation::Descendant,
    })
}

pub(crate) fn path_relation(target: &NormalizedPath, base: &NormalizedPath) -> Option<Relation> {
    if !target.same_root(base) {
        return None;
    }
    relation(target.keys(), base.keys().len(), |i, k| base.keys()[i] == k)
}

impl KnownFolders {
    fn exact_rules(&self) -> Vec<(&NormalizedPath, Scope, ProtectedKind)> {
        let mut rules = Vec::new();
        for w in &self.windows {
            rules.push((w, Subtree, K::WindowsDirectory));
        }
        rules.extend(
            self.program_files
                .iter()
                .map(|p| (p, Subtree, K::ProgramFiles)),
        );
        rules.extend(
            self.program_data
                .iter()
                .map(|p| (p, RootThenRecommend, K::ProgramData)),
        );
        rules.extend(
            self.user_profiles
                .iter()
                .map(|p| (p, RootOnly, K::UserProfileRoot)),
        );
        rules.extend(
            self.roaming_app_data
                .iter()
                .map(|p| (p, RootOnly, K::AppDataRoot)),
        );
        rules.extend(
            self.local_app_data
                .iter()
                .map(|p| (p, RootOnly, K::AppDataRoot)),
        );
        rules.extend(
            self.temp
                .iter()
                .map(|p| (p, RecommendSelf, K::TemporaryFolderRoot)),
        );
        rules
    }

    /// Protection verdict for a local path, ignoring user preferences.
    pub fn protection(&self, path: &NormalizedPath) -> Option<Verdict> {
        let mut ancestor = None;
        let mut recommend = None;
        let mut visit = |scope: Scope, kind: ProtectedKind, rel: Relation| -> Option<Verdict> {
            let forbid = |relation| {
                Some(Verdict::Forbidden(ForbidReason::Protected {
                    kind,
                    relation,
                }))
            };
            match (scope, rel) {
                (RecommendSelf, Relation::Target) => {
                    recommend.get_or_insert(RecommendReason::TemporaryFolder);
                }
                (RecommendSelf, _) => {}
                (_, Relation::Ancestor) => {
                    ancestor = ancestor.or(forbid(Relation::Ancestor));
                }
                (Subtree, r) | (RootOnly | RootThenRecommend, r @ Relation::Target) => {
                    return forbid(r)
                }
                (RootThenRecommend, Relation::Descendant) => {
                    recommend.get_or_insert(RecommendReason::ProgramDataContent);
                }
                (RootOnly, Relation::Descendant) => {}
            }
            None
        };
        if path.drive().is_some() {
            for (segs, scope, kind) in PATTERNS {
                if let Some(rel) = relation(path.keys(), segs.len(), |i, k| segs[i].matches(k)) {
                    if let Some(v) = visit(*scope, *kind, rel) {
                        return Some(v);
                    }
                }
            }
        }
        for (base, scope, kind) in self.exact_rules() {
            if let Some(rel) = path_relation(path, base) {
                if let Some(v) = visit(scope, kind, rel) {
                    return Some(v);
                }
            }
        }
        ancestor.or(recommend.map(Verdict::RecommendOnly))
    }

    /// Path-level verdict: protection, network refusal and user path exclusions.
    /// Content-based rules (attributes, classification, adapters) are applied by `Policy::classify`.
    pub fn check_target(&self, path: &NormalizedPath, prefs: &Preferences) -> Verdict {
        if path.is_network() {
            return Verdict::Forbidden(ForbidReason::NetworkPath);
        }
        let protection = self.protection(path);
        if let Some(v @ Verdict::Forbidden(_)) = protection {
            return v;
        }
        let mut excluded = None;
        for ex in &prefs.excluded_paths {
            match path_relation(path, ex) {
                Some(Relation::Ancestor) => excluded = excluded.or(Some(Relation::Ancestor)),
                Some(r) => return Verdict::Forbidden(ForbidReason::UserExcluded { relation: r }),
                None => {}
            }
        }
        if let Some(relation) = excluded {
            return Verdict::Forbidden(ForbidReason::UserExcluded { relation });
        }
        protection.unwrap_or(Verdict::Allowed)
    }
}

/// Path-level verdict using only built-in default-layout patterns (no environment lookup).
pub fn check_target(path: &NormalizedPath, prefs: &Preferences) -> Verdict {
    KnownFolders::default().check_target(path, prefs)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn v(s: &str) -> Verdict {
        check_target(&NormalizedPath::parse(s).unwrap(), &Preferences::default())
    }
    fn kind(s: &str) -> (ProtectedKind, Relation) {
        match v(s) {
            Verdict::Forbidden(ForbidReason::Protected { kind, relation }) => (kind, relation),
            other => panic!("{s}: {other:?}"),
        }
    }
    use Relation::*;

    #[test]
    fn tricky_spellings_still_hit_protection() {
        for (s, k) in [
            ("c:/WINDOWS/./installer", K::WindowsInstallerCache),
            (r"\\?\C:\Windows\WinSxS\", K::ComponentStore),
            (r"C:\Windows.", K::WindowsDirectory),
            (r"C:\Windows \System32\drivers", K::System32),
            (r"c:\windows\SYSTEM32\config\SAM", K::System32),
            (r"D:\windows\temp\x.tmp", K::WindowsDirectory),
            (
                r"C:\Windows\Installer\{guid}\x.msi",
                K::WindowsInstallerCache,
            ),
        ] {
            assert_eq!(kind(s).0, k, "{s}");
        }
    }
    #[test]
    fn root_level_system_items_on_any_drive() {
        for (s, k) in [
            (r"C:\pagefile.sys", K::PagingFile),
            (r"c:\HIBERFIL.SYS", K::HibernationFile),
            (r"C:\swapfile.sys", K::PagingFile),
            (
                r"E:\System Volume Information\x",
                K::SystemVolumeInformation,
            ),
            (r"C:\$Recycle.Bin\S-1-5-21\$R1.txt", K::RecycleBin),
            (r"C:\Recovery\WindowsRE", K::RecoveryData),
            (r"C:\Windows.old\Users", K::PreviousWindowsInstallation),
            (r"C:\$WINDOWS.~BT\Sources", K::WindowsUpgradeFiles),
            (r"C:\Program Files\App\app.exe", K::ProgramFiles),
            (r"C:\Program Files (x86)\App", K::ProgramFiles),
            (r"D:\Program Files", K::ProgramFiles),
        ] {
            assert_eq!(kind(s).0, k, "{s}");
        }
        assert_eq!(
            kind(r"C:\Program Files\App").0.official_tool(),
            Some(OfficialTool::InstalledApps)
        );
    }
    #[test]
    fn roots_are_protected_but_not_their_whole_subtree() {
        assert_eq!(kind(r"C:\"), (K::DriveRoot, Target));
        assert_eq!(kind(r"C:\Users"), (K::UsersRoot, Target));
        assert_eq!(kind(r"C:\Users\Bob"), (K::UserProfileRoot, Target));
        assert_eq!(kind(r"C:\Users\Bob\AppData"), (K::AppDataRoot, Target));
        assert_eq!(
            kind(r"C:\Users\Bob\AppData\Local"),
            (K::AppDataRoot, Target)
        );
        assert_eq!(
            kind(r"C:\Users\Bob\AppData\LocalLow"),
            (K::AppDataRoot, Target)
        );
        assert_eq!(
            kind(r"C:\Users\Bob\AppData\Roaming"),
            (K::AppDataRoot, Target)
        );
        assert_eq!(kind(r"C:\ProgramData"), (K::ProgramData, Target));
        assert_eq!(
            v(r"C:\ProgramData\Vendor\cache"),
            Verdict::RecommendOnly(RecommendReason::ProgramDataContent)
        );
        assert_eq!(
            v(r"C:\Users\Bob\AppData\Local\Temp"),
            Verdict::RecommendOnly(RecommendReason::TemporaryFolder)
        );
        assert_eq!(
            v(r"C:\Users\Bob\AppData\Local\Temp\x.tmp"),
            Verdict::Allowed
        );
        assert_eq!(v(r"C:\Users\Bob\Downloads\setup.exe"), Verdict::Allowed);
        assert_eq!(v(r"D:\Data\file.bin"), Verdict::Allowed);
    }
    #[test]
    fn deleting_a_parent_of_a_protected_path_is_forbidden() {
        let prefs = Preferences::default();
        let known = KnownFolders::from_lookup(|k| (k == "WINDIR").then(|| r"D:\OS\Win".into()));
        let check = |s: &str| known.check_target(&NormalizedPath::parse(s).unwrap(), &prefs);
        assert_eq!(
            check(r"D:\OS"),
            Verdict::Forbidden(ForbidReason::Protected {
                kind: K::WindowsDirectory,
                relation: Ancestor
            })
        );
        assert!(check(r"D:\OS\Win\System32").is_forbidden());
        assert!(check(r"D:\OS\Other").is_allowed());
        assert!(v(r"D:\OS").is_allowed());
    }
    #[test]
    fn relocated_known_folders_from_injected_env() {
        let env = |k: &str| {
            Some(match k {
                "USERPROFILE" => r"E:\Profiles\bob".to_string(),
                "LOCALAPPDATA" => r"E:\Profiles\bob\Local".into(),
                "ProgramData" => r"\\server\share\pd".into(),
                "TEMP" => "relative\\temp".into(),
                "ProgramFiles" => r"F:\Apps".into(),
                _ => return None,
            })
        };
        let known = KnownFolders::from_lookup(env);
        assert_eq!(known.ignored_values, 2);
        let prefs = Preferences::default();
        let check = |s: &str| known.check_target(&NormalizedPath::parse(s).unwrap(), &prefs);
        assert!(check(r"E:\Profiles\bob").is_forbidden());
        assert!(check(r"E:\Profiles").is_forbidden());
        assert!(check(r"E:\Profiles\bob\Local").is_forbidden());
        assert!(check(r"E:\Profiles\bob\Local\Cache\x").is_allowed());
        assert!(check(r"F:\Apps\Tool\tool.exe").is_forbidden());
    }
    #[test]
    fn network_paths_and_user_exclusions() {
        assert_eq!(
            v(r"\\nas\share\old.iso"),
            Verdict::Forbidden(ForbidReason::NetworkPath)
        );
        let prefs = Preferences {
            excluded_paths: vec![NormalizedPath::parse(r"D:\Keep\Projects").unwrap()],
            ..Preferences::default()
        };
        let check = |s: &str| check_target(&NormalizedPath::parse(s).unwrap(), &prefs);
        assert_eq!(
            check(r"d:\keep\projects"),
            Verdict::Forbidden(ForbidReason::UserExcluded { relation: Target })
        );
        assert_eq!(
            check(r"D:\Keep\Projects\a\b"),
            Verdict::Forbidden(ForbidReason::UserExcluded {
                relation: Descendant
            })
        );
        assert_eq!(
            check(r"D:\Keep"),
            Verdict::Forbidden(ForbidReason::UserExcluded { relation: Ancestor })
        );
        assert!(check(r"D:\Keep\Other").is_allowed());
    }
    #[test]
    fn verdict_serialization_is_tagged() {
        let json = serde_json::to_value(v(r"C:\Windows")).unwrap();
        assert_eq!(json["verdict"], "forbidden");
        assert_eq!(json["reason"]["code"], "protected");
        assert_eq!(json["reason"]["kind"], "windows_directory");
    }
}
