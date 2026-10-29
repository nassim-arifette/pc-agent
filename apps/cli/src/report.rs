//! Local daily report: one point-in-time snapshot, the local rules, a dated file. No network,
//! no model, no administrator.
use crate::Result;
use pcagent_diagnostics::{analyze, Classification, Finding};
use pcagent_shared::unix_ms;
use pcagent_telemetry::Collector;
use serde_json::json;
use std::fs;
use std::path::{Path, PathBuf};

const KEEP_DAYS: usize = 30;
const PREFIX: &str = "rapport-";

/// `YYYY-MM-DD` (UTC) from a Unix time in milliseconds, without a calendar dependency.
pub(super) fn date_stamp(unix_ms: u64) -> String {
    let days = (unix_ms / 86_400_000) as i64;
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}")
}

fn needs_attention(finding: &Finding) -> bool {
    !matches!(
        finding.classification,
        Classification::NoContentionObserved | Classification::InsufficientEvidence
    )
}

pub(super) fn markdown(date: &str, scheduled: bool, findings: &[Finding]) -> String {
    let attention: Vec<&Finding> = findings.iter().filter(|f| needs_attention(f)).collect();
    let mut out = format!("# PC Agent report, {date}\n\n");
    if attention.is_empty() {
        out.push_str("Nothing needs attention in this measurement.\n\n");
    } else {
        out.push_str(&format!("{} point(s) to review.\n\n", attention.len()));
    }
    for finding in &attention {
        out.push_str(&format!("## {}\n\n", finding.summary));
        for fact in &finding.facts {
            out.push_str(&format!("- {}\n", fact.text));
        }
        out.push_str(&format!("\nNext step: {}\n\n", finding.next_step));
    }
    out.push_str(
        "Coverage: a single measurement taken when this report ran. History is incomplete when \
         PC Agent was closed, so no trend is claimed.\n",
    );
    if scheduled {
        out.push_str("Run by the daily scheduled task; nothing was sent anywhere.\n");
    }
    out
}

/// Keeps the newest `KEEP_DAYS` days of report files; names sort by date.
fn rotate(dir: &Path) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    let mut names: Vec<String> = entries
        .flatten()
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|n| n.starts_with(PREFIX))
        .collect();
    names.sort();
    let days: Vec<&str> = names
        .iter()
        .map(|n| &n[..n.rfind('.').unwrap_or(n.len())])
        .collect();
    let mut unique: Vec<&str> = days;
    unique.dedup();
    if unique.len() > KEEP_DAYS {
        let old: Vec<&str> = unique[..unique.len() - KEEP_DAYS].to_vec();
        for name in names
            .iter()
            .filter(|n| old.iter().any(|o| n.starts_with(*o)))
        {
            let _ = fs::remove_file(dir.join(name));
        }
    }
}

fn write_atomic(path: &Path, contents: &str) -> Result<()> {
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, contents)?;
    fs::rename(&tmp, path)?;
    Ok(())
}

pub(super) fn default_dir() -> Option<PathBuf> {
    Some(
        pcagent_agent::credentials::file_path()?
            .parent()?
            .join("reports"),
    )
}

pub(super) fn run(options: crate::args::Report) -> Result<u8> {
    let dir = options
        .out_dir
        .or_else(default_dir)
        .ok_or("No local directory is available for the report.")?;
    fs::create_dir_all(&dir)?;
    let now = unix_ms();
    let date = date_stamp(now);
    let mut collector = Collector::new();
    let snapshot = collector.snapshot(false, 0);
    let findings = analyze(&snapshot);
    let attention = findings.iter().filter(|f| needs_attention(f)).count();
    let document = json!({
        "schema_version": 1,
        "generated_at_unix_ms": now,
        "scheduled": options.scheduled,
        "points_to_review": attention,
        "coverage": "single point-in-time measurement; history may be incomplete",
        "findings": findings,
    });
    write_atomic(
        &dir.join(format!("{PREFIX}{date}.json")),
        &serde_json::to_string_pretty(&document)?,
    )?;
    write_atomic(
        &dir.join(format!("{PREFIX}{date}.md")),
        &markdown(&date, options.scheduled, &findings),
    )?;
    rotate(&dir);
    println!("Report written: {} point(s) to review.", attention);
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates_match_known_days() {
        assert_eq!(date_stamp(0), "1970-01-01");
        assert_eq!(date_stamp(1_782_777_600_000), "2026-06-30");
        assert_eq!(date_stamp(1_793_232_000_000), "2026-10-29");
        assert_eq!(date_stamp(951_782_400_000), "2000-02-29");
    }

    #[test]
    fn rotation_keeps_the_newest_thirty_days() {
        let dir = std::env::temp_dir().join(format!("pcagent-report-{}", unix_ms()));
        fs::create_dir_all(&dir).unwrap();
        for day in 1..=35 {
            for ext in ["json", "md"] {
                fs::write(dir.join(format!("{PREFIX}2026-01-{day:02}.{ext}")), "x").unwrap();
            }
        }
        rotate(&dir);
        let left = fs::read_dir(&dir).unwrap().count();
        assert_eq!(left, 60);
        assert!(!dir.join(format!("{PREFIX}2026-01-05.md")).exists());
        assert!(dir.join(format!("{PREFIX}2026-01-06.md")).exists());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn report_runs_offline_and_writes_both_files() {
        let dir = std::env::temp_dir().join(format!("pcagent-report-run-{}", unix_ms()));
        run(crate::args::Report {
            scheduled: true,
            out_dir: Some(dir.clone()),
        })
        .unwrap();
        let names: Vec<String> = fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        assert!(names.iter().any(|n| n.ends_with(".json")));
        assert!(names.iter().any(|n| n.ends_with(".md")));
        fs::remove_dir_all(&dir).unwrap();
    }
}
