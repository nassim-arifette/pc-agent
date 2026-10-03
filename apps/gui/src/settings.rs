//! Local C5 sharing preferences. Corrupt or missing state never silently authorizes cloud use.
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

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PrivacySettings {
    pub mode: PrivacyMode,
    pub confirmed: bool,
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
    fn first_launch_proposes_complete_without_authorizing_a_request() {
        let settings = PrivacySettings::default();
        assert_eq!(settings.mode, PrivacyMode::Complete);
        assert!(!settings.confirmed);
        assert_eq!(PrivacyMode::Local.sharing(), None);
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
