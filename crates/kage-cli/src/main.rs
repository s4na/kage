use kage_container::{
    ensure_workspace_mountable, run as run_container, ContainerRunSpec, ContainerRuntime,
};
use kage_core::{
    list_workspaces, read_workspace, remove_workspace, write_workspace, LowerKind, RuntimePaths,
    WorkspaceSpec,
};
use kage_git::GitRepo;
use kage_overlay::{backend_for, BackendKind, BackendPaths};
use std::{
    env, fs,
    os::unix::ffi::OsStringExt,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};
type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

fn main() -> Result<()> {
    let mut args: Vec<String> = env::args().skip(1).collect();
    let home = if args.first().is_some_and(|a| a == "--home") {
        args.drain(0..2)
            .nth(1)
            .map(PathBuf::from)
            .unwrap_or_else(|| ".kage".into())
    } else {
        env::var_os("KAGE_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| ".kage".into())
    };
    let paths = RuntimePaths::new(home);
    match args.first().map(String::as_str) {
        Some("repo") => repo(&args[1..]),
        Some("workspace") => workspace(paths, &args[1..]),
        Some("exec") => exec(paths, &args[1..]),
        Some("run") => run_workspace_container(paths, &args[1..]),
        Some("rofs-serve") => rofs_serve(&args[1..]),
        _ => {
            eprintln!("usage: kage [--home DIR] <repo|workspace|exec|run> ...");
            Ok(())
        }
    }
}

fn repo(args: &[String]) -> Result<()> {
    match args.first().map(String::as_str) {
        Some("init") => {
            let path = PathBuf::from(args.get(1).map(String::as_str).unwrap_or("."));
            let url = flag(args, "--url");
            GitRepo::init(&path, url.as_deref())?;
            println!("initialized {}", path.display());
            Ok(())
        }
        Some("fetch") => {
            GitRepo::open(args.get(1).map(PathBuf::from).unwrap_or_else(|| ".".into())).fetch()
        }
        Some("status") => {
            let repo = GitRepo::open(args.get(1).map(PathBuf::from).unwrap_or_else(|| ".".into()));
            println!("HEAD {}", repo.rev_parse("HEAD")?);
            Ok(())
        }
        _ => Err("usage: kage repo <init|fetch|status>".into()),
    }
}

fn workspace(paths: RuntimePaths, args: &[String]) -> Result<()> {
    paths.ensure()?;
    match args.first().map(String::as_str) {
        Some("create") => {
            let reference = flag(args, "--reference")
                .or_else(|| flag(args, "--ref"))
                .unwrap_or_else(|| "HEAD".into());
            let id = flag(args, "--id");
            let backend_kind = BackendKind::parse(
                &flag(args, "--backend")
                    .or_else(|| env::var("KAGE_BACKEND").ok())
                    .unwrap_or_else(|| "fallback".to_string()),
            )?;
            let lower_kind = LowerKind::parse(
                &flag(args, "--lower")
                    .or_else(|| env::var("KAGE_LOWER").ok())
                    .unwrap_or_else(|| "exported".to_string()),
            )?;
            let repo_path =
                PathBuf::from(flag(args, "--repo").unwrap_or_else(|| ".".into())).canonicalize()?;
            let repo = GitRepo::open(repo_path);
            let parent = repo.rev_parse(&reference)?;
            let id = id.unwrap_or_else(|| format!("ws-{}", &parent[..12]));
            let root = paths.workspace_dir(&id);
            let ws = WorkspaceSpec {
                id: id.clone(),
                repo: repo.path.clone(),
                reference,
                parent_commit: parent,
                lower: root.join("lower"),
                upper: root.join("upper"),
                work: root.join("work"),
                merged: root.join("merged"),
                backend: backend_kind.as_str().to_string(),
                lower_kind: lower_kind.as_str().to_string(),
            };
            match lower_kind {
                LowerKind::Exported => repo.export_tree(&ws.parent_commit, &ws.lower)?,
                LowerKind::GitRofs => {
                    if backend_kind != BackendKind::OverlayFs {
                        return Err("--lower git-rofs currently requires --backend overlayfs because the fallback backend needs an exported lower directory".into());
                    }
                    start_rofs_daemon(&ws)?;
                }
            }
            std::fs::create_dir_all(&ws.upper)?;
            std::fs::create_dir_all(&ws.work)?;
            let backend_paths = BackendPaths::new(&ws.lower, &ws.upper, &ws.work, &ws.merged);
            if let Err(err) = backend_for(backend_kind).mount(&backend_paths) {
                let _ = stop_rofs_daemon(&root);
                return Err(err);
            }
            write_workspace(&paths, &ws)?;
            println!("{} {}", ws.id, ws.merged.display());
            Ok(())
        }
        Some("list") => {
            for ws in list_workspaces(&paths)? {
                println!("{} {} {}", ws.id, ws.reference, ws.merged.display());
            }
            Ok(())
        }
        Some("mount") => {
            let ws = read_workspace(&paths, need(args, 1)?)?;
            let kind = BackendKind::parse(&ws.backend)?;
            backend_for(kind).mount(&BackendPaths::new(
                &ws.lower, &ws.upper, &ws.work, &ws.merged,
            ))?;
            println!("{}", ws.merged.display());
            Ok(())
        }
        Some("diff") => {
            let ws = read_workspace(&paths, need(args, 1)?)?;
            let kind = BackendKind::parse(&ws.backend)?;
            backend_for(kind).sync_before_upper_read(&BackendPaths::new(
                &ws.lower, &ws.upper, &ws.work, &ws.merged,
            ))?;
            let repo = GitRepo::open(&ws.repo);
            for entry in repo.layer_diff_name_status(&ws.parent_commit, &ws.upper)? {
                println!("{}\t{}", entry.status, entry.path.display());
            }
            Ok(())
        }
        Some("commit") => {
            let ws = read_workspace(&paths, need(args, 1)?)?;
            let msg = flag(args, "-m")
                .or_else(|| flag(args, "--message"))
                .ok_or("missing -m/--message")?;
            let kind = BackendKind::parse(&ws.backend)?;
            backend_for(kind).sync_before_upper_read(&BackendPaths::new(
                &ws.lower, &ws.upper, &ws.work, &ws.merged,
            ))?;
            println!(
                "{}",
                GitRepo::open(&ws.repo).commit_from_layer(
                    &ws.reference,
                    &ws.parent_commit,
                    &ws.upper,
                    &msg
                )?
            );
            Ok(())
        }
        Some("discard") => {
            let id = need(args, 1)?;
            if let Ok(ws) = read_workspace(&paths, id) {
                let kind = BackendKind::parse(&ws.backend)?;
                backend_for(kind).unmount(&BackendPaths::new(
                    &ws.lower, &ws.upper, &ws.work, &ws.merged,
                ))?;
                let _ = stop_rofs_daemon(&paths.workspace_dir(id));
            }
            remove_workspace(&paths, id)
        }
        Some("gc") => {
            println!("gc complete");
            Ok(())
        }
        _ => Err("usage: kage workspace <create|list|mount|diff|commit|discard|gc>".into()),
    }
}
fn rofs_serve(args: &[String]) -> Result<()> {
    if env::var_os("KAGE_TEST_ROFS_FORCE_STARTUP_FAILURE").is_some() {
        return Err("forced kage-rofs startup failure for test".into());
    }
    let repo = PathBuf::from(flag(args, "--repo").ok_or("missing --repo")?);
    let reference = flag(args, "--ref").ok_or("missing --ref")?;
    let mountpoint = PathBuf::from(flag(args, "--mountpoint").ok_or("missing --mountpoint")?);
    let view = kage_rofs::GitTreeView::open(repo, &reference)?;
    let _mount = kage_rofs::mount_rofs_strict(&view, &mountpoint)?;
    loop {
        thread::park_timeout(Duration::from_secs(3600));
    }
}

fn start_rofs_daemon(ws: &WorkspaceSpec) -> Result<()> {
    fs::create_dir_all(&ws.lower)?;
    let exe = env::current_exe()?;
    let root = ws
        .lower
        .parent()
        .ok_or("workspace lower path has no parent directory")?;
    let pid_path = root.join("rofs.pid");
    let err_path = root.join("rofs.stderr");
    let err = fs::File::create(&err_path)?;
    let mut child = rofs_serve_command(&exe, ws, err).spawn()?;
    fs::write(&pid_path, child.id().to_string())?;
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if let Some(status) = child.try_wait()? {
            let stderr = fs::read_to_string(&err_path).unwrap_or_default();
            let _ = fs::remove_file(&pid_path);
            return Err(
                format!("kage-rofs daemon exited during mount with {status}: {stderr}").into(),
            );
        }
        if rofs_mount_ready(&ws.lower) {
            return Ok(());
        }
        thread::sleep(Duration::from_millis(50));
    }
    let _ = child.kill();
    let _ = child.wait();
    let _ = fs::remove_file(&pid_path);
    Err("timed out waiting for kage-rofs mount to become readable".into())
}

