//! Nebius settings: environment variables first, then the per-user file `%APPDATA%\pcagent\.env`.
//! A `.env` in the current directory is never read: running pcagent inside someone else's folder
//! must not send your question and measurements to their account.
use std::{
    collections::HashSet,
    fs,
    io::Read,
    path::{Path, PathBuf},
};

/// The only names read from the file; any other line is ignored.
pub const KEYS: [&str; 4] = [
    "NEBIUS_API_KEY",
    "NEBIUS_MODEL_ID",
    "NEBIUS_REASONING",
    "TAVILY_API_KEY",
];
const MAX_FILE_BYTES: u64 = 16 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    Environment,
    File,
}

pub struct Credentials {
    file: Option<PathBuf>,
    file_found: bool,
    values: Vec<(&'static str, String, Source)>,
    /// Why the file could not be used, if it exists but is unreadable.
    pub problem: Option<String>,
}

impl Credentials {
    pub fn load() -> Self {
        Self::load_from(|name| std::env::var(name).ok(), file_path())
    }
    /// Only the given file, ignoring environment variables (tests and settings previews).
    pub fn load_file(path: &Path) -> Self {
        Self::load_from(|_| None, Some(path.to_owned()))
    }
    fn load_from(env: impl Fn(&str) -> Option<String>, file: Option<PathBuf>) -> Self {
        let (from_file, file_found, problem) = match &file {
            Some(path) => match read(path) {
                Ok(Some(text)) => (parse(&text), true, None),
                Ok(None) => (Vec::new(), false, None),
                Err(problem) => (Vec::new(), true, Some(problem)),
            },
            None => (Vec::new(), false, None),
        };
        let values = KEYS
            .iter()
            .filter_map(|&name| {
                let from_env = env(name).filter(|v| !v.trim().is_empty());
                match from_env {
                    Some(v) => Some((name, v, Source::Environment)),
                    None => from_file
                        .iter()
                        .find(|(k, v)| *k == name && !v.trim().is_empty())
                        .map(|(_, v)| (name, v.clone(), Source::File)),
                }
            })
            .collect();
        Self {
            file,
            file_found,
            values,
            problem,
        }
    }
    pub fn get(&self, name: &str) -> Option<String> {
        self.values
            .iter()
            .find(|(k, _, _)| *k == name)
            .map(|(_, v, _)| v.clone())
    }
    pub fn source(&self, name: &str) -> Option<Source> {
        self.values
            .iter()
            .find(|(k, _, _)| *k == name)
            .map(|(_, _, s)| *s)
    }
    pub fn file(&self) -> Option<&Path> {
        self.file.as_deref()
    }
    pub fn file_found(&self) -> bool {
        self.file_found
    }
}

pub fn file_path() -> Option<PathBuf> {
    std::env::var_os("APPDATA")
        .filter(|d| !d.is_empty())
        .map(|d| PathBuf::from(d).join("pcagent").join(".env"))
}

fn read(path: &Path) -> Result<Option<String>, String> {
    let file = match fs::File::open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("cannot read {}: {e}", path.display())),
    };
    let mut bytes = Vec::new();
    file.take(MAX_FILE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    if bytes.len() as u64 > MAX_FILE_BYTES {
        return Err(format!("{} is larger than 16 KiB", path.display()));
    }
    let text =
        String::from_utf8(bytes).map_err(|_| format!("{} is not UTF-8 text", path.display()))?;
    Ok(Some(text.trim_start_matches('\u{feff}').to_owned()))
}

/// The setting a `NAME=value` line assigns, if it is one of [`KEYS`], and its raw value.
fn setting(line: &str) -> Option<(&'static str, &str)> {
    let line = line.trim();
    if line.is_empty() || line.starts_with('#') {
        return None;
    }
    let line = line.strip_prefix("export ").unwrap_or(line);
    let (name, value) = line.split_once('=')?;
    let key = KEYS.iter().find(|k| **k == name.trim())?;
    Some((key, value))
}

