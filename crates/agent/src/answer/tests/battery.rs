use super::*;
use crate::answer::quantities::claims;

#[test]
fn historical_energy_estimates_convert_units_without_proving_capacity_or_power() {
    let received = vec![json!({"observation_id":"O1", "historical":true,
        "observations":[{"observation_id":"energy-fixture-1", "entity_ref":"battery-app-1", "metric":"battery_app_energy_estimated",
        "value":3_600_000,"unit":"mJ","status":"ok","source":"synthetic E3 fixture"}]})];
    let references = refs();
    let context = Context {
        received: &received,
        references: &references,
        plans: &[],
    };
    for text in [
        "Earlier measurement: estimated energy consumption was 3 600 000 mJ [O1].",
        "Earlier measurement: estimated energy consumption was 3 600 J [O1].",
        "Mesure précédente : la consommation d'énergie estimée était de 1 Wh [O1].",
    ] {
        assert!(review(&submit(text), &context).is_ok(), "{text}");
    }
    for text in [
        "Earlier measurement: estimated energy consumption was 3 600 000 MJ [O1].",
        "Earlier measurement: measured energy consumption was 1 Wh [O1].",
        "La capacité de la batterie était de 1 Wh lors de la mesure précédente [O1].",
        "Earlier measurement: estimated power consumption was 1 W [O1].",
        "Estimated energy consumption is currently 1 Wh [O1].",
    ] {
        assert!(review(&submit(text), &context).is_err(), "{text}");
    }
}

#[test]
fn battery_energy_runtime_and_signed_power_require_matching_measured_units() {
    let rows: Vec<_> = [
        ("battery_design_capacity_mwh", 50_000, "milliwatt_hours"),
        (
            "battery_full_charge_capacity_mwh",
            40_000,
            "milliwatt_hours",
        ),
        ("battery_power_rate_mw", -15_000, "milliwatts"),
        (
            "battery_estimated_runtime_windows_seconds",
            7_200,
            "seconds",
        ),
        ("battery_cycle_count", 200, "count"),
        ("gpu_power_milliwatts", 42_000, "milliwatts"),
        ("battery_discharge_power_mw", 15_000, "milliwatts"),
        ("battery_remaining_capacity_mwh", 20_000, "milliwatt_hours"),
        ("battery_estimated_wear_percent", 20, "percent"),
        ("battery_capacity_retention_percent", 80, "percent"),
        ("battery_voltage_mv", 12_000, "millivolts"),
    ]
    .iter()
    .enumerate()
    .map(|(i, (metric, value, unit))| {
        json!({
            "observation_id":format!("measurement-{i}"), "entity_ref":"system",
            "metric":metric, "value":value, "unit":unit, "status":"ok",
            "source":"fixture", "measured_at_unix_ms":1,
        })
    })
    .collect();
    let received = vec![json!({"observation_id":"O1", "observations":rows})];
    let references = refs();
    let ctx = Context {
        received: &received,
        references: &references,
        plans: &[],
    };
    for text in [
        "La batterie a une capacité d'origine de 50 Wh [O1].",
        "La batterie a une capacité actuelle de 40 000 mWh [O1].",
        "La batterie a une capacité restante de 20 Wh [O1].",
        "Battery full charge capacity: 40 Wh [O1].",
        "Battery discharge consumption: 15 W [O1].",
        "L'usure estimée de la batterie est de 20 % [O1].",
        "La rétention de la batterie est de 80 % [O1].",
        "Le débit net de la batterie est de -15 W [O1].",
        "Le débit net de la batterie est de −15 W [O1].",
        "Le débit net de la batterie est de − 15 W [O1].",
        "Le débit net de la batterie est de -\u{a0}15 W [O1].",
        "L'autonomie estimée de la batterie est de 2 heures [O1].",
        "La batterie indique 200 cycles [O1].",
        "La tension de la batterie est de 12 V [O1].",
        "Battery voltage: 12 000 mV [O1].",
    ] {
        assert!(review(&submit(text), &ctx).is_ok(), "{text}");
    }
    for text in [
        "La batterie a une capacité de 90 Wh [O1].",
        "Battery full charge capacity: 50 Wh [O1].",
        "La batterie a une capacité d'origine de 40 Wh [O1].",
        "La batterie a une capacité restante de 40 Wh [O1].",
        "L'usure estimée de la batterie est de 80 % [O1].",
        "La rétention de la batterie est de 20 % [O1].",
        "La batterie consomme 42 W [O1].",
        "Le débit net de la batterie est de 15 W [O1].",
        "La batterie a une capacité de 50 W [O1].",
        "L'autonomie estimée de la batterie est de 2 Wh [O1].",
        "La batterie indique 500 cycles [O1].",
        "La tension de la batterie est de 24 V [O1].",
        "Battery voltage: 12 mV [O1].",
    ] {
        assert!(review(&submit(text), &ctx).is_err(), "{text}");
    }
    let mut positive = received.clone();
    positive[0]["observations"][2]["value"] = json!(15_000);
    let ctx = Context {
        received: &positive,
        references: &references,
        plans: &[],
    };
    assert!(review(&submit("La batterie : − 15 W [O1]."), &ctx).is_err());
    assert!(review(&submit("La batterie : -\u{a0}15 W [O1]."), &ctx).is_err());
    assert!(review(&submit("- 15 W pour la batterie [O1]."), &ctx).is_ok());
}

