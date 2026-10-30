//! Local-only specialist preference; no credentials or selectable network destinations.
use pcagent_agent::credentials;
use pcagent_nebius::specialist::{Identity, Local, ENDPOINT};
use serde::Serialize;

#[derive(Serialize)]
pub struct View {
    pub enabled: bool,
    pub endpoint: &'static str,
    pub identity: Option<Identity>,
    pub error: Option<&'static str>,
}

fn path() -> Option<std::path::PathBuf> {
    Some(
        credentials::file_path()?
            .parent()?
            .join("specialist-local.json"),
    )
}
pub fn enabled() -> bool {
    path()
        .and_then(|path| std::fs::read(path).ok())
        .is_some_and(|bytes| bytes == b"{\"enabled\":true}\n")
}
pub fn set_enabled(enabled: bool) -> Result<(), &'static str> {
    let path = path().ok_or("Local settings directory is unavailable.")?;
    let parent = path
        .parent()
        .ok_or("Local settings directory is unavailable.")?;
    std::fs::create_dir_all(parent).map_err(|_| "Specialist setting could not be saved.")?;
    std::fs::write(
        path,
        if enabled {
            b"{\"enabled\":true}\n".as_slice()
        } else {
            b"{\"enabled\":false}\n".as_slice()
        },
    )
    .map_err(|_| "Specialist setting could not be saved.")
}
pub fn view(check: bool) -> View {
    let health = check.then(Local::health);
    let (identity, error) = match health {
        Some(Ok(identity)) => (Some(identity), None),
        Some(Err(error)) => (None, Some(error)),
        None => (None, None),
    };
    View {
        enabled: enabled(),
        endpoint: ENDPOINT,
        identity,
        error,
    }
}
