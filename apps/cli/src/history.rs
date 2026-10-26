//! Local historical measurements and bounded recording commands.
use crate::local::write_json;
use crate::{args as options, Result};
use serde_json::json;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

pub(super) fn history_path(path: Option<PathBuf>) -> Result<PathBuf> {
    path.or_else(|| {
        pcagent_agent::credentials::file_path()
            .and_then(|path| path.parent().map(|dir| dir.join("pcagent.sqlite")))
    })
    .ok_or_else(|| "The local history location is unknown; use --database.".into())
}
pub(super) fn cli_sampler(path: Option<PathBuf>) -> Result<pcagent_store::sampler::Sampler> {
    Ok(pcagent_store::sampler::Sampler::start(&history_path(
        path,
    )?)?)
}

pub(super) fn history(options: options::History) -> Result<u8> {
    let options::History {
        metric,
        entity_ref,
        window_seconds,
        end_unix_ms,
        database,
    } = options;
    let metric = serde_json::from_value::<pcagent_shared::Metric>(json!(metric))?;
    let end = end_unix_ms.unwrap_or_else(pcagent_shared::unix_ms);
    if end > pcagent_shared::unix_ms() {
        return Err("History cannot read a future window.".into());
    }
    let store = pcagent_store::Store::open(&history_path(database)?)?;
    let result = store.metric_history(&pcagent_store::HistoryQuery {
        metric,
        entity_ref,
        from_unix_ms: end.saturating_sub(window_seconds * 1000),
        to_unix_ms: end,
    })?;
    write_json(&result)?;
    Ok(0)
}

pub(super) fn record(options: options::Record) -> Result<u8> {
    let options::Record { seconds, database } = options;
    let sampler = cli_sampler(database)?;
    let recording = sampler.record(seconds, &AtomicBool::new(false))?;
    let end = recording
        .ended_at_unix_ms
        .unwrap_or_else(pcagent_shared::unix_ms);
    let history = sampler.history(&pcagent_store::HistoryQuery {
        metric: pcagent_shared::Metric::CpuBusyBasisPoints,
        entity_ref: "system".into(),
        from_unix_ms: recording.started_at_unix_ms,
        to_unix_ms: end.max(recording.started_at_unix_ms + 1),
    })?;
    let process_activity = sampler.process_peaks(
        recording.started_at_unix_ms,
        end.max(recording.started_at_unix_ms + 1),
        10,
    )?;
    let battery = sampler.battery_recording(&recording.id)?;
    let resource_activity = sampler.process_resource_peaks(
        recording.started_at_unix_ms,
        end.max(recording.started_at_unix_ms + 1),
        10,
    )?;
    write_json(&json!({"recording":recording,"history":history,
            "process_activity":process_activity,"battery":battery,
            "resource_activity":resource_activity,
            "sampler":sampler.status()}))?;
    Ok(0)
}

pub(super) fn sample(options: options::Sample) -> Result<u8> {
    let options::Sample { seconds, database } = options;
    let sampler = cli_sampler(database)?;
    std::thread::sleep(Duration::from_secs(seconds));
    sampler.stop();
    write_json(&sampler.status())?;
    Ok(0)
}
