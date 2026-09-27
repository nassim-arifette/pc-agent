//! Windows path normalization for policy comparisons only. Never used to open files.
use serde::{Deserialize, Serialize};
use std::fmt;
use std::hash::{Hash, Hasher};

pub const MAX_PATH_CHARS: usize = 32_767;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, thiserror::Error)]
#[serde(rename_all = "snake_case")]
pub enum PathRefusal {
    #[error("empty path")]
    Empty,
    #[error("path exceeds the Windows maximum length")]
    TooLong,
    #[error("relative or drive-relative path")]
    Relative,
    #[error("device namespace path")]
    DeviceNamespace,
    #[error("malformed UNC path")]
    MalformedUnc,
    #[error("parent or ambiguous dot segment")]
    ParentSegment,
    #[error("trailing dot or space in a verbatim path")]
    VerbatimTrailingDotOrSpace,
    #[error("alternate data stream or colon in a component")]
    AlternateDataStream,
    #[error("possible 8.3 short name")]
    ShortName,
    #[error("invalid or control character")]
    InvalidCharacter,
    #[error("reserved device name")]
    ReservedName,
}

#[derive(Debug, Clone)]
enum Root {
    Drive(char),
    Unc { server: String, share: String },
}

/// Absolute Windows path compared case-insensitively. Construction refuses spellings whose
/// target cannot be determined unambiguously from the text alone.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct NormalizedPath {
    root: Root,
    root_key: String,
    parts: Vec<String>,
    keys: Vec<String>,
}

impl PartialEq for NormalizedPath {
    fn eq(&self, other: &Self) -> bool {
        self.root_key == other.root_key && self.keys == other.keys
    }
}
impl Eq for NormalizedPath {}
impl Hash for NormalizedPath {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.root_key.hash(state);
        self.keys.hash(state);
    }
}
impl PartialOrd for NormalizedPath {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for NormalizedPath {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        (&self.root_key, &self.keys).cmp(&(&other.root_key, &other.keys))
    }
}

fn strip_prefix_ci<'a>(s: &'a str, prefix: &str) -> Option<&'a str> {
    let head = s.get(..prefix.len())?;
    head.eq_ignore_ascii_case(prefix)
        .then(|| &s[prefix.len()..])
}

fn is_reserved(name: &str) -> bool {
    let stem = name
        .split('.')
        .next()
        .unwrap_or(name)
        .trim_end()
        .to_lowercase();
    matches!(
        stem.as_str(),
        "con" | "prn" | "aux" | "nul" | "conin$" | "conout$"
    ) || ((stem.starts_with("com") || stem.starts_with("lpt"))
        && stem.len() == 4
        && stem[3..]
            .chars()
            .all(|c| matches!(c, '1'..='9' | '¹' | '²' | '³')))
}

fn is_short_name(name: &str) -> bool {
    let (base, ext) = name.split_once('.').unwrap_or((name, ""));
    let Some((head, digits)) = base.split_once('~') else {
        return false;
    };
    !head.is_empty()
        && !digits.is_empty()
        && digits.chars().all(|c| c.is_ascii_digit())
        && base.chars().count() <= 8
        && ext.chars().count() <= 3
}

fn check_name(name: &str) -> Result<(), PathRefusal> {
    if name.contains(':') {
        Err(PathRefusal::AlternateDataStream)
    } else if name
        .chars()
        .any(|c| matches!(c, '<' | '>' | '"' | '|' | '?' | '*'))
    {
        Err(PathRefusal::InvalidCharacter)
    } else {
        Ok(())
    }
}

