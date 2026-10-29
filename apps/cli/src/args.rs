//! CLI argument contracts; parsing grants no cloud or system authority.
use clap::{Parser, Subcommand};
use std::path::PathBuf;

const EXAMPLES: &str = r#"Examples:
  pcagent snapshot
  pcagent diagnose "Why is my PC slow?"
  pcagent diagnose "Why is my PC slow?" --allow-cloud --verbose
  pcagent chat --allow-cloud
  pcagent storage "$env:USERPROFILE\Downloads" --limit 10
  pcagent doctor --check-cloud

Nothing is sent to the cloud without --allow-cloud, and nothing on your PC is changed unless you
approve a proposed action (diagnose --allow-cloud --propose-actions)."#;
#[derive(Parser)]
#[command(
    name = "pcagent",
    version,
    about = "Windows investigation prototype: local diagnosis and explicitly approved actions.",
    after_help = EXAMPLES
)]
pub(super) struct Args {
    /// Language for local human-readable output; JSON and model/user prose remain unchanged.
    #[arg(long, global = true, default_value = "en")]
    pub(super) language: pcagent_shared::presentation::Language,
    /// Print machine-readable JSON instead of text (byte counts are 64-bit integers).
    #[arg(long, global = true)]
    pub(super) json: bool,
    /// Model routing for cloud diagnoses. Configured preserves the existing model choice.
    #[arg(long, global = true, default_value = "configured", value_parser = ["configured", "super", "ultra", "automatic"])]
    pub(super) routing: String,
    #[arg(long, global = true)]
    pub(super) super_model: Option<String>,
    #[arg(long, global = true)]
    pub(super) ultra_model: Option<String>,
    /// Local human-reviewed paired Nemotron calibration; required for automatic routing.
    #[arg(long, global = true)]
    pub(super) routing_calibration: Option<PathBuf>,
    #[command(subcommand)]
    pub(super) command: Command,
}
#[derive(Subcommand)]
pub(super) enum Command {
    /// Write today's local report (one measurement, local rules; nothing is sent anywhere).
    Report(Report),
    /// Manage the optional daily scheduled report, which runs even when the app is closed.
    Schedule {
        #[command(subcommand)]
        action: ScheduleAction,
    },
    /// Review restoring one startup entry previously disabled by a completed local action.
    RestoreStartup(RestoreStartup),
    /// Prepare one fixed read-only elevated inspection; execution requires local approval.
    ElevatedInspect(ElevatedInspect),
    /// List the NVIDIA Nemotron models actually available to the saved account.
    Models(Models),
    /// Run a closed-schema read-only tool locally, without a provider request.
    Inspect(Inspect),
    /// Read persisted measurements, including windows before the last app restart.
    History(History),
    /// Record a problem at one-second cadence, then print its local measurements.
    Record(Record),
    /// Collect at five-second cadence and report the sampler's measured footprint.
    Sample(Sample),
    /// Measure memory, CPU, disk and GPU now and list the biggest processes.
    Snapshot(Snapshot),
    /// List readable processes ranked by a selected resource. Never terminates a process.
    Processes(Processes),
    /// Find the largest files or folders (metadata only unless --compare-content; no links).
    Storage(Storage),
    /// Check this PC's support and the cloud configuration (offline unless --check-cloud).
    Doctor(Doctor),
    /// Answer a question such as "Why is my PC slow?". Local analysis unless --allow-cloud.
    Diagnose(Diagnose),
    /// Talk with the model about this PC: each question can follow up on the previous answers.
    /// Type an empty line or `exit` to stop. Conversation text stays in this session;
    /// local performance samples use the same persistent history as the desktop app.
    Chat(Chat),
    /// Create the clearly labelled synthetic cache used to demonstrate the cleanup adapter.
    DemoCache(DemoCache),
}

#[derive(clap::Args)]
pub(super) struct RestoreStartup {
    pub(super) source_plan_id: String,
    #[arg(long, default_value_t = 0)]
    pub(super) item_index: usize,
    #[arg(long)]
    pub(super) journal_dir: Option<PathBuf>,
}