/// `NAME=value` lines; blank lines, `#` comments and an `export ` prefix are accepted, and one
/// pair of matching quotes around the value is removed. Only [`KEYS`] are kept.
fn parse(text: &str) -> Vec<(&'static str, String)> {
    let mut out: Vec<(&'static str, String)> = Vec::new();
    for line in text.lines() {
        let Some((key, value)) = setting(line) else {
            continue;
        };
        let value = value.trim();
        let value = ['"', '\'']
            .iter()
            .find_map(|q| value.strip_prefix(*q).and_then(|v| v.strip_suffix(*q)))
            .unwrap_or(value);
        out.retain(|(k, _)| *k != key);
        out.push((key, value.to_owned()));
    }
    out
}

/// Writes `updates` to the per-user settings file at `path`, creating it if needed. Other lines
/// (comments, other variables) are kept; a setting that appears twice is written once. Values
/// must be single-line, without spaces or quotes. The file is replaced atomically; its contents
/// are never logged.
pub fn save(path: &Path, updates: &[(&str, &str)]) -> Result<(), String> {
    for (name, value) in updates {
        if !KEYS.contains(name) {
            return Err(format!("{name} is not a PC Agent setting"));
        }
        let bad = |c: char| c.is_whitespace() || c.is_control() || c == '"' || c == '\'';
        if value.is_empty() || value.len() > 4096 || value.chars().any(bad) {
            return Err(format!(
                "{name} must be a single word without spaces or quotes"
            ));
        }
    }
    let existing = read(path)?.unwrap_or_default();
    let mut written = HashSet::new();
    let mut lines = Vec::new();
    for line in existing.lines() {
        let update =
            setting(line).and_then(|(key, _)| updates.iter().find(|(name, _)| *name == key));
        match update {
            Some((name, value)) => {
                if written.insert(*name) {
                    lines.push(format!("{name}={value}"));
                }
            }
            None => lines.push(line.to_owned()),
        }
    }
    for (name, value) in updates {
        if !written.contains(name) {
            lines.push(format!("{name}={value}"));
        }
    }
    let dir = path
        .parent()
        .ok_or_else(|| format!("{} has no folder", path.display()))?;
    fs::create_dir_all(dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    let temp = dir.join(".env.saving");
    fs::write(&temp, lines.join("\n") + "\n")
        .map_err(|e| format!("cannot write {}: {e}", temp.display()))?;
    fs::rename(&temp, path).map_err(|e| {
        let _ = fs::remove_file(&temp);
        format!("cannot replace {}: {e}", path.display())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn saving_replaces_only_pc_agent_settings_and_keeps_other_lines() {
        let dir = std::env::temp_dir().join(format!("pcagent-save-{}", std::process::id()));
        let file = dir.join("pcagent").join(".env");
        save(&file, &[("NEBIUS_MODEL_ID", "nvidia/nemotron-a")]).unwrap();
        let c = Credentials::load_from(|_| None, Some(file.clone()));
        assert_eq!(
            c.get("NEBIUS_MODEL_ID").as_deref(),
            Some("nvidia/nemotron-a")
        );
        assert_eq!(c.get("NEBIUS_API_KEY"), None);

        fs::write(
            &file,
            "# mine\nNEBIUS_API_KEY=old\nOTHER=kept\nexport NEBIUS_API_KEY='older'\n",
        )
        .unwrap();
        save(
            &file,
            &[
                ("NEBIUS_API_KEY", "v1.new-key"),
                ("NEBIUS_MODEL_ID", "nvidia/nemotron-b"),
            ],
        )
        .unwrap();
        assert_eq!(
            fs::read_to_string(&file).unwrap(),
            "# mine\nNEBIUS_API_KEY=v1.new-key\nOTHER=kept\nNEBIUS_MODEL_ID=nvidia/nemotron-b\n"
        );
        for (name, value) in [
            ("PATH", r"C:\evil"),
            ("NEBIUS_API_KEY", ""),
            ("NEBIUS_API_KEY", "two words"),
            ("NEBIUS_API_KEY", "line\nbreak"),
            ("NEBIUS_MODEL_ID", "\"quoted\""),
        ] {
            assert!(save(&file, &[(name, value)]).is_err(), "{name}={value:?}");
        }
        assert!(fs::read_to_string(&file).unwrap().contains("v1.new-key"));
        assert!(!dir.join("pcagent").join(".env.saving").exists());
        fs::remove_dir_all(&dir).unwrap();
    }
    #[test]
    fn only_nebius_keys_are_read_and_quotes_are_removed() {
        let parsed = parse(
            "\u{feff}# comment\n\nNEBIUS_API_KEY = \"k1\"\nexport NEBIUS_MODEL_ID='nvidia/nemotron-x'\nPATH=C:\\evil\nLOCALAPPDATA=x\nnot a line\n",
        );
        assert_eq!(
            parsed,
            [
                ("NEBIUS_API_KEY", "k1".to_owned()),
                ("NEBIUS_MODEL_ID", "nvidia/nemotron-x".to_owned())
            ]
        );
    }
    #[test]
    fn environment_overrides_the_file_and_empty_values_are_missing() {
        let dir = std::env::temp_dir().join(format!("pcagent-creds-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let file = dir.join(".env");
        fs::write(
            &file,
            "NEBIUS_API_KEY=from-file\nNEBIUS_MODEL_ID=nvidia/nemotron-file\n",
        )
        .unwrap();
        let env =
            |name: &str| (name == "NEBIUS_MODEL_ID").then(|| "nvidia/nemotron-env".to_owned());
        let c = Credentials::load_from(env, Some(file.clone()));
        assert_eq!(c.get("NEBIUS_API_KEY").as_deref(), Some("from-file"));
        assert_eq!(c.source("NEBIUS_API_KEY"), Some(Source::File));
        assert_eq!(
            c.get("NEBIUS_MODEL_ID").as_deref(),
            Some("nvidia/nemotron-env")
        );
        assert_eq!(c.source("NEBIUS_MODEL_ID"), Some(Source::Environment));

        fs::write(&file, "NEBIUS_API_KEY=\n").unwrap();
        let c = Credentials::load_from(|_| Some(" ".into()), Some(file.clone()));
        assert!(c.file_found());
        assert_eq!(c.get("NEBIUS_API_KEY"), None);

        fs::write(&file, vec![b'a'; 20 * 1024]).unwrap();
        let c = Credentials::load_from(|_| None, Some(file));
        assert!(c.problem.is_some());
        assert_eq!(c.get("NEBIUS_API_KEY"), None);
        fs::remove_dir_all(&dir).unwrap();

        let missing = Credentials::load_from(|_| None, Some(dir.join("absent.env")));
        assert!(!missing.file_found());
        assert!(missing.problem.is_none());
    }
}
