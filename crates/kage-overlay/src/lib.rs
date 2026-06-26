use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Backend {
    DirectoryMerge,
    NativeOverlayFs,
    FuseOverlayFs,
    AppleManagedLinux,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackendKind {
    Fallback,
    OverlayFs,
}

impl BackendKind {
    pub fn parse(value: &str) -> Result<Self> {
        match value {
            "fallback" | "directory-merge" => Ok(Self::Fallback),
            "overlayfs" | "linux-overlayfs" => Ok(Self::OverlayFs),
            other => Err(format!("unsupported backend: {other}").into()),
        }
    }
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Fallback => "fallback",
            Self::OverlayFs => "overlayfs",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackendPaths {
    pub lower: PathBuf,
    pub upper: PathBuf,
    pub work: PathBuf,
    pub merged: PathBuf,
}

impl BackendPaths {
    pub fn new(
        lower: impl AsRef<Path>,
        upper: impl AsRef<Path>,
        work: impl AsRef<Path>,
        merged: impl AsRef<Path>,
    ) -> Self {
        Self {
            lower: lower.as_ref().to_path_buf(),
            upper: upper.as_ref().to_path_buf(),
            work: work.as_ref().to_path_buf(),
            merged: merged.as_ref().to_path_buf(),
        }
    }
}

pub trait WorkspaceBackend {
    fn kind(&self) -> BackendKind;
    fn mount(&self, paths: &BackendPaths) -> Result<()>;
    fn unmount(&self, paths: &BackendPaths) -> Result<()>;
    fn refresh_upper_from_merged(&self, paths: &BackendPaths) -> Result<()>;
}

#[derive(Debug, Default, Clone, Copy)]
pub struct DirectoryMergeBackend;

impl WorkspaceBackend for DirectoryMergeBackend {
    fn kind(&self) -> BackendKind {
        BackendKind::Fallback
    }
    fn mount(&self, paths: &BackendPaths) -> Result<()> {
        mount_directory_merge(&paths.lower, &paths.upper, &paths.merged)
    }
    fn unmount(&self, paths: &BackendPaths) -> Result<()> {
        unmount_directory_merge(&paths.merged)
    }
    fn refresh_upper_from_merged(&self, paths: &BackendPaths) -> Result<()> {
        refresh_upper_from_merged(&paths.lower, &paths.merged, &paths.upper)
    }
}

#[derive(Debug, Default, Clone, Copy)]
pub struct LinuxOverlayBackend;

impl WorkspaceBackend for LinuxOverlayBackend {
    fn kind(&self) -> BackendKind {
        BackendKind::OverlayFs
    }
    fn mount(&self, paths: &BackendPaths) -> Result<()> {
        mount_linux_overlay(paths)
    }
    fn unmount(&self, paths: &BackendPaths) -> Result<()> {
        unmount_linux_overlay(&paths.merged)
    }
    fn refresh_upper_from_merged(&self, _paths: &BackendPaths) -> Result<()> {
        Ok(())
    }
}

pub fn backend_for(kind: BackendKind) -> Box<dyn WorkspaceBackend> {
    match kind {
        BackendKind::Fallback => Box::new(DirectoryMergeBackend),
        BackendKind::OverlayFs => Box::new(LinuxOverlayBackend),
    }
}

pub fn mount_directory_merge(lower: &Path, upper: &Path, merged: &Path) -> Result<()> {
    validate_layout(lower, upper, Path::new("__fallback_workdir__"), merged)?;
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
    validate_layout(lower, upper, Path::new("__fallback_workdir__"), merged)?;
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

pub fn mount_linux_overlay(paths: &BackendPaths) -> Result<()> {
    require_linux()?;
    validate_layout(&paths.lower, &paths.upper, &paths.work, &paths.merged)?;
    if !paths.lower.is_dir() {
        return Err(format!("overlay lowerdir does not exist: {}", paths.lower.display()).into());
    }
    fs::create_dir_all(&paths.upper)?;
    fs::create_dir_all(&paths.work)?;
    fs::create_dir_all(&paths.merged)?;
    if is_mounted(&paths.merged)? {
        return Ok(());
    }
    ensure_workdir_safe(&paths.work)?;
    let opts = format!(
        "lowerdir={},upperdir={},workdir={}",
        paths.lower.display(),
        paths.upper.display(),
        paths.work.display()
    );
    let status = Command::new("mount")
        .args(["-t", "overlay", "overlay", "-o", opts.as_str()])
        .arg(&paths.merged)
        .status()?;
    if status.success() {
        Ok(())
    } else {
        cleanup_empty_dir(&paths.merged)?;
        Err(
            format!("overlay mount failed with {status}; root/CAP_SYS_ADMIN may be required")
                .into(),
        )
    }
}

pub fn unmount_linux_overlay(merged: &Path) -> Result<()> {
    require_linux()?;
    if !merged.exists() || !is_mounted(merged)? {
        return Ok(());
    }
    let status = Command::new("umount").arg(merged).status()?;
    if status.success() || !is_mounted(merged)? {
        Ok(())
    } else {
        Err(format!("umount failed with {status}: {}", merged.display()).into())
    }
}

pub fn validate_layout(lower: &Path, upper: &Path, work: &Path, merged: &Path) -> Result<()> {
    let paths = [
        ("lower", lower),
        ("upper", upper),
        ("work", work),
        ("merged", merged),
    ];
    for (i, (a_name, a)) in paths.iter().enumerate() {
        for (b_name, b) in paths.iter().skip(i + 1) {
            if a == b {
                return Err(format!("{a_name} and {b_name} directories must be distinct").into());
            }
        }
    }
    for (parent_name, parent) in [
        ("lower", lower),
        ("upper", upper),
        ("work", work),
        ("merged", merged),
    ] {
        for (child_name, child) in [
            ("lower", lower),
            ("upper", upper),
            ("work", work),
            ("merged", merged),
        ] {
            if parent_name != child_name && child.starts_with(parent) {
                return Err(format!("{child_name} must not be nested inside {parent_name}").into());
            }
        }
    }
    Ok(())
}

pub fn detect_linux_overlayfs() -> Result<Backend> {
    require_linux()?;
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

fn require_linux() -> Result<()> {
    if cfg!(target_os = "linux") {
        Ok(())
    } else {
        Err("Linux overlayfs backend is only available on Linux".into())
    }
}

fn ensure_workdir_safe(work: &Path) -> Result<()> {
    let mut entries = fs::read_dir(work)?;
    if entries.next().transpose()?.is_some() {
        return Err(format!(
            "overlay workdir must be empty and not reused: {}",
            work.display()
        )
        .into());
    }
    Ok(())
}

fn is_mounted(path: &Path) -> Result<bool> {
    if !cfg!(target_os = "linux") {
        return Ok(false);
    }
    let canonical = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let mountinfo = fs::read_to_string("/proc/self/mountinfo")?;
    Ok(mountinfo.lines().any(|line| {
        let mut fields = line.split_whitespace();
        fields
            .nth(4)
            .is_some_and(|mount_point| Path::new(mount_point) == canonical)
    }))
}

fn cleanup_empty_dir(path: &Path) -> Result<()> {
    if path.is_dir() && fs::read_dir(path)?.next().is_none() {
        fs::remove_dir(path)?;
    }
    Ok(())
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

fn walk(root: &Path) -> Result<Vec<PathBuf>> {
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

    fn temp(name: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("kage-overlay-{name}-{nonce}"))
    }
    fn paths(root: &Path) -> BackendPaths {
        BackendPaths::new(
            root.join("lower"),
            root.join("upper"),
            root.join("work"),
            root.join("merged"),
        )
    }

    #[test]
    fn refresh_records_added_modified_and_deleted_paths() {
        let root = temp("refresh");
        let p = paths(&root);
        fs::create_dir_all(p.lower.join("src")).unwrap();
        fs::create_dir_all(p.merged.join("src")).unwrap();
        fs::write(p.lower.join("src/lib.rs"), "old").unwrap();
        fs::write(p.lower.join("README.md"), "delete me").unwrap();
        fs::write(p.merged.join("src/lib.rs"), "new").unwrap();
        fs::write(p.merged.join("new.txt"), "added").unwrap();
        DirectoryMergeBackend.refresh_upper_from_merged(&p).unwrap();
        assert_eq!(
            fs::read_to_string(p.upper.join("src/lib.rs")).unwrap(),
            "new"
        );
        assert_eq!(
            fs::read_to_string(p.upper.join("new.txt")).unwrap(),
            "added"
        );
        assert_eq!(
            fs::read_to_string(p.upper.join(".kage/deleted")).unwrap(),
            "README.md"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn mount_replays_deletions_from_upper_metadata() {
        let root = temp("mount");
        let p = paths(&root);
        fs::create_dir_all(&p.lower).unwrap();
        fs::create_dir_all(p.upper.join(".kage")).unwrap();
        fs::write(p.lower.join("README.md"), "delete me").unwrap();
        fs::write(p.upper.join(".kage/deleted"), "README.md").unwrap();
        DirectoryMergeBackend.mount(&p).unwrap();
        assert!(!p.merged.join("README.md").exists());
        assert!(!p.merged.join(".kage").exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn layout_rejects_reused_or_nested_directories() {
        let root = temp("layout");
        let p = paths(&root);
        assert!(validate_layout(&p.lower, &p.upper, &p.work, &p.merged).is_ok());
        assert!(validate_layout(&p.lower, &p.lower, &p.work, &p.merged).is_err());
        assert!(validate_layout(&p.lower, &p.lower.join("upper"), &p.work, &p.merged).is_err());
        assert!(validate_layout(&p.lower, &p.upper, &p.work, &p.upper.join("merged")).is_err());
    }

    #[test]
    fn overlay_mount_rejects_non_empty_workdir_before_privileged_mount() {
        let root = temp("workdir");
        let p = paths(&root);
        fs::create_dir_all(&p.lower).unwrap();
        fs::create_dir_all(&p.upper).unwrap();
        fs::create_dir_all(&p.work).unwrap();
        fs::write(p.work.join("reuse"), "unsafe").unwrap();
        let err = mount_linux_overlay(&p).unwrap_err().to_string();
        assert!(err.contains("workdir must be empty") || err.contains("only available on Linux"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn unmount_is_idempotent() {
        let root = temp("unmount");
        let p = paths(&root);
        fs::create_dir_all(&p.merged).unwrap();
        DirectoryMergeBackend.unmount(&p).unwrap();
        DirectoryMergeBackend.unmount(&p).unwrap();
        assert!(!p.merged.exists());
    }

    #[test]
    fn overlayfs_detection_is_explicitly_environment_dependent() {
        if std::env::var_os("KAGE_TEST_OVERLAY").is_none() || !cfg!(target_os = "linux") {
            eprintln!("skipping overlayfs detection; set KAGE_TEST_OVERLAY=1 on Linux");
            return;
        }
        let detected = detect_linux_overlayfs();
        assert!(
            detected.is_ok(),
            "overlayfs should be available when KAGE_TEST_OVERLAY=1: {detected:?}"
        );
    }
}
