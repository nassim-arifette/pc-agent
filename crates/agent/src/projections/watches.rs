//! C5 projection of stored alerts. This module neither collects nor writes anything.
use crate::{PrivacyContext, Sharing, ToolOutput};
use pcagent_shared::{Evidence, Metric, Status};
use pcagent_store::{watches::WatchAlert, EventObservation};
use serde_json::{json, Value};

const MAX_RESPONSE_BYTES: usize = 128 * 1024;

fn observation(
    value: &EventObservation,
    target: &str,
    reference: &str,
    evidence: &mut Vec<Evidence>,
    remaining: &mut usize,
) -> Value {
    *remaining = remaining.saturating_sub(1);
    let entity = if value.entity_ref == "system" || super::history::process_ref(&value.entity_ref) {
        value.entity_ref.as_str()
    } else {
        target
    };
    if let (Ok(metric), Some(window)) = (
        serde_json::from_value::<Metric>(json!(value.metric)),
        value.window.clone(),
    ) {
        let mut measured = Evidence::measured(metric, entity, value.value, &value.source, window);
        measured.origin = value.origin;
        measured.status = value.status;
        evidence.push(measured);
    }
    let usable = value.window.is_some()
        && value.measured_at_unix_ms.is_some()
        && !value.source.is_empty()
        && matches!(value.status, Status::Ok | Status::Partial);
    let mut inputs = Vec::new();
    for (index, input) in value.inputs.iter().enumerate() {
        if *remaining == 0 {
            break;
        }
        inputs.push(observation(
            input,
            target,
            &format!("{reference}-input-{}", index + 1),
            evidence,
            remaining,
        ));
    }
    let incomplete =
        inputs.len() < value.inputs.len() || inputs.iter().any(|input| input["status"] != "ok");
    json!({
        "observation_id": reference,
        "entity_ref": entity,
        "metric": value.metric,
        "value": usable.then_some(value.value),
        "unit": value.unit,
        "source": value.source,
        "status": if !usable || incomplete { Status::Partial } else { value.status },
        "origin": value.origin,
        "measured_at_unix_ms": value.measured_at_unix_ms,
        "window": value.window,
        "derivation": value.derivation,
        "inputs": inputs,
        "input_coverage": {"total": value.inputs.len(), "returned": inputs.len(), "complete": !incomplete},
    })
}

pub fn project(
    alerts: &[WatchAlert],
    total: u64,
    offset: u64,
    sharing: Sharing,
    privacy: &PrivacyContext,
) -> ToolOutput {
    let mut rows = Vec::new();
    let mut evidence = Vec::new();
    let mut bytes = 0;
    let mut partial = false;
    for (index, alert) in alerts.iter().enumerate() {
        let alias = format!("watch-target-{}", offset + index as u64 + 1);
        let reference = format!("watch-alert-{}", offset + index as u64 + 1);
        let mut rule = serde_json::to_value(&alert.rule).expect("closed watch rule");
        let object = rule.as_object_mut().expect("tagged rule");
        object.remove("root");
        object.remove("nvml_index");
        let name = object.remove("executable");
        object.insert("target_ref".into(), json!(alias));
        let shared_name = if sharing == Sharing::Complete {
            name.as_ref()
                .and_then(Value::as_str)
                .and_then(|n| privacy.shared_name(n))
        } else {
            None
        };
        let mut measured = Vec::new();
        let first = observation(
            &alert.observation,
            &alias,
            &format!("{reference}-initial"),
            &mut measured,
            &mut 64,
        );
        let extreme = observation(
            &alert.extreme_observation,
            &alias,
            &format!("{reference}-extreme"),
            &mut measured,
            &mut 64,
        );
        let mut row = json!({
            "alert_ref": reference,
            "rule_at_trigger": rule,
            "shared_names": shared_name.map(|name| json!({alias: name})).unwrap_or(json!({})),
            "started_at_unix_ms": alert.started_at_unix_ms,
            "last_checked_unix_ms": alert.last_checked_unix_ms,
            "ended_at_unix_ms": alert.ended_at_unix_ms,
            "close_reason": alert.close_reason,
            "lower_bound": alert.lower_bound,
            "initial_observation": first,
            "extreme_observation": extreme,
        });
        privacy.sanitize_value(&mut row);
        let mut size = serde_json::to_vec(&row).expect("projected alert").len();
        if size > MAX_RESPONSE_BYTES - 4096 {
            row["initial_observation"] = Value::Null;
            row["extreme_observation"] = Value::Null;
            row["limitations"] = json!([
                "Recorded provenance exceeds this response's byte limit; inspect it locally."
            ]);
            size = serde_json::to_vec(&row)
                .expect("bounded alert header")
                .len();
            measured.clear();
            partial = true;
        }
        if bytes + size > MAX_RESPONSE_BYTES - 4096 {
            break;
        }
        bytes += size;
        partial |= alert.lower_bound || first["status"] != "ok" || extreme["status"] != "ok";
        rows.push(row);
        evidence.extend(measured);
    }
    let next = offset.saturating_add(rows.len() as u64);
    let status = if partial || rows.is_empty() || next < total {
        Status::Partial
    } else {
        Status::Ok
    };
    let measured_at = alerts
        .iter()
        .take(rows.len())
        .flat_map(|alert| [&alert.observation, &alert.extreme_observation])
        .filter_map(|o| o.measured_at_unix_ms)
        .max();
    let limitations = vec![
        "Alerts describe sampled past windows, not the current PC or proven causes. Between samples the resource is unknown.".into(),
        "Application values are lower bounds from observed exact-basename matches; missing processes are not zero. Alert thresholds are configured values, not measurements.".into(),
    ];
    ToolOutput {
        status,
        inspection_complete: true,
        cloud: json!({
            "status": status, "historical": true, "alerts": rows,
            "measured_at_unix_ms": measured_at,
            "coverage": {"total": total, "offset": offset, "returned": next-offset,
                "next_offset": (next < total).then_some(next),
                "response_byte_limit": MAX_RESPONSE_BYTES},
            "limitations": limitations,
        }),
        evidence,
        findings: vec![],
        processes: vec![],
        storage_items: vec![],
        limitations,
    }
}
