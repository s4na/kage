use kage_git::{run_at, GitRepo};
use kage_overlay::{BackendPaths, DirectoryMergeBackend, LinuxOverlayBackend, WorkspaceBackend};
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};

fn temp(name: &str) -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!("kage-backend-tree-{name}-{nonce}"))
}
fn setup_repo() -> (PathBuf, GitRepo, String) {
    let repo_dir = temp("repo");
    fs::create_dir_all(&repo_dir).unwrap();
    run_at(&repo_dir, "git", &["init", "-b", "main"]).unwrap();
    run_at(
        &repo_dir,
        "git",
        &["config", "user.email", "kage@example.invalid"],
    )
    .unwrap();
    run_at(&repo_dir, "git", &["config", "user.name", "kage test"]).unwrap();
    fs::write(repo_dir.join("README.md"), "hello").unwrap();
    fs::write(repo_dir.join("delete.txt"), "delete").unwrap();
    fs::write(repo_dir.join("rename.txt"), "rename").unwrap();
    fs::create_dir_all(repo_dir.join("dir/sub")).unwrap();
    fs::write(repo_dir.join("dir/sub/file.txt"), "dir file").unwrap();
    fs::write(repo_dir.join("script.sh"), "#!/bin/sh\nexit 0\n").unwrap();
    let mut perms = fs::metadata(repo_dir.join("script.sh"))
        .unwrap()
        .permissions();
    perms.set_mode(0o755);
    fs::set_permissions(repo_dir.join("script.sh"), perms).unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink("README.md", repo_dir.join("link")).unwrap();
    run_at(&repo_dir, "git", &["add", "."]).unwrap();
    run_at(&repo_dir, "git", &["commit", "-m", "initial"]).unwrap();
    let repo = GitRepo::open(&repo_dir);
    let parent = repo.rev_parse("refs/heads/main").unwrap();
    (repo_dir, repo, parent)
}
fn apply_edits(merged: &Path) {
    fs::write(merged.join("README.md"), "modified").unwrap();
    fs::write(merged.join("added.txt"), "added").unwrap();
    fs::write(merged.join("binary.bin"), [0, 1, 2, 255]).unwrap();
    fs::remove_file(merged.join("delete.txt")).unwrap();
    fs::rename(merged.join("rename.txt"), merged.join("renamed.txt")).unwrap();
    fs::remove_dir_all(merged.join("dir")).unwrap();
    fs::write(merged.join("dir"), "replacement file").unwrap();
    fs::write(merged.join("script.sh"), "#!/bin/sh\nexit 1\n").unwrap();
    let mut perms = fs::metadata(merged.join("script.sh"))
        .unwrap()
        .permissions();
    perms.set_mode(0o755);
    fs::set_permissions(merged.join("script.sh"), perms).unwrap();
    let _ = fs::remove_file(merged.join("link"));
    #[cfg(unix)]
    std::os::unix::fs::symlink("README.md", merged.join("link")).unwrap();
}
fn fallback_tree(repo: &GitRepo, parent: &str, root: &Path) -> String {
    let paths = BackendPaths::new(
        root.join("lower"),
        root.join("upper"),
        root.join("work"),
        root.join("merged"),
    );
    repo.export_tree(parent, &paths.lower).unwrap();
    DirectoryMergeBackend.mount(&paths).unwrap();
    apply_edits(&paths.merged);
    DirectoryMergeBackend
        .sync_before_upper_read(&paths)
        .unwrap();
    repo.tree_from_layer(parent, &paths.upper).unwrap()
}

