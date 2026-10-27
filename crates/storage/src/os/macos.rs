//! Native capacity; directory traversal remains descriptor-relative and never follows links.
pub(crate) use super::unix::{
    authorize_root, check_root, child_name, profile_dir, Child, Directory, VolumeId,
};
use crate::VolumeSpace;
use pcagent_shared::Status;
use std::path::Path;

pub(crate) fn volume_space(root: &Path) -> VolumeSpace {
    let stat = match rustix::fs::statfs(root) {
        Ok(stat) => stat,
        Err(error) => {
            return VolumeSpace {
                total_bytes: None,
                free_bytes: None,
                available_bytes: None,
                occupied_bytes: None,
                filesystem: None,
                source: "Darwin.statfs".into(),
                status: if error == rustix::io::Errno::ACCESS || error == rustix::io::Errno::PERM {
                    Status::PermissionDenied
                } else {
                    Status::Error
                },
                limitations: vec!["Native volume capacity could not be read.".into()],
            };
        }
    };
    let name: Vec<u8> = stat
        .f_fstypename
        .iter()
        .map(|b| *b as u8)
        .take_while(|b| *b != 0)
        .collect();
    let filesystem = std::str::from_utf8(&name)
        .ok()
        .filter(|name| {
            !name.is_empty() && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
        })
        .map(str::to_owned);
    capacity(
        u64::from(stat.f_bsize),
        stat.f_blocks,
        stat.f_bfree,
        stat.f_bavail,
        filesystem,
    )
}

fn capacity(
    block_size: u64,
    blocks: u64,
    free: u64,
    available: u64,
    filesystem: Option<String>,
) -> VolumeSpace {
    let valid = block_size != 0 && free <= blocks && available <= free;
    let total = valid.then(|| blocks.checked_mul(block_size)).flatten();
    let free = valid.then(|| free.checked_mul(block_size)).flatten();
    let available = valid.then(|| available.checked_mul(block_size)).flatten();
    let occupied = total
        .zip(free)
        .and_then(|(total, free)| total.checked_sub(free));
    VolumeSpace {
        total_bytes: total,
        free_bytes: free,
        available_bytes: available,
        occupied_bytes: occupied,
        filesystem,
        source: "Darwin.statfs".into(),
        status: if occupied.is_some() {
            Status::Ok
        } else {
            Status::Error
        },
        limitations: vec![
            "APFS volumes can share a container; capacities must not be summed across mounts."
                .into(),
            "Free blocks exclude purgeable data and may exceed the caller's quota allowance."
                .into(),
            "Occupied space includes metadata, snapshots and data outside the authorized scope."
                .into(),
        ],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invalid_or_overflowed_native_capacity_stays_unknown() {
        for (size, total, free) in [(0, 100, 20), (4096, 10, 20), (4096, u64::MAX, u64::MAX)] {
            let result = capacity(size, total, free, free, None);
            assert_eq!(result.status, Status::Error);
            assert!(result.occupied_bytes.is_none());
        }
        let result = capacity(4096, 100, 20, 15, Some("apfs".into()));
        assert_eq!(result.total_bytes, Some(409600));
        assert_eq!(result.free_bytes, Some(81920));
        assert_eq!(result.available_bytes, Some(61440));
        assert_eq!(result.occupied_bytes, Some(327680));
        assert_eq!(result.status, Status::Ok);
        assert!(capacity(4096, 100, 20, 21, None).available_bytes.is_none());
    }

    #[test]
    fn native_statfs_reads_the_current_volume_without_exporting_a_path() {
        let result = volume_space(Path::new("/"));
        assert_eq!(result.status, Status::Ok);
        assert!(result.total_bytes.is_some_and(|size| size > 0));
        assert!(result.free_bytes <= result.total_bytes);
        let failed = volume_space(Path::new("/nonexistent-pcagent-capacity-fixture"));
        assert_eq!(failed.status, Status::Error);
        assert!(failed.total_bytes.is_none());
        assert!(!serde_json::to_string(&failed)
            .unwrap()
            .contains("nonexistent-pcagent"));
    }
}
