//! Locally approved elevation for one fixed, read-only task. No model or network entry point.
#[cfg(any(windows, test))]
mod cache;
pub mod energy;
#[cfg(windows)]
#[allow(unsafe_code)]
mod energy_native;
mod job;
mod lifecycle;
pub mod mft;
#[cfg(windows)]
#[allow(unsafe_code)]
mod mft_native;
#[cfg(windows)]
#[allow(unsafe_code)]
mod native;
mod projection;
mod protocol;
pub mod system;
#[cfg(windows)]
#[allow(unsafe_code)]
mod system_native;
pub mod trace;
#[cfg(windows)]
#[allow(unsafe_code)]
mod trace_native;
mod worker;

pub use job::{ElevationApproval, PreparedElevation};
pub use lifecycle::ElevationCompletion;
pub use protocol::{
    CacheCategory, CacheCoverage, CacheSummary, DriveLetter, ElevatedTask, ElevationError,
    ElevationLimit, ElevationRequest, ElevationResponse, ElevationStatus, WorkerArguments,
    MAX_BUDGET_MS, MAX_REQUEST_BYTES, MAX_RESPONSE_BYTES, PROTOCOL_VERSION,
};
pub use worker::worker_main;

/// The desktop and model host must remain unelevated; only the fixed worker may be admin.
pub fn ensure_unelevated_host() -> Result<(), ElevationError> {
    #[cfg(windows)]
    if native::is_elevated()? {
        return Err(ElevationError::ElevatedHost);
    }
    Ok(())
}

/// Convert only validated aggregate measurements to C4. Job paths, identities and nonce stay local.
pub fn to_inspection(response: &ElevationResponse) -> crate::Inspection {
    use crate::CoverageStatus as C;
    let mut out = crate::Inspection::new();
    out.measured_at_unix_ms = response.window.finished_at_unix_ms;
    out.window = response.window.clone();
    for cache in &response.caches {
        let entity = out.entity("system-cache", None);
        let source = "pcagent-elevated / pcagent-storage ClassicBackend metadata";
        out.value(
            &entity,
            "system_cache_category",
            cache.category.as_str(),
            "category",
            source,
        );
        let status = match cache.status {
            ElevationStatus::Completed => C::Ok,
            ElevationStatus::Unsupported => C::Unsupported,
            ElevationStatus::Denied => C::Denied,
            ElevationStatus::Error => C::Error,
            _ => C::Partial,
        };
        out.value(
            &entity,
            "system_cache_exists",
            cache
                .exists
                .map(serde_json::Value::from)
                .unwrap_or(serde_json::Value::Null),
            "boolean",
            source,
        );
        let first_measurement = out.observations.len() - 1;
        for (metric, value, unit) in [
            (
                "system_cache_logical_bytes",
                cache.observed_logical_bytes,
                "bytes",
            ),
            (
                "system_cache_allocated_known_bytes",
                cache.observed_allocated_bytes,
                "bytes",
            ),
            ("system_cache_file_count", cache.files, "count"),
            (
                "system_cache_allocation_unknown_files",
                cache.coverage.as_ref().map(|c| c.allocation_unknown_files),
                "count",
            ),
        ] {
            out.value(
                &entity,
                metric,
                value
                    .map(serde_json::Value::from)
                    .unwrap_or(serde_json::Value::Null),
                unit,
                source,
            );
        }
        for observation in &mut out.observations[first_measurement..] {
            observation.status = if observation.value.is_null() && status == C::Ok {
                C::Unsupported
            } else {
                status
            };
        }
        let detail = format!(
            "Metadata snapshot only; observed allocation is not reclaimable space. Missing values remain unknown. Limits: {:?}; coverage: {:?}.",
            cache.limitations, cache.coverage,
        );
        out.coverage(cache.category.as_str(), status, source, &detail);
    }
    projection::add(&mut out, response);
    if response.caches.is_empty()
        && response.mft.is_none()
        && response.shadow_storage.is_none()
        && response.disk_reliability.is_none()
        && response.process_images.is_none()
        && response.battery_energy.is_none()
        && response.performance_trace.is_none()
    {
        let detail = format!("No usable aggregate measurement was returned; no cleanup recommendation is available. Limits: {:?}.", response.limitations);
        out.coverage(
            "elevated_task",
            match response.status {
                ElevationStatus::Unsupported => C::Unsupported,
                ElevationStatus::Denied => C::Denied,
                ElevationStatus::Error => C::Error,
                _ => C::Partial,
            },
            "pcagent-elevated",
            &detail,
        );
    }
    out.finish()
}