impl NormalizedPath {
    pub fn parse(raw: &str) -> Result<Self, PathRefusal> {
        if raw.is_empty() {
            return Err(PathRefusal::Empty);
        }
        if raw.chars().count() > MAX_PATH_CHARS {
            return Err(PathRefusal::TooLong);
        }
        if raw.chars().any(char::is_control) {
            return Err(PathRefusal::InvalidCharacter);
        }
        let s = raw.replace('/', "\\");
        let (verbatim, rest) = if let Some(r) = s.strip_prefix(r"\\?\") {
            (true, r)
        } else if s.starts_with(r"\\.\") || s.starts_with(r"\??\") {
            return Err(PathRefusal::DeviceNamespace);
        } else {
            (false, s.as_str())
        };
        let unc = if verbatim {
            strip_prefix_ci(rest, "UNC\\")
        } else {
            rest.strip_prefix(r"\\")
        };
        let (root, tail) = match unc {
            Some(u) => {
                let mut it = u.splitn(3, '\\');
                let server = it.next().unwrap_or_default();
                let share = it.next().unwrap_or_default();
                if server.is_empty() || share.is_empty() || server == "?" || server == "." {
                    return Err(PathRefusal::MalformedUnc);
                }
                check_name(server)?;
                check_name(share)?;
                let root = Root::Unc {
                    server: server.into(),
                    share: share.trim_end_matches([' ', '.']).into(),
                };
                (root, it.next().unwrap_or_default())
            }
            None => {
                let mut chars = rest.chars();
                match (chars.next(), chars.next(), chars.next()) {
                    (Some(d), Some(':'), Some('\\')) if d.is_ascii_alphabetic() => {
                        (Root::Drive(d.to_ascii_uppercase()), &rest[3..])
                    }
                    _ => return Err(PathRefusal::Relative),
                }
            }
        };
        let mut parts = Vec::new();
        for comp in tail.split('\\') {
            match comp {
                "" => continue,
                "." if !verbatim => continue,
                "." | ".." => return Err(PathRefusal::ParentSegment),
                _ => {}
            }
            let trimmed = comp.trim_end_matches([' ', '.']);
            if trimmed.is_empty() {
                return Err(PathRefusal::ParentSegment);
            }
            if verbatim && trimmed.len() != comp.len() {
                return Err(PathRefusal::VerbatimTrailingDotOrSpace);
            }
            check_name(comp)?;
            if is_reserved(trimmed) {
                return Err(PathRefusal::ReservedName);
            }
            if is_short_name(trimmed) {
                return Err(PathRefusal::ShortName);
            }
            parts.push(trimmed.to_string());
        }
        let root_key = match &root {
            Root::Drive(d) => format!("{}:", d.to_ascii_lowercase()),
            Root::Unc { server, share } => format!(r"\\{}\{}", server, share).to_lowercase(),
        };
        let keys = parts.iter().map(|p| p.to_lowercase()).collect();
        Ok(Self {
            root,
            root_key,
            parts,
            keys,
        })
    }
    pub fn is_network(&self) -> bool {
        matches!(self.root, Root::Unc { .. })
    }
    pub fn drive(&self) -> Option<char> {
        match self.root {
            Root::Drive(d) => Some(d),
            Root::Unc { .. } => None,
        }
    }
    pub fn is_volume_root(&self) -> bool {
        self.parts.is_empty()
    }
    /// Components after the drive or share root, in their original case.
    pub fn components(&self) -> &[String] {
        &self.parts
    }
    pub(crate) fn keys(&self) -> &[String] {
        &self.keys
    }
    pub fn same_root(&self, other: &Self) -> bool {
        self.root_key == other.root_key
    }
    /// Strict ancestor: `self` contains `other` and is not equal to it.
    pub fn is_ancestor_of(&self, other: &Self) -> bool {
        self.same_root(other)
            && self.keys.len() < other.keys.len()
            && other.keys.starts_with(&self.keys)
    }
    /// `self` equals `base` or lies below it.
    pub fn starts_with(&self, base: &Self) -> bool {
        self == base || base.is_ancestor_of(self)
    }
    pub fn file_name(&self) -> Option<&str> {
        self.parts.last().map(String::as_str)
    }
    pub fn extension(&self) -> Option<String> {
        let name = self.keys.last()?;
        let (stem, ext) = name.rsplit_once('.')?;
        (!stem.is_empty() && !ext.is_empty()).then(|| ext.to_string())
    }
    pub fn join(&self, component: &str) -> Result<Self, PathRefusal> {
        if component.contains(['\\', '/']) {
            return Err(PathRefusal::InvalidCharacter);
        }
        Self::parse(&format!(r"{}\{}", self, component))
    }
}

