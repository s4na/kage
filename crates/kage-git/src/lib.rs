use std::{
    fs,
    io::Write,
    os::unix::fs::{FileTypeExt, MetadataExt, PermissionsExt},
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LayerDiffEntry {
    pub status: String,
    pub path: PathBuf,
}

pub struct GitRepo {
    pub path: PathBuf,
}
impl GitRepo {
    pub fn open(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }
    pub fn init(path: &Path, url: Option<&str>) -> Result<Self> {
        if let Some(url) = url {
            run_at(
                Path::new("."),
                "git",
                &["clone", "--bare", url, path.to_str().unwrap()],
            )?;
        } else {
            fs::create_dir_all(path)?;
            run_at(path, "git", &["init"])?;
        }
        Ok(Self::open(path))
    }
    pub fn fetch(&self) -> Result<()> {
        run_at(&self.path, "git", &["fetch", "--all", "--prune"])
    }
    pub fn rev_parse(&self, reference: &str) -> Result<String> {
        Ok(output_at(&self.path, "git", &["rev-parse", reference])?
            .trim()
            .to_string())
    }
    pub fn require_update_ref(&self, reference: &str) -> Result<String> {
        let out = output_at(
            &self.path,
            "git",
            &["rev-parse", "--verify", "--symbolic-full-name", reference],
        )?;
        let full = out.trim();
        if full.starts_with("refs/heads/") || full.starts_with("refs/kage/") {
            Ok(full.to_string())
        } else {
            Err(format!("reference is not an updatable ref: {reference}").into())
        }
    }
    pub fn export_tree(&self, reference: &str, dest: &Path) -> Result<()> {
        if dest.exists() {
            fs::remove_dir_all(dest)?;
        }
        fs::create_dir_all(dest)?;
        let cmd = format!(
            "git archive {} | tar -x -C {}",
            shell_escape(reference),
            shell_escape(&dest.display().to_string())
        );
        run_at(&self.path, "sh", &["-c", &cmd])
    }
    pub fn diff_name_status(&self, old: &str, new: &str) -> Result<Vec<(String, String)>> {
        let out = output_at(&self.path, "git", &["diff", "--name-status", old, new])?;
        Ok(out
            .lines()
            .filter_map(|l| {
                l.split_once('\t')
                    .map(|(s, p)| (s.to_string(), p.to_string()))
            })
            .collect())
    }
    pub fn tree_from_dir(&self, tree_dir: &Path) -> Result<String> {
        let index = temp_index();
        let git_dir = self.git_dir();
        let env = format!(
            "GIT_INDEX_FILE={} GIT_DIR={}",
            shell_escape(&index.display().to_string()),
            shell_escape(&git_dir.display().to_string())
        );
        run_at(
            &self.path,
            "sh",
            &["-c", &format!("{} git read-tree --empty", env)],
        )?;
        run_at(
            &self.path,
            "sh",
            &[
                "-c",
                &format!(
                    "cd {} && {} git add -A . ':!.kage'",
                    shell_escape(&tree_dir.display().to_string()),
                    env
                ),
            ],
        )?;
        let tree = output_at(
            &self.path,
            "sh",
            &["-c", &format!("{} git write-tree", env)],
        )?
        .trim()
        .to_string();
        let _ = fs::remove_file(index);
        Ok(tree)
    }
    pub fn commit_from_tree(
        &self,
        tree_dir: &Path,
        reference: &str,
        message: &str,
    ) -> Result<String> {
        let parent = self.rev_parse(reference)?;
        let tree = self.tree_from_dir(tree_dir)?;
        self.commit_tree_and_update(reference, &parent, &tree, message)
    }

    pub fn layer_diff_name_status(
        &self,
        parent_commit: &str,
        upper: &Path,
    ) -> Result<Vec<LayerDiffEntry>> {
        let tree = self.tree_from_layer(parent_commit, upper)?;
        Ok(self
            .diff_name_status(parent_commit, &tree)?
            .into_iter()
            .map(|(status, path)| LayerDiffEntry {
                status,
                path: path.into(),
            })
            .collect())
    }

    pub fn commit_from_layer(
        &self,
        reference: &str,
        expected_parent: &str,
        upper: &Path,
        message: &str,
    ) -> Result<String> {
        let update_ref = self.require_update_ref(reference)?;
        let current = self.rev_parse(&update_ref)?;
        if current != expected_parent {
            return Err(format!(
                "ref advanced: {update_ref} was {expected_parent}, now {current}; rebase/merge/create-ref is required"
            )
            .into());
        }
        let tree = self.tree_from_layer(expected_parent, upper)?;
        if tree == self.rev_parse(&format!("{expected_parent}^{{tree}}"))? {
            return Err("workspace has no changes to commit".into());
        }
        self.commit_tree_and_update(&update_ref, expected_parent, &tree, message)
    }

    pub fn tree_from_layer(&self, parent_commit: &str, upper: &Path) -> Result<String> {
        let index = temp_index();
        let git_dir = self.git_dir();
        index_run(&git_dir, &index, &["read-tree", parent_commit])?;
        for deleted in read_deletions(upper)? {
            validate_rel(&deleted)?;
            index_run(
                &git_dir,
                &index,
                &[
                    "update-index",
                    "--force-remove",
                    path_arg(&deleted).as_str(),
                ],
            )?;
        }
        for opaque in read_opaque_dirs(upper)? {
            validate_rel(&opaque)?;
            for path in self.tree_paths(parent_commit, &opaque)? {
                index_run(
                    &git_dir,
                    &index,
                    &["update-index", "--force-remove", path_arg(&path).as_str()],
                )?;
            }
        }
        for entry in walk_files(upper)? {
            let rel = entry.strip_prefix(upper)?.to_path_buf();
            if rel
                .components()
                .next()
                .is_some_and(|c| c.as_os_str() == ".kage")
            {
                continue;
            }
            validate_rel(&rel)?;
            let meta = fs::symlink_metadata(&entry)?;
            if is_overlay_whiteout(&entry, &meta) {
                index_run(
                    &git_dir,
                    &index,
                    &["update-index", "--force-remove", path_arg(&rel).as_str()],
                )?;
                continue;
            }
            let (mode, oid) = if meta.file_type().is_symlink() {
                (
                    "120000",
                    hash_bytes(
                        &self.path,
                        fs::read_link(&entry)?.to_string_lossy().as_bytes(),
                    )?,
                )
            } else if meta.is_file() {
                let mode = if meta.permissions().mode() & 0o111 != 0 {
                    "100755"
                } else {
                    "100644"
                };
                (mode, hash_file(&self.path, &entry)?)
            } else {
                continue;
            };
            remove_index_conflicts(&git_dir, &index, &rel)?;
            index_run(
                &git_dir,
                &index,
                &[
                    "update-index",
                    "--add",
                    "--cacheinfo",
                    mode,
                    oid.as_str(),
                    path_arg(&rel).as_str(),
                ],
            )?;
        }
        let tree = index_output(&git_dir, &index, &["write-tree"])?
            .trim()
            .to_string();
        let _ = fs::remove_file(index);
        Ok(tree)
    }

    fn tree_paths(&self, treeish: &str, prefix: &Path) -> Result<Vec<PathBuf>> {
        let prefix_arg = path_arg(prefix);
        let out = output_at(
            &self.path,
            "git",
            &[
                "ls-tree",
                "-r",
                "--name-only",
                treeish,
                "--",
                prefix_arg.as_str(),
            ],
        )?;
        Ok(out.lines().map(PathBuf::from).collect())
    }

    fn commit_tree_and_update(
        &self,
        reference: &str,
        parent: &str,
        tree: &str,
        message: &str,
    ) -> Result<String> {
        let commit = output_at(
            &self.path,
            "git",
            &["commit-tree", tree, "-p", parent, "-m", message],
        )?
        .trim()
        .to_string();
        run_at(
            &self.path,
            "git",
            &["update-ref", reference, &commit, parent],
        )?;
        Ok(commit)
    }
    fn git_dir(&self) -> PathBuf {
        let dotgit = self.path.join(".git");
        if dotgit.exists() {
            dotgit
        } else {
            self.path.clone()
        }
    }
}

pub fn run_at(cwd: &Path, program: &str, args: &[&str]) -> Result<()> {
    let status = Command::new(program).args(args).current_dir(cwd).status()?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("{program} exited with {status}").into())
    }
}
pub fn output_at(cwd: &Path, program: &str, args: &[&str]) -> Result<String> {
    let out = Command::new(program).args(args).current_dir(cwd).output()?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    } else {
        Err(format!("{program} failed: {}", String::from_utf8_lossy(&out.stderr)).into())
    }
}

