use kage_container::{
    ensure_workspace_mountable, run as run_container, ContainerRunSpec, ContainerRuntime,
};
use kage_core::{
    list_workspaces, read_workspace, remove_workspace, write_workspace, RuntimePaths, WorkspaceSpec,
};
use kage_git::GitRepo;
use kage_overlay::{backend_for, BackendKind, BackendPaths};
use std::{env, path::PathBuf};
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
            };
            repo.export_tree(&ws.parent_commit, &ws.lower)?;
            std::fs::create_dir_all(&ws.upper)?;
            std::fs::create_dir_all(&ws.work)?;
            let backend_paths = BackendPaths::new(&ws.lower, &ws.upper, &ws.work, &ws.merged);
            backend_for(backend_kind).mount(&backend_paths)?;
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
            backend_for(kind).refresh_upper_from_merged(&BackendPaths::new(
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
            backend_for(kind).refresh_upper_from_merged(&BackendPaths::new(
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
