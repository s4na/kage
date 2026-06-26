use std::{
    fs,
    path::PathBuf,
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};

fn temp(name: &str) -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!("kage-cli-{name}-{nonce}"))
}

fn git(repo: &PathBuf, args: &[&str]) {
    let status = Command::new("git")
        .args(args)
        .current_dir(repo)
        .status()
        .unwrap();
    assert!(status.success(), "git {args:?} failed");
}

fn git_out(repo: &PathBuf, args: &[&str]) -> String {
    let out = Command::new("git")
        .args(args)
        .current_dir(repo)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn setup_repo() -> PathBuf {
    let repo = temp("repo");
    fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "-b", "main"]);
    git(&repo, &["config", "user.email", "kage@example.invalid"]);
    git(&repo, &["config", "user.name", "kage test"]);
    fs::write(repo.join("README.md"), "hello").unwrap();
    fs::write(repo.join("delete.txt"), "delete").unwrap();
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-m", "initial"]);
    repo
}

fn kage(home: &PathBuf, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_kage"))
        .arg("--home")
        .arg(home)
        .args(args)
        .output()
        .unwrap()
}

#[test]
fn workspace_lifecycle_diff_commit_and_discard() {
    let repo = setup_repo();
    let home = temp("home");

    let out = kage(
        &home,
        &[
            "workspace",
            "create",
            "--ref",
            "main",
            "--repo",
            repo.to_str().unwrap(),
            "--id",
            "ws_a",
        ],
    );
    assert!(
        out.status.success(),
        "create failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let merged = home.join("workspaces/ws_a/merged");
    let upper = home.join("workspaces/ws_a/upper");
    assert!(merged.join("README.md").exists());
    assert!(upper.exists());

    fs::write(merged.join("README.md"), "modified").unwrap();
    fs::write(merged.join("added.txt"), "added").unwrap();
    fs::remove_file(merged.join("delete.txt")).unwrap();

    let out = kage(&home, &["workspace", "diff", "ws_a"]);
    assert!(
        out.status.success(),
        "diff failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let diff = String::from_utf8_lossy(&out.stdout);
    assert!(diff.contains("M\tREADME.md"), "diff was {diff}");
    assert!(diff.contains("A\tadded.txt"), "diff was {diff}");
    assert!(diff.contains("D\tdelete.txt"), "diff was {diff}");

    let out = kage(
        &home,
        &["workspace", "commit", "ws_a", "-m", "workspace commit"],
    );
    assert!(
        out.status.success(),
        "commit failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(git_out(&repo, &["show", "main:README.md"]), "modified");
    assert_eq!(git_out(&repo, &["show", "main:added.txt"]), "added");
    assert_eq!(
        git_out(
            &repo,
            &["ls-tree", "--name-only", "main", "--", "delete.txt"]
        ),
        ""
    );

    let out = kage(&home, &["workspace", "discard", "ws_a"]);
    assert!(
        out.status.success(),
        "discard failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(!home.join("workspaces/ws_a").exists());
    fs::remove_dir_all(repo).unwrap();
    fs::remove_dir_all(home).unwrap();
}

#[test]
fn two_workspaces_from_same_ref_have_isolated_layers_and_conflict_on_second_commit() {
    let repo = setup_repo();
    let home = temp("home");
    for id in ["ws_a", "ws_b"] {
        let out = kage(
            &home,
            &[
                "workspace",
                "create",
                "--ref",
                "main",
                "--repo",
                repo.to_str().unwrap(),
                "--id",
                id,
            ],
        );
        assert!(
            out.status.success(),
            "create {id} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    fs::write(home.join("workspaces/ws_a/merged/README.md"), "from a").unwrap();
    fs::write(home.join("workspaces/ws_b/merged/README.md"), "from b").unwrap();
    assert_ne!(
        fs::read_to_string(home.join("workspaces/ws_a/merged/README.md")).unwrap(),
        fs::read_to_string(home.join("workspaces/ws_b/merged/README.md")).unwrap()
    );

    let out = kage(&home, &["workspace", "commit", "ws_a", "-m", "from a"]);
    assert!(
        out.status.success(),
        "commit A failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let out = kage(&home, &["workspace", "commit", "ws_b", "-m", "from b"]);
    assert!(!out.status.success(), "stale commit should fail");
    assert!(String::from_utf8_lossy(&out.stderr).contains("ref advanced"));
    fs::remove_dir_all(repo).unwrap();
    fs::remove_dir_all(home).unwrap();
}

#[test]
fn workspace_create_records_exported_lower_and_rejects_git_rofs_mount_until_fuse_exists() {
    let repo = setup_repo();
    let home = temp("home");
    let out = kage(
        &home,
        &[
            "workspace",
            "create",
            "--ref",
            "main",
            "--repo",
            repo.to_str().unwrap(),
            "--id",
            "ws_exported",
            "--lower",
            "exported",
        ],
    );
    assert!(
        out.status.success(),
        "exported lower create failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let metadata = fs::read_to_string(home.join("workspaces/ws_exported/workspace.tsv")).unwrap();
    assert!(metadata.contains("lower_kind\texported"));

    let out = kage(
        &home,
        &[
            "workspace",
            "create",
            "--ref",
            "main",
            "--repo",
            repo.to_str().unwrap(),
            "--id",
            "ws_rofs",
            "--lower",
            "git-rofs",
        ],
    );
    assert!(
        !out.status.success(),
        "git-rofs workspace mount should currently fail clearly"
    );
    assert!(String::from_utf8_lossy(&out.stderr)
        .contains("rofs filesystem mount is not implemented yet"));
    fs::remove_dir_all(repo).unwrap();
    fs::remove_dir_all(home).unwrap();
}
