use super::*;
use crate::answer::quantities::{claims, Bound};

#[test]
fn measured_numbers_pass_in_either_language_and_unit_convention() {
    for text in [
        "Il reste 1,54 Go de mémoire disponible sur 16 Go [O0].",
        "Only 1.54 GiB of 16 GiB is available [O0].",
        "Available memory: 1.65 GB [O0], about 1.5 GB.",
        "Le processeur est occupé à 17,4 % [O0].",
        "Memory is 90 % in use [O0].",
        "Used memory is 14.46 GiB [O0].",
        "Le disque répond en 1,3 ms [O0], sous 3 ms.",
        "Hard faults: 12.4 pages/s [F1].",
        "CPU use varied between 5 % and 9 % over 2 s [O1].",
        "The largest process uses 2.48 GB [O0], about 14 % of the memory.",
        "C: has 64 GB free of 512 GB [O1], 87.5 % full.",
        "The PC reports 16 Go [O0].",
        "chrome.exe uses 2.3 GiB [O0].",
    ] {
        assert_eq!(kinds(text), Vec::<IssueKind>::new(), "{text}");
    }
}

#[test]
fn invented_numbers_and_missing_citations_are_reported() {
    assert_eq!(
        kinds("Windows needs 32 GB [O1]."),
        [IssueKind::UnsupportedNumber]
    );
    assert_eq!(
        kinds("The CPU runs at 3.2 GHz [O1]."),
        [IssueKind::UnsupportedNumber]
    );
    assert_eq!(
        kinds("La température atteint 85 °C [O1]."),
        [IssueKind::UnsupportedNumber]
    );
    assert_eq!(
        kinds("Memory is 90 % in use."),
        [IssueKind::MissingCitation]
    );
    // Counts, versions and identifiers carry no unit and are not checked.
    assert!(kinds("Windows 11 on an RTX 4060 shows proc-1a2b3c4d [O1].").is_empty());
}

#[test]
fn written_numbers_read_like_a_person_writes_them() {
    let values = |text: &str| -> Vec<f64> {
        claims(text)
            .into_iter()
            .flat_map(|c| c.values.into_iter().map(|(v, _)| v))
            .collect()
    };
    assert_eq!(values("1 234 ms"), [1_234.0]);
    assert_eq!(values("1\u{202f}234 ms"), [1_234.0]);
    assert_eq!(values("1,5 ms"), [1.5]);
    assert_eq!(values("1,234 ms"), [1_234.0, 1.234]);
    assert_eq!(values("12.25 %"), [12.25]);
    assert_eq!(values("3Go"), [3e9, 3.0 * 1_073_741_824.0]);
    assert!(values("x86 and F1 and 2026 and version 2.1").is_empty());
    assert_eq!(claims("moins de 3 ms")[0].bound, Bound::AtMost);
    assert_eq!(claims("plus de 2 Go")[0].bound, Bound::AtLeast);
    assert_eq!(claims("environ 80 %")[0].bound, Bound::About);
}

#[test]
fn activity_rates_growth_and_gpu_use_the_matching_measured_dimension() {
    let received = vec![
        json!({"observation_id":"O1", "process_names":{"proc-a":"browser.exe"},
        "measurements":{"values":{"proc-a":{
            "process_working_set_bytes":2_000_000_000_u64,
            "process_working_set_increase_bytes":100_000_000,
            "process_working_set_decrease_bytes":20_000_000,
            "process_io_read_bytes_per_second":5_000_000,
            "process_gpu_basis_points":4200,
            "page_reads_per_second_x100":1_700_000_000_u64}},
        "units":{"process_working_set_bytes":"bytes", "process_working_set_increase_bytes":"bytes",
            "process_working_set_decrease_bytes":"bytes", "process_io_read_bytes_per_second":"bytes_per_second",
            "process_gpu_basis_points":"basis_points_of_engine_time", "page_reads_per_second_x100":"pages_per_second_x100"}}}),
    ];
    let references = refs();
    let ctx = Context {
        received: &received,
        references: &references,
        plans: &[],
    };
    for text in [
        "browser.exe disk I/O: 5 MB/s [O1].",
        "browser.exe memory increased by 100 MB [O1].",
        "browser.exe memory decreased by 20 MB [O1].",
        "browser.exe GPU activity: 42 % [O1].",
    ] {
        assert!(
            review(&submit(text), &ctx).is_ok(),
            "valid activity claim rejected: {text}"
        );
    }
    for text in [
        "browser.exe disk I/O: 17 MB/s [O1].",
        "browser.exe memory increased by 2 GB [O1].",
        "browser.exe memory decreased by 100 MB [O1].",
        "browser.exe memory: 100 MB [O1].",
    ] {
        assert!(
            review(&submit(text), &ctx).is_err(),
            "unmeasured activity claim accepted: {text}"
        );
    }
}

