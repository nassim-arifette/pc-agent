//! Local state paths and machine-readable CLI output.
use crate::Result;
use serde::Serialize;
use std::io;
use std::io::Write;
use std::path::PathBuf;

pub(super) fn local_data_dir() -> PathBuf {
    std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("pcagent")
}
pub(super) fn write_json(value: &impl Serialize) -> Result<()> {
    let mut out = io::stdout().lock();
    serde_json::to_writer_pretty(&mut out, value)?;
    writeln!(out)?;
    Ok(())
}