fn rofs_serve_command(exe: &Path, ws: &WorkspaceSpec, stderr: fs::File) -> Command {
    let mut command = Command::new(exe);
    command
        .arg("rofs-serve")
        .arg("--repo")
        .arg(&ws.repo)
        .arg("--ref")
        .arg(&ws.parent_commit)
        .arg("--mountpoint")
        .arg(&ws.lower)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::from(stderr));
    command
}

fn rofs_mount_ready(mountpoint: &Path) -> bool {
    path_is_mountpoint(mountpoint).unwrap_or(false)
}

fn path_is_mountpoint(path: &Path) -> Result<bool> {
    let needle = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let mountinfo = fs::read_to_string("/proc/self/mountinfo")?;
    Ok(mountinfo.lines().any(|line| {
        line.split_whitespace()
            .nth(4)
            .map(unescape_mountinfo_path)
            .is_some_and(|mount| mount == needle)
    }))
}

fn unescape_mountinfo_path(raw: &str) -> PathBuf {
    let bytes = raw.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut idx = 0;
    while idx < bytes.len() {
        if bytes[idx] == b'\\' && idx + 3 < bytes.len() {
            if let Ok(octal) = std::str::from_utf8(&bytes[idx + 1..idx + 4]) {
                if let Ok(value) = u8::from_str_radix(octal, 8) {
                    out.push(value);
                    idx += 4;
                    continue;
                }
            }
        }
        out.push(bytes[idx]);
        idx += 1;
    }
    PathBuf::from(std::ffi::OsString::from_vec(out))
}