#[derive(clap::Args)]
pub(super) struct ElevatedInspect {
    #[arg(value_parser = ["system-caches", "mft", "shadow-storage", "disk-reliability", "process-image", "battery-energy", "performance-trace"])]
    pub(super) task: String,
    /// One uppercase local drive letter for the bounded native MFT inventory.
    #[arg(long, default_value = "C")]
    pub(super) volume: String,
    /// A local process identity; PID alone is insufficient because Windows can reuse it.
    #[arg(long, requires = "creation_time_filetime")]
    pub(super) pid: Option<u32>,
    #[arg(long, requires = "pid")]
    pub(super) creation_time_filetime: Option<u64>,
    /// Fixed ETW duration; collection remains gated until hard-exit cleanup is validated.
    #[arg(long, value_parser = parse_trace_duration)]
    pub(super) duration_seconds: Option<u8>,
    #[arg(long, default_value_t = 120, value_parser = clap::value_parser!(u64).range(1..=120))]
    pub(super) timeout_seconds: u64,
    /// Explicit local approval after reading the task; then Windows asks for UAC consent.
    #[arg(long)]
    pub(super) approve_local: bool,
}

#[derive(clap::Args)]
pub(super) struct Models {
    /// Authorize a catalog request with the local key; no measurements are sent.
    #[arg(long)]
    pub(super) allow_cloud: bool,
}

#[derive(clap::Args)]
pub(super) struct Inspect {
    pub(super) tool: String,
    #[arg(long, default_value = "{}")]
    pub(super) arguments: String,
    #[arg(long)]
    pub(super) anonymize: bool,
    /// Local history database; defaults to the desktop app's database.
    #[arg(long)]
    pub(super) database: Option<PathBuf>,
}

#[derive(clap::Args)]
pub(super) struct History {
    #[arg(long, default_value = "cpu_busy_basis_points")]
    pub(super) metric: String,
    #[arg(long, default_value = "system")]
    pub(super) entity_ref: String,
    #[arg(long, default_value_t = 3600, value_parser=clap::value_parser!(u64).range(1..=604_800))]
    pub(super) window_seconds: u64,
    #[arg(long)]
    pub(super) end_unix_ms: Option<u64>,
    #[arg(long)]
    pub(super) database: Option<PathBuf>,
}

#[derive(clap::Args)]
pub(super) struct Record {
    #[arg(long, default_value_t = 30, value_parser=clap::value_parser!(u64).range(30..=120))]
    pub(super) seconds: u64,
    #[arg(long)]
    pub(super) database: Option<PathBuf>,
}

#[derive(clap::Args)]
pub(super) struct Sample {
    #[arg(long, default_value_t = 60, value_parser=clap::value_parser!(u64).range(10..=3600))]
    pub(super) seconds: u64,
    #[arg(long)]
    pub(super) database: Option<PathBuf>,
}

#[derive(clap::Args)]
pub(super) struct Snapshot {
    /// Number of processes to show (1-50).
    #[arg(long,default_value_t=10,value_parser=clap::value_parser!(u16).range(1..=50))]
    pub(super) limit: u16,
}

#[derive(clap::Args)]
pub(super) struct Processes {
    /// Number of processes to show (1-50).
    #[arg(long,default_value_t=10,value_parser=clap::value_parser!(u16).range(1..=50))]
    pub(super) limit: u16,
    #[arg(long, default_value="working_set", value_parser=["working_set", "private_commit", "cpu", "io_read", "io_write"])]
    pub(super) sort_by: String,
}

#[derive(clap::Args)]
pub(super) struct Storage {
    /// Folder to scan.
    pub(super) root: PathBuf,
    /// Number of items to show (1-100).
    #[arg(long,default_value_t=20,value_parser=clap::value_parser!(u16).range(1..=100))]
    pub(super) limit: u16,
    /// Rank directories instead of files.
    #[arg(long)]
    pub(super) dirs: bool,
    /// Read file contents locally to find same-content copies (bounded; no deletion).
    #[arg(long)]
    pub(super) compare_content: bool,
}

