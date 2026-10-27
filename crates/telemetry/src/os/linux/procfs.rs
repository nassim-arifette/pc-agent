use pcagent_shared::{ProcessCoverage, Status};
use std::{
    collections::{BTreeMap, HashSet},
    fs,
    io::{self, Read},
    path::Path,
};

pub(super) fn read(path: &Path) -> io::Result<String> {
    let mut text = String::new();
    fs::File::open(path)?
        .take(1_048_577)
        .read_to_string(&mut text)?;
    if text.len() > 1_048_576 {
        return Err(io::ErrorKind::InvalidData.into());
    }
    Ok(text)
}

pub(super) fn fields(text: &str) -> BTreeMap<&str, u64> {
    text.lines()
        .filter_map(|line| {
            let (key, value) = line.split_once(':')?;
            let mut parts = value.split_whitespace();
            let n = parts.next()?.parse::<u64>().ok()?;
            let n = match parts.next() {
                Some("kB") => n.checked_mul(1024)?,
                None => n,
                _ => return None,
            };
            Some((key, n))
        })
        .collect()
}

#[derive(Clone, Debug)]
pub(super) struct Cpu {
    pub total: u64,
    pub idle: u64,
}
pub(super) fn cpus(text: &str) -> BTreeMap<String, Cpu> {
    text.lines()
        .filter_map(|line| {
            let mut parts = line.split_whitespace();
            let key = parts.next()?;
            if key != "cpu" && !key.strip_prefix("cpu")?.bytes().all(|b| b.is_ascii_digit()) {
                return None;
            }
            // guest and guest_nice are already included in user and nice.
            let ticks = parts
                .take(8)
                .map(str::parse)
                .collect::<Result<Vec<u64>, _>>()
                .ok()?;
            if ticks.len() < 4 {
                return None;
            }
            let total = ticks.iter().try_fold(0u64, |s, n| s.checked_add(*n))?;
            let idle = ticks[3].checked_add(ticks.get(4).copied().unwrap_or(0))?;
            Some((key.into(), Cpu { total, idle }))
        })
        .collect()
}
pub(super) fn busy(before: &Cpu, after: &Cpu) -> Option<u64> {
    let total = after.total.checked_sub(before.total)?;
    let idle = after.idle.checked_sub(before.idle)?;
    let busy = total.checked_sub(idle)?;
    (total > 0).then(|| (u128::from(busy) * 10_000 / u128::from(total)) as u64)
}

#[derive(Clone, Debug)]
pub(super) struct Process {
    pub pid: u32,
    pub parent: u32,
    pub session: u32,
    pub start: u64,
    pub cpu: u64,
    pub rss: Option<u64>,
    pub io: Option<[u64; 2]>,
    pub descriptors: Option<u32>,
    pub threads: u32,
    pub name: String,
    pub kind: &'static str,
}
pub(super) fn stat(text: &str, hz: u64, page_size: u64) -> Option<Process> {
    let (pid, rest) = text.split_once(" (")?;
    let (name, rest) = rest.rsplit_once(") ")?;
    let f: Vec<_> = rest.split_whitespace().collect();
    let n = |i: usize| f.get(i)?.parse::<u64>().ok();
    let cpu = n(11)?.checked_add(n(12)?)?;
    Some(Process {
        pid: pid.parse().ok()?,
        parent: u32::try_from(n(1)?).ok()?,
        session: u32::try_from(n(3)?).ok()?,
        start: n(19)?,
        cpu: cpu.checked_mul(10_000_000)?.checked_div(hz)?,
        rss: n(21).and_then(|v| v.checked_mul(page_size)),
        io: None,
        descriptors: None,
        threads: u32::try_from(n(17)?).ok()?,
        name: name.chars().filter(|c| !c.is_control()).collect(),
        kind: "unknown",
    })
}

pub(super) fn kind(cmdline: &[u8]) -> &'static str {
    for arg in cmdline.split(|b| *b == 0) {
        match arg {
            b"--type=renderer" => return "renderer",
            b"--type=gpu-process" => return "gpu",
            b"--type=utility" => return "utility",
            b"--type=zygote" => return "zygote",
            _ => {}
        }
    }
    if cmdline.is_empty() {
        "kernel_or_unavailable"
    } else {
        "application"
    }
}

pub(super) struct Processes {
    pub rows: Vec<Process>,
    pub pids: HashSet<u32>,
    pub coverage: ProcessCoverage,
    pub complete: bool,
}