fn stop_rofs_daemon(root: &Path) -> Result<()> {
    let pid_path = root.join("rofs.pid");
    let Ok(pid) = fs::read_to_string(&pid_path) else {
        return Ok(());
    };
    let pid = pid.trim();
    if !pid.is_empty() {
        let _ = Command::new("kill").arg(pid).status();
    }
    let _ = fs::remove_file(pid_path);
    Ok(())
}

fn exec(paths: RuntimePaths, args: &[String]) -> Result<()> {
    let ws = read_workspace(&paths, need(args, 0)?)?;
    let split = args.iter().position(|a| a == "--").unwrap_or(1);
    let cmd = &args[split + 1..];
    if cmd.is_empty() {
        return Err("no command provided".into());
    }
    let status = std::process::Command::new(&cmd[0])
        .args(&cmd[1..])
        .current_dir(&ws.merged)
        .status()?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("command exited with {status}").into())
    }
}
fn run_workspace_container(paths: RuntimePaths, args: &[String]) -> Result<()> {
    let workspace_id = need(args, 0)?;
    let ws = read_workspace(&paths, workspace_id)?;
    ensure_workspace_mountable(&ws.merged)?;
    let runtime = if args.iter().any(|a| a == "--apple-container") {
        ContainerRuntime::AppleContainer
    } else if args.iter().any(|a| a == "--podman") {
        ContainerRuntime::Podman
    } else {
        ContainerRuntime::Docker
    };
    let image = flag(args, "--image").ok_or("missing --image")?;
    let split = args
        .iter()
        .position(|a| a == "--")
        .ok_or("missing -- before command")?;
    let command = args[split + 1..].to_vec();
    if command.is_empty() {
        return Err("no container command provided".into());
    }
    let spec = ContainerRunSpec::new(runtime, &ws.merged, image, command);
    let status = run_container(&spec)?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("container exited with {status}").into())
    }
}

