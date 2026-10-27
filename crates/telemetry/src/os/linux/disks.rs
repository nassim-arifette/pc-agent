use super::{procfs, value};
use pcagent_shared::{Metric, Snapshot, Window};
use std::{collections::BTreeMap, path::Path};

#[derive(Clone, Debug)]
pub(super) struct Disk {
    reads: u64,
    read_ms: u64,
    writes: u64,
    write_ms: u64,
    busy_ms: u64,
    queued: u64,
}
pub(super) fn parse(text: &str) -> BTreeMap<String, Disk> {
    text.lines()
        .filter_map(|line| {
            let f: Vec<_> = line.split_whitespace().collect();
            let n = |i: usize| f.get(i)?.parse().ok();
            Some((
                f.get(2)?.to_string(),
                Disk {
                    reads: n(3)?,
                    read_ms: n(6)?,
                    writes: n(7)?,
                    write_ms: n(10)?,
                    queued: n(11)?,
                    busy_ms: n(12)?,
                },
            ))
        })
        .collect()
}
pub(super) fn read(proc: &Path, sys: &Path) -> BTreeMap<String, Disk> {
    let mut disks = parse(&procfs::read(&proc.join("diskstats")).unwrap_or_default());
    disks.retain(|name, _| {
        let block = sys.join("block").join(name);
        !name.starts_with("loop")
            && !name.starts_with("ram")
            && block.exists()
            && std::fs::read_dir(block.join("slaves"))
                .is_ok_and(|mut entries| entries.next().is_none())
    });
    disks
}
fn latency(before: &Disk, after: &Disk, write: bool) -> Option<(u64, u64)> {
    let (ops, ms) = if write {
        (
            after.writes.checked_sub(before.writes)?,
            after.write_ms.checked_sub(before.write_ms)?,
        )
    } else {
        (
            after.reads.checked_sub(before.reads)?,
            after.read_ms.checked_sub(before.read_ms)?,
        )
    };
    Some((ops, ms))
}
pub(super) fn evidence(
    out: &mut Snapshot,
    before: &BTreeMap<String, Disk>,
    after: &BTreeMap<String, Disk>,
    window: &Window,
) {
    let matched: Vec<_> = after
        .iter()
        .filter_map(|(name, disk)| before.get(name).map(|b| (b, disk)))
        .collect();
    let all_present =
        !after.is_empty() && matched.len() == after.len() && before.len() == after.len();
    for (metric, write) in [
        (Metric::DiskReadLatencyMicroseconds, false),
        (Metric::DiskWriteLatencyMicroseconds, true),
    ] {
        let total = matched.iter().try_fold((0u64, 0u64), |(ops, ms), (b, a)| {
            let (n, t) = latency(b, a, write)?;
            Some((ops.checked_add(n)?, ms.checked_add(t)?))
        });
        let result = total
            .filter(|(ops, _)| all_present && *ops > 0)
            .and_then(|(ops, ms)| ms.checked_mul(1000)?.checked_div(ops));
        value(out, metric, "system", result, "/proc/diskstats (leaf block devices)", window,
              "Latency needs completed I/O and unchanged devices in both samples; no I/O is unknown latency, not zero.");
    }
    let idle = matched
        .iter()
        .try_fold(0u64, |sum, (b, a)| {
            let busy = a.busy_ms.checked_sub(b.busy_ms)?;
            sum.checked_add(
                window
                    .duration_ms
                    .saturating_sub(busy.min(window.duration_ms)),
            )
        })
        .filter(|_| all_present && window.duration_ms > 0)
        .map(|idle| {
            (u128::from(idle) * 10_000 / u128::from(window.duration_ms) / matched.len() as u128)
                as u64
        });
    value(out,Metric::DiskIdleBasisPoints,"system",idle,"/proc/diskstats (mean idle across leaf devices)",window,
          "Linux busy milliseconds count intervals with requests in flight; this is not a Windows PDH counter.");
    let queue = (!after.is_empty())
        .then(|| {
            after
                .values()
                .try_fold(0u64, |sum, d| sum.checked_add(d.queued))
        })
        .flatten();
    value(
        out,
        Metric::DiskQueueLength,
        "system",
        queue,
        "/proc/diskstats (in-flight requests)",
        &Window::instant(),
        "No readable block device queue.",
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn disk_latency_uses_completed_operations_not_elapsed_time() {
        let b = parse("8 0 sda 10 0 10 20 30 0 10 100 1 20 0");
        let a = parse("8 0 sda 12 0 12 30 34 0 12 120 2 70 0");
        let mut out = Snapshot::empty("fixture");
        evidence(
            &mut out,
            &b,
            &a,
            &Window {
                started_at_unix_ms: 0,
                finished_at_unix_ms: 100,
                duration_ms: 100,
            },
        );
        assert_eq!(
            out.system_value(Metric::DiskReadLatencyMicroseconds)
                .unwrap()
                .1,
            5000
        );
        assert_eq!(
            out.system_value(Metric::DiskIdleBasisPoints).unwrap().1,
            5000
        );
        assert_eq!(out.system_value(Metric::DiskQueueLength).unwrap().1, 2);
        let mut zero = Snapshot::empty("fixture");
        evidence(&mut zero, &b, &b, &Window::instant());
        assert!(zero
            .system_value(Metric::DiskReadLatencyMicroseconds)
            .is_none());
    }
}
