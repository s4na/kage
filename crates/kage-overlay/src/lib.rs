use std::{fs, path::Path};
pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Backend {
    DirectoryMerge,
    NativeOverlayFs,
    FuseOverlayFs,
}
pub fn mount_directory_merge(lower: &Path, upper: &Path, merged: &Path) -> Result<()> {
    validate_layout(lower, upper, merged)?;
    if merged.exists() {
        fs::remove_dir_all(merged)?;
    }
    copy_dir(lower, merged)?;
    if upper.exists() {
        copy_dir(upper, merged)?;
        apply_deletions(upper, merged)?;
        let metadata = merged.join(".kage");
        if metadata.exists() {
            fs::remove_dir_all(metadata)?;
        }
    }
    Ok(())
}
pub fn refresh_upper_from_merged(lower: &Path, merged: &Path, upper: &Path) -> Result<()> {
    validate_layout(lower, upper, merged)?;
    if upper.exists() {
        fs::remove_dir_all(upper)?;
    }
    fs::create_dir_all(upper)?;
    diff_copy(lower, merged, upper)?;
    write_deletions(lower, merged, upper)?;
    Ok(())
}

pub fn unmount_directory_merge(merged: &Path) -> Result<()> {
    if merged.exists() {
        fs::remove_dir_all(merged)?;
    }
    Ok(())
}

pub fn validate_layout(lower: &Path, upper: &Path, merged: &Path) -> Result<()> {
    if lower == upper || lower == merged || upper == merged {
        return Err("lower, upper, and merged directories must be distinct".into());
    }
    if upper.starts_with(lower) || merged.starts_with(lower) {
        return Err("upper/merged must not be nested inside lower".into());
    }
    if merged.starts_with(upper) || upper.starts_with(merged) {
        return Err("upper and merged must not be nested in each other".into());
    }
    Ok(())
}

pub fn detect_linux_overlayfs() -> Result<Backend> {
    let filesystems = fs::read_to_string("/proc/filesystems")?;
    if filesystems
        .lines()
        .any(|line| line.split_whitespace().last() == Some("overlay"))
    {
        Ok(Backend::NativeOverlayFs)
    } else {
        Err("Linux overlayfs is not listed in /proc/filesystems".into())
    }
}

fn copy_dir(src: &Path, dst: &Path) -> Result<()> {
    fs::create_dir_all(dst)?;
    for entry in walk(src)? {
        let rel = entry.strip_prefix(src)?;
        if rel.as_os_str().is_empty() {
            continue;
        }
        let target = dst.join(rel);
        let meta = fs::symlink_metadata(&entry)?;
        if meta.is_dir() {
            fs::create_dir_all(&target)?;
        } else if meta.file_type().is_symlink() {
            let link = fs::read_link(&entry)?;
            #[cfg(unix)]
            std::os::unix::fs::symlink(link, target)?;
        } else {
            if let Some(p) = target.parent() {
                fs::create_dir_all(p)?;
            }
            fs::copy(&entry, target)?;
        }
    }
    Ok(())
}
fn diff_copy(base: &Path, merged: &Path, upper: &Path) -> Result<()> {
    for entry in walk(merged)? {
        if fs::metadata(&entry)?.is_dir() {
            continue;
        }
        let rel = entry.strip_prefix(merged)?;
        let base_path = base.join(rel);
        let changed = !base_path.exists() || fs::read(&entry)? != fs::read(&base_path)?;
        if changed {
            let target = upper.join(rel);
            if let Some(p) = target.parent() {
                fs::create_dir_all(p)?;
            }
            fs::copy(&entry, target)?;
        }
    }
    Ok(())
}
fn walk(root: &Path) -> Result<Vec<std::path::PathBuf>> {
    let mut out = vec![root.to_path_buf()];
    let mut i = 0;
    while i < out.len() {
        let p = out[i].clone();
        i += 1;
        if fs::symlink_metadata(&p)?.is_dir() {
            for e in fs::read_dir(p)? {
                out.push(e?.path());
            }
        }
    }
    Ok(out)
}