impl fmt::Display for NormalizedPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.root {
            Root::Drive(d) => write!(f, r"{d}:\")?,
            Root::Unc { server, share } => {
                write!(f, r"\\{server}\{share}")?;
                if !self.parts.is_empty() {
                    f.write_str("\\")?;
                }
            }
        }
        f.write_str(&self.parts.join("\\"))
    }
}
impl From<NormalizedPath> for String {
    fn from(p: NormalizedPath) -> Self {
        p.to_string()
    }
}
impl TryFrom<String> for NormalizedPath {
    type Error = PathRefusal;
    fn try_from(s: String) -> Result<Self, Self::Error> {
        Self::parse(&s)
    }
}
impl std::str::FromStr for NormalizedPath {
    type Err = PathRefusal;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse(s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn p(s: &str) -> NormalizedPath {
        NormalizedPath::parse(s).unwrap()
    }
    fn refused(s: &str) -> PathRefusal {
        NormalizedPath::parse(s).unwrap_err()
    }

    #[test]
    fn spellings_of_the_same_directory_are_equal() {
        let base = p(r"C:\Windows\Installer");
        for s in [
            "c:/WINDOWS/./installer",
            r"C:\Windows\Installer\",
            r"C:\\Windows\\\Installer",
            r"\\?\C:\Windows\Installer",
            r"C:\Windows.\Installer",
            r"C:\Windows \Installer . ",
            "//?/c:/windows/installer",
        ] {
            assert_eq!(p(s), base, "{s}");
        }
        assert_eq!(
            p(r"\\?\C:\Windows\WinSxS\").to_string(),
            r"C:\Windows\WinSxS"
        );
    }
    #[test]
    fn unc_forms_are_network_paths() {
        let a = p(r"\\Server\Share\dir");
        let b = p(r"\\?\UNC\server\SHARE\DIR");
        assert!(a.is_network() && b.is_network());
        assert_eq!(a, b);
        assert_eq!(a.to_string(), r"\\Server\Share\dir");
        assert_eq!(refused(r"\\server"), PathRefusal::MalformedUnc);
        assert_eq!(refused(r"\\?\UNC\server"), PathRefusal::MalformedUnc);
    }
    #[test]
    fn ambiguous_spellings_are_refused() {
        use PathRefusal::*;
        for (s, r) in [
            ("", Empty),
            (r"Windows\System32", Relative),
            (r"C:Windows", Relative),
            (r"C:", Relative),
            (r"\Windows", Relative),
            (r"\\.\C:\Windows", DeviceNamespace),
            (r"\??\C:\Windows", DeviceNamespace),
            (r"C:\Users\bob\..\..\Windows", ParentSegment),
            (r"C:\Users\...\x", ParentSegment),
            (r"\\?\C:\Windows.", VerbatimTrailingDotOrSpace),
            (r"\\?\C:\Windows\.\System32", ParentSegment),
            (r"C:\Windows\System32:stream", AlternateDataStream),
            (r"C:\file.txt::$DATA", AlternateDataStream),
            (r"C:\PROGRA~1\App", ShortName),
            (r"C:\Users\BOBSMI~2\AppData", ShortName),
            (r"C:\dir\a*b", InvalidCharacter),
            ("C:\\dir\\a\u{0}b", InvalidCharacter),
            (r"C:\dir\NUL", ReservedName),
            (r"C:\dir\com1.txt", ReservedName),
            (r"C:\dir\LPT9 .log", ReservedName),
        ] {
            assert_eq!(refused(s), r, "{s}");
        }
        assert_eq!(
            refused(&format!(r"C:\{}", "a".repeat(MAX_PATH_CHARS))),
            TooLong
        );
    }
    #[test]
    fn tilde_without_short_name_shape_is_accepted() {
        assert!(NormalizedPath::parse(r"C:\dir\~temp.txt").is_ok());
        assert!(NormalizedPath::parse(r"C:\dir\long-file-name~1.txt").is_ok());
        assert!(NormalizedPath::parse(r"C:\$WINDOWS.~BT").is_ok());
    }
    #[test]
    fn ancestry_is_component_based_and_case_insensitive() {
        let win = p(r"C:\Windows");
        assert!(win.is_ancestor_of(&p(r"c:\windows\system32")));
        assert!(!win.is_ancestor_of(&p(r"C:\Windows.old\x")));
        assert!(!win.is_ancestor_of(&p(r"D:\Windows\x")));
        assert!(!win.is_ancestor_of(&win));
        assert!(win.starts_with(&win));
        assert!(p(r"C:\").is_volume_root());
        assert_eq!(p(r"C:\a\Setup.EXE").extension().as_deref(), Some("exe"));
        assert_eq!(p(r"C:\a\.gitignore").extension(), None);
        assert_eq!(p(r"C:\a").join("b").unwrap(), p(r"C:\a\b"));
        assert!(p(r"C:\a").join(r"..\b").is_err());
    }
    #[test]
    fn serde_round_trips_and_refuses_invalid() {
        let json = serde_json::to_string(&p("c:/Users/Bob")).unwrap();
        assert_eq!(json, r#""C:\\Users\\Bob""#);
        assert_eq!(
            serde_json::from_str::<NormalizedPath>(&json).unwrap(),
            p(r"C:\users\bob")
        );
        assert!(serde_json::from_str::<NormalizedPath>(r#""C:\\a\\..\\b""#).is_err());
    }
}
