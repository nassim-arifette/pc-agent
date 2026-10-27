use super::*;
use std::{io::Write, os::unix::ffi::OsStrExt};

fn owned(path: &Path) -> Result<OwnedFd, EffectError> {
    let fd = directory(path, true)
        .map_err(|_| error("Cannot open the private trash directory without links."))?;
    let stat = fs::fstat(&fd).map_err(|_| error("Cannot check trash ownership."))?;
    if stat.st_uid != rustix::process::geteuid().as_raw() || stat.st_mode & 0o077 != 0 {
        return Err(error(
            "Trash must be owned by this user and have private 0700 permissions.",
        ));
    }
    Ok(fd)
}
fn escaped(path: &Path) -> String {
    path.as_os_str()
        .as_bytes()
        .iter()
        .map(|&b| {
            if b.is_ascii_alphanumeric() || b"/-._~".contains(&b) {
                (b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect()
}
pub(super) fn recycle(file: &FileTarget) -> Result<EffectOutcome, EffectError> {
    let root = xdg("XDG_DATA_HOME", ".local/share")
        .ok_or_else(|| error("No absolute XDG data directory."))?
        .join("Trash");
    recycle_at(file, &root)
}

fn volume_trash(file: &FileTarget) -> Result<(PathBuf, PathBuf), EffectError> {
    let mut top = file
        .path
        .parent()
        .ok_or_else(|| error("No source directory."))?
        .to_owned();
    while let Some(parent) = top.parent() {
        let fd = directory(parent, false).map_err(|_| error("Cannot verify the volume root."))?;
        let stat = fs::fstat(&fd).map_err(|_| error("Cannot verify the volume identity."))?;
        if stat.st_dev != file.identity.volume_serial {
            break;
        }
        top = parent.to_owned();
    }
    let uid = rustix::process::geteuid().as_raw();
    let common = top.join(".Trash");
    if let Ok(fd) = directory(&common, false) {
        if fs::fstat(fd).is_ok_and(|stat| stat.st_mode & 0o1000 != 0) {
            let path = common.join(uid.to_string());
            if owned(&path).is_ok() {
                return Ok((path, top));
            }
        }
    }
    let path = top.join(format!(".Trash-{uid}"));
    owned(&path)?;
    Ok((path, top))
}

fn recycle_at(file: &FileTarget, home_trash: &Path) -> Result<EffectOutcome, EffectError> {
    let (parent, pin, name) = pinned(file)?;
    let home = owned(home_trash)?;
    let home_volume = fs::fstat(home)
        .map_err(|_| error("Cannot inspect home trash volume."))?
        .st_dev;
    let (root, recovery_path) = if home_volume == file.identity.volume_serial {
        (home_trash.to_owned(), file.path.clone())
    } else {
        let (root, top) = volume_trash(file)?;
        let path = file
            .path
            .strip_prefix(top)
            .map_err(|_| error("File is outside its trash volume."))?
            .to_owned();
        (root, path)
    };
    let _root = owned(&root)?;
    let files = owned(&root.join("files"))?;
    let info = owned(&root.join("info"))?;
    let volume = fs::fstat(&files)
        .map_err(|_| error("Cannot inspect trash volume."))?
        .st_dev;
    if volume != file.identity.volume_serial {
        return Err(EffectError::new(Status::Unsupported,"This trash directory is on another filesystem; no copy/unlink or permanent-delete fallback is allowed."));
    }
    let key = format!("pcagent-{}", uuid::Uuid::new_v4());
    let info_name = format!("{key}.trashinfo");
    let fd = fs::openat(
        &info,
        info_name.as_str(),
        OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::from_raw_mode(0o600),
    )
    .map_err(|_| error("Cannot exclusively create trash recovery metadata."))?;
    let mut metadata = std::fs::File::from(fd);
    let text = format!(
        "[Trash Info]\nPath={}\nDeletionDate={}\n",
        escaped(&recovery_path),
        chrono::Local::now().format("%Y-%m-%dT%H:%M:%S")
    );
    if metadata
        .write_all(text.as_bytes())
        .and_then(|_| metadata.sync_all())
        .and_then(|_| fs::fsync(&info).map_err(std::io::Error::from))
        .is_err()
    {
        let _ = fs::unlinkat(&info, info_name.as_str(), AtFlags::empty());
        return Err(error(
            "Cannot persist trash recovery metadata; original file is untouched.",
        ));
    }
    if fs::statat(&parent, name, AtFlags::SYMLINK_NOFOLLOW)
        .ok()
        .and_then(|s| identity(&s).ok())
        .as_ref()
        != Some(&file.identity)
    {
        let _ = fs::unlinkat(&info, info_name.as_str(), AtFlags::empty());
        return Err(error("The approved file changed before the move."));
    }
    if fs::renameat_with(
        &parent,
        name,
        &files,
        key.as_str(),
        fs::RenameFlags::NOREPLACE,
    )
    .is_err()
    {
        let _ = fs::unlinkat(&info, info_name.as_str(), AtFlags::empty());
        return Err(error(
            "The file could not be moved to trash; no deletion fallback was used.",
        ));
    }
    let moved = fs::statat(&files, key.as_str(), AtFlags::SYMLINK_NOFOLLOW)
        .ok()
        .and_then(|s| identity(&s).ok());
    let pinned = fs::fstat(&pin).ok().and_then(|s| identity(&s).ok());
    if moved.as_ref() != Some(&file.identity) || pinned.as_ref() != Some(&file.identity) {
        if fs::renameat_with(
            &files,
            key.as_str(),
            &parent,
            name,
            fs::RenameFlags::NOREPLACE,
        )
        .is_ok()
        {
            let _ = fs::unlinkat(&info, info_name.as_str(), AtFlags::empty());
            return Err(error(
                "A concurrent file replacement was detected; the moved entry was restored.",
            ));
        }
        return Err(error("A concurrent replacement prevented confirmation or rollback. The moved entry and its recovery metadata remain in the user's trash; nothing was permanently deleted."));
    }
    fs::fsync(&files).and_then(|_|fs::fsync(&info)).and_then(|_|fs::fsync(&parent))
        .map_err(|_|error("The move completed but directory durability could not be confirmed; inspect the trash before retrying."))?;
    Ok(EffectOutcome::Done(Some("Moved the identity-checked file to XDG Trash. It remains recoverable and still occupies disk space.".into())))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn recovery_path_percent_encodes_newlines_and_non_utf8() {
        assert_eq!(escaped(Path::new("/tmp/a b\nc")), "/tmp/a%20b%0Ac");
        assert_eq!(
            escaped(Path::new(OsStr::from_bytes(b"/tmp/\xff"))),
            "/tmp/%FF"
        );
    }

    #[test]
    fn native_recycle_is_recoverable_and_refuses_replacement_and_links() {
        let fixture = std::env::temp_dir().join(format!("pcagent-trash-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&fixture).unwrap();
        let source = fixture.join("a file.bin");
        std::fs::write(&source, b"fixture").unwrap();
        let target = FileTarget {
            path: source.clone(),
            identity: observe_file(&source).unwrap().unwrap(),
        };
        let trash = fixture.join("Trash");
        recycle_at(&target, &trash).unwrap();
        assert!(!source.exists());
        let moved = std::fs::read_dir(trash.join("files"))
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        assert_eq!(std::fs::read(&moved).unwrap(), b"fixture");
        let info = std::fs::read_dir(trash.join("info"))
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        let text = std::fs::read_to_string(info).unwrap();
        assert!(text.contains(&format!("Path={}\n", escaped(&source))));
        assert!(text.contains("\nDeletionDate="));
        std::fs::write(&source, b"new").unwrap();
        assert!(recycle_at(&target, &trash).is_err());
        assert_eq!(std::fs::read(&source).unwrap(), b"new");
        let link = fixture.join("link");
        std::os::unix::fs::symlink(&source, &link).unwrap();
        let target = FileTarget {
            path: link.clone(),
            identity: observe_file(&link).unwrap().unwrap(),
        };
        assert!(recycle_at(&target, &trash).is_err());
        let hard = fixture.join("hard");
        std::fs::hard_link(&source, &hard).unwrap();
        let target = FileTarget {
            path: hard.clone(),
            identity: observe_file(&hard).unwrap().unwrap(),
        };
        assert!(recycle_at(&target, &trash).is_err());
        std::fs::remove_dir_all(fixture).unwrap();
    }
}
