use super::procfs;
use pcagent_shared::{
    new_id, unix_ms, HardwareInventory, Observed, Origin, OsVersion, Status, VolumeInfo,
    VolumeKind, SCHEMA_VERSION,
};
use std::{collections::HashSet, path::Path};

fn unescape(value: &str) -> String {
    value
        .replace("\\040", " ")
        .replace("\\011", "\t")
        .replace("\\012", "\n")
        .replace("\\134", "\\")
}
pub(super) fn volumes(proc: &Path) -> Observed<Vec<VolumeInfo>> {
    let text = match procfs::read(&proc.join("self/mountinfo")) {
        Ok(text) => text,
        Err(e) => return Observed::missing("/proc/self/mountinfo", procfs::status(&e)),
    };
    let mut seen = HashSet::new();
    let mut rows = Vec::new();
    for line in text.lines() {
        let Some((left, right)) = line.split_once(" - ") else {
            continue;
        };
        let fields: Vec<_> = left.split_whitespace().collect();
        let Some(root) = fields.get(4).map(|s| unescape(s)) else {
            continue;
        };
        let Some(fs) = right.split_whitespace().next() else {
            continue;
        };
        let kind = match fs {
            "ext2" | "ext3" | "ext4" | "btrfs" | "xfs" | "zfs" | "overlay" | "f2fs" | "ntfs"
            | "ntfs3" | "vfat" | "exfat" => VolumeKind::Fixed,
            "tmpfs" | "ramfs" => VolumeKind::RamDisk,
            "nfs" | "nfs4" | "cifs" | "smb3" => VolumeKind::Network,
            _ => continue,
        };
        if !seen.insert(root.clone()) {
            continue;
        }
        // Do not stat network mounts: an unresponsive server must not block the sampler.
        let space = if kind == VolumeKind::Network {
            None
        } else {
            rustix::fs::statvfs(root.as_str()).ok()
        };
        let counts = space.and_then(|s| {
            Some((
                s.f_blocks.checked_mul(s.f_frsize)?,
                s.f_bfree.checked_mul(s.f_frsize)?,
                s.f_bavail.checked_mul(s.f_frsize)?,
            ))
        });
        rows.push(VolumeInfo {
            root,
            kind,
            total_bytes: counts.map(|s| s.0),
            free_bytes: counts.map(|s| s.1),
            available_to_caller_bytes: counts.map(|s| s.2),
            source: "/proc/self/mountinfo + statvfs".into(),
            status: if counts.is_some() {
                Status::Ok
            } else {
                Status::Partial
            },
            limitations: if counts.is_some() {
                Vec::new()
            } else {
                vec!["Space is unreadable or the network mount was not probed.".into()]
            },
        });
    }
    Observed::ok(rows, "/proc/self/mountinfo + statvfs")
}
pub(super) fn collect() -> HardwareInventory {
    let mem = procfs::read(Path::new("/proc/meminfo")).ok();
    let total = mem
        .as_deref()
        .and_then(|s| procfs::fields(s).get("MemTotal").copied());
    let version = procfs::read(Path::new("/proc/sys/kernel/osrelease"))
        .ok()
        .and_then(|s| {
            let mut parts = s.trim().split(['.', '-']);
            Some(OsVersion {
                major: parts.next()?.parse().ok()?,
                minor: parts.next()?.parse().ok()?,
                build: parts.next()?.parse().ok()?,
                update_build_revision: None,
                display_version: None,
                family: Some("linux".into()),
            })
        });
    let observed = |v: Option<u64>| {
        v.map(|v| Observed::ok(v, "/proc/meminfo.MemTotal"))
            .unwrap_or_else(|| Observed::missing("/proc/meminfo", Status::Error))
    };
    let volumes = volumes(Path::new("/proc"));
    let mut snapshot = pcagent_shared::Snapshot::empty("inventory");
    super::gpu::collect(&mut snapshot, &mut None);
    let adapters = snapshot
        .gpus
        .into_iter()
        .map(|g| pcagent_shared::GpuAdapter {
            description: g.name.unwrap_or_else(|| "GPU".into()),
            dedicated_video_memory_bytes: None,
        })
        .collect();
    HardwareInventory {
        schema_version:SCHEMA_VERSION,id:new_id("inventory"),origin:Origin::Live,status:Status::Partial,collected_at_unix_ms:unix_ms(),
        os_version:version.map(|v|Observed::ok(v,"/proc/sys/kernel/osrelease")).unwrap_or_else(||Observed::missing("Linux kernel release",Status::Error)),
        architecture:Observed::ok(std::env::consts::ARCH.into(),"compiled architecture"),
        logical_processors:Observed::ok(super::processors(),"/proc/stat"),
        installed_memory_bytes:observed(total),volumes,
        gpu_adapters:Observed::ok(adapters,"NVML or /sys/class/drm"),
        limitations:vec!["MemTotal is kernel-usable RAM, not DIMM capacity. Inventory omits serials, host name and kernel build suffix. Mount roots remain local.".into()],
    }
}