#[test]
fn cpu_and_gpu_sensors_cannot_borrow_each_others_values_or_firmware_temperatures() {
    let readings = [
        ("cpu_frequency_megahertz", 3600, "megahertz"),
        ("cpu_temperature_celsius", 45, "degrees_celsius"),
        ("cpu_power_milliwatts", 65_000, "milliwatts"),
        ("gpu_sm_clock_megahertz", 2100, "megahertz"),
        ("gpu_temperature_celsius", 80, "degrees_celsius"),
        ("gpu_power_milliwatts", 120_000, "milliwatts"),
        ("thermal_zone_temperature_celsius", 65, "degrees_celsius"),
    ];
    let observations: Vec<_> = readings.iter().map(|(metric, value, unit)| json!({
        "observation_id":format!("measurement-{metric}"), "entity_ref":"system", "metric":metric,
        "value":value, "unit":unit, "status":"ok", "source":"fixture",
    })).collect();
    let received = vec![json!({"observation_id":"O1", "observations":observations})];
    let references = refs();
    let ctx = Context {
        received: &received,
        references: &references,
        plans: &[],
    };
    for text in [
        "The CPU frequency is 3.6 GHz [O1].",
        "Le processeur est à 45 °C [O1].",
        "Processor power: 65 W [O1].",
        "GPU frequency: 2100 MHz [O1].",
        "La carte graphique est à 80 °C [O1].",
        "GPU power: 120 W [O1].",
        "The firmware thermal zone reports 65 °C [O1].",
    ] {
        assert!(
            review(&submit(text), &ctx).is_ok(),
            "measured sensor claim rejected: {text}"
        );
    }
    for text in [
        "CPU frequency: 2.1 GHz [O1].",
        "CPU temperature: 80 °C [O1].",
        "Le processeur est à 65 °C [O1].",
        "Processor power: 120 W [O1].",
        "GPU frequency: 3.6 GHz [O1].",
        "GPU temperature: 45 °C [O1].",
        "La carte graphique est à 65 °C [O1].",
        "GPU power: 65 W [O1].",
        "CPU and GPU temperature: 80 °C [O1].",
    ] {
        let (issues, _) = review(&submit(text), &ctx).unwrap_err();
        assert!(
            issues
                .iter()
                .any(|issue| issue.kind == IssueKind::UnsupportedNumber),
            "unrelated sensor supplied the claim: {text}"
        );
    }
}

#[test]
fn c4_rows_require_usable_values_matching_the_named_entity() {
    let row = |entity: &str, value: u64, status: &str| {
        json!({
            "observation_id":"measurement-1", "entity_ref":entity, "metric":"estimated_bytes",
            "value":value, "unit":"bytes", "status":status, "source":"fixture", "measured_at_unix_ms":1,
        })
    };
    let received = vec![
        json!({"observation_id":"O1", "shared_names":{"app-a":"Editor", "app-b":"Player"},
        "observations":[row("app-a", 2_000_000_000, "ok"), row("app-b", 8_000_000_000, "denied")],
        "volume":{"free_bytes":12_000_000_000_u64}}),
    ];
    let references = refs();
    let ctx = Context {
        received: &received,
        references: &references,
        plans: &[],
    };
    assert!(review(&submit("Editor uses 2 GB [O1]."), &ctx).is_ok());
    for text in [
        "Player uses 8 GB [O1].",
        "Player uses 2 GB [O1].",
        "Editor uses 12 GB [O1].",
    ] {
        assert!(
            review(&submit(text), &ctx).is_err(),
            "unsupported C4 claim accepted: {text}"
        );
    }
    let received = vec![json!({"observation_id":"O1", "observations":[{
        "observation_id":"measurement-date", "entity_ref":"system", "metric":"last_update", "value":1234,
        "unit":"unix_ms", "status":"ok", "source":"fixture"}]})];
    let ctx = Context {
        received: &received,
        references: &references,
        plans: &[],
    };
    assert!(review(&submit("The delay was 1234 ms [O1]."), &ctx).is_err());
}

