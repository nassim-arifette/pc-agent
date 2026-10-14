//! Local C5 sharing preferences. Since 9 October nothing waits for a confirmation dialog: the
//! composer's toggles show what leaves, on by default, and the exact preview is one click away.
//! Corrupt or missing state falls back to those defaults; a cloud request still needs a saved key
//! and a single-use preview ticket for its question.
use pcagent_agent::{credentials, Sharing};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::Read,
    path::{Path, PathBuf},
};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PrivacyMode {
    #[default]
    Complete,
    Anonymized,
    Local,
}
impl PrivacyMode {
    pub fn sharing(self) -> Option<Sharing> {
        match self {
            Self::Complete => Some(Sharing::Complete),
            Self::Anonymized => Some(Sharing::Anonymized),
            Self::Local => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PrivacySettings {
    pub mode: PrivacyMode,
    /// Set by the first cloud send; kept for the record, no longer required before one.
    pub confirmed: bool,
    /// Web lookups (C6): their own remembered toggle, on by default; never in local mode, and
    /// without effect until a Tavily key is saved.
    #[serde(default = "on")]
    pub web_search: bool,
    /// Names from the finite list of an attached scanned folder (C5), on by default; complete
    /// sharing only, and bound to each preview ticket.
    #[serde(default = "on")]
    pub file_names: bool,
}
impl PrivacySettings {
    /// Every disclosure choice belongs to the preview ticket, including the file-name allowlist.
    pub fn invalidates_preview(&self, previous: &Self) -> bool {
        self.mode != previous.mode
            || self.web_search != previous.web_search
            || self.file_names != previous.file_names
            || !self.confirmed
    }
}
fn on() -> bool {
    true
}
impl Default for PrivacySettings {
    fn default() -> Self {
        Self {
            mode: PrivacyMode::default(),
            confirmed: false,
            web_search: true,
            file_names: true,
        }
    }
}

fn path() -> Option<PathBuf> {
    Some(credentials::file_path()?.parent()?.join("privacy.json"))
}
pub fn load() -> PrivacySettings {
    path()
        .and_then(|path| load_from(&path).ok())
        .unwrap_or_default()
}
pub fn save(settings: &PrivacySettings) -> Result<(), String> {
    save_to(
        &path().ok_or("The local settings directory is unavailable.")?,
        settings,
    )
}
fn load_from(path: &Path) -> Result<PrivacySettings, String> {
    let file = fs::File::open(path).map_err(|_| "Privacy settings are unavailable.")?;
    let mut bytes = Vec::new();
    file.take(4097)
        .read_to_end(&mut bytes)
        .map_err(|_| "Privacy settings could not be read.")?;
    if bytes.len() > 4096 {
        return Err("Privacy settings exceed the supported size.".into());
    }
    let value: serde_json::Value =
        serde_json::from_slice(&bytes).map_err(|_| "Privacy settings are malformed.")?;
    if value["schema_version"] != 1 {
        return Err("Privacy settings have an unsupported version.".into());
    }
    serde_json::from_value(value["settings"].clone())
        .map_err(|_| "Privacy settings are malformed.".into())
}
fn save_to(path: &Path, settings: &PrivacySettings) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or("Privacy settings have no local directory.")?;
    fs::create_dir_all(parent).map_err(|_| "The local settings directory could not be created.")?;
    let temp = parent.join(format!("{}.tmp", pcagent_shared::new_id("privacy")));
    let result = (|| {
        let bytes =
            serde_json::to_vec(&serde_json::json!({"schema_version":1,"settings":settings}))
                .map_err(|_| "Privacy settings could not be encoded.")?;
        fs::write(&temp, bytes).map_err(|_| "Privacy settings could not be saved.")?;
        fs::rename(&temp, path)
            .map_err(|_| "Privacy settings could not replace the previous choice.")
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result.map_err(str::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn file_name_changes_revoke_previews_after_the_first_send() {
        let previous = PrivacySettings {
            confirmed: true,
            ..PrivacySettings::default()
        };
        assert!(!previous.invalidates_preview(&previous));
        let without_names = PrivacySettings {
            file_names: false,
            ..previous.clone()
        };
        assert!(without_names.invalidates_preview(&previous));
        assert!(previous.invalidates_preview(&without_names));
        assert!(!without_names.invalidates_preview(&without_names));
    }

    #[test]
    fn first_launch_uses_complete_sharing_with_its_toggles_on() {
        let settings = PrivacySettings::default();
        assert_eq!(settings.mode, PrivacyMode::Complete);
        assert!(!settings.confirmed);
        assert!(settings.web_search && settings.file_names);
        assert_eq!(PrivacyMode::Local.sharing(), None);
        // A file saved before the toggles existed keeps its mode and gets them on.
        let older: PrivacySettings =
            serde_json::from_str(r#"{"mode":"anonymized","confirmed":true}"#).unwrap();
        assert_eq!(older.mode, PrivacyMode::Anonymized);
        assert!(older.web_search && older.file_names);
        // A toggle turned off stays off.
        let off: PrivacySettings = serde_json::from_str(
            r#"{"mode":"complete","confirmed":true,"web_search":false,"file_names":false}"#,
        )
        .unwrap();
        assert!(!off.web_search && !off.file_names);
        assert!(serde_json::from_str::<PrivacySettings>(
            r#"{"mode":"complete","confirmed":true,"api_key":"not-accepted"}"#
        )
        .is_err());
    }
    #[test]
    fn settings_survive_reopening_and_corruption_never_grants_consent() {
        let directory = std::env::temp_dir().join(pcagent_shared::new_id("privacy-test"));
        fs::create_dir_all(&directory).unwrap();
        let path = directory.join("privacy.json");
        for mode in [
            PrivacyMode::Complete,
            PrivacyMode::Anonymized,
            PrivacyMode::Local,
        ] {
            let expected = PrivacySettings {
                mode,
                confirmed: true,
                web_search: mode != PrivacyMode::Local,
                file_names: mode == PrivacyMode::Complete,
            };
            save_to(&path, &expected).unwrap();
            assert_eq!(load_from(&path).unwrap(), expected);
        }
        fs::write(
            &path,
            r#"{"schema_version":99,"settings":{"mode":"complete","confirmed":true}}"#,
        )
        .unwrap();
        assert!(load_from(&path).is_err());
        fs::write(&path, "x".repeat(4097)).unwrap();
        assert!(load_from(&path).is_err());
        fs::remove_file(&path).unwrap();
        fs::remove_dir(&directory).unwrap();
    }
}
