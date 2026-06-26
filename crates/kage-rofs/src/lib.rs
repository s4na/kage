use kage_core::validate_relative_path;
use std::{
    path::{Path, PathBuf},
    process::Command,
};

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GitEntryKind {
    Tree,
    Blob,
    Symlink,
    Executable,
    Gitlink,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitMetadata {
    pub path: PathBuf,
    pub mode: u32,
    pub oid: String,
    pub kind: GitEntryKind,
    pub size: Option<u64>,
}

impl GitMetadata {
    pub fn is_dir(&self) -> bool {
        self.kind == GitEntryKind::Tree
    }
    pub fn is_file(&self) -> bool {
        matches!(self.kind, GitEntryKind::Blob | GitEntryKind::Executable)
    }
    pub fn is_symlink(&self) -> bool {
        self.kind == GitEntryKind::Symlink
    }
    pub fn is_executable(&self) -> bool {
        self.kind == GitEntryKind::Executable
    }
}

#[derive(Debug, Clone)]
pub struct GitTreeView {
    repo: PathBuf,
    commit: String,
    tree: String,
}

impl GitTreeView {
    pub fn open(repo: impl Into<PathBuf>, reference: &str) -> Result<Self> {
        let repo = repo.into();
        let commit = git_output(&repo, &["rev-parse", reference])?
            .trim()
            .to_string();
        let tree = git_output(&repo, &["rev-parse", &format!("{commit}^{{tree}}")])?
            .trim()
            .to_string();
        Ok(Self { repo, commit, tree })
    }

    pub fn commit(&self) -> &str {
        &self.commit
    }
    pub fn tree(&self) -> &str {
        &self.tree
    }

    pub fn lookup(&self, path: &Path) -> Result<GitMetadata> {
        if path.as_os_str().is_empty() || path == Path::new(".") {
            return Ok(GitMetadata {
                path: PathBuf::new(),
                mode: 0o040000,
                oid: self.tree.clone(),
                kind: GitEntryKind::Tree,
                size: None,
            });
        }
        validate_relative_path(path)?;
        let entries = self.ls_tree(Some(path), false)?;
        entries
            .into_iter()
            .next()
            .ok_or_else(|| format!("path not found in Git tree: {}", path.display()).into())
    }

    pub fn read_dir(&self, path: &Path) -> Result<Vec<GitMetadata>> {
        if !path.as_os_str().is_empty() && path != Path::new(".") {
            let meta = self.lookup(path)?;
            if !meta.is_dir() {
                return Err(format!("not a directory: {}", path.display()).into());
            }
        }
        self.ls_tree(
            if path.as_os_str().is_empty() || path == Path::new(".") {
                None
            } else {
                Some(path)
            },
            true,
        )
    }

    pub fn read_file(&self, path: &Path, offset: u64, size: usize) -> Result<Vec<u8>> {
        let meta = self.lookup(path)?;
        if !meta.is_file() {
            return Err(format!("not a regular file: {}", path.display()).into());
        }
        let bytes = git_bytes(&self.repo, &["cat-file", "-p", meta.oid.as_str()])?;
        let start = usize::try_from(offset)
            .unwrap_or(usize::MAX)
            .min(bytes.len());
        let end = start.saturating_add(size).min(bytes.len());
        Ok(bytes[start..end].to_vec())
    }

    pub fn read_link(&self, path: &Path) -> Result<PathBuf> {
        let meta = self.lookup(path)?;
        if !meta.is_symlink() {
            return Err(format!("not a symlink: {}", path.display()).into());
        }
        let bytes = git_bytes(&self.repo, &["cat-file", "-p", meta.oid.as_str()])?;
        Ok(PathBuf::from(String::from_utf8(bytes)?))
    }

    fn ls_tree(&self, path: Option<&Path>, directory_contents: bool) -> Result<Vec<GitMetadata>> {
        let mut args = vec!["ls-tree", "-z"];
        let target;
        if directory_contents {
            target = match path {
                Some(p) => format!("{}:{}", self.tree, path_arg(p)?),
                None => self.tree.clone(),
            };
            args.push(target.as_str());
        } else {
            target = self.tree.clone();
            args.push(target.as_str());
            args.push("--");
        }
        let path_string;
        if !directory_contents {
            if let Some(p) = path {
                path_string = path_arg(p)?;
                args.push(path_string.as_str());
            }
        }
        let out = git_bytes(&self.repo, &args)?;
        parse_ls_tree(
            &self.repo,
            path.unwrap_or_else(|| Path::new("")),
            &out,
            directory_contents,
        )
    }
}

fn parse_ls_tree(
    repo: &Path,
    requested: &Path,
    bytes: &[u8],
    directory_contents: bool,
) -> Result<Vec<GitMetadata>> {
    let mut out = Vec::new();
    for raw in bytes.split(|b| *b == 0).filter(|raw| !raw.is_empty()) {
        let Some(tab) = raw.iter().position(|b| *b == b'\t') else {
            continue;
        };
        let header = String::from_utf8(raw[..tab].to_vec())?;
        let name = String::from_utf8(raw[tab + 1..].to_vec())?;
        let mut parts = header.split_whitespace();
        let mode_text = parts.next().ok_or("missing ls-tree mode")?;
        let ty = parts.next().ok_or("missing ls-tree type")?;
        let oid = parts.next().ok_or("missing ls-tree oid")?.to_string();
        let mode = u32::from_str_radix(mode_text, 8)?;
        let kind = match (mode_text, ty) {
            ("040000", "tree") => GitEntryKind::Tree,
            ("100755", "blob") => GitEntryKind::Executable,
            ("100644", "blob") | ("100664", "blob") => GitEntryKind::Blob,
            ("120000", "blob") => GitEntryKind::Symlink,
            ("160000", "commit") => GitEntryKind::Gitlink,
            _ => {
                return Err(format!(
                    "unsupported Git tree entry mode/type: {mode_text} {ty} {name}"
                )
                .into())
            }
        };
        let path = if directory_contents {
            if requested.as_os_str().is_empty() {
                PathBuf::from(name)
            } else {
                requested.join(name)
            }
        } else {
            PathBuf::from(name)
        };
        let size = if matches!(
            kind,
            GitEntryKind::Blob | GitEntryKind::Executable | GitEntryKind::Symlink
        ) {
            Some(
                git_output(repo, &["cat-file", "-s", oid.as_str()])?
                    .trim()
                    .parse()?,
            )
        } else {
            None
        };
        out.push(GitMetadata {
            path,
            mode,
            oid,
            kind,
            size,
        });
    }
    Ok(out)
}

fn path_arg(path: &Path) -> Result<String> {
    validate_relative_path(path)?;
    Ok(path.to_string_lossy().into_owned())
}

fn git_output(repo: &Path, args: &[&str]) -> Result<String> {
    Ok(String::from_utf8(git_bytes(repo, args)?)?)
}

fn git_bytes(repo: &Path, args: &[&str]) -> Result<Vec<u8>> {
    let out = Command::new("git").args(args).current_dir(repo).output()?;
    if out.status.success() {
        Ok(out.stdout)
    } else {
        Err(format!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr)
        )
        .into())
    }
}

