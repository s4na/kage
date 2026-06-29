use crate::{
    git_tree::{GitEntryKind, GitMetadata, GitTreeView},
    sys::*,
};
use std::{
    collections::HashMap,
    os::fd::RawFd,
    os::unix::ffi::OsStrExt,
    path::PathBuf,
    sync::{Arc, Mutex},
};

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

pub(crate) struct FuseServer {
    view: GitTreeView,
    inodes: Arc<Mutex<InodeTable>>,
}

impl FuseServer {
    pub(crate) fn new(view: GitTreeView) -> Self {
        Self {
            view,
            inodes: Arc::new(Mutex::new(InodeTable::new())),
        }
    }

    pub(crate) fn serve(self, fd: RawFd) {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        git_tree::git_output,
        mount::{mount_rofs_strict, RofsBackend},
    };
    use std::{
        fs,
        os::unix::fs::PermissionsExt,
        path::Path,
        process::{Command, Stdio},
        time::{Duration, Instant},
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
        if std::env::var_os("KAGE_ROFS_STRICT_CHILD").is_some() {
            match rofs_mount_strict_test_body() {
                Ok(()) => return,
                Err(err) if std::env::var_os("KAGE_TEST_ROFS_ALLOW_SKIP").is_some() => {
                    eprintln!("WARNING: skipping rofs mount body: {err}");
                    return;
                }
                Err(err) => panic!("KAGE_TEST_ROFS=1 requires a real rofs mount: {err}"),
            }
        }

        let mut child = Command::new(std::env::current_exe().expect("current test exe"))
            .arg("rofs_mount_strict_requires_real_read_only_mount")
            .arg("--exact")
            .arg("--nocapture")
            .arg("--test-threads=1")
            .env("KAGE_ROFS_STRICT_CHILD", "1")
            .stdin(Stdio::null())
            .spawn()
            .expect("spawn strict rofs child test process");
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            if let Some(status) = child.try_wait().expect("poll strict rofs child") {
                assert!(status.success(), "strict rofs child failed with {status}");
                return;
            }
            if Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                panic!("error_kind=fuser_first_read_timeout KAGE_TEST_ROFS=1 rofs strict command timed out after 20s");
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    fn rofs_mount_strict_test_body() -> std::result::Result<(), String> {
        let repo = fixture_repo();
        let view = GitTreeView::open(&repo, "main").map_err(|err| err.to_string())?;
        let mount = temp("mount");
        fs::create_dir_all(&mount).map_err(|err| err.to_string())?;
        let backend = RofsBackend::selected().map_err(|err| err.to_string())?;
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
            Err(err) => return Err(format!("backend={} error_detail={err}", backend.name())),
        }
        fs::remove_dir_all(repo).map_err(|err| err.to_string())?;
        fs::remove_dir_all(mount).map_err(|err| err.to_string())?;
        Ok(())
    }
}
