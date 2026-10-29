//! The daily local report (plan, chantier 17): one point-in-time measurement, the local rules,
//! what the local history and watches recorded in the last 24 hours, written as dated JSON and
//! Markdown files. No network, no model and no administrator; nothing leaves the PC.
use pcagent_diagnostics::{analyze, Classification, Finding};
use pcagent_shared::presentation::Language;
use pcagent_shared::{format_utc_minute, Quantity};
use pcagent_store::Store;
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};

pub const SCHEMA_VERSION: u32 = 1;
/// Days of reports kept; older files are removed after each write.
pub const KEEP_DAYS: usize = 30;
const PREFIX: &str = "rapport-";
const DAY_MS: u64 = 86_400_000;
/// A sample older than this means the app (and its sampler) was not running recently.
const RECENT_HISTORY_MS: u64 = 26 * 3_600_000;
const MAX_ALERTS: u64 = 50;
const MAX_REPORT_BYTES: u64 = 2 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HistoryStatus {
    /// Samples from the last 26 hours exist: the app was open recently.
    Recent,
    /// Only older samples: no trend for today can be claimed.
    Stale,
    /// No local history at all.
    Missing,
    /// The history file exists but could not be read.
    Unavailable,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HistoryContext {
    pub status: HistoryStatus,
    pub latest_sample_unix_ms: Option<u64>,
    /// Activity events (CPU, memory or disk pressure) recorded in the last 24 hours.
    pub events_last_day: Option<u64>,
}

/// One watch alert of the last 24 hours, as recorded by the app while it ran.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AlertSummary {
    pub id: String,
    /// Closed rule target (`resource:Cpu`, `volume:C:\`, `app_memory:name.exe`, ...).
    pub target: String,
    pub started_at_unix_ms: u64,
    pub ended_at_unix_ms: Option<u64>,
    pub metric: String,
    pub extreme_value: u64,
    pub unit: String,
    pub lower_bound: bool,
    pub close_reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Report {
    pub schema_version: u32,
    pub generated_at_unix_ms: u64,
    /// UTC calendar day of the run, `YYYY-MM-DD`; also the file name.
    pub date: String,
    pub scheduled: bool,
    pub points_to_review: usize,
    pub findings: Vec<Finding>,
    pub history: HistoryContext,
    pub alerts: Vec<AlertSummary>,
    pub alerts_total: u64,
}

/// `YYYY-MM-DD` (UTC) from a Unix time in milliseconds.
pub fn date_stamp(unix_ms: u64) -> String {
    format_utc_minute(unix_ms)[..10].to_owned()
}

pub fn needs_attention(finding: &Finding) -> bool {
    !matches!(
        finding.classification,
        Classification::NoContentionObserved | Classification::InsufficientEvidence
    )
}

/// What the local history and the watches recorded in the 24 hours before `now`.
pub fn read_history(store: &Store, now: u64) -> (HistoryContext, Vec<AlertSummary>, u64) {
    let unavailable = HistoryContext {
        status: HistoryStatus::Unavailable,
        latest_sample_unix_ms: None,
        events_last_day: None,
    };
    let from = now.saturating_sub(DAY_MS);
    let history = match store.history_overview(from, now) {
        Ok(overview) => {
            let latest = overview.latest_sample_unix_ms;
            HistoryContext {
                status: match latest {
                    None => HistoryStatus::Missing,
                    Some(at) if now.saturating_sub(at) <= RECENT_HISTORY_MS => {
                        HistoryStatus::Recent
                    }
                    Some(_) => HistoryStatus::Stale,
                },
                latest_sample_unix_ms: latest,
                events_last_day: overview
                    .periods
                    .iter()
                    .find(|p| p.id == "day")
                    .map(|p| p.event_count),
            }
        }
        Err(_) => unavailable,
    };
    let (alerts, total) = match store.watch_alerts(from, now, MAX_ALERTS, 0) {
        Ok(page) => (
            page.alerts
                .into_iter()
                .map(|alert| AlertSummary {
                    target: alert.rule.target(),
                    started_at_unix_ms: alert.started_at_unix_ms,
                    ended_at_unix_ms: alert.ended_at_unix_ms,
                    metric: alert.extreme_observation.metric.clone(),
                    extreme_value: alert.extreme_observation.value,
                    unit: alert.extreme_observation.unit.clone(),
                    lower_bound: alert.lower_bound,
                    close_reason: alert.close_reason.and_then(|reason| {
                        serde_json::to_value(reason)
                            .ok()
                            .and_then(|v| v.as_str().map(str::to_owned))
                    }),
                    id: alert.id,
                })
                .collect(),
            page.total,
        ),
        Err(_) => (Vec::new(), 0),
    };
    (history, alerts, total)
}

pub fn assemble(
    findings: Vec<Finding>,
    (history, alerts, alerts_total): (HistoryContext, Vec<AlertSummary>, u64),
    now: u64,
    scheduled: bool,
) -> Report {
    Report {
        schema_version: SCHEMA_VERSION,
        generated_at_unix_ms: now,
        date: date_stamp(now),
        scheduled,
        points_to_review: findings.iter().filter(|f| needs_attention(f)).count()
            + usize::from(alerts_total > 0),
        findings,
        history,
        alerts,
        alerts_total,
    }
}

/// Measures now and reads the local history at `database` when that file already exists; a
/// missing history is reported, never created.
pub fn collect(database: Option<&Path>, now: u64, scheduled: bool) -> Report {
    let mut collector = pcagent_telemetry::Collector::new();
    let findings = analyze(&collector.snapshot(true, 5));
    let history = match database.filter(|path| path.is_file()) {
        None => (
            HistoryContext {
                status: HistoryStatus::Missing,
                latest_sample_unix_ms: None,
                events_last_day: None,
            },
            Vec::new(),
            0,
        ),
        Some(path) => match Store::open(path) {
            Ok(store) => read_history(&store, now),
            Err(_) => (
                HistoryContext {
                    status: HistoryStatus::Unavailable,
                    latest_sample_unix_ms: None,
                    events_last_day: None,
                },
                Vec::new(),
                0,
            ),
        },
    };
    assemble(findings, history, now, scheduled)
}

struct Words {
    title: &'static str,
    none: &'static str,
    some: &'static str,
    next: &'static str,
    alerts: &'static str,
    more_alerts: &'static str,
    open: &'static str,
    lower_bound: &'static str,
    recent: &'static str,
    stale: &'static str,
    missing: &'static str,
    unavailable: &'static str,
    events: &'static str,
    coverage: &'static str,
    scheduled: &'static str,
}

const EN: Words = Words {
    title: "PC Agent report",
    none: "Nothing needs attention in this measurement.",
    some: "point(s) to review.",
    next: "Next step:",
    alerts: "Watch alerts in the last 24 hours",
    more_alerts: "more alert(s) are in the app's history.",
    open: "still open",
    lower_bound: "lower bound",
    recent: "Local history: recorded in the last 24 hours.",
    stale: "Local history: the last sample is older than a day; PC Agent was closed, so no trend is claimed.",
    missing: "Local history: none yet; PC Agent records it while the app is open.",
    unavailable: "Local history: could not be read this time.",
    events: "activity event(s) recorded in the last 24 hours.",
    coverage: "Coverage: one measurement taken when this report ran. A single sample cannot prove a sustained problem.",
    scheduled: "Written by the daily scheduled task. Nothing was sent anywhere.",
};

const FR: Words = Words {
    title: "Rapport PC Agent",
    none: "Rien ne demande votre attention dans cette mesure.",
    some: "point(s) à voir.",
    next: "Étape suivante :",
    alerts: "Alertes de surveillance des dernières 24 heures",
    more_alerts: "autre(s) alerte(s) dans l’historique de l’app.",
    open: "toujours en cours",
    lower_bound: "borne basse",
    recent: "Historique local : enregistré dans les dernières 24 heures.",
    stale: "Historique local : la dernière mesure date de plus d’un jour ; PC Agent était fermé, aucune tendance n’est donc affirmée.",
    missing: "Historique local : aucun pour l’instant ; PC Agent l’enregistre tant que l’app est ouverte.",
    unavailable: "Historique local : illisible cette fois-ci.",
    events: "événement(s) d’activité enregistré(s) dans les dernières 24 heures.",
    coverage: "Couverture : une seule mesure, prise au moment du rapport. Un seul relevé ne prouve pas un problème durable.",
    scheduled: "Écrit par la tâche planifiée quotidienne. Rien n’a été envoyé.",
};

fn words(language: Language) -> &'static Words {
    match language {
        Language::En => &EN,
        Language::Fr => &FR,
    }
}