pub fn rofs_mount_available() -> bool {
    Path::new("/dev/fuse").exists()
}

pub fn mount_rofs_strict(_view: &GitTreeView, _mountpoint: &Path) -> Result<()> {
    Err("kage-rofs FUSE mount is not implemented yet; GitTreeView is available as a lazy read-only model".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        fs,
        os::unix::fs::PermissionsExt,
        time::{SystemTime, UNIX_EPOCH},
    };

    fn temp(name: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("kage-rofs-{name}-{nonce}"))
    }

    fn run(repo: &Path, args: &[&str]) {
        let status = Command::new("git")
            .args(args)
            .current_dir(repo)
            .status()
            .unwrap();
        assert!(status.success(), "git {args:?} failed");
    }

    fn fixture_repo() -> PathBuf {
        let repo = temp("repo");
        fs::create_dir_all(repo.join("nested")).unwrap();
        run(&repo, &["init", "-b", "main"]);
        run(&repo, &["config", "user.email", "kage@example.invalid"]);
        run(&repo, &["config", "user.name", "kage test"]);
        fs::write(repo.join("README.md"), "hello world").unwrap();
        fs::write(repo.join("binary.bin"), [0, 1, 2, 255]).unwrap();
        fs::write(repo.join("run.sh"), "#!/bin/sh\nexit 0\n").unwrap();
        let mut perms = fs::metadata(repo.join("run.sh")).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(repo.join("run.sh"), perms).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink("README.md", repo.join("link")).unwrap();
        fs::write(repo.join("nested/file with spaces.txt"), "spaces").unwrap();
        fs::write(repo.join("nested/ユニコード.txt"), "unicode").unwrap();
        run(&repo, &["add", "."]);
        run(&repo, &["commit", "-m", "initial"]);
        repo
    }

    #[test]
    fn git_tree_view_reads_files_modes_symlinks_and_directories() {
        let repo = fixture_repo();
        let view = GitTreeView::open(&repo, "main").unwrap();
        assert_eq!(
            view.read_file(Path::new("README.md"), 0, 5).unwrap(),
            b"hello"
        );
        assert_eq!(
            view.read_file(Path::new("binary.bin"), 0, 99).unwrap(),
            vec![0, 1, 2, 255]
        );
        let run = view.lookup(Path::new("run.sh")).unwrap();
        assert!(run.is_executable());
        assert_eq!(run.mode, 0o100755);
        assert_eq!(
            view.read_link(Path::new("link")).unwrap(),
            PathBuf::from("README.md")
        );
        let entries = view.read_dir(Path::new("nested")).unwrap();
        assert!(entries
            .iter()
            .any(|e| e.path == Path::new("nested/file with spaces.txt")));
        assert!(entries
            .iter()
            .any(|e| e.path == Path::new("nested/ユニコード.txt")));
        fs::remove_dir_all(repo).unwrap();
    }

    #[test]
    fn git_tree_view_reports_errors_for_bad_paths_and_type_mismatches() {
        let repo = fixture_repo();
        let view = GitTreeView::open(&repo, "main").unwrap();
        assert!(view.lookup(Path::new("missing")).is_err());
        assert!(view.read_dir(Path::new("README.md")).is_err());
        assert!(view.read_file(Path::new("nested"), 0, 1).is_err());
        assert!(view.lookup(Path::new("../outside")).is_err());
        assert!(view.lookup(Path::new("/absolute")).is_err());
        assert!(view.lookup(Path::new(".git/config")).is_err());
        fs::remove_dir_all(repo).unwrap();
    }

    #[test]
    fn rofs_mount_strict_fails_until_fuse_mount_is_implemented() {
        if std::env::var_os("KAGE_TEST_ROFS").is_none() {
            eprintln!(
                "skipping rofs mount test; set KAGE_TEST_ROFS=1 to require a real rofs mount"
            );
            return;
        }
        let repo = fixture_repo();
        let view = GitTreeView::open(&repo, "main").unwrap();
        let mount = temp("mount");
        fs::create_dir_all(&mount).unwrap();
        let err = mount_rofs_strict(&view, &mount).unwrap_err().to_string();
        if std::env::var_os("KAGE_TEST_ROFS_ALLOW_SKIP").is_some() {
            eprintln!("WARNING: skipping rofs mount body: {err}");
        } else {
            panic!("KAGE_TEST_ROFS=1 requires a real rofs mount: {err}");
        }
        fs::remove_dir_all(repo).unwrap();
        fs::remove_dir_all(mount).unwrap();
    }
}