fn index_run(git_dir: &Path, index: &Path, args: &[&str]) -> Result<()> {
    let status = Command::new("git")
        .env("GIT_DIR", git_dir)
        .env("GIT_INDEX_FILE", index)
        .args(args)
        .status()?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("git {} exited with {status}", args.join(" ")).into())
    }
}

fn index_output(git_dir: &Path, index: &Path, args: &[&str]) -> Result<String> {
    let out = Command::new("git")
        .env("GIT_DIR", git_dir)
        .env("GIT_INDEX_FILE", index)
        .args(args)
        .output()?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    } else {
        Err(format!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr)
        )
        .into())
    }
}

fn hash_file(repo: &Path, file: &Path) -> Result<String> {
    Ok(output_at(
        repo,
        "git",
        &["hash-object", "-w", file.to_str().ok_or("non-utf8 path")?],
    )?
    .trim()
    .to_string())
}

fn hash_bytes(repo: &Path, bytes: &[u8]) -> Result<String> {
    let mut child = Command::new("git")
        .arg("hash-object")
        .arg("-w")
        .arg("--stdin")
        .current_dir(repo)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()?;
    child
        .stdin
        .as_mut()
        .ok_or("missing stdin")?
        .write_all(bytes)?;
    let out = child.wait_with_output()?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    } else {
        Err(format!(
            "git hash-object failed: {}",
            String::from_utf8_lossy(&out.stderr)
        )
        .into())
    }
}