#[test]
fn persisted_history_extrema_and_event_peaks_retain_units_and_entity_scope() {
    let received = vec![
        json!({"observation_id":"O1", "historical":true, "metric":"process_cpu_basis_points", "entity_ref":"proc-a",
        "process_names":{"proc-a":"browser.exe", "proc-b":"other.exe"},
        "points":[{"value":5000,"min":1700,"max":8700,"status":"partial","measured_samples":3,"expected_samples":5}],
        "events":[{"metric":"cpu_busy_basis_points","unit":"basis_points","entity_ref":"system",
            "peak":9900,"started_at_unix_ms":1000,"ended_at_unix_ms":5000,
            "contributor_ref":"proc-b","contributor_value":3600,
            "contributor_metric":"process_cpu_basis_points","contributor_unit":"basis_points_of_machine_time"}]}),
    ];
    let references = refs();
    let ctx = Context {
        received: &received,
        references: &references,
        plans: &[],
    };
    for text in [
        "During recording browser.exe CPU peak: 87 % [O1].",
        "Recorded browser.exe CPU average: 50 % [O1].",
        "At 14:00 browser.exe CPU peak: 87 % [O1].",
        "À 14 h browser.exe : pic CPU de 87 % [O1].",
        "System CPU peak during recording: 99 % [O1].",
        "During recording other.exe CPU: 36 % [O1].",
    ] {
        assert!(
            review(&submit(text), &ctx).is_ok(),
            "measured history rejected: {text}"
        );
    }
    for text in [
        "During recording browser.exe CPU peak: 99 % [O1].",
        "During recording other.exe CPU peak: 99 % [O1].",
        "During recording other.exe CPU peak: 87 % [O1].",
        "During recording browser.exe GPU peak: 87 % [O1].",
        "browser.exe CPU average now: 50 % [O1].",
        "During recording browser.exe CPU is still 50 % [O1].",
    ] {
        assert!(
            review(&submit(text), &ctx).is_err(),
            "history borrowed unrelated measurement: {text}"
        );
    }
}

#[test]
fn historical_event_values_without_measurement_metadata_remain_unverified() {
    let mut received = vec![json!({"observation_id":"O1","historical":true,
        "process_names":{"proc-b":"other.exe"},
        "events":[{"metric":"cpu_busy_basis_points","unit":"basis_points","entity_ref":"system",
            "peak":9900,"started_at_unix_ms":1000,"ended_at_unix_ms":5000,
            "contributor_ref":"proc-b","contributor_value":3600,
            "contributor_metric":"process_cpu_basis_points","contributor_unit":"basis_points",
            "measurement_metadata_available":false,"peak_observation":null,"contributor_observation":null}]})];
    let references = refs();
    for text in [
        "System CPU peak during recording: 99 % [O1].",
        "During recording other.exe CPU: 36 % [O1].",
    ] {
        let context = Context {
            received: &received,
            references: &references,
            plans: &[],
        };
        assert!(review(&submit(text), &context).is_err(), "{text}");
    }
    received[0]["events"][0]["measurement_metadata_available"] = json!(true);
    received[0]["events"][0]["peak_observation"] = json!({"observation_id":"event-peak","entity_ref":"system",
        "metric":"cpu_busy_basis_points","unit":"basis_points","value":9900,"status":"ok"});
    received[0]["events"][0]["contributor_observation"] = json!({"observation_id":"event-contributor","entity_ref":"proc-b",
        "metric":"process_cpu_basis_points","unit":"basis_points","value":3600,"status":"ok"});
    for text in [
        "System CPU peak during recording: 99 % [O1].",
        "During recording other.exe CPU: 36 % [O1].",
    ] {
        let context = Context {
            received: &received,
            references: &references,
            plans: &[],
        };
        assert!(review(&submit(text), &context).is_ok(), "{text}");
    }
    let context = Context {
        received: &received,
        references: &references,
        plans: &[],
    };
    assert!(review(
        &submit("During recording other.exe CPU: 99 % [O1]."),
        &context
    )
    .is_err());
}