#[test]
fn fallback_backend_tree_matches_expected_git_tree() {
    let (repo_dir, repo, parent) = setup_repo();
    let root = temp("fallback");
    let tree = fallback_tree(&repo, &parent, &root);
    assert_eq!(
        Command::new("git")
            .args(["show", &format!("{tree}:README.md")])
            .current_dir(&repo_dir)
            .output()
            .unwrap()
            .stdout,
        b"modified"
    );
    assert_eq!(
        Command::new("git")
            .args(["show", &format!("{tree}:binary.bin")])
            .current_dir(&repo_dir)
            .output()
            .unwrap()
            .stdout,
        vec![0, 1, 2, 255]
    );
    assert_eq!(
        Command::new("git")
            .args(["show", &format!("{tree}:dir")])
            .current_dir(&repo_dir)
            .output()
            .unwrap()
            .stdout,
        b"replacement file"
    );
    assert!(
        Command::new("git")
            .args(["cat-file", "-e", &format!("{tree}:delete.txt")])
            .current_dir(&repo_dir)
            .output()
            .unwrap()
            .status
            .code()
            != Some(0)
    );
    fs::remove_dir_all(repo_dir).unwrap();
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn overlayfs_backend_tree_matches_fallback_tree_when_enabled() {
    if std::env::var_os("KAGE_TEST_OVERLAY").is_none() || !cfg!(target_os = "linux") {
        eprintln!("skipping overlayfs integration test; set KAGE_TEST_OVERLAY=1 on Linux with mount privileges");
        return;
    }
    let (repo_dir, repo, parent) = setup_repo();
    let fallback_root = temp("fallback");
    let expected = fallback_tree(&repo, &parent, &fallback_root);
    let root = temp("overlay");
    let paths = BackendPaths::new(
        root.join("lower"),
        root.join("upper"),
        root.join("work"),
        root.join("merged"),
    );
    repo.export_tree(&parent, &paths.lower).unwrap();
    if let Err(err) = LinuxOverlayBackend.mount(&paths) {
        let _ = fs::remove_dir_all(repo_dir);
        let _ = fs::remove_dir_all(fallback_root);
        let _ = fs::remove_dir_all(root);
        if std::env::var_os("KAGE_TEST_OVERLAY_ALLOW_SKIP").is_some() {
            eprintln!(
                "WARNING: skipping overlayfs integration body because mount is unavailable: {err}"
            );
            return;
        }
        panic!("KAGE_TEST_OVERLAY=1 requires a real overlay mount: {err}");
    }
    assert_eq!(
        fs::read_to_string(paths.merged.join("README.md")).unwrap(),
        "hello"
    );
    apply_edits(&paths.merged);
    assert_eq!(
        fs::read_to_string(paths.upper.join("README.md")).unwrap(),
        "modified"
    );
    assert!(paths.upper.join("added.txt").exists());
    assert!(!paths.merged.join("delete.txt").exists());
    assert!(paths.upper.join("renamed.txt").exists());
    LinuxOverlayBackend.unmount(&paths).unwrap();
    LinuxOverlayBackend.unmount(&paths).unwrap();
    assert!(paths.upper.join("README.md").exists());
    let actual = repo.tree_from_layer(&parent, &paths.upper).unwrap();
    assert_eq!(actual, expected);
    fs::remove_dir_all(repo_dir).unwrap();
    fs::remove_dir_all(fallback_root).unwrap();
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn overlay_xattr_whiteout_and_opaque_directory_when_enabled() {
    if std::env::var_os("KAGE_TEST_OVERLAY_XATTR").is_none() {
        eprintln!("skipping xattr whiteout/opaque test; set KAGE_TEST_OVERLAY_XATTR=1 on a filesystem that permits trusted.overlay.* xattrs");
        return;
    }
    let (repo_dir, repo, parent) = setup_repo();
    let upper = temp("xattr-upper");
    fs::create_dir_all(upper.join("dir")).unwrap();
    let whiteout = upper.join("delete.txt");
    fs::write(&whiteout, []).unwrap();
    let status = Command::new("setfattr")
        .args(["-n", "trusted.overlay.whiteout", "-v", "y"])
        .arg(&whiteout)
        .status()
        .expect("setfattr must be installed for KAGE_TEST_OVERLAY_XATTR=1");
    assert!(status.success(), "setting trusted.overlay.whiteout failed");
    let status = Command::new("setfattr")
        .args(["-n", "trusted.overlay.opaque", "-v", "y"])
        .arg(upper.join("dir"))
        .status()
        .expect("setfattr must be installed for KAGE_TEST_OVERLAY_XATTR=1");
    assert!(status.success(), "setting trusted.overlay.opaque failed");
    fs::write(upper.join("dir/new.txt"), "new opaque content").unwrap();

    let tree = repo.tree_from_layer(&parent, &upper).unwrap();
    assert!(
        Command::new("git")
            .args(["cat-file", "-e", &format!("{tree}:delete.txt")])
            .current_dir(&repo_dir)
            .output()
            .unwrap()
            .status
            .code()
            != Some(0)
    );
    assert!(
        Command::new("git")
            .args(["cat-file", "-e", &format!("{tree}:dir/sub/file.txt")])
            .current_dir(&repo_dir)
            .output()
            .unwrap()
            .status
            .code()
            != Some(0)
    );
    assert_eq!(
        Command::new("git")
            .args(["show", &format!("{tree}:dir/new.txt")])
            .current_dir(&repo_dir)
            .output()
            .unwrap()
            .stdout,
        b"new opaque content"
    );
    fs::remove_dir_all(repo_dir).unwrap();
    fs::remove_dir_all(upper).unwrap();
}