fn localized(language: Language, text: &str) -> String {
    pcagent_diagnostics::phrase(text)
        .and_then(|phrase| language.render(&phrase))
        .unwrap_or_else(|| text.to_owned())
}

fn classification(language: Language, classification: Classification) -> String {
    serde_json::to_value(classification)
        .ok()
        .and_then(|code| {
            language
                .label(&format!("classification.{}", code.as_str()?))
                .map(str::to_owned)
        })
        .unwrap_or_else(|| classification.label().to_owned())
}

fn value(language: Language, value: u64, unit: &str) -> String {
    let quantity = match unit {
        "bytes" => Some(Quantity::Bytes(value)),
        "bytes_per_second" => Some(Quantity::BytesPerSecond(value)),
        unit if unit.starts_with("basis_points") => Some(Quantity::BasisPoints(value)),
        "microseconds" => Some(Quantity::Microseconds(value)),
        "degrees_celsius" => Some(Quantity::Celsius(value)),
        _ => None,
    };
    quantity
        .and_then(|q| language.quantity(&q))
        .unwrap_or_else(|| format!("{value} {unit}"))
}

/// Human-readable copy. It carries no user name, command line or profile path: findings
/// render from closed phrases, alerts from closed rule targets.
pub fn markdown(report: &Report, language: Language) -> String {
    let w = words(language);
    let mut out = format!("# {}, {}\n\n", w.title, report.date);
    if report.points_to_review == 0 {
        out.push_str(&format!("{}\n\n", w.none));
    } else {
        out.push_str(&format!("{} {}\n\n", report.points_to_review, w.some));
    }
    for finding in report.findings.iter().filter(|f| needs_attention(f)) {
        out.push_str(&format!(
            "## {}\n\n{}\n\n",
            classification(language, finding.classification),
            localized(language, &finding.summary)
        ));
        for fact in &finding.facts {
            let text = fact
                .phrase
                .as_ref()
                .and_then(|p| language.render(p))
                .unwrap_or_else(|| fact.text.clone());
            out.push_str(&format!("- {text}\n"));
        }
        if !finding.next_step.is_empty() {
            out.push_str(&format!(
                "\n{} {}\n\n",
                w.next,
                localized(language, &finding.next_step)
            ));
        }
    }
    if !report.alerts.is_empty() {
        out.push_str(&format!("## {}\n\n", w.alerts));
        for alert in &report.alerts {
            let end = match alert.ended_at_unix_ms {
                Some(at) => format_utc_minute(at),
                None => w.open.to_owned(),
            };
            out.push_str(&format!(
                "- {} · {} → {} · {}{}\n",
                alert.target,
                format_utc_minute(alert.started_at_unix_ms),
                end,
                value(language, alert.extreme_value, &alert.unit),
                if alert.lower_bound {
                    format!(" ({})", w.lower_bound)
                } else {
                    String::new()
                }
            ));
        }
        let more = report
            .alerts_total
            .saturating_sub(report.alerts.len() as u64);
        if more > 0 {
            out.push_str(&format!("- {more} {}\n", w.more_alerts));
        }
        out.push('\n');
    }
    out.push_str(match report.history.status {
        HistoryStatus::Recent => w.recent,
        HistoryStatus::Stale => w.stale,
        HistoryStatus::Missing => w.missing,
        HistoryStatus::Unavailable => w.unavailable,
    });
    out.push('\n');
    if let Some(events) = report.history.events_last_day {
        out.push_str(&format!("{events} {}\n", w.events));
    }
    out.push_str(&format!("\n{}\n", w.coverage));
    if report.scheduled {
        out.push_str(&format!("{}\n", w.scheduled));
    }
    out
}