fn is_overlay_whiteout(path: &Path, meta: &fs::Metadata) -> bool {
    (meta.file_type().is_char_device() && meta.rdev() == 0)
        || (meta.is_file()
            && meta.len() == 0
            && xattr_value(path, "trusted.overlay.whiteout")
                .is_some_and(|value| value.trim() == "y"))
}

fn is_overlay_opaque(path: &Path) -> bool {
    xattr_value(path, "trusted.overlay.opaque").is_some_and(|value| value.trim() == "y")
}

fn xattr_value(path: &Path, name: &str) -> Option<String> {
    let out = Command::new("getfattr")
        .arg("--only-values")
        .arg("-n")
        .arg(name)
        .arg(path)
        .output()
        .ok()?;
    if out.status.success() {
        Some(String::from_utf8_lossy(&out.stdout).into_owned())
    } else {
        None
    }
}

fn read_deletions(upper: &Path) -> Result<Vec<PathBuf>> {
    let path = upper.join(".kage").join("deleted");
    if !path.exists() {
        return Ok(Vec::new());
    }
    Ok(fs::read_to_string(path)?
        .lines()
        .map(PathBuf::from)
        .collect())
}

fn read_opaque_dirs(upper: &Path) -> Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    let mut stack = vec![upper.to_path_buf()];
    while let Some(path) = stack.pop() {
        let meta = fs::symlink_metadata(&path)?;
        if !meta.is_dir() {
            continue;
        }
        if path != upper && is_overlay_opaque(&path) {
            out.push(path.strip_prefix(upper)?.to_path_buf());
        }
        for entry in fs::read_dir(path)? {
            stack.push(entry?.path());
        }
    }
    out.sort();
    Ok(out)
}

fn walk_files(root: &Path) -> Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(path) = stack.pop() {
        let meta = fs::symlink_metadata(&path)?;
        if meta.is_dir() {
            for entry in fs::read_dir(path)? {
                stack.push(entry?.path());
            }
        } else {
            out.push(path);
        }
    }
    out.sort();
    Ok(out)
}

