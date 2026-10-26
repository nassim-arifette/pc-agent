//! Synthetic messages cross the same closed protocol boundary as a worker response.
use super::*;
use pcagent_shared::{new_id, unix_ms, Window};
use serde_json::json;
use std::time::Duration;

pub(super) fn request(task: ElevatedTask, budget: Duration) -> ElevationRequest {
    let now = unix_ms();
    ElevationRequest::from_bytes(
        &serde_json::to_vec(&json!({
            "protocol_version": PROTOCOL_VERSION,
            "job_id": new_id("elevjob"),
            "nonce": new_id("nonce"),
            "task": task,
            "created_at_unix_ms": now,
            "expires_at_unix_ms": now + MAX_BUDGET_MS,
            "budget_ms": budget.as_millis() as u64,
        }))
        .unwrap(),
    )
    .unwrap()
}

pub(super) fn empty_response(
    request: &ElevationRequest,
    status: ElevationStatus,
    limit: ElevationLimit,
) -> ElevationResponse {
    let fields = serde_json::to_value(request).unwrap();
    serde_json::from_value(json!({
        "protocol_version": PROTOCOL_VERSION,
        "job_id": fields["job_id"],
        "nonce": fields["nonce"],
        "task": request.task(),
        "status": status,
        "window": Window::instant(),
        "caches": [],
        "mft": null,
        "shadow_storage": null,
        "disk_reliability": null,
        "process_images": null,
        "battery_energy": null,
        "performance_trace": null,
        "limitations": [limit],
    }))
    .unwrap()
}

pub(super) fn validate_response(
    response: &ElevationResponse,
    request: &ElevationRequest,
) -> Result<(), ElevationError> {
    ElevationResponse::from_bytes(&serde_json::to_vec(response).unwrap(), request).map(|_| ())
}