pub(super) fn process(root: &Path, pid: u32) -> io::Result<Process> {
    let dir = root.join(pid.to_string());
    let parse = |text: &str| {
        stat(
            text,
            rustix::param::clock_ticks_per_second(),
            rustix::param::page_size() as u64,
        )
    };
    let mut p = parse(&read(&dir.join("stat"))?).ok_or(io::ErrorKind::InvalidData)?;
    if p.pid != pid {
        return Err(io::ErrorKind::InvalidData.into());
    }
    if let Ok(text) = read(&dir.join("io")) {
        let fields = fields(&text);
        // rchar/wchar include cached/file/socket I/O, unlike read_bytes/write_bytes.
        p.io = fields
            .get("rchar")
            .zip(fields.get("wchar"))
            .map(|(r, w)| [*r, *w]);
    }
    p.descriptors = fs::read_dir(dir.join("fd")).ok().and_then(|iter| {
        let entries = iter.collect::<Result<Vec<_>, _>>().ok()?;
        u32::try_from(entries.len()).ok()
    });
    if let Ok(mut file) = fs::File::open(dir.join("cmdline")) {
        let mut args = Vec::new();
        if file.by_ref().take(65_536).read_to_end(&mut args).is_ok() {
            p.kind = kind(&args);
        }
        // Only a finite type survives; command-line bytes are never stored or serialized.
    }
    if parse(&read(&dir.join("stat"))?).is_none_or(|last| last.start != p.start) {
        return Err(io::ErrorKind::NotFound.into());
    }
    Ok(p)
}

pub(super) fn processes(root: &Path) -> Processes {
    let mut out = Processes {
        rows: Vec::new(),
        pids: HashSet::new(),
        coverage: ProcessCoverage {
            status: Status::Ok,
            ..ProcessCoverage::default()
        },
        complete: true,
    };
    let entries = match fs::read_dir(root) {
        Ok(entries) => entries,
        Err(error) => {
            out.coverage.status = status(&error);
            out.complete = false;
            return out;
        }
    };
    for entry in entries {
        let Ok(entry) = entry else {
            out.complete = false;
            continue;
        };
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|s| s.parse::<u32>().ok())
        else {
            continue;
        };
        out.pids.insert(pid);
        out.coverage.enumerated += 1;
        match process(root, pid) {
            Ok(p) => {
                out.coverage.readable += 1;
                out.rows.push(p);
            }
            Err(e) if e.kind() == io::ErrorKind::PermissionDenied => {
                out.coverage.permission_denied += 1
            }
            Err(_) => out.coverage.unavailable += 1,
        }
    }
    if !out.complete || out.coverage.readable != out.coverage.enumerated {
        out.coverage.status = Status::Partial;
    }
    out.coverage.enumeration_truncated = !out.complete;
    out
}

pub(super) fn status(error: &io::Error) -> Status {
    match error.kind() {
        io::ErrorKind::PermissionDenied => Status::PermissionDenied,
        io::ErrorKind::NotFound => Status::Unsupported,
        _ => Status::Error,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn memory_units_overflow_and_missing_fields() {
        let text = "MemTotal: 1024 kB\nMemAvailable: 512 kB\nBad: 18446744073709551615 kB\nWrong: 12 bananas\n";
        let f = fields(text);
        assert_eq!(f["MemTotal"], 1_048_576);
        assert!(!f.contains_key("Committed_AS"));
        assert!(!f.contains_key("Bad"));
        assert!(!f.contains_key("Wrong"));
    }
    #[test]
    fn cpu_ignores_guest_and_rejects_resets() {
        let before = cpus("cpu 10 0 10 70 10 0 0 0 999 999\nintr 99\n");
        let after = cpus("cpu 30 0 20 120 30 0 0 0 999 999\n");
        assert_eq!(busy(&before["cpu"], &after["cpu"]), Some(3000));
        assert_eq!(busy(&after["cpu"], &before["cpu"]), None);
        assert!(!before.contains_key("intr"));
    }
    #[test]
    fn stat_handles_parentheses_and_distinct_start_identity() {
        let text = "42 (my (worker)) S 1 0 7 0 0 0 0 0 0 0 50 50 0 0 0 0 2 0 900 4096 8";
        let p = stat(text, 100, 4096).unwrap();
        assert_eq!(
            (p.pid, p.parent, p.session, p.start, p.cpu, p.rss),
            (42, 1, 7, 900, 10_000_000, Some(32768))
        );
        assert_eq!(p.name, "my (worker)");
        assert!(stat("truncated", 100, 4096).is_none());
        assert!(stat(text, 0, 4096).is_none());
    }
    #[test]
    fn command_line_projection_is_finite() {
        assert_eq!(
            kind(b"/home/alice/private\0--token=secret\0--type=renderer\0"),
            "renderer"
        );
        assert_eq!(
            kind(b"/home/alice/private\0--type=unknown-private\0"),
            "application"
        );
    }
}
