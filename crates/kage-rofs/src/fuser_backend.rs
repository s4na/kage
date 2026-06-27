use crate::{GitEntryKind, GitMetadata, GitTreeView, Result, RofsMount};
use fuser::{
    AccessFlags, BsdFileFlags, Config, Errno, FileAttr, FileHandle, FileType, Filesystem,
    FopenFlags, Generation, INodeNo, KernelConfig, LockOwner, MountOption, OpenAccMode, OpenFlags,
    RenameFlags, ReplyAttr, ReplyCreate, ReplyData, ReplyDirectory, ReplyEmpty, ReplyEntry,
    ReplyOpen, ReplyStatfs, ReplyWrite, Request, TimeOrNow, WriteFlags,
};
use std::{
    collections::HashMap,
    ffi::OsStr,
    os::unix::ffi::OsStrExt,
    path::{Path, PathBuf},
    sync::Mutex,
    time::{Duration, UNIX_EPOCH},
};

const TTL: Duration = Duration::from_secs(1);

pub fn mount_rofs_fuser(view: &GitTreeView, mountpoint: &Path) -> Result<RofsMount> {
    std::fs::create_dir_all(mountpoint)?;
    let fs = FuserRofs::new(view.clone());
    let mut config = Config::default();
    config.mount_options = vec![
        MountOption::RO,
        MountOption::NoDev,
        MountOption::NoSuid,
        MountOption::DefaultPermissions,
        MountOption::FSName("kage-rofs".to_string()),
        MountOption::Subtype("kage-rofs".to_string()),
    ];
    let session = fuser::spawn_mount2(fs, mountpoint, &config)
        .map_err(|err| format!("backend=fuser error_kind=fuser_mount_error error_detail={err}"))?;
    Ok(RofsMount::fuser(mountpoint.to_path_buf(), session))
}

#[derive(Debug)]
struct FuserRofs {
    view: GitTreeView,
    inodes: Mutex<InodeTable>,
}

impl FuserRofs {
    fn new(view: GitTreeView) -> Self {
        Self {
            view,
            inodes: Mutex::new(InodeTable::new()),
        }
    }

    fn path_for(&self, ino: INodeNo) -> Option<PathBuf> {
        self.inodes.lock().unwrap().path(ino)
    }

    fn ino_for(&self, path: PathBuf) -> INodeNo {
        self.inodes.lock().unwrap().ino_for(path)
    }

    fn attr_for(&self, ino: INodeNo, meta: &GitMetadata) -> FileAttr {
        let kind = match meta.kind {
            GitEntryKind::Tree => FileType::Directory,
            GitEntryKind::Symlink => FileType::Symlink,
            GitEntryKind::Blob | GitEntryKind::Executable | GitEntryKind::Gitlink => {
                FileType::RegularFile
            }
        };
        let perm = match meta.kind {
            GitEntryKind::Tree | GitEntryKind::Executable => 0o555,
            GitEntryKind::Symlink => 0o777,
            GitEntryKind::Blob | GitEntryKind::Gitlink => 0o444,
        };
        FileAttr {
            ino,
            size: meta.size.unwrap_or(0),
            blocks: meta.size.unwrap_or(0).div_ceil(512),
            atime: UNIX_EPOCH,
            mtime: UNIX_EPOCH,
            ctime: UNIX_EPOCH,
            crtime: UNIX_EPOCH,
            kind,
            perm,
            nlink: if meta.is_dir() { 2 } else { 1 },
            uid: unsafe { libc::getuid() },
            gid: unsafe { libc::getgid() },
            rdev: 0,
            blksize: 512,
            flags: 0,
        }
    }
}

impl Filesystem for FuserRofs {
    fn init(&mut self, _req: &Request<'_>, _config: &mut KernelConfig) -> std::io::Result<()> {
        Ok(())
    }

    fn lookup(&self, _req: &Request<'_>, parent: INodeNo, name: &OsStr, reply: ReplyEntry) {
        let Some(parent_path) = self.path_for(parent) else {
            reply.error(Errno::ENOENT);
            return;
        };
        let path = if parent_path.as_os_str().is_empty() {
            PathBuf::from(name)
        } else {
            parent_path.join(name)
        };
        match self.view.lookup(&path) {
            Ok(meta) => {
                let ino = self.ino_for(path);
                reply.entry(&TTL, &self.attr_for(ino, &meta), Generation(0));
            }
            Err(_) => reply.error(Errno::ENOENT),
        }
    }