#[test]
fn battery_claims_cannot_borrow_another_battery_or_gpu_even_with_unicode_refs() {
    let rows = [
        (
            "battery-a",
            "battery_full_charge_capacity_mwh",
            40_000,
            "milliwatt_hours",
        ),
        (
            "battery-b",
            "battery_full_charge_capacity_mwh",
            60_000,
            "milliwatt_hours",
        ),
        ("battery-a", "battery_power_rate_mw", -15_000, "milliwatts"),
        ("battery-b", "battery_power_rate_mw", -25_000, "milliwatts"),
        ("system", "gpu_power_milliwatts", 42_000, "milliwatts"),
    ]
    .into_iter()
    .enumerate()
    .map(|(i, (entity, metric, value, unit))| {
        json!({
            "observation_id":format!("measurement-{i}"), "entity_ref":entity,
            "metric":metric, "value":value, "unit":unit, "status":"ok", "source":"fixture",
        })
    })
    .collect::<Vec<_>>();
    let received = vec![json!({"observation_id":"O1", "observations":rows})];
    let references = refs();
    let ctx = Context {
        received: &received,
        references: &references,
        plans: &[],
    };
    for text in [
        "Battery battery-a full charge capacity: 40 Wh [O1].",
        "Battery battery‑a full charge capacity: 40 Wh [O1].",
        "Le débit net de la batterie battery‑a est de − 15 W [O1].",
    ] {
        assert!(review(&submit(text), &ctx).is_ok(), "{text}");
    }
    for text in [
        "Battery battery-a full charge capacity: 60 Wh [O1].",
        "Battery battery‑a full charge capacity: 60 Wh [O1].",
        "Le débit net de la batterie battery‑a est de − 25 W [O1].",
        "La batería consume 42 W [O1].",
        "Der Akku verbraucht 42 W [O1].",
        "De batterij gebruikt 42 W [O1].",
        "Батарея потребляет 42 W [O1].",
        "电池 42 W [O1].",
    ] {
        assert!(review(&submit(text), &ctx).is_err(), "{text}");
    }
    // The hexadecimal part of an opaque ID is not a quantity with a byte unit.
    assert!(claims("Process proc‑18b90893 was observed [O1].").is_empty());
    assert!(claims("Process proc‑18bytes90893 was observed [O1].").is_empty());
}

#[test]
fn c4_provenance_windows_and_inputs_cannot_supply_battery_runtime() {
    let mut received = vec![json!({"observation_id":"O1",
    "window":{"duration_ms":1}, "observations":[{
        "observation_id":"capacity", "entity_ref":"battery-a",
        "metric":"battery_remaining_capacity_mwh", "value":50_000,
        "unit":"milliwatt_hours", "status":"ok", "source":"fixture",
        "window":{"duration_ms":1},
        "inputs":[{"metric":"battery_estimated_runtime_windows_seconds",
            "unit":"seconds", "value":7_200, "status":"ok",
            "observation_id":"operand", "entity_ref":"battery-a"}]
    }]})];
    let references = refs();
    let ctx = Context {
        received: &received,
        references: &references,
        plans: &[],
    };
    assert!(review(
        &submit("L'autonomie de la batterie est de 1 ms [O1]."),
        &ctx
    )
    .is_err());
    assert!(review(
        &submit("L'autonomie de la batterie est de 2 heures [O1]."),
        &ctx
    )
    .is_err());
    assert!(review(
        &submit("La capacité restante de la batterie est de 50 Wh [O1]."),
        &ctx
    )
    .is_ok());
    received[0]["observations"][0]["metric"] = json!("battery_estimated_runtime_windows_seconds");
    received[0]["observations"][0]["value"] = json!(7_200);
    received[0]["observations"][0]["unit"] = json!("seconds");
    let ctx = Context {
        received: &received,
        references: &references,
        plans: &[],
    };
    assert!(review(
        &submit("L'autonomie de la batterie est de 2 heures [O1]."),
        &ctx
    )
    .is_ok());
}