#[derive(clap::Args)]
pub(super) struct Doctor {
    /// Authorize a catalog request to Nebius using your key; sends no telemetry.
    #[arg(long)]
    pub(super) check_cloud: bool,
}

#[derive(clap::Args)]
pub(super) struct Diagnose {
    /// Your question, in quotes.
    pub(super) question: String,
    /// Explicitly keep this investigation offline (also the default).
    #[arg(long, conflicts_with = "allow_cloud")]
    pub(super) offline: bool,
    /// Send the sanitized question and minimized observations to Nebius for this run.
    #[arg(long)]
    pub(super) allow_cloud: bool,
    /// Inspect the initial cloud payload locally, without sending it. Later samples may differ.
    #[arg(long,conflicts_with_all=["allow_cloud","offline"])]
    pub(super) preview_cloud: bool,
    /// Also show each completed tool call, evidence IDs and collection notes (never model
    /// reasoning or secrets).
    #[arg(long)]
    pub(super) verbose: bool,
    /// Authorize a folder the model may scan (metadata only), as an opaque scope. Repeatable.
    #[arg(long)]
    pub(super) scan_root: Vec<PathBuf>,
    /// Read contents locally in --scan-root folders to compare copies (256 MiB total).
    #[arg(long, requires = "scan_root")]
    pub(super) compare_content: bool,
    /// Allow selected basenames to be sent; use --preview-cloud to review them offline.
    #[arg(long, requires = "scan_root", conflicts_with_all = ["offline", "anonymize"])]
    pub(super) share_file_names: bool,
    /// Send opaque process references instead of executable names (for example chrome.exe).
    /// Paths, PIDs and your user name are never sent either way.
    #[arg(long, conflicts_with = "offline")]
    pub(super) anonymize: bool,
    /// JSON preferences file (exclusions, preserved apps, permissions).
    #[arg(long)]
    pub(super) preferences: Option<PathBuf>,
    /// Let the model propose action plans; each one is shown here for explicit approval.
    #[arg(long)]
    pub(super) propose_actions: bool,
    /// Directory for the local action journal (default: %LOCALAPPDATA%\pcagent\journal).
    #[arg(long)]
    pub(super) journal_dir: Option<PathBuf>,
}

#[derive(clap::Args)]
pub(super) struct Chat {
    /// Send each sanitized question and minimized observations to Nebius (required).
    #[arg(long)]
    pub(super) allow_cloud: bool,
    /// Send opaque process references instead of executable names (for example chrome.exe).
    #[arg(long)]
    pub(super) anonymize: bool,
    /// Also show each completed tool call, evidence IDs and collection notes.
    #[arg(long)]
    pub(super) verbose: bool,
}

#[derive(clap::Args)]
pub(super) struct DemoCache {
    /// Size of the synthetic cache in MiB (1-256).
    #[arg(long,default_value_t=64,value_parser=clap::value_parser!(u16).range(1..=256))]
    pub(super) mb: u16,
}

fn parse_trace_duration(text: &str) -> std::result::Result<u8, String> {
    match text {
        "30" => Ok(30),
        "60" => Ok(60),
        _ => Err("ETW duration must be 30 or 60 seconds.".into()),
    }
}

#[derive(clap::Args)]
pub(super) struct Report {
    /// Marks a run started by the scheduled task.
    #[arg(long)]
    pub(super) scheduled: bool,
    /// Write here instead of the private application directory.
    #[arg(long)]
    pub(super) out_dir: Option<PathBuf>,
}

#[derive(Subcommand)]
pub(super) enum ScheduleAction {
    /// Show whether the daily task is installed.
    Status,
    /// Install the daily task for this user only (no administrator, no network).
    Enable {
        #[arg(long, default_value = "09:00")]
        time: String,
    },
    /// Remove the daily task.
    Disable,
}