fn write_atomic(path: &Path, contents: &[u8]) -> io::Result<()> {
    let tmp = path.with_file_name(format!(
        ".{}.{}.tmp",
        path.file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("report"),
        uuid::Uuid::new_v4()
    ));
    fs::write(&tmp, contents)?;
    fs::rename(&tmp, path).inspect_err(|_| {
        let _ = fs::remove_file(&tmp);
    })
}

/// Day stem (`rapport-YYYY-MM-DD`) of a report file name, if it is one.
fn day_of(name: &str) -> Option<&str> {
    let stem = name
        .strip_suffix(".json")
        .or_else(|| name.strip_suffix(".md"))?;
    let date = stem.strip_prefix(PREFIX)?;
    let valid = date.len() == 10
        && date.bytes().enumerate().all(|(i, b)| {
            if i == 4 || i == 7 {
                b == b'-'
            } else {
                b.is_ascii_digit()
            }
        });
    valid.then_some(stem)
}

/// Keeps the newest [`KEEP_DAYS`] days; touches only files named like reports.
fn rotate(dir: &Path) -> io::Result<()> {
    let mut names: Vec<String> = fs::read_dir(dir)?
        .flatten()
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|n| day_of(n).is_some())
        .collect();
    names.sort();
    let mut days: Vec<&str> = names.iter().filter_map(|n| day_of(n)).collect();
    days.dedup();
    if days.len() > KEEP_DAYS {
        let old = &days[..days.len() - KEEP_DAYS];
        for name in names
            .iter()
            .filter(|n| day_of(n).is_some_and(|d| old.contains(&d)))
        {
            fs::remove_file(dir.join(name))?;
        }
    }
    Ok(())
}