#[cfg(test)]
mod gitlink_tests {
    use super::*;
    use std::{
        fs,
        time::{SystemTime, UNIX_EPOCH},
    };

    fn temp(name: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("kage-rofs-gitlink-{name}-{nonce}"))
    }
    fn run(repo: &Path, args: &[&str]) {
        let status = Command::new("git")
            .args(args)
            .current_dir(repo)
            .status()
            .unwrap();
        assert!(status.success(), "git {args:?} failed");
    }

    #[test]
    fn gitlink_is_reported_as_unsupported_for_file_reads() {
        let repo = temp("repo");
        fs::create_dir_all(&repo).unwrap();
        run(&repo, &["init", "-b", "main"]);
        run(&repo, &["config", "user.email", "kage@example.invalid"]);
        run(&repo, &["config", "user.name", "kage test"]);
        fs::write(repo.join("README.md"), "hello").unwrap();
        run(&repo, &["add", "README.md"]);
        run(&repo, &["commit", "-m", "initial"]);
        let oid = git_output(&repo, &["rev-parse", "HEAD"]).unwrap();
        run(
            &repo,
            &[
                "update-index",
                "--add",
                "--cacheinfo",
                "160000",
                oid.trim(),
                "submodule",
            ],
        );
        run(&repo, &["commit", "-m", "gitlink"]);
        let view = GitTreeView::open(&repo, "main").unwrap();
        let meta = view.lookup(Path::new("submodule")).unwrap();
        assert_eq!(meta.kind, GitEntryKind::Gitlink);
        assert!(view.read_file(Path::new("submodule"), 0, 1).is_err());
        fs::remove_dir_all(repo).unwrap();
    }
}