    fn getattr(&self, _req: &Request<'_>, ino: INodeNo, _fh: Option<FileHandle>, reply: ReplyAttr) {
        let Some(path) = self.path_for(ino) else {
            reply.error(Errno::ENOENT);
            return;
        };
        match self.view.lookup(&path) {
            Ok(meta) => reply.attr(&TTL, &self.attr_for(ino, &meta)),
            Err(_) => reply.error(Errno::ENOENT),
        }
    }

    fn open(&self, _req: &Request<'_>, ino: INodeNo, flags: OpenFlags, reply: ReplyOpen) {
        if flags.acc_mode() != OpenAccMode::O_RDONLY {
            reply.error(Errno::EROFS);
            return;
        }
        let Some(path) = self.path_for(ino) else {
            reply.error(Errno::ENOENT);
            return;
        };
        match self.view.lookup(&path) {
            Ok(meta) if meta.is_file() => reply.opened(FileHandle(0), FopenFlags::empty()),
            Ok(_) => reply.error(Errno::EINVAL),
            Err(_) => reply.error(Errno::ENOENT),
        }
    }

    fn opendir(&self, _req: &Request<'_>, ino: INodeNo, _flags: OpenFlags, reply: ReplyOpen) {
        let Some(path) = self.path_for(ino) else {
            reply.error(Errno::ENOENT);
            return;
        };
        match self.view.lookup(&path) {
            Ok(meta) if meta.is_dir() => reply.opened(FileHandle(0), FopenFlags::empty()),
            Ok(_) => reply.error(Errno::ENOTDIR),
            Err(_) => reply.error(Errno::ENOENT),
        }
    }

    fn read(
        &self,
        _req: &Request<'_>,
        ino: INodeNo,
        _fh: FileHandle,
        offset: u64,
        size: u32,
        _flags: OpenFlags,
        _lock_owner: Option<LockOwner>,
        reply: ReplyData,
    ) {
        let Some(path) = self.path_for(ino) else {
            reply.error(Errno::ENOENT);
            return;
        };
        match self.view.read_file(&path, offset, size as usize) {
            Ok(bytes) => reply.data(&bytes),
            Err(_) => reply.error(Errno::EINVAL),
        }
    }

    fn readdir(
        &self,
        _req: &Request<'_>,
        ino: INodeNo,
        _fh: FileHandle,
        offset: u64,
        mut reply: ReplyDirectory,
    ) {
        let Some(path) = self.path_for(ino) else {
            reply.error(Errno::ENOENT);
            return;
        };
        let entries = match self.view.read_dir(&path) {
            Ok(entries) => entries,
            Err(_) => {
                reply.error(Errno::ENOTDIR);
                return;
            }
        };
        let mut all = vec![
            (ino, FileType::Directory, ".".to_string()),
            (INodeNo::ROOT, FileType::Directory, "..".to_string()),
        ];
        for entry in entries {
            let entry_ino = self.ino_for(entry.path.clone());
            let kind = match entry.kind {
                GitEntryKind::Tree => FileType::Directory,
                GitEntryKind::Symlink => FileType::Symlink,
                _ => FileType::RegularFile,
            };
            let Some(name) = entry
                .path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
            else {
                continue;
            };
            all.push((entry_ino, kind, name));
        }
        for (idx, (entry_ino, kind, name)) in all.into_iter().enumerate().skip(offset as usize) {
            if reply.add(entry_ino, (idx + 1) as u64, kind, name) {
                break;
            }
        }
        reply.ok();
    }

    fn readlink(&self, _req: &Request<'_>, ino: INodeNo, reply: ReplyData) {
        let Some(path) = self.path_for(ino) else {
            reply.error(Errno::ENOENT);
            return;
        };
        match self.view.read_link(&path) {
            Ok(target) => reply.data(target.as_os_str().as_bytes()),
            Err(_) => reply.error(Errno::EINVAL),
        }
    }

    fn statfs(&self, _req: &Request<'_>, _ino: INodeNo, reply: ReplyStatfs) {
        reply.statfs(0, 0, 0, 0, 0, 512, 255, 512);
    }

