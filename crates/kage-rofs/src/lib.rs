use kage_core::validate_relative_path;
use std::{
    collections::HashMap,
    ffi::CString,
    fs,
    os::fd::RawFd,
    os::unix::ffi::OsStrExt,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{Arc, Mutex},
    thread::{self, JoinHandle},
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

#[derive(Debug)]
pub struct RofsMount {
    mountpoint: PathBuf,
    fd: RawFd,
    worker: Option<JoinHandle<()>>,
}

impl RofsMount {
    pub fn mountpoint(&self) -> &Path {
        &self.mountpoint
    }

    pub fn unmount(mut self) -> Result<()> {
        unmount_path(&self.mountpoint)?;
        unsafe {
            close_fd(self.fd);
        }
        self.fd = -1;
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
        Ok(())
    }
}

impl Drop for RofsMount {
    fn drop(&mut self) {
        let _ = unmount_path(&self.mountpoint);
        unsafe {
            close_fd(self.fd);
        }
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

pub fn mount_rofs_strict(view: &GitTreeView, mountpoint: &Path) -> Result<RofsMount> {
    if !rofs_mount_available() {
        return Err("/dev/fuse is unavailable; cannot mount kage-rofs".into());
    }
    fs::create_dir_all(mountpoint)?;
    let fd = match mount_fuse_direct(mountpoint) {
        Ok(fd) => fd,
        Err(direct_err) if fusermount3_available() => mount_fuse_with_fusermount3(mountpoint)
            .map_err(|helper_err| {
                format!(
                    "kage-rofs fuse mount failed via direct mount and fusermount3 helper; direct_error={direct_err}; helper_error={helper_err}"
                )
            })?,
        Err(err) => return Err(err),
    };
    let server = FuseServer::new(view.clone());
    let worker_fd = fd;
    let worker = thread::spawn(move || server.serve(worker_fd));
    Ok(RofsMount {
        mountpoint: mountpoint.to_path_buf(),
        fd,
        worker: Some(worker),
    })
}

fn mount_fuse_direct(mountpoint: &Path) -> Result<RawFd> {
    eprintln!(
        "kage-rofs direct mount: attempting mountpoint={}",
        mountpoint.display()
    );
    let fd = unsafe { open_fuse()? };
    if let Err(err) = unsafe { mount_fuse(fd, mountpoint) } {
        unsafe {
            close_fd(fd);
        }
        eprintln!("kage-rofs direct mount: failed: {err}");
        return Err(err);
    }
    eprintln!("kage-rofs direct mount: succeeded fd={fd}");
    Ok(fd)
}

#[derive(Clone)]
struct InodeTable {
    by_ino: HashMap<u64, PathBuf>,
    by_path: HashMap<PathBuf, u64>,
    next: u64,
}

impl InodeTable {
    fn new() -> Self {
        let mut by_ino = HashMap::new();
        let mut by_path = HashMap::new();
        by_ino.insert(1, PathBuf::new());
        by_path.insert(PathBuf::new(), 1);
        Self {
            by_ino,
            by_path,
            next: 2,
        }
    }
    fn path(&self, ino: u64) -> Option<PathBuf> {
        self.by_ino.get(&ino).cloned()
    }
    fn ino_for(&mut self, path: PathBuf) -> u64 {
        if path.as_os_str().is_empty() {
            return 1;
        }
        if let Some(ino) = self.by_path.get(&path) {
            return *ino;
        }
        let ino = self.next;
        self.next += 1;
        self.by_path.insert(path.clone(), ino);
        self.by_ino.insert(ino, path);
        ino
    }
}

struct FuseServer {
    view: GitTreeView,
    inodes: Arc<Mutex<InodeTable>>,
}

impl FuseServer {
    fn new(view: GitTreeView) -> Self {
        Self {
            view,
            inodes: Arc::new(Mutex::new(InodeTable::new())),
        }
    }

    fn serve(self, fd: RawFd) {
        let mut buf = vec![0_u8; 1024 * 1024];
        loop {
            let n = unsafe { read_fd(fd, buf.as_mut_ptr(), buf.len()) };
            if n <= 0 {
                break;
            }
            if n < FUSE_IN_HEADER_SIZE as isize {
                continue;
            }
            let req = FuseInHeader::from_bytes(&buf[..n as usize]);
            let response = self.handle(req, &buf[FUSE_IN_HEADER_SIZE..n as usize]);
            if let Some(bytes) = response {
                let _ = unsafe { write_fd(fd, bytes.as_ptr(), bytes.len()) };
            }
        }
    }

    fn handle(&self, req: FuseInHeader, body: &[u8]) -> Option<Vec<u8>> {
        match req.opcode {
            FUSE_LOOKUP if req.nodeid == 0 => Some(error_reply(req.unique, ENOENT)),
            FUSE_INIT => Some(self.init(req.unique)),
            FUSE_LOOKUP => Some(self.lookup(req.unique, req.nodeid, body)),
            FUSE_GETATTR => Some(self.getattr(req.unique, req.nodeid)),
            FUSE_OPENDIR | FUSE_OPEN => Some(open_reply(req.unique)),
            FUSE_READDIR => Some(self.readdir(req.unique, req.nodeid, body)),
            FUSE_READ => Some(self.read(req.unique, req.nodeid, body)),
            FUSE_READLINK => Some(self.readlink(req.unique, req.nodeid)),
            FUSE_STATFS => Some(self.statfs(req.unique)),
            FUSE_RELEASE | FUSE_RELEASEDIR | FUSE_FLUSH => Some(empty_reply(req.unique)),
            FUSE_FORGET | FUSE_DESTROY => None,
            FUSE_SETATTR | FUSE_MKDIR | FUSE_UNLINK | FUSE_RMDIR | FUSE_RENAME | FUSE_WRITE
            | FUSE_CREATE | FUSE_SYMLINK | FUSE_LINK => Some(error_reply(req.unique, EROFS)),
            _ => Some(error_reply(req.unique, ENOSYS)),
        }
    }

    fn lookup(&self, unique: u64, parent: u64, body: &[u8]) -> Vec<u8> {
        let Some(name_end) = body.iter().position(|b| *b == 0) else {
            return error_reply(unique, EINVAL);
        };
        let name = std::ffi::OsStr::from_bytes(&body[..name_end]);
        let Some(parent_path) = self.inodes.lock().unwrap().path(parent) else {
            return error_reply(unique, ENOENT);
        };
        let path = if parent_path.as_os_str().is_empty() {
            PathBuf::from(name)
        } else {
            parent_path.join(name)
        };
        match self.view.lookup(&path) {
            Ok(meta) => {
                let ino = self.inodes.lock().unwrap().ino_for(path);
                entry_reply(unique, ino, attr_for(ino, &meta))
            }
            Err(_) => error_reply(unique, ENOENT),
        }
    }

    fn getattr(&self, unique: u64, ino: u64) -> Vec<u8> {
        let Some(path) = self.inodes.lock().unwrap().path(ino) else {
            return error_reply(unique, ENOENT);
        };
        match self.view.lookup(&path) {
            Ok(meta) => attr_reply(unique, attr_for(ino, &meta)),
            Err(_) => error_reply(unique, ENOENT),
        }
    }

    fn readdir(&self, unique: u64, ino: u64, body: &[u8]) -> Vec<u8> {
        let read = FuseReadIn::from_bytes(body);
        let Some(path) = self.inodes.lock().unwrap().path(ino) else {
            return error_reply(unique, ENOENT);
        };
        let entries = match self.view.read_dir(&path) {
            Ok(entries) => entries,
            Err(_) => return error_reply(unique, ENOTDIR),
        };
        let mut packed = Vec::new();
        let mut all = Vec::new();
        all.push((ino, ".".as_bytes().to_vec(), DT_DIR));
        all.push((1, "..".as_bytes().to_vec(), DT_DIR));
        for entry in entries {
            let entry_ino = self.inodes.lock().unwrap().ino_for(entry.path.clone());
            let name = entry
                .path
                .file_name()
                .map(|name| name.as_bytes().to_vec())
                .unwrap_or_default();
            all.push((entry_ino, name, dirent_type(&entry.kind)));
        }
        for (idx, (entry_ino, name, kind)) in all.into_iter().enumerate().skip(read.offset as usize)
        {
            let next_offset = (idx + 1) as i64;
            let reclen = align8(FUSE_DIRENT_SIZE + name.len());
            if packed.len() + reclen > read.size as usize {
                break;
            }
            push_u64(&mut packed, entry_ino);
            push_i64(&mut packed, next_offset);
            push_u32(&mut packed, name.len() as u32);
            push_u32(&mut packed, kind as u32);
            packed.extend_from_slice(&name);
            packed.resize(packed.len() + (reclen - FUSE_DIRENT_SIZE - name.len()), 0);
        }
        data_reply(unique, &packed)
    }

    fn read(&self, unique: u64, ino: u64, body: &[u8]) -> Vec<u8> {
        let read = FuseReadIn::from_bytes(body);
        let Some(path) = self.inodes.lock().unwrap().path(ino) else {
            return error_reply(unique, ENOENT);
        };
        match self.view.read_file(&path, read.offset, read.size as usize) {
            Ok(bytes) => data_reply(unique, &bytes),
            Err(_) => error_reply(unique, EINVAL),
        }
    }

    fn readlink(&self, unique: u64, ino: u64) -> Vec<u8> {
        let Some(path) = self.inodes.lock().unwrap().path(ino) else {
            return error_reply(unique, ENOENT);
        };
        match self.view.read_link(&path) {
            Ok(target) => data_reply(unique, target.as_os_str().as_bytes()),
            Err(_) => error_reply(unique, EINVAL),
        }
    }

    fn init(&self, unique: u64) -> Vec<u8> {
        let mut out = out_header(unique, 80);
        push_u32(&mut out, 7);
        push_u32(&mut out, 31);
        push_u32(&mut out, 0);
        push_u32(&mut out, 0);
        push_u32(&mut out, 128 * 1024);
        push_u32(&mut out, 0);
        push_u16(&mut out, 0);
        push_u16(&mut out, 0);
        push_u32(&mut out, 0);
        push_u32(&mut out, 0);
        push_u32(&mut out, 0);
        push_u32(&mut out, 0);
        out.resize(80, 0);
        out
    }

    fn statfs(&self, unique: u64) -> Vec<u8> {
        let mut out = out_header(unique, 16 + 80);
        push_u64(&mut out, 0);
        push_u64(&mut out, 0);
        push_u64(&mut out, 0);
        push_u64(&mut out, 0);
        push_u64(&mut out, 0);
        push_u32(&mut out, 512);
        push_u32(&mut out, 255);
        push_u32(&mut out, 0);
        push_u32(&mut out, 0);
        push_u32(&mut out, 0);
        push_u32(&mut out, 0);
        out.resize(96, 0);
        out
    }
}

fn attr_for(ino: u64, meta: &GitMetadata) -> FuseAttr {
    let perm = match meta.kind {
        GitEntryKind::Tree => 0o555,
        GitEntryKind::Executable => 0o555,
        GitEntryKind::Symlink => 0o777,
        GitEntryKind::Blob | GitEntryKind::Gitlink => 0o444,
    };
    let kind = match meta.kind {
        GitEntryKind::Tree => S_IFDIR,
        GitEntryKind::Symlink => S_IFLNK,
        _ => S_IFREG,
    };
    FuseAttr {
        ino,
        size: meta.size.unwrap_or(0),
        blocks: meta.size.unwrap_or(0).div_ceil(512),
        atime: 0,
        mtime: 0,
        ctime: 0,
        atimensec: 0,
        mtimensec: 0,
        ctimensec: 0,
        mode: kind | perm,
        nlink: if meta.is_dir() { 2 } else { 1 },
        uid: unsafe { getuid() },
        gid: unsafe { getgid() },
        rdev: 0,
        blksize: 512,
        flags: 0,
    }
}

fn dirent_type(kind: &GitEntryKind) -> u8 {
    match kind {
        GitEntryKind::Tree => DT_DIR,
        GitEntryKind::Symlink => DT_LNK,
        _ => DT_REG,
    }
}

fn open_reply(unique: u64) -> Vec<u8> {
    let mut out = out_header(unique, 32);
    push_u64(&mut out, 0);
    push_u32(&mut out, 0);
    push_u32(&mut out, 0);
    out
}

fn attr_reply(unique: u64, attr: FuseAttr) -> Vec<u8> {
    let mut out = out_header(unique, 16 + 104);
    push_u64(&mut out, 1);
    push_u32(&mut out, 0);
    push_u32(&mut out, 0);
    attr.push(&mut out);
    out.resize(120, 0);
    out
}

fn entry_reply(unique: u64, ino: u64, attr: FuseAttr) -> Vec<u8> {
    let mut out = out_header(unique, 16 + 120);
    push_u64(&mut out, ino);
    push_u64(&mut out, 0);
    push_u64(&mut out, 1);
    push_u64(&mut out, 1);
    push_u32(&mut out, 0);
    push_u32(&mut out, 0);
    push_u32(&mut out, 0);
    push_u32(&mut out, 0);
    attr.push(&mut out);
    out.resize(136, 0);
    out
}

fn data_reply(unique: u64, data: &[u8]) -> Vec<u8> {
    let mut out = out_header(unique, 16 + data.len());
    out.extend_from_slice(data);
    out
}

fn empty_reply(unique: u64) -> Vec<u8> {
    out_header(unique, 16)
}

fn error_reply(unique: u64, errno: i32) -> Vec<u8> {
    let mut out = Vec::with_capacity(16);
    push_u32(&mut out, 16);
    push_i32(&mut out, -errno);
    push_u64(&mut out, unique);
    out
}

fn out_header(unique: u64, len: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(len);
    push_u32(&mut out, len as u32);
    push_i32(&mut out, 0);
    push_u64(&mut out, unique);
    out
}

#[derive(Clone, Copy)]
struct FuseInHeader {
    opcode: u32,
    unique: u64,
    nodeid: u64,
}

impl FuseInHeader {
    fn from_bytes(bytes: &[u8]) -> Self {
        Self {
            opcode: u32_at(bytes, 8),
            unique: u64_at(bytes, 16),
            nodeid: u64_at(bytes, 24),
        }
    }
}

struct FuseReadIn {
    offset: u64,
    size: u32,
}

impl FuseReadIn {
    fn from_bytes(bytes: &[u8]) -> Self {
        Self {
            offset: u64_at(bytes, 8),
            size: u32_at(bytes, 24),
        }
    }
}

struct FuseAttr {
    ino: u64,
    size: u64,
    blocks: u64,
    atime: u64,
    mtime: u64,
    ctime: u64,
    atimensec: u32,
    mtimensec: u32,
    ctimensec: u32,
    mode: u32,
    nlink: u32,
    uid: u32,
    gid: u32,
    rdev: u32,
    blksize: u32,
    flags: u32,
}

impl FuseAttr {
    fn push(&self, out: &mut Vec<u8>) {
        push_u64(out, self.ino);
        push_u64(out, self.size);
        push_u64(out, self.blocks);
        push_u64(out, self.atime);
        push_u64(out, self.mtime);
        push_u64(out, self.ctime);
        push_u32(out, self.atimensec);
        push_u32(out, self.mtimensec);
        push_u32(out, self.ctimensec);
        push_u32(out, self.mode);
        push_u32(out, self.nlink);
        push_u32(out, self.uid);
        push_u32(out, self.gid);
        push_u32(out, self.rdev);
        push_u32(out, self.blksize);
        push_u32(out, self.flags);
    }
}

fn u32_at(bytes: &[u8], offset: usize) -> u32 {
    u32::from_ne_bytes(bytes[offset..offset + 4].try_into().unwrap())
}
fn u64_at(bytes: &[u8], offset: usize) -> u64 {
    u64::from_ne_bytes(bytes[offset..offset + 8].try_into().unwrap())
}
fn push_u16(out: &mut Vec<u8>, value: u16) {
    out.extend_from_slice(&value.to_ne_bytes());
}
fn push_u32(out: &mut Vec<u8>, value: u32) {
    out.extend_from_slice(&value.to_ne_bytes());
}
fn push_i32(out: &mut Vec<u8>, value: i32) {
    out.extend_from_slice(&value.to_ne_bytes());
}
fn push_u64(out: &mut Vec<u8>, value: u64) {
    out.extend_from_slice(&value.to_ne_bytes());
}
fn push_i64(out: &mut Vec<u8>, value: i64) {
    out.extend_from_slice(&value.to_ne_bytes());
}
fn align8(value: usize) -> usize {
    (value + 7) & !7
}

unsafe fn open_fuse() -> Result<RawFd> {
    let path = CString::new("/dev/fuse")?;
    let fd = c_open(path.as_ptr(), O_RDWR | O_CLOEXEC, 0);
    if fd < 0 {
        Err(std::io::Error::last_os_error().into())
    } else {
        Ok(fd)
    }
}

unsafe fn mount_fuse(fd: RawFd, mountpoint: &Path) -> Result<()> {
    let source = CString::new("kage-rofs")?;
    let target = CString::new(mountpoint.as_os_str().as_bytes())?;
    let fstype = CString::new("fuse")?;
    let opts_string = fuse_mount_options(fd);
    let opts = CString::new(opts_string.as_str())?;
    let rc = c_mount(
        source.as_ptr(),
        target.as_ptr(),
        fstype.as_ptr(),
        MS_NOSUID | MS_NODEV | MS_RDONLY,
        opts.as_ptr().cast(),
    );
    if rc == 0 {
        Ok(())
    } else {
        let err = std::io::Error::last_os_error();
        let kind = match err.raw_os_error() {
            Some(1) => "kernel denied direct FUSE mount (EPERM); CAP_SYS_ADMIN or fusermount3 helper may be required",
            Some(13) => "/dev/fuse or mountpoint permission denied (EACCES)",
            Some(22) => "direct FUSE mount returned EINVAL; kage-rofs mount options may be incompatible with this kernel",
            _ => "direct FUSE mount failed",
        };
        Err(format!(
            "kage-rofs fuse mount failed: {err}; context: source=kage-rofs fstype=fuse target={} flags=MS_NOSUID|MS_NODEV|MS_RDONLY option_keys={} error_kind={kind}",
            mountpoint.display(),
            fuse_mount_option_keys(&opts_string).join(",")
        )
        .into())
    }
}

fn fuse_mount_options(fd: RawFd) -> String {
    format!(
        "fd={fd},rootmode=040000,user_id={},group_id={},default_permissions,ro,fsname=kage-rofs,subtype=kage-rofs",
        unsafe { getuid() },
        unsafe { getgid() }
    )
}

fn fuse_mount_option_keys(options: &str) -> Vec<&str> {
    options
        .split(',')
        .map(|option| option.split_once('=').map_or(option, |(key, _)| key))
        .collect()
}

fn fusermount3_available() -> bool {
    Command::new("fusermount3")
        .arg("--version")
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false)
}

fn fusermount3_options() -> String {
    "ro,nosuid,nodev,default_permissions,fsname=kage-rofs,subtype=kage-rofs".to_string()
}

fn mount_fuse_with_fusermount3(mountpoint: &Path) -> Result<RawFd> {
    eprintln!(
        "kage-rofs fusermount3 helper: attempting mountpoint={} argv=fusermount3 -o {} -- {}",
        mountpoint.display(),
        fusermount3_options(),
        mountpoint.display()
    );
    let mut fds = [0; 2];
    if unsafe { socketpair(AF_UNIX, SOCK_STREAM, 0, fds.as_mut_ptr()) } != 0 {
        return Err(format!(
            "fusermount3 socketpair failed: {}",
            std::io::Error::last_os_error()
        )
        .into());
    }
    unsafe {
        let _ = c_fcntl(fds[1], F_SETFD, FD_CLOEXEC);
    }
    let commfd = fds[0].to_string();
    eprintln!("kage-rofs fusermount3 helper: _FUSE_COMMFD={commfd}");
    let child = Command::new("fusermount3")
        .env("_FUSE_COMMFD", &commfd)
        .arg("-o")
        .arg(fusermount3_options())
        .arg("--")
        .arg(mountpoint)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn();
    unsafe {
        close_fd(fds[0]);
    }
    let child = match child {
        Ok(child) => {
            eprintln!("kage-rofs fusermount3 helper: spawned pid={}", child.id());
            child
        }
        Err(err) => {
            unsafe {
                close_fd(fds[1]);
            }
            return Err(format!("failed to spawn fusermount3 helper: {err}").into());
        }
    };
    eprintln!("kage-rofs fusermount3 helper: waiting for fd");
    let fd = unsafe { receive_fd_with_timeout(fds[1], 15_000) };
    eprintln!("kage-rofs fusermount3 helper: fd receive result={fd}");
    unsafe {
        close_fd(fds[1]);
    }
    if fd >= 0 {
        thread::spawn(move || {
            let _ = child.wait_with_output();
        });
        unsafe {
            let _ = c_fcntl(fd, F_SETFD, FD_CLOEXEC);
        }
        Ok(fd)
    } else {
        let output = child.wait_with_output()?;
        Err(format!(
            "fusermount3 failed status={} stderr={}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        )
        .into())
    }
}

unsafe fn receive_fd_with_timeout(socket: RawFd, timeout_ms: i32) -> RawFd {
    let mut pfd = PollFd {
        fd: socket,
        events: POLLIN,
        revents: 0,
    };
    if poll(&mut pfd, 1, timeout_ms) <= 0 || (pfd.revents & POLLIN) == 0 {
        return -1;
    }
    let mut byte = [0_u8; 1];
    let mut iov = Iovec {
        iov_base: byte.as_mut_ptr().cast(),
        iov_len: byte.len(),
    };
    let mut control = [0_u8; CMSG_SPACE_I32];
    let mut msg = Msghdr {
        msg_name: std::ptr::null_mut(),
        msg_namelen: 0,
        msg_iov: &mut iov,
        msg_iovlen: 1,
        msg_control: control.as_mut_ptr().cast(),
        msg_controllen: control.len(),
        msg_flags: 0,
    };
    let received = recvmsg(socket, &mut msg, 0);
    if received <= 0 {
        return -1;
    }
    let cmsg = msg.msg_control.cast::<Cmsghdr>();
    if cmsg.is_null()
        || (*cmsg).cmsg_level != SOL_SOCKET
        || (*cmsg).cmsg_type != SCM_RIGHTS
        || (*cmsg).cmsg_len < CMSG_LEN_I32
    {
        return -1;
    }
    let data = (cmsg.cast::<u8>()).add(cmsg_align(std::mem::size_of::<Cmsghdr>()));
    *(data.cast::<i32>())
}

const fn cmsg_align(len: usize) -> usize {
    (len + std::mem::size_of::<usize>() - 1) & !(std::mem::size_of::<usize>() - 1)
}

const CMSG_LEN_I32: usize = cmsg_align(std::mem::size_of::<Cmsghdr>()) + std::mem::size_of::<i32>();
const CMSG_SPACE_I32: usize = cmsg_align(CMSG_LEN_I32);

fn unmount_path(path: &Path) -> Result<()> {
    let target = CString::new(path.as_os_str().as_bytes())?;
    let rc = unsafe { c_umount2(target.as_ptr(), MNT_DETACH) };
    if rc == 0 || std::io::Error::last_os_error().raw_os_error() == Some(EINVAL) {
        Ok(())
    } else if fusermount3_available() {
        let output = Command::new("fusermount3")
            .arg("--unmount")
            .arg("--quiet")
            .arg("--lazy")
            .arg("--")
            .arg(path)
            .output()?;
        if output.status.success() {
            Ok(())
        } else {
            Err(format!(
                "umount2 and fusermount3 unmount failed: umount2_error={}; fusermount3_status={} stderr={}",
                std::io::Error::last_os_error(),
                output.status,
                String::from_utf8_lossy(&output.stderr)
            )
            .into())
        }
    } else {
        Err(std::io::Error::last_os_error().into())
    }
}

unsafe fn close_fd(fd: RawFd) {
    if fd >= 0 {
        let _ = c_close(fd);
    }
}
unsafe fn read_fd(fd: RawFd, buf: *mut u8, len: usize) -> isize {
    c_read(fd, buf.cast(), len)
}
unsafe fn write_fd(fd: RawFd, buf: *const u8, len: usize) -> isize {
    c_write(fd, buf.cast(), len)
}

const O_RDWR: i32 = 0o2;
const O_CLOEXEC: i32 = 0o2000000;
const MS_RDONLY: usize = 1;
const MS_NOSUID: usize = 2;
const MS_NODEV: usize = 4;
const MNT_DETACH: i32 = 2;
const EINVAL: i32 = 22;
const AF_UNIX: i32 = 1;
const SOCK_STREAM: i32 = 1;
const SOL_SOCKET: i32 = 1;
const SCM_RIGHTS: i32 = 1;
const F_SETFD: i32 = 2;
const FD_CLOEXEC: i32 = 1;
const POLLIN: i16 = 0x0001;
const ENOENT: i32 = 2;
const ENOSYS: i32 = 38;
const ENOTDIR: i32 = 20;
const EROFS: i32 = 30;
const S_IFREG: u32 = 0o100000;
const S_IFDIR: u32 = 0o040000;
const S_IFLNK: u32 = 0o120000;
const DT_REG: u8 = 8;
const DT_DIR: u8 = 4;
const DT_LNK: u8 = 10;
const FUSE_LOOKUP: u32 = 1;
const FUSE_FORGET: u32 = 2;
const FUSE_GETATTR: u32 = 3;
const FUSE_SETATTR: u32 = 4;
const FUSE_READLINK: u32 = 5;
const FUSE_SYMLINK: u32 = 6;
const FUSE_MKDIR: u32 = 9;
const FUSE_UNLINK: u32 = 10;
const FUSE_RMDIR: u32 = 11;
const FUSE_RENAME: u32 = 12;
const FUSE_LINK: u32 = 13;
const FUSE_OPEN: u32 = 14;
const FUSE_READ: u32 = 15;
const FUSE_WRITE: u32 = 16;
const FUSE_STATFS: u32 = 17;
const FUSE_RELEASE: u32 = 18;
const FUSE_FLUSH: u32 = 25;
const FUSE_INIT: u32 = 26;
const FUSE_OPENDIR: u32 = 27;
const FUSE_READDIR: u32 = 28;
const FUSE_RELEASEDIR: u32 = 29;
const FUSE_DESTROY: u32 = 38;
const FUSE_CREATE: u32 = 35;
const FUSE_IN_HEADER_SIZE: usize = 40;
const FUSE_DIRENT_SIZE: usize = 24;

#[repr(C)]
struct Iovec {
    iov_base: *mut std::ffi::c_void,
    iov_len: usize,
}

#[repr(C)]
struct Msghdr {
    msg_name: *mut std::ffi::c_void,
    msg_namelen: u32,
    msg_iov: *mut Iovec,
    msg_iovlen: usize,
    msg_control: *mut std::ffi::c_void,
    msg_controllen: usize,
    msg_flags: i32,
}

#[repr(C)]
struct Cmsghdr {
    cmsg_len: usize,
    cmsg_level: i32,
    cmsg_type: i32,
}

#[repr(C)]
struct PollFd {
    fd: i32,
    events: i16,
    revents: i16,
}

unsafe extern "C" {
    fn open(path: *const i8, flags: i32, mode: u32) -> i32;
    fn close(fd: i32) -> i32;
    fn read(fd: i32, buf: *mut std::ffi::c_void, count: usize) -> isize;
    fn write(fd: i32, buf: *const std::ffi::c_void, count: usize) -> isize;
    fn mount(
        source: *const i8,
        target: *const i8,
        filesystemtype: *const i8,
        mountflags: usize,
        data: *const std::ffi::c_void,
    ) -> i32;
    fn umount2(target: *const i8, flags: i32) -> i32;
    fn socketpair(domain: i32, kind: i32, protocol: i32, sv: *mut i32) -> i32;
    fn recvmsg(sockfd: i32, msg: *mut Msghdr, flags: i32) -> isize;
    fn poll(fds: *mut PollFd, nfds: usize, timeout: i32) -> i32;
    fn fcntl(fd: i32, cmd: i32, arg: i32) -> i32;
    fn getuid() -> u32;
    fn getgid() -> u32;
}

unsafe fn c_open(path: *const i8, flags: i32, mode: u32) -> i32 {
    open(path, flags, mode)
}
unsafe fn c_close(fd: i32) -> i32 {
    close(fd)
}
unsafe fn c_read(fd: i32, buf: *mut std::ffi::c_void, count: usize) -> isize {
    read(fd, buf, count)
}
unsafe fn c_write(fd: i32, buf: *const std::ffi::c_void, count: usize) -> isize {
    write(fd, buf, count)
}
unsafe fn c_mount(
    source: *const i8,
    target: *const i8,
    filesystemtype: *const i8,
    mountflags: usize,
    data: *const std::ffi::c_void,
) -> i32 {
    mount(source, target, filesystemtype, mountflags, data)
}
unsafe fn c_umount2(target: *const i8, flags: i32) -> i32 {
    umount2(target, flags)
}
unsafe fn c_fcntl(fd: i32, cmd: i32, arg: i32) -> i32 {
    fcntl(fd, cmd, arg)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        fs,
        os::unix::fs::PermissionsExt,
        sync::mpsc,
        thread,
        time::Duration,
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

    fn lookup_request(name: &str) -> Vec<u8> {
        let mut body = name.as_bytes().to_vec();
        body.push(0);
        body
    }

    fn read_request(offset: u64, size: u32) -> Vec<u8> {
        let mut body = vec![0_u8; 40];
        body[8..16].copy_from_slice(&offset.to_ne_bytes());
        body[24..28].copy_from_slice(&size.to_ne_bytes());
        body
    }

    fn status(reply: &[u8]) -> i32 {
        i32::from_ne_bytes(reply[4..8].try_into().unwrap())
    }

    fn body(reply: &[u8]) -> &[u8] {
        let len = u32_at(reply, 0) as usize;
        assert_eq!(len, reply.len());
        &reply[16..]
    }

    fn entry_ino(reply: &[u8]) -> u64 {
        assert_eq!(status(reply), 0);
        u64_at(body(reply), 0)
    }

    fn attr_mode(reply: &[u8]) -> u32 {
        assert_eq!(status(reply), 0);
        u32_at(body(reply), 76)
    }

    fn attr_size(reply: &[u8]) -> u64 {
        assert_eq!(status(reply), 0);
        u64_at(body(reply), 24)
    }

    fn data(reply: &[u8]) -> Vec<u8> {
        assert_eq!(status(reply), 0);
        body(reply).to_vec()
    }

    fn dirent_names(reply: &[u8]) -> Vec<String> {
        assert_eq!(status(reply), 0);
        let mut out = Vec::new();
        let mut pos = 0;
        let bytes = body(reply);
        while pos + FUSE_DIRENT_SIZE <= bytes.len() {
            let namelen = u32_at(bytes, pos + 16) as usize;
            let name_start = pos + FUSE_DIRENT_SIZE;
            let name_end = name_start + namelen;
            if name_end > bytes.len() {
                break;
            }
            out.push(String::from_utf8(bytes[name_start..name_end].to_vec()).unwrap());
            pos += align8(FUSE_DIRENT_SIZE + namelen);
        }
        out
    }

    fn server_for(repo: &Path) -> FuseServer {
        FuseServer::new(GitTreeView::open(repo, "main").unwrap())
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
    fn fuse_protocol_lookup_getattr_read_readlink_and_readdir_are_mount_free() {
        let repo = fixture_repo();
        let head_before = git_output(&repo, &["rev-parse", "HEAD"]).unwrap();
        let server = server_for(&repo);

        let init = server
            .handle(
                FuseInHeader {
                    opcode: FUSE_INIT,
                    unique: 100,
                    nodeid: 0,
                },
                &[],
            )
            .unwrap();
        assert_eq!(status(&init), 0);
        assert_eq!(u32_at(&init, 0), 80);

        let root_attr = server.getattr(1, 1);
        assert_eq!(attr_mode(&root_attr) & S_IFDIR, S_IFDIR);

        let readme_a = server.lookup(2, 1, &lookup_request("README.md"));
        let readme_b = server.lookup(3, 1, &lookup_request("README.md"));
        let readme_ino = entry_ino(&readme_a);
        assert_eq!(readme_ino, entry_ino(&readme_b));

        let readme_attr = server.getattr(4, readme_ino);
        assert_eq!(attr_mode(&readme_attr) & S_IFREG, S_IFREG);
        assert_eq!(attr_mode(&readme_attr) & 0o777, 0o444);
        assert_eq!(attr_size(&readme_attr), "hello world".len() as u64);

        assert_eq!(
            data(&server.read(5, readme_ino, &read_request(0, 99))),
            b"hello world"
        );
        assert_eq!(
            data(&server.read(6, readme_ino, &read_request(6, 5))),
            b"world"
        );
        assert_eq!(data(&server.read(7, readme_ino, &read_request(99, 5))), b"");

        let run_ino = entry_ino(&server.lookup(8, 1, &lookup_request("run.sh")));
        let run_attr = server.getattr(9, run_ino);
        assert_eq!(attr_mode(&run_attr) & S_IFREG, S_IFREG);
        assert_eq!(attr_mode(&run_attr) & 0o111, 0o111);

        let link_ino = entry_ino(&server.lookup(10, 1, &lookup_request("link")));
        let link_attr = server.getattr(11, link_ino);
        assert_eq!(attr_mode(&link_attr) & S_IFLNK, S_IFLNK);
        assert_eq!(attr_size(&link_attr), "README.md".len() as u64);
        assert_eq!(data(&server.readlink(12, link_ino)), b"README.md");

        let root_names = dirent_names(&server.readdir(13, 1, &read_request(0, 4096)));
        assert!(root_names.contains(&".".to_string()));
        assert!(root_names.contains(&"..".to_string()));
        assert!(root_names.contains(&"README.md".to_string()));
        assert!(root_names.contains(&"nested".to_string()));
        let continued_names = dirent_names(&server.readdir(14, 1, &read_request(2, 4096)));
        assert!(!continued_names.contains(&".".to_string()));
        assert!(!continued_names.contains(&"..".to_string()));

        let nested_ino = entry_ino(&server.lookup(15, 1, &lookup_request("nested")));
        let nested_names = dirent_names(&server.readdir(16, nested_ino, &read_request(0, 4096)));
        assert!(nested_names.contains(&"file with spaces.txt".to_string()));
        assert!(nested_names.contains(&"ユニコード.txt".to_string()));

        assert_eq!(
            status(&server.lookup(17, 1, &lookup_request("missing"))),
            -ENOENT
        );
        let head_after = git_output(&repo, &["rev-parse", "HEAD"]).unwrap();
        assert_eq!(head_before, head_after);
        fs::remove_dir_all(repo).unwrap();
    }

    #[test]
    fn fuse_protocol_binary_reads_and_mutations_are_read_only() {
        let repo = fixture_repo();
        let server = server_for(&repo);
        let binary_ino = entry_ino(&server.lookup(20, 1, &lookup_request("binary.bin")));
        assert_eq!(
            data(&server.read(21, binary_ino, &read_request(0, 99))),
            vec![0, 1, 2, 255]
        );
        assert_eq!(
            data(&server.read(22, binary_ino, &read_request(2, 2))),
            vec![2, 255]
        );
        let forget = server.handle(
            FuseInHeader {
                opcode: FUSE_FORGET,
                unique: 23,
                nodeid: binary_ino,
            },
            &[],
        );
        assert!(forget.is_none(), "FUSE_FORGET must not emit a reply");

        for opcode in [
            FUSE_SETATTR,
            FUSE_MKDIR,
            FUSE_UNLINK,
            FUSE_RMDIR,
            FUSE_RENAME,
            FUSE_WRITE,
            FUSE_CREATE,
        ] {
            let req = FuseInHeader {
                opcode,
                unique: u64::from(opcode),
                nodeid: 1,
            };
            let reply = server.handle(req, &[]).unwrap();
            assert_eq!(status(&reply), -EROFS, "opcode {opcode} must be read-only");
        }
        fs::remove_dir_all(repo).unwrap();
    }

    #[test]
    fn fuse_protocol_large_directory_and_large_file_are_mount_free() {
        let repo = temp("large-repo");
        fs::create_dir_all(repo.join("many")).unwrap();
        run(&repo, &["init", "-b", "main"]);
        run(&repo, &["config", "user.email", "kage@example.invalid"]);
        run(&repo, &["config", "user.name", "kage test"]);
        let large = "0123456789".repeat(1024);
        fs::write(repo.join("large.txt"), &large).unwrap();
        for idx in 0..64 {
            fs::write(
                repo.join(format!("many/file-{idx:02}.txt")),
                idx.to_string(),
            )
            .unwrap();
        }
        run(&repo, &["add", "."]);
        run(&repo, &["commit", "-m", "large"]);
        let server = server_for(&repo);
        let large_ino = entry_ino(&server.lookup(30, 1, &lookup_request("large.txt")));
        assert_eq!(
            data(&server.read(31, large_ino, &read_request(10, 10))),
            b"0123456789"
        );
        let many_ino = entry_ino(&server.lookup(32, 1, &lookup_request("many")));
        let names = dirent_names(&server.readdir(33, many_ino, &read_request(0, 16 * 1024)));
        assert!(names.contains(&"file-00.txt".to_string()));
        assert!(names.contains(&"file-63.txt".to_string()));
        fs::remove_dir_all(repo).unwrap();
    }

    #[test]
    fn fuse_mount_options_are_kernel_context_diagnostic_friendly() {
        let options = fuse_mount_options(42);
        let keys = fuse_mount_option_keys(&options);
        assert!(options.contains("fd=42"));
        assert!(options.contains("rootmode=040000"));
        assert!(options.contains("default_permissions"));
        assert!(options.contains("ro"));
        assert!(options.contains("fsname=kage-rofs"));
        assert!(options.contains("subtype=kage-rofs"));
        assert!(keys.contains(&"fd"));
        assert!(keys.contains(&"rootmode"));
        assert!(keys.contains(&"user_id"));
        assert!(keys.contains(&"group_id"));
    }

    #[test]
    fn fusermount3_options_are_read_only_and_do_not_include_direct_fd() {
        let options = fusermount3_options();
        assert!(options.contains("ro"));
        assert!(options.contains("nosuid"));
        assert!(options.contains("nodev"));
        assert!(options.contains("default_permissions"));
        assert!(options.contains("fsname=kage-rofs"));
        assert!(options.contains("subtype=kage-rofs"));
        assert!(!options.contains("fd="));
        assert!(!options.contains("rootmode="));
    }

    #[test]
    fn rofs_mount_strict_requires_real_read_only_mount() {
        if std::env::var_os("KAGE_TEST_ROFS").is_none() {
            eprintln!(
                "skipping rofs mount test; set KAGE_TEST_ROFS=1 to require a real rofs mount"
            );
            return;
        }
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            let result = rofs_mount_strict_test_body();
            let _ = tx.send(result);
        });
        match rx.recv_timeout(Duration::from_secs(20)) {
            Ok(Ok(())) => {}
            Ok(Err(err)) if std::env::var_os("KAGE_TEST_ROFS_ALLOW_SKIP").is_some() => {
                eprintln!("WARNING: skipping rofs mount body: {err}");
            }
            Ok(Err(err)) => panic!("KAGE_TEST_ROFS=1 requires a real rofs mount: {err}"),
            Err(mpsc::RecvTimeoutError::Timeout) => panic!(
                "KAGE_TEST_ROFS=1 rofs strict mount body timed out after 20s; inspect helper/direct mount diagnostics"
            ),
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                panic!("KAGE_TEST_ROFS=1 rofs strict mount body exited without reporting a result")
            }
        }
    }

    fn rofs_mount_strict_test_body() -> std::result::Result<(), String> {
        let repo = fixture_repo();
        let view = GitTreeView::open(&repo, "main").map_err(|err| err.to_string())?;
        let mount = temp("mount");
        fs::create_dir_all(&mount).map_err(|err| err.to_string())?;
        match mount_rofs_strict(&view, &mount) {
            Ok(handle) => {
                let findmnt = Command::new("findmnt").arg(&mount).output();
                eprintln!("kage-rofs strict test findmnt: {findmnt:?}");
                eprintln!("kage-rofs strict test: reading README.md");
                assert_eq!(
                    fs::read_to_string(mount.join("README.md")).map_err(|err| err.to_string())?,
                    "hello world"
                );
                eprintln!("kage-rofs strict test: reading binary.bin");
                assert_eq!(
                    fs::read(mount.join("binary.bin")).map_err(|err| err.to_string())?,
                    vec![0, 1, 2, 255]
                );
                eprintln!("kage-rofs strict test: reading symlink");
                assert_eq!(
                    fs::read_link(mount.join("link")).map_err(|err| err.to_string())?,
                    PathBuf::from("README.md")
                );
                eprintln!("kage-rofs strict test: verifying read-only write failure");
                assert!(fs::write(mount.join("new.txt"), "nope").is_err());
                eprintln!("kage-rofs strict test: unmounting");
                handle.unmount().map_err(|err| err.to_string())?;
            }
            Err(err) => return Err(err.to_string()),
        }
        fs::remove_dir_all(repo).map_err(|err| err.to_string())?;
        fs::remove_dir_all(mount).map_err(|err| err.to_string())?;
        Ok(())
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