fn write_deletions(base: &Path, merged: &Path, upper: &Path) -> Result<()> {
    let mut deleted = Vec::new();
    for entry in walk(base)? {
        if fs::metadata(&entry)?.is_dir() {
            continue;
        }
        let rel = entry.strip_prefix(base)?;
        if !merged.join(rel).exists() {
            deleted.push(rel.to_string_lossy().into_owned());
        }
    }
    if !deleted.is_empty() {
        let metadata = upper.join(".kage");
        fs::create_dir_all(&metadata)?;
        fs::write(metadata.join("deleted"), deleted.join("\n"))?;
    }
    Ok(())
}

fn apply_deletions(upper: &Path, merged: &Path) -> Result<()> {
    let deleted = upper.join(".kage").join("deleted");
    if !deleted.exists() {
        return Ok(());
    }
    for line in fs::read_to_string(deleted)?.lines() {
        let target = merged.join(line);
        if target.is_dir() {
            fs::remove_dir_all(target)?;
        } else if target.exists() {
            fs::remove_file(target)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp(name: &str) -> std::path::PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("kage-overlay-{name}-{nonce}"))
    }

    #[test]
    fn refresh_records_added_modified_and_deleted_paths() {
        let root = temp("refresh");
        let lower = root.join("lower");
        let merged = root.join("merged");
        let upper = root.join("upper");
        fs::create_dir_all(lower.join("src")).unwrap();
        fs::create_dir_all(merged.join("src")).unwrap();
        fs::write(lower.join("src/lib.rs"), "old").unwrap();
        fs::write(lower.join("README.md"), "delete me").unwrap();
        fs::write(merged.join("src/lib.rs"), "new").unwrap();
        fs::write(merged.join("new.txt"), "added").unwrap();

        refresh_upper_from_merged(&lower, &merged, &upper).unwrap();

        assert_eq!(fs::read_to_string(upper.join("src/lib.rs")).unwrap(), "new");
        assert_eq!(fs::read_to_string(upper.join("new.txt")).unwrap(), "added");
        assert_eq!(
            fs::read_to_string(upper.join(".kage/deleted")).unwrap(),
            "README.md"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn mount_replays_deletions_from_upper_metadata() {
        let root = temp("mount");
        let lower = root.join("lower");
        let upper = root.join("upper");
        let merged = root.join("merged");
        fs::create_dir_all(&lower).unwrap();
        fs::create_dir_all(upper.join(".kage")).unwrap();
        fs::write(lower.join("README.md"), "delete me").unwrap();
        fs::write(upper.join(".kage/deleted"), "README.md").unwrap();

        mount_directory_merge(&lower, &upper, &merged).unwrap();

        assert!(!merged.join("README.md").exists());
        assert!(!merged.join(".kage").exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn layout_rejects_reused_or_nested_directories() {
        let root = temp("layout");
        let lower = root.join("lower");
        let upper = root.join("upper");
        let merged = root.join("merged");
        assert!(validate_layout(&lower, &upper, &merged).is_ok());
        assert!(validate_layout(&lower, &lower, &merged).is_err());
        assert!(validate_layout(&lower, &lower.join("upper"), &merged).is_err());
        assert!(validate_layout(&lower, &upper, &upper.join("merged")).is_err());
    }

    #[test]
    fn unmount_is_idempotent() {
        let root = temp("unmount");
        let merged = root.join("merged");
        fs::create_dir_all(&merged).unwrap();
        unmount_directory_merge(&merged).unwrap();
        unmount_directory_merge(&merged).unwrap();
        assert!(!merged.exists());
    }

    #[test]
    fn overlayfs_detection_is_explicitly_environment_dependent() {
        if std::env::var_os("KAGE_TEST_OVERLAY").is_none() {
            return;
        }
        let detected = detect_linux_overlayfs();
        assert!(
            detected.is_ok(),
            "overlayfs should be available when KAGE_TEST_OVERLAY=1: {detected:?}"
        );
    }
}
