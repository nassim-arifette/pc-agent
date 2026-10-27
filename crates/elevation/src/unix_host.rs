//! Root and set-id hosts must never create the desktop or model runtime.
pub(super) fn is_privileged() -> bool {
    // SAFETY: these process-credential getters have no pointer or initialization preconditions.
    let (uid, euid, gid, egid) = unsafe {
        (
            libc::getuid(),
            libc::geteuid(),
            libc::getgid(),
            libc::getegid(),
        )
    };
    privileged(uid, euid, gid, egid)
}

fn privileged(uid: u32, euid: u32, gid: u32, egid: u32) -> bool {
    uid == 0 || euid == 0 || uid != euid || gid != egid
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn root_and_set_id_hosts_fail_closed() {
        assert!(privileged(0, 0, 0, 0));
        assert!(privileged(501, 0, 20, 20));
        assert!(privileged(0, 501, 20, 20));
        assert!(privileged(501, 502, 20, 20));
        assert!(privileged(501, 501, 20, 80));
        assert!(!privileged(501, 501, 20, 20));
        assert!(!privileged(1000, 1000, 1000, 1000));
    }
}
