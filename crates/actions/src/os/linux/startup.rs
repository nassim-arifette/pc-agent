use super::*;
use crate::StartupApproval;
use sha2::{Digest, Sha256};
use std::io::{Read, Write};

fn root() -> Result<PathBuf, Status> {
    xdg("XDG_CONFIG_HOME", ".config")
        .map(|p| p.join("autostart"))
        .ok_or(Status::Unsupported)
}
fn valid(name: &str) -> bool {
    !name.is_empty()
        && name.ends_with(".desktop")
        && name.len() <= 255
        && !name.contains(['/', '\\'])
        && !name.chars().any(char::is_control)
}
fn read(dir: &OwnedFd, name: &str) -> Result<(std::fs::File, String), Status> {
    let fd = fs::openat(
        dir,
        name,
        OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(io_status)?;
    let s = fs::fstat(&fd).map_err(io_status)?;
    if FileType::from_raw_mode(s.st_mode) != FileType::RegularFile
        || s.st_nlink != 1
        || s.st_uid != rustix::process::geteuid().as_raw()
    {
        return Err(Status::Unsupported);
    }
    let mut file = std::fs::File::from(fd);
    let mut text = String::new();
    Read::by_ref(&mut file)
        .take(1_048_577)
        .read_to_string(&mut text)
        .map_err(io_status)?;
    if text.len() > 1_048_576 {
        return Err(Status::Unsupported);
    }
    Ok((file, text))
}
/// Preserve every field except the one explicit Hidden key; never retain command text in plans.
fn parsed(text: &str) -> Result<(StartupIdentity, String), Status> {
    let mut in_entry = false;
    let mut found = false;
    let mut hidden = None;
    let mut saw_hidden = false;
    let mut application = false;
    let mut canonical = String::new();
    for line in text.lines() {
        if line.trim().starts_with('[') {
            in_entry = line.trim() == "[Desktop Entry]";
            if in_entry && found {
                return Err(Status::Unsupported);
            }
            found |= in_entry;
        }
        if in_entry && line.trim() == "Type=Application" {
            application = true;
        }
        if in_entry && line.trim().starts_with("Hidden=") {
            if saw_hidden {
                return Err(Status::Unsupported);
            }
            saw_hidden = true;
            hidden = Some(match line.trim().strip_prefix("Hidden=").unwrap() {
                "true" => true,
                "false" => false,
                _ => return Err(Status::Unsupported),
            });
            continue;
        }
        canonical.push_str(line);
        canonical.push('\n');
    }
    if !found || !application {
        return Err(Status::Unsupported);
    }
    let digest = Sha256::digest(canonical.as_bytes());
    let stamp = u64::from_le_bytes(digest[..8].try_into().unwrap()).max(1);
    let content_sha256 = format!("{digest:x}");
    Ok((
        StartupIdentity {
            run_key_last_write_filetime: stamp,
            approval: StartupApproval::Xdg {
                hidden,
                content_sha256,
            },
        },
        canonical,
    ))
}
pub(super) fn observe(name: &str) -> Result<Option<StartupIdentity>, Status> {
    observe_at(&root()?, name)
}
fn observe_at(root: &Path, name: &str) -> Result<Option<StartupIdentity>, Status> {
    if !valid(name) {
        return Err(Status::Unsupported);
    }
    let dir = directory(root, false)?;
    let (_, text) = match read(&dir, name) {
        Ok(v) => v,
        Err(Status::Unsupported) if !root.join(name).exists() => return Ok(None),
        Err(e) => return Err(e),
    };
    parsed(&text).map(|v| Some(v.0))
}
pub(super) fn apply(target: &StartupTarget) -> Result<EffectOutcome, EffectError> {
    apply_at(&root().map_err(|_| error("No XDG config root."))?, target)
}
fn apply_at(root: &Path, target: &StartupTarget) -> Result<EffectOutcome, EffectError> {
    if !valid(target.value_name()) {
        return Err(error("Invalid XDG startup filename."));
    }
    let dir =
        directory(root, false).map_err(|_| error("Startup directory is unavailable or linked."))?;
    let (pin, text) =
        read(&dir, target.value_name()).map_err(|_| error("Startup file could not be pinned."))?;
    let (identity, canonical) = parsed(&text).map_err(|_| error("Invalid desktop entry."))?;
    if &identity != target.identity() {
        return Err(error("Startup entry changed since review."));
    }
    let StartupApproval::Xdg {
        hidden,
        content_sha256,
    } = target.desired_approval()
    else {
        return Err(error("Not an XDG startup action."));
    };
    let StartupApproval::Xdg {
        content_sha256: before,
        ..
    } = &identity.approval
    else {
        unreachable!()
    };
    if content_sha256 != before {
        return Err(error("Startup content fingerprint changed."));
    }
    let mut output = String::new();
    for line in canonical.lines() {
        output.push_str(line);
        output.push('\n');
        if line.trim() == "[Desktop Entry]" {
            if let Some(hidden) = hidden {
                output.push_str(if *hidden {
                    "Hidden=true\n"
                } else {
                    "Hidden=false\n"
                });
            }
        }
    }
    let temp = format!(".pcagent-{}", uuid::Uuid::new_v4());
    let fd = fs::openat(
        &dir,
        temp.as_str(),
        OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::from_raw_mode(0o600),
    )
    .map_err(|_| error("Cannot create an exclusive startup transaction."))?;
    let mut file = std::fs::File::from(fd);
    let original = fs::fstat(&pin).map_err(|_| error("Pinned startup file unavailable."))?;
    let mut exchanged = false;
    let result = (|| {
        file.write_all(output.as_bytes())
            .and_then(|_| file.sync_all())
            .map_err(|_| error("Cannot persist startup change."))?;
        let (_, latest) =
            read(&dir, target.value_name()).map_err(|_| error("Startup entry changed."))?;
        if latest != text {
            return Err(error("Startup entry changed during approval."));
        }
        let now = fs::statat(&dir, target.value_name(), AtFlags::SYMLINK_NOFOLLOW)
            .map_err(|_| error("Startup entry disappeared."))?;
        if now.st_ino != original.st_ino || now.st_dev != original.st_dev {
            return Err(error("Startup file was replaced."));
        }
        fs::renameat_with(
            &dir,
            temp.as_str(),
            &dir,
            target.value_name(),
            fs::RenameFlags::EXCHANGE,
        )
        .map_err(|_| error("Cannot atomically exchange startup entry."))?;
        exchanged = true;
        let displaced = fs::statat(&dir, temp.as_str(), AtFlags::SYMLINK_NOFOLLOW)
            .map_err(|_| error("Cannot verify displaced startup entry."))?;
        let displaced_text = read(&dir, &temp).map(|(_, text)| text).ok();
        if displaced.st_ino != original.st_ino
            || displaced.st_dev != original.st_dev
            || displaced_text.as_deref() != Some(text.as_str())
        {
            return Err(error("Concurrent replacement detected after exchange. The displaced entry is preserved in the autostart directory under a .pcagent- recovery name; no recovery file was deleted."));
        }
        fs::fsync(&dir).map_err(|_| error("Startup change could not be synced."))?;
        Ok(EffectOutcome::Done(Some("Changed only the XDG Hidden startup state; the previous state is in the local journal.".into())))
    })();
    if !exchanged || result.is_ok() {
        let _ = fs::unlinkat(&dir, temp.as_str(), AtFlags::empty());
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn startup_identity_keeps_content_private_and_restoration_stable() {
        let text="[Desktop Entry]\nType=Application\nName=Fixture\nExec=/home/private/secret --key=private\n";
        let (a, _) = parsed(text).unwrap();
        let (b, _) = parsed(&(text.to_string() + "Hidden=true\n")).unwrap();
        assert_eq!(a.run_key_last_write_filetime, b.run_key_last_write_filetime);
        assert_eq!(a.approval.enabled(), Some(true));
        assert_eq!(b.approval.enabled(), Some(false));
        assert!(!serde_json::to_string(&a).unwrap().contains("private"));
        assert_ne!(
            parsed(&text.replace("--key=private", "--different"))
                .unwrap()
                .0,
            a
        );
        assert!(parsed(&(text.to_string() + "Hidden=true\nHidden=false\n")).is_err());
    }

    #[test]
    fn native_startup_disable_restore_and_changed_content() {
        struct Fixture(PathBuf);
        impl crate::Guard for Fixture {
            fn observe_file(&self, _: &Path) -> Result<Option<FileIdentity>, Status> {
                Err(Status::Unsupported)
            }
            fn observe_process(&self, _: u32) -> Result<Option<ProcessIdentity>, Status> {
                Err(Status::Unsupported)
            }
            fn observe_user_startup(&self, name: &str) -> Result<Option<StartupIdentity>, Status> {
                observe_at(&self.0, name)
            }
        }
        let guard =
            Fixture(std::env::temp_dir().join(format!("pcagent-startup-{}", uuid::Uuid::new_v4())));
        std::fs::create_dir(&guard.0).unwrap();
        let path = guard.0.join("fixture.desktop");
        let source =
            "[Desktop Entry]\nType=Application\nName=Fixture\nExec=/not/executed --private\n";
        std::fs::write(&path, source).unwrap();
        let target = StartupTarget::disable("fixture.desktop", &guard).unwrap();
        assert!(!serde_json::to_string(&target)
            .unwrap()
            .contains("/not/executed"));
        apply_at(&guard.0, &target).unwrap();
        assert!(std::fs::read_to_string(&path)
            .unwrap()
            .contains("Hidden=true\n"));
        let restore = target.prepare_restore(&guard).unwrap();
        apply_at(&guard.0, &restore).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), source);
        let target = StartupTarget::disable("fixture.desktop", &guard).unwrap();
        std::fs::write(&path, source.replace("--private", "--changed")).unwrap();
        assert!(apply_at(&guard.0, &target).is_err());
        let link = guard.0.join("linked.desktop");
        std::os::unix::fs::symlink(&path, &link).unwrap();
        assert!(StartupTarget::disable("linked.desktop", &guard).is_err());
        assert!(StartupTarget::disable("../fixture.desktop", &guard).is_err());
        std::fs::remove_dir_all(guard.0).unwrap();
    }
}