    fn access(&self, _req: &Request<'_>, ino: INodeNo, mask: AccessFlags, reply: ReplyEmpty) {
        if mask.intersects(AccessFlags::W_OK) {
            reply.error(Errno::EROFS);
            return;
        }
        if self.path_for(ino).is_some() {
            reply.ok();
        } else {
            reply.error(Errno::ENOENT);
        }
    }

    fn write(
        &self,
        _req: &Request<'_>,
        _ino: INodeNo,
        _fh: FileHandle,
        _offset: u64,
        _data: &[u8],
        _write_flags: WriteFlags,
        _flags: OpenFlags,
        _lock_owner: Option<LockOwner>,
        reply: ReplyWrite,
    ) {
        reply.error(Errno::EROFS);
    }
    fn create(
        &self,
        _req: &Request<'_>,
        _parent: INodeNo,
        _name: &OsStr,
        _mode: u32,
        _umask: u32,
        _flags: i32,
        reply: ReplyCreate,
    ) {
        reply.error(Errno::EROFS);
    }
    fn mkdir(
        &self,
        _req: &Request<'_>,
        _parent: INodeNo,
        _name: &OsStr,
        _mode: u32,
        _umask: u32,
        reply: ReplyEntry,
    ) {
        reply.error(Errno::EROFS);
    }
    fn unlink(&self, _req: &Request<'_>, _parent: INodeNo, _name: &OsStr, reply: ReplyEmpty) {
        reply.error(Errno::EROFS);
    }
    fn rmdir(&self, _req: &Request<'_>, _parent: INodeNo, _name: &OsStr, reply: ReplyEmpty) {
        reply.error(Errno::EROFS);
    }
    fn rename(
        &self,
        _req: &Request<'_>,
        _parent: INodeNo,
        _name: &OsStr,
        _newparent: INodeNo,
        _newname: &OsStr,
        _flags: RenameFlags,
        reply: ReplyEmpty,
    ) {
        reply.error(Errno::EROFS);
    }
    fn symlink(
        &self,
        _req: &Request<'_>,
        _parent: INodeNo,
        _link_name: &OsStr,
        _target: &Path,
        reply: ReplyEntry,
    ) {
        reply.error(Errno::EROFS);
    }
    fn link(
        &self,
        _req: &Request<'_>,
        _ino: INodeNo,
        _newparent: INodeNo,
        _newname: &OsStr,
        reply: ReplyEntry,
    ) {
        reply.error(Errno::EROFS);
    }
    fn setattr(
        &self,
        _req: &Request<'_>,
        _ino: INodeNo,
        _mode: Option<u32>,
        _uid: Option<u32>,
        _gid: Option<u32>,
        _size: Option<u64>,
        _atime: Option<TimeOrNow>,
        _mtime: Option<TimeOrNow>,
        _ctime: Option<std::time::SystemTime>,
        _fh: Option<FileHandle>,
        _crtime: Option<std::time::SystemTime>,
        _chgtime: Option<std::time::SystemTime>,
        _bkuptime: Option<std::time::SystemTime>,
        _flags: Option<BsdFileFlags>,
        reply: ReplyAttr,
    ) {
        reply.error(Errno::EROFS);
    }
}

#[derive(Debug, Clone)]
struct InodeTable {
    by_ino: HashMap<INodeNo, PathBuf>,
    by_path: HashMap<PathBuf, INodeNo>,
    next: u64,
}

impl InodeTable {
    fn new() -> Self {
        let mut by_ino = HashMap::new();
        let mut by_path = HashMap::new();
        by_ino.insert(INodeNo::ROOT, PathBuf::new());
        by_path.insert(PathBuf::new(), INodeNo::ROOT);
        Self {
            by_ino,
            by_path,
            next: u64::from(INodeNo::ROOT) + 1,
        }
    }
    fn path(&self, ino: INodeNo) -> Option<PathBuf> {
        self.by_ino.get(&ino).cloned()
    }
    fn ino_for(&mut self, path: PathBuf) -> INodeNo {
        if path.as_os_str().is_empty() {
            return INodeNo::ROOT;
        }
        if let Some(ino) = self.by_path.get(&path) {
            return *ino;
        }
        let ino = INodeNo(self.next);
        self.next += 1;
        self.by_path.insert(path.clone(), ino);
        self.by_ino.insert(ino, path);
        ino
    }
}
