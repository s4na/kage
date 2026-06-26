use std::{
    fs,
    path::{Component, Path, PathBuf},
};
pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkspaceSpec {
    pub id: String,
    pub repo: PathBuf,
    pub reference: String,
    pub parent_commit: String,
    pub lower: PathBuf,
    pub upper: PathBuf,
    pub work: PathBuf,
    pub merged: PathBuf,
    pub backend: String,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MutationKind {
    Added,
    Modified,
    Deleted,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Mutation {
    pub path: PathBuf,
    pub kind: MutationKind,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkspaceDiff {
    pub workspace: String,
    pub mutations: Vec<Mutation>,
}
pub struct RuntimePaths {
    pub root: PathBuf,
}
impl RuntimePaths {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }
    pub fn workspaces(&self) -> PathBuf {
        self.root.join("workspaces")
    }
    pub fn workspace_dir(&self, id: &str) -> PathBuf {
        self.workspaces().join(id)
    }
    pub fn metadata_path(&self, id: &str) -> PathBuf {
        self.workspace_dir(id).join("workspace.tsv")
    }
    pub fn ensure(&self) -> Result<()> {
        fs::create_dir_all(self.workspaces())?;
        Ok(())
    }
}
pub fn validate_workspace_id(id: &str) -> Result<()> {
    let ok = !id.is_empty()
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
    if ok {
        Ok(())
    } else {
        Err(format!("invalid workspace id: {id}").into())
    }
}

pub fn validate_relative_path(path: &Path) -> Result<()> {
    if path.as_os_str().is_empty() {
        return Err("empty workspace-relative path".into());
    }
    if path.is_absolute() {
        return Err(format!("absolute path is not allowed: {}", path.display()).into());
    }
    for component in path.components() {
        match component {
            Component::Normal(name) if name == ".git" => {
                return Err(
                    ".git paths are reserved and not allowed in workspace mutations".into(),
                );
            }
            Component::Normal(_) => {}
            Component::CurDir => {}
            Component::ParentDir => {
                return Err(format!("path traversal is not allowed: {}", path.display()).into());
            }
            Component::RootDir | Component::Prefix(_) => {
                return Err(format!("non-relative path is not allowed: {}", path.display()).into());
            }
        }
    }
    Ok(())
}

pub fn validate_child_path(root: &Path, child: &Path) -> Result<PathBuf> {
    let rel = child.strip_prefix(root)?;
    validate_relative_path(rel)?;
    Ok(rel.to_path_buf())
}

pub fn write_workspace(paths: &RuntimePaths, ws: &WorkspaceSpec) -> Result<()> {
    validate_workspace_id(&ws.id)?;
    fs::create_dir_all(paths.workspace_dir(&ws.id))?;
    let data = format!(
        "id\t{}\nrepo\t{}\nreference\t{}\nparent\t{}\nlower\t{}\nupper\t{}\nwork\t{}\nmerged\t{}\nbackend\t{}\n",
        ws.id,
        ws.repo.display(),
        ws.reference,
        ws.parent_commit,
        ws.lower.display(),
        ws.upper.display(),
        ws.work.display(),
        ws.merged.display(),
        ws.backend
    );
    fs::write(paths.metadata_path(&ws.id), data)?;
    Ok(())
}
pub fn read_workspace(paths: &RuntimePaths, id: &str) -> Result<WorkspaceSpec> {
    validate_workspace_id(id)?;
    let s = fs::read_to_string(paths.metadata_path(id))?;
    let mut m = std::collections::HashMap::new();
    for l in s.lines() {
        if let Some((k, v)) = l.split_once('\t') {
            m.insert(k, v.to_string());
        }
    }
    Ok(WorkspaceSpec {
        id: m["id"].clone(),
        repo: m["repo"].clone().into(),
        reference: m["reference"].clone(),
        parent_commit: m["parent"].clone(),
        lower: m["lower"].clone().into(),
        upper: m["upper"].clone().into(),
        work: m["work"].clone().into(),
        merged: m["merged"].clone().into(),
        backend: m
            .get("backend")
            .cloned()
            .unwrap_or_else(|| "fallback".to_string()),
    })
}
pub fn list_workspaces(paths: &RuntimePaths) -> Result<Vec<WorkspaceSpec>> {
    paths.ensure()?;
    let mut out = Vec::new();
    for e in fs::read_dir(paths.workspaces())? {
        let e = e?;
        if e.path().join("workspace.tsv").exists() {
            if let Some(id) = e.file_name().to_str() {
                out.push(read_workspace(paths, id)?);
            }
        }
    }
    out.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(out)
}
pub fn remove_workspace(paths: &RuntimePaths, id: &str) -> Result<()> {
    validate_workspace_id(id)?;
    let dir = paths.workspace_dir(id);
    if dir.exists() {
        fs::remove_dir_all(dir)?;
    }
    Ok(())
}
pub trait BaseLayer {
    fn metadata(&self, path: &Path) -> Result<std::fs::Metadata>;
    fn read_dir(&self, path: &Path) -> Result<Vec<PathBuf>>;
    fn read_file(&self, path: &Path, offset: u64, size: usize) -> Result<Vec<u8>>;
    fn read_link(&self, path: &Path) -> Result<PathBuf>;
}
pub fn is_hidden_metadata(path: &Path) -> bool {
    path.file_name().is_some_and(|n| n == ".kage")
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
        std::env::temp_dir().join(format!("kage-core-{name}-{nonce}"))
    }

    #[test]
    fn workspace_metadata_round_trips_and_lists() {
        let paths = RuntimePaths::new(temp("metadata"));
        let ws = WorkspaceSpec {
            id: "ws_test".to_string(),
            repo: PathBuf::from("/repo"),
            reference: "main".to_string(),
            parent_commit: "abc123".to_string(),
            lower: PathBuf::from("/kage/lower"),
            upper: PathBuf::from("/kage/upper"),
            work: PathBuf::from("/kage/work"),
            merged: PathBuf::from("/kage/merged"),
            backend: "fallback".to_string(),
        };

        write_workspace(&paths, &ws).unwrap();

        assert_eq!(read_workspace(&paths, "ws_test").unwrap(), ws);
        assert_eq!(list_workspaces(&paths).unwrap().len(), 1);
        remove_workspace(&paths, "ws_test").unwrap();
        assert!(list_workspaces(&paths).unwrap().is_empty());
        fs::remove_dir_all(paths.root).unwrap();
    }

    #[test]
    fn workspace_ids_are_restricted_to_safe_path_segments() {
        assert!(validate_workspace_id("agent_42-main").is_ok());
        assert!(validate_workspace_id("../escape").is_err());
        assert!(validate_workspace_id("").is_err());
    }

    #[test]
    fn relative_paths_reject_traversal_absolute_and_git_metadata() {
        assert!(validate_relative_path(Path::new("src/lib.rs")).is_ok());
        assert!(validate_relative_path(Path::new("dir with spaces/ユニコード.txt")).is_ok());
        assert!(validate_relative_path(Path::new("../outside")).is_err());
        assert!(validate_relative_path(Path::new("/absolute")).is_err());
        assert!(validate_relative_path(Path::new(".git/config")).is_err());
        assert!(validate_relative_path(Path::new("nested/.git/config")).is_err());
    }
}