fn validate_rel(path: &Path) -> Result<()> {
    if path.as_os_str().is_empty() || path.is_absolute() {
        return Err(format!("invalid workspace path: {}", path.display()).into());
    }
    for c in path.components() {
        match c {
            std::path::Component::Normal(name) if name == ".git" => {
                return Err(".git paths are reserved".into());
            }
            std::path::Component::Normal(_) | std::path::Component::CurDir => {}
            _ => return Err(format!("unsafe workspace path: {}", path.display()).into()),
        }
    }
    Ok(())
}
fn path_arg(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}
fn remove_index_conflicts(git_dir: &Path, index: &Path, path: &Path) -> Result<()> {
    // If an upper entry replaces a parent directory with a file/symlink, remove
    // every existing descendant first. Git's index cannot contain both `dir`
    // and `dir/sub/file.txt`.
    let descendant_prefix = format!("{}/", path_arg(path));
    let descendants = index_output(
        git_dir,
        index,
        &["ls-files", "-z", "--", &descendant_prefix],
    )?;
    for descendant in descendants.split('\0').filter(|p| !p.is_empty()) {
        index_run(
            git_dir,
            index,
            &["update-index", "--force-remove", descendant],
        )?;
    }

    // If an upper entry adds a nested path below a file from the parent tree,
    // remove the ancestor file before adding the nested entry.
    let mut ancestor = PathBuf::new();
    for component in path.components() {
        ancestor.push(component.as_os_str());
        if ancestor == path {
            break;
        }
        index_run(
            git_dir,
            index,
            &[
                "update-index",
                "--force-remove",
                path_arg(&ancestor).as_str(),
            ],
        )?;
    }

    // Replacing a file with a file/symlink is also delete + add.
    index_run(
        git_dir,
        index,
        &["update-index", "--force-remove", path_arg(path).as_str()],
    )?;
    Ok(())
}
fn temp_index() -> PathBuf {
    std::env::temp_dir().join(format!(
        "kage-index-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ))
}
fn shell_escape(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
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
        std::env::temp_dir().join(format!("kage-git-{name}-{nonce}"))
    }
    fn repo() -> (PathBuf, GitRepo) {
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
        run_at(&repo_dir, "git", &["add", "README.md"]).unwrap();
        run_at(&repo_dir, "git", &["commit", "-m", "initial"]).unwrap();
        let repo = GitRepo::open(&repo_dir);
        (repo_dir, repo)
    }

    #[test]
    fn tree_from_dir_excludes_kage_metadata() {
        let (repo_dir, repo) = repo();
        let tree_dir = temp("tree");
        fs::create_dir_all(tree_dir.join(".kage")).unwrap();
        fs::write(tree_dir.join("README.md"), "hello").unwrap();
        fs::write(tree_dir.join(".kage/deleted"), "ignored").unwrap();
        let tree = repo.tree_from_dir(&tree_dir).unwrap();
        let names = output_at(&repo_dir, "git", &["ls-tree", "--name-only", &tree]).unwrap();

        assert_eq!(names.trim(), "README.md");
        fs::remove_dir_all(repo_dir).unwrap();
        fs::remove_dir_all(tree_dir).unwrap();
    }

    #[test]
    fn commit_from_layer_handles_add_modify_delete_rename_exec_symlink_and_binary() {
        let (repo_dir, repo) = repo();
        fs::write(repo_dir.join("old-name.txt"), "old").unwrap();
        fs::write(repo_dir.join("script.sh"), "#!/bin/sh\nexit 0\n").unwrap();
        run_at(&repo_dir, "chmod", &["+x", "script.sh"]).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink("README.md", repo_dir.join("link")).unwrap();
        run_at(&repo_dir, "git", &["add", "."]).unwrap();
        run_at(&repo_dir, "git", &["commit", "-m", "fixtures"]).unwrap();
        let parent = repo.rev_parse("refs/heads/main").unwrap();
        let upper = temp("upper");
        fs::create_dir_all(upper.join(".kage")).unwrap();
        fs::write(upper.join("README.md"), "modified").unwrap();
        fs::write(upper.join("new file.txt"), "added").unwrap();
        fs::write(upper.join("binary.bin"), [0, 159, 146, 150, 255]).unwrap();
        fs::write(upper.join("renamed.txt"), "old").unwrap();
        fs::write(upper.join("script.sh"), "#!/bin/sh\nexit 1\n").unwrap();
        let mut perms = fs::metadata(upper.join("script.sh")).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(upper.join("script.sh"), perms).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink("README.md", upper.join("link")).unwrap();
        fs::write(upper.join(".kage/deleted"), "old-name.txt\n").unwrap();

        let commit = repo
            .commit_from_layer("refs/heads/main", &parent, &upper, "layer commit")
            .unwrap();

        assert_eq!(repo.rev_parse("refs/heads/main").unwrap(), commit);
        assert_eq!(
            output_at(&repo_dir, "git", &["show", "HEAD:README.md"]).unwrap(),
            "modified"
        );
        assert_eq!(
            output_at(&repo_dir, "git", &["show", "HEAD:new file.txt"]).unwrap(),
            "added"
        );
        assert!(output_at(&repo_dir, "git", &["cat-file", "-e", "HEAD:old-name.txt"]).is_err());
        assert_eq!(
            output_at(&repo_dir, "git", &["show", "HEAD:renamed.txt"]).unwrap(),
            "old"
        );
        assert_eq!(
            output_at(&repo_dir, "git", &["ls-tree", "HEAD", "script.sh"])
                .unwrap()
                .split_whitespace()
                .next()
                .unwrap(),
            "100755"
        );
        assert_eq!(
            output_at(&repo_dir, "git", &["ls-tree", "HEAD", "link"])
                .unwrap()
                .split_whitespace()
                .next()
                .unwrap(),
            "120000"
        );
        let binary = Command::new("git")
            .args(["show", "HEAD:binary.bin"])
            .current_dir(&repo_dir)
            .output()
            .unwrap();
        assert!(binary.status.success());
        assert_eq!(binary.stdout, vec![0, 159, 146, 150, 255]);
        fs::remove_dir_all(repo_dir).unwrap();
        fs::remove_dir_all(upper).unwrap();
    }

    #[test]
    fn tree_from_layer_replaces_parent_directory_with_file() {
        let (repo_dir, repo) = repo();
        fs::create_dir_all(repo_dir.join("dir/sub")).unwrap();
        fs::write(repo_dir.join("dir/sub/file.txt"), "parent").unwrap();
        run_at(&repo_dir, "git", &["add", "dir/sub/file.txt"]).unwrap();
        run_at(&repo_dir, "git", &["commit", "-m", "parent-dir"]).unwrap();
        let parent = repo.rev_parse("refs/heads/main").unwrap();
        let upper = temp("upper-dir-to-file");
        fs::create_dir_all(&upper).unwrap();
        fs::write(upper.join("dir"), "replacement file").unwrap();

        let tree = repo.tree_from_layer(&parent, &upper).unwrap();

        assert_eq!(
            output_at(&repo_dir, "git", &["show", &format!("{tree}:dir")]).unwrap(),
            "replacement file"
        );
        assert!(output_at(
            &repo_dir,
            "git",
            &["cat-file", "-e", &format!("{tree}:dir/sub/file.txt")]
        )
        .is_err());
        fs::remove_dir_all(repo_dir).unwrap();
        fs::remove_dir_all(upper).unwrap();
    }

    #[test]
    fn tree_from_layer_replaces_parent_file_with_directory() {
        let (repo_dir, repo) = repo();
        fs::write(repo_dir.join("dir"), "parent file").unwrap();
        run_at(&repo_dir, "git", &["add", "dir"]).unwrap();
        run_at(&repo_dir, "git", &["commit", "-m", "parent-file"]).unwrap();
        let parent = repo.rev_parse("refs/heads/main").unwrap();
        let upper = temp("upper-file-to-dir");
        fs::create_dir_all(upper.join("dir/sub")).unwrap();
        fs::write(upper.join("dir/sub/file.txt"), "nested replacement").unwrap();

        let tree = repo.tree_from_layer(&parent, &upper).unwrap();

        assert_eq!(
            output_at(
                &repo_dir,
                "git",
                &["cat-file", "-t", &format!("{tree}:dir")]
            )
            .unwrap()
            .trim(),
            "tree"
        );
        assert_eq!(
            output_at(
                &repo_dir,
                "git",
                &["show", &format!("{tree}:dir/sub/file.txt")]
            )
            .unwrap(),
            "nested replacement"
        );
        fs::remove_dir_all(repo_dir).unwrap();
        fs::remove_dir_all(upper).unwrap();
    }

    #[test]
    fn commit_from_layer_rejects_advanced_ref_and_detached_reference() {
        let (repo_dir, repo) = repo();
        let parent = repo.rev_parse("refs/heads/main").unwrap();
        fs::write(repo_dir.join("advance.txt"), "advance").unwrap();
        run_at(&repo_dir, "git", &["add", "advance.txt"]).unwrap();
        run_at(&repo_dir, "git", &["commit", "-m", "advance"]).unwrap();
        let upper = temp("upper");
        fs::create_dir_all(&upper).unwrap();
        fs::write(upper.join("README.md"), "modified").unwrap();

        let err = repo
            .commit_from_layer("refs/heads/main", &parent, &upper, "stale")
            .unwrap_err()
            .to_string();
        assert!(err.contains("ref advanced"));
        let head = repo.rev_parse("refs/heads/main").unwrap();
        assert_ne!(head, parent);
        let err = repo
            .commit_from_layer(&parent, &parent, &upper, "detached")
            .unwrap_err()
            .to_string();
        assert!(err.contains("not an updatable ref") || err.contains("Needed a single revision"));
        fs::remove_dir_all(repo_dir).unwrap();
        fs::remove_dir_all(upper).unwrap();
    }

    #[test]
    fn commit_from_layer_rejects_empty_diff() {
        let (repo_dir, repo) = repo();
        let parent = repo.rev_parse("refs/heads/main").unwrap();
        let upper = temp("upper-empty");
        fs::create_dir_all(&upper).unwrap();
        let err = repo
            .commit_from_layer("refs/heads/main", &parent, &upper, "empty")
            .unwrap_err()
            .to_string();
        assert!(err.contains("no changes"));
        assert_eq!(repo.rev_parse("refs/heads/main").unwrap(), parent);
        fs::remove_dir_all(repo_dir).unwrap();
        fs::remove_dir_all(upper).unwrap();
    }
}
