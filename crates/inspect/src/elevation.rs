//! C4 projection of privileged results; host APIs remain available at their existing paths.
pub use pcagent_elevation::*;

mod notes;
mod projection;
pub use notes::{coverage_notes, CoverageNote, CoverageReason};

#[cfg(test)]
mod fixtures;

/// Convert only validated aggregate measurements to C4. Job paths, identities and nonce stay local.
pub fn to_inspection(response: &ElevationResponse) -> crate::Inspection {
    use crate::CoverageStatus as C;
    let mut out = crate::Inspection::new();
    out.measured_at_unix_ms = response.window.finished_at_unix_ms;
    out.window = response.window.clone();
    if matches!(response.task, ElevatedTask::DeepAnalysis { .. }) {
        for section in &response.sections {
            let projected = to_inspection(section);
            out.observations.extend(projected.observations);
            out.coverage.extend(projected.coverage);
            out.names.extend(projected.names);
        }
        return out.finish();
    }
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

#[cfg(test)]
mod tests {
    use super::fixtures::{empty_response, request};
    use super::*;
    use std::time::Duration;

    #[test]
    fn empty_response_projection_keeps_job_identifiers_local() {
        let request = request(ElevatedTask::SystemCaches, Duration::from_secs(10));
        let response = empty_response(
            &request,
            ElevationStatus::Unsupported,
            ElevationLimit::MftAccuracyUnvalidated,
        );
        let projected = serde_json::to_string(&to_inspection(&response)).unwrap();
        let request = serde_json::to_value(&request).unwrap();
        assert!(!projected.contains(request["nonce"].as_str().unwrap()));
        assert!(!projected.contains(request["job_id"].as_str().unwrap()));
        assert!(to_inspection(&response).observations.is_empty());
        assert_eq!(
            to_inspection(&response).status,
            pcagent_shared::Status::Unsupported
        );
    }
}