/// Writes `rapport-<date>.json` and `.md` (a rerun on the same day replaces them), then rotates.
pub fn write(dir: &Path, report: &Report, language: Language) -> io::Result<PathBuf> {
    fs::create_dir_all(dir)?;
    let stem = format!("{PREFIX}{}", report.date);
    let json = dir.join(format!("{stem}.json"));
    write_atomic(&json, &serde_json::to_vec_pretty(report)?)?;
    write_atomic(
        &dir.join(format!("{stem}.md")),
        markdown(report, language).as_bytes(),
    )?;
    // Removing old days is housekeeping: today's report is already written either way.
    let _ = rotate(dir);
    Ok(json)
}

/// The newest readable report, bounded in size; a damaged newest file is skipped.
pub fn latest(dir: &Path) -> io::Result<Option<Report>> {
    Ok(latest_file(dir)?.map(|(_, report)| report))
}

/// The newest readable report with the path of its Markdown copy. The path comes from the
/// validated file name, never from the report's own content.
pub fn latest_file(dir: &Path) -> io::Result<Option<(PathBuf, Report)>> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    let mut names: Vec<String> = entries
        .flatten()
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|n| n.ends_with(".json") && day_of(n).is_some())
        .collect();
    names.sort();
    for name in names.iter().rev() {
        let mut text = String::new();
        let Ok(file) = fs::File::open(dir.join(name)) else {
            continue;
        };
        if file
            .take(MAX_REPORT_BYTES + 1)
            .read_to_string(&mut text)
            .is_err()
            || text.len() as u64 > MAX_REPORT_BYTES
        {
            continue;
        }
        if let Ok(report) = serde_json::from_str::<Report>(&text) {
            if report.schema_version == SCHEMA_VERSION {
                let stem = day_of(name).unwrap_or_default();
                return Ok(Some((dir.join(format!("{stem}.md")), report)));
            }
        }
    }
    Ok(None)
}

#[cfg(test)]
mod tests;