fn flag(args: &[String], name: &str) -> Option<String> {
    args.windows(2).find(|w| w[0] == name).map(|w| w[1].clone())
}
fn need(args: &[String], idx: usize) -> Result<&str> {
    args.get(idx)
        .map(String::as_str)
        .ok_or_else(|| "missing argument".into())
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
        std::env::temp_dir().join(format!("kage-cli-unit-{name}-{nonce}"))
    }

    fn spec(root: &Path) -> WorkspaceSpec {
        WorkspaceSpec {
            id: "ws".to_string(),
            repo: PathBuf::from("/repo path/with spaces"),
            reference: "main".to_string(),
            parent_commit: "abc123".to_string(),
            lower: root.join("lower"),
            upper: root.join("upper"),
            work: root.join("work"),
            merged: root.join("merged"),
            backend: "overlayfs".to_string(),
            lower_kind: "git-rofs".to_string(),
        }
    }

    #[test]
    fn rofs_serve_command_uses_argument_array() {
        let root = temp("cmd");
        fs::create_dir_all(&root).unwrap();
        let stderr = fs::File::create(root.join("stderr")).unwrap();
        let ws = spec(&root);
        let command = rofs_serve_command(Path::new("/bin/kage"), &ws, stderr);
        let args: Vec<_> = command
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();
        assert_eq!(args[0], "rofs-serve");
        assert_eq!(args[1], "--repo");
        assert_eq!(args[2], "/repo path/with spaces");
        assert!(args.contains(&"--mountpoint".to_string()));
        assert!(!args.iter().any(|arg| arg.contains("sh -c")));
        fs::remove_dir_all(root).unwrap();
    }

    #[derive(Default)]
    struct FakeRuntimeLifecycle {
        events: Vec<&'static str>,
        overlay_fails: bool,
        commit_fails: bool,
        metadata_written: bool,
        ref_updated: bool,
    }

    impl FakeRuntimeLifecycle {
        fn create(&mut self) -> Result<()> {
            self.events.push("start-rofs");
            self.events.push("mount-overlay");
            if self.overlay_fails {
                self.events.push("stop-rofs");
                return Err("overlay mount failed".into());
            }
            self.metadata_written = true;
            self.events.push("write-metadata");
            Ok(())
        }

        fn discard(&mut self) {
            self.events.push("unmount-overlay");
            self.events.push("stop-rofs");
            self.events.push("remove-metadata");
            self.metadata_written = false;
        }

        fn commit(&mut self) -> Result<()> {
            self.events.push("commit-from-upper");
            if self.commit_fails {
                return Err("commit failed before ref update".into());
            }
            self.ref_updated = true;
            self.events.push("update-ref");
            Ok(())
        }
    }

    #[test]
    fn fake_runtime_lifecycle_rolls_back_rofs_when_overlay_mount_fails() {
        let mut lifecycle = FakeRuntimeLifecycle {
            overlay_fails: true,
            ..Default::default()
        };
        assert!(lifecycle.create().is_err());
        assert_eq!(
            lifecycle.events,
            ["start-rofs", "mount-overlay", "stop-rofs"]
        );
        assert!(!lifecycle.metadata_written);
    }

    #[test]
    fn fake_runtime_lifecycle_records_metadata_unmounts_in_order_and_preserves_failed_commit_mounts(
    ) {
        let mut lifecycle = FakeRuntimeLifecycle::default();
        lifecycle.create().unwrap();
        assert!(lifecycle.metadata_written);
        assert_eq!(
            lifecycle.events,
            ["start-rofs", "mount-overlay", "write-metadata"]
        );

        lifecycle.commit_fails = true;
        assert!(lifecycle.commit().is_err());
        assert!(!lifecycle.ref_updated);
        assert!(
            lifecycle.metadata_written,
            "failed commit must not tear down a mounted workspace"
        );

        lifecycle.discard();
        lifecycle.discard();
        assert!(lifecycle
            .events
            .windows(2)
            .any(|w| w == ["unmount-overlay", "stop-rofs"]));
        assert!(!lifecycle.metadata_written);
    }

    #[test]
    fn rofs_mount_ready_rejects_plain_directory_and_decodes_mountinfo_escapes() {
        let root = temp("mount-ready");
        fs::create_dir_all(&root).unwrap();
        assert!(!rofs_mount_ready(&root));
        assert_eq!(
            unescape_mountinfo_path("/tmp/path\\040with\\011space"),
            PathBuf::from("/tmp/path with\tspace")
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn stop_rofs_daemon_is_idempotent_for_stale_pid() {
        let root = temp("stop");
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("rofs.pid"), "999999").unwrap();
        stop_rofs_daemon(&root).unwrap();
        stop_rofs_daemon(&root).unwrap();
        assert!(!root.join("rofs.pid").exists());
        fs::remove_dir_all(root).unwrap();
    }
}
