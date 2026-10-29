pub(crate) use super::unix::*;
use crate::VolumeSpace;
use pcagent_shared::Status;
use std::path::Path;

pub(crate) fn volume_space(root: &Path) -> VolumeSpace {
    match rustix::fs::statvfs(root) {
        Ok(stat) => space(stat.f_blocks, stat.f_bfree, stat.f_bavail, stat.f_frsize),
        Err(error) => VolumeSpace {
            total_bytes: None,
            free_bytes: None,
            available_bytes: None,
            occupied_bytes: None,
            filesystem: None,
            source: "statvfs".into(),
            status: if error == rustix::io::Errno::ACCESS {
                Status::PermissionDenied
            } else {
                Status::Error
            },
            limitations: vec!["The volume could not be read; no capacity is inferred.".into()],
        },
    }
}
fn space(blocks: u64, free: u64, available: u64, fragment: u64) -> VolumeSpace {
    let total = blocks.checked_mul(fragment).filter(|_| fragment > 0);
    let free = free
        .checked_mul(fragment)
        .filter(|v| total.is_some_and(|total| *v <= total));
    let available = available
        .checked_mul(fragment)
        .filter(|v| free.is_some_and(|free| *v <= free));
    let valid = total.is_some() && free.is_some() && available.is_some();
    VolumeSpace {
        total_bytes:total,free_bytes:free,available_bytes:available,
        occupied_bytes:total.zip(free).and_then(|(total,free)|total.checked_sub(free)),
        filesystem:None,source:"statvfs".into(),status:if valid {Status::Ok} else {Status::Partial},
        limitations:vec!["Caller-available space excludes reserved blocks; free and occupied describe the whole volume, not this scan or reclaimable bytes.".into()],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reserved_blocks_do_not_disappear_from_volume_free_space() {
        let result = space(100, 20, 10, 4096);
        assert_eq!(result.total_bytes, Some(409600));
        assert_eq!(result.free_bytes, Some(81920));
        assert_eq!(result.available_bytes, Some(40960));
        assert_eq!(result.occupied_bytes, Some(327680));
        assert_eq!(result.status, Status::Ok);
    }
    #[test]
    fn overflow_inconsistency_and_inaccessible_volumes_are_not_zero() {
        assert_eq!(space(u64::MAX, 1, 1, 4096).total_bytes, None);
        assert_eq!(space(10, 20, 2, 4096).free_bytes, None);
        assert_eq!(space(10, 2, 3, 4096).available_bytes, None);
        assert_eq!(space(10, 2, 1, 0).total_bytes, None);
        let missing = volume_space(Path::new("/pcagent-volume-fixture-missing"));
        assert_eq!(missing.free_bytes, None);
        assert_eq!(missing.available_bytes, None);
    }
}
