use crate::{
    fuse::FuseServer,
    fuser_backend,
    git_tree::{GitTreeView, Result},
    sys::*,
};
use std::{
    fs,
    os::fd::RawFd,
    path::{Path, PathBuf},
    thread::{self, JoinHandle},
};

pub fn rofs_mount_available() -> bool {
    Path::new("/dev/fuse").exists()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RofsBackend {
    Fuser,
    Handwritten,
}

impl RofsBackend {
    pub fn selected() -> Result<Self> {
        match std::env::var("KAGE_ROFS_BACKEND").ok().as_deref() {
            None | Some("") => Ok(Self::Fuser),
            Some("fuser") => Ok(Self::Fuser),
            Some("handwritten") => Ok(Self::Handwritten),
            Some(other) => Err(format!(
                "unsupported KAGE_ROFS_BACKEND={other}; expected fuser or handwritten"
            )
            .into()),
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Fuser => "fuser",
            Self::Handwritten => "handwritten",
        }
    }
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
    let backend = RofsBackend::selected()?;
    eprintln!("kage-rofs strict mount backend={}", backend.name());
    if backend == RofsBackend::Fuser {
        return fuser_backend::mount_rofs_fuser(view, mountpoint);
    }
    mount_rofs_handwritten(view, mountpoint)
}

pub(crate) fn mount_rofs_handwritten(view: &GitTreeView, mountpoint: &Path) -> Result<RofsMount> {
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
