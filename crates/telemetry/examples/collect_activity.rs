//! Local read-only activity capture; the explicit output file must not already exist.
#![forbid(unsafe_code)]
use pcagent_telemetry::{current_resource_usage, Collector, DEFAULT_ACTIVITY_WINDOW_SECONDS};
use std::{
    fs::OpenOptions,
    io::{BufWriter, Write},
    time::Instant,
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let output = std::env::args_os()
        .nth(1)
        .ok_or("usage: collect_activity <local-output.json>")?;
    let file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&output)?;
    let mut collector = Collector::new();
    let before = current_resource_usage();
    let started = Instant::now();
    let activity = collector.process_activity(DEFAULT_ACTIVITY_WINDOW_SECONDS, 50);
    let elapsed_ms = started.elapsed().as_millis();
    let after = current_resource_usage();
    let mut writer = BufWriter::new(file);
    serde_json::to_writer_pretty(&mut writer, &activity)?;
    writer.flush()?;
    let cpu_ms = before
        .thread_cpu_100ns
        .zip(after.thread_cpu_100ns)
        .and_then(|(first, last)| last.checked_sub(first))
        .map(|ticks| ticks as f64 / 10_000.0);
    println!(
        "{}",
        serde_json::json!({
            "status":activity.status,"processes_enumerated":activity.coverage.processes.enumerated,
            "processes_readable":activity.coverage.processes.readable,"stable_instances":activity.coverage.stable_instances,
            "services_attributed":activity.coverage.services_attributed,"gpu_processes":activity.coverage.gpu_processes,
            "gpu_status":activity.coverage.gpu_status,"applications_total":activity.coverage.applications_total,
            "window_ms":activity.window.duration_ms,"elapsed_ms":elapsed_ms,"collector_thread_cpu_ms":cpu_ms,
            "whole_process_working_set_bytes":after.process_working_set_bytes,
            "note":"Working set covers this entire standalone process; CPU covers this calling thread. Raw names and local identities remain only in the explicit local capture file."
        })
    );
    Ok(())
}
