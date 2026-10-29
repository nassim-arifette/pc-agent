//! Today's local report: one measurement, the local rules, the last 24 hours of local history
//! and watch alerts. No network, no model, no administrator.
use crate::history::history_path;
use crate::Result;
use pcagent_shared::unix_ms;
use std::path::PathBuf;

/// `%APPDATA%\pcagent\reports`, next to the history and settings.
pub(super) fn default_dir() -> Option<PathBuf> {
    Some(
        pcagent_agent::credentials::file_path()?
            .parent()?
            .join("reports"),
    )
}

pub(super) fn run(options: crate::args::Report) -> Result<u8> {
    let dir = options
        .out_dir
        .or_else(default_dir)
        .ok_or("No local directory is available for the report; use --out-dir.")?;
    let database = history_path(options.database).ok();
    let report = pcagent_report::collect(database.as_deref(), unix_ms(), options.scheduled);
    let path = pcagent_report::write(&dir, &report, crate::text::language())?;
    println!(
        "{}",
        crate::text::label("cli.report.written")
            .replace("{count}", &report.points_to_review.to_string())
            .replace("{path}", &path.with_extension("md").display().to_string())
    );
    Ok(0)
}
