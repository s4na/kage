use crate::git_tree::Result;
use std::{
    ffi::CString,
    os::fd::RawFd,
    os::unix::ffi::OsStrExt,
    path::Path,
    process::{Command, Stdio},
    thread,
};

pub(crate) unsafe fn open_fuse() -> Result<RawFd> {
    let path = CString::new("/dev/fuse")?;
    let fd = c_open(path.as_ptr(), O_RDWR | O_CLOEXEC, 0);
    if fd < 0 {
        Err(std::io::Error::last_os_error().into())
    } else {
        Ok(fd)
    }
}

pub(crate) unsafe fn mount_fuse(fd: RawFd, mountpoint: &Path) -> Result<()> {
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

pub(crate) fn fuse_mount_options(fd: RawFd) -> String {
    format!(
        "fd={fd},rootmode=040000,user_id={},group_id={},default_permissions,ro,fsname=kage-rofs,subtype=kage-rofs",
        unsafe { getuid() },
        unsafe { getgid() }
    )
}

pub(crate) fn fuse_mount_option_keys(options: &str) -> Vec<&str> {
    options
        .split(',')
        .map(|option| option.split_once('=').map_or(option, |(key, _)| key))
        .collect()
}

pub(crate) fn fusermount3_available() -> bool {
    Command::new("fusermount3")
        .arg("--version")
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false)
}

pub(crate) fn fusermount3_options() -> String {
    "ro,nosuid,nodev,default_permissions,fsname=kage-rofs,subtype=kage-rofs".to_string()
}

pub(crate) fn mount_fuse_with_fusermount3(mountpoint: &Path) -> Result<RawFd> {
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

pub(crate) unsafe fn receive_fd_with_timeout(socket: RawFd, timeout_ms: i32) -> RawFd {
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

pub(crate) const CMSG_LEN_I32: usize =
    cmsg_align(std::mem::size_of::<Cmsghdr>()) + std::mem::size_of::<i32>();
pub(crate) const CMSG_SPACE_I32: usize = cmsg_align(CMSG_LEN_I32);

pub(crate) fn unmount_path(path: &Path) -> Result<()> {
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

pub(crate) unsafe fn close_fd(fd: RawFd) {
    if fd >= 0 {
        let _ = c_close(fd);
    }
}
pub(crate) unsafe fn read_fd(fd: RawFd, buf: *mut u8, len: usize) -> isize {
    c_read(fd, buf.cast(), len)
}
pub(crate) unsafe fn write_fd(fd: RawFd, buf: *const u8, len: usize) -> isize {
    c_write(fd, buf.cast(), len)
}

pub(crate) const O_RDWR: i32 = 0o2;
pub(crate) const O_CLOEXEC: i32 = 0o2000000;
pub(crate) const MS_RDONLY: usize = 1;
pub(crate) const MS_NOSUID: usize = 2;
pub(crate) const MS_NODEV: usize = 4;
pub(crate) const MNT_DETACH: i32 = 2;
pub(crate) const EINVAL: i32 = 22;
pub(crate) const AF_UNIX: i32 = 1;
pub(crate) const SOCK_STREAM: i32 = 1;
pub(crate) const SOL_SOCKET: i32 = 1;
pub(crate) const SCM_RIGHTS: i32 = 1;
pub(crate) const F_SETFD: i32 = 2;
pub(crate) const FD_CLOEXEC: i32 = 1;
pub(crate) const POLLIN: i16 = 0x0001;
pub(crate) const ENOENT: i32 = 2;
pub(crate) const ENOSYS: i32 = 38;
pub(crate) const ENOTDIR: i32 = 20;
pub(crate) const EROFS: i32 = 30;
pub(crate) const S_IFREG: u32 = 0o100000;
pub(crate) const S_IFDIR: u32 = 0o040000;
pub(crate) const S_IFLNK: u32 = 0o120000;
pub(crate) const DT_REG: u8 = 8;
pub(crate) const DT_DIR: u8 = 4;
pub(crate) const DT_LNK: u8 = 10;
pub(crate) const FUSE_LOOKUP: u32 = 1;
pub(crate) const FUSE_FORGET: u32 = 2;
pub(crate) const FUSE_GETATTR: u32 = 3;
pub(crate) const FUSE_SETATTR: u32 = 4;
pub(crate) const FUSE_READLINK: u32 = 5;
pub(crate) const FUSE_SYMLINK: u32 = 6;
pub(crate) const FUSE_MKDIR: u32 = 9;
pub(crate) const FUSE_UNLINK: u32 = 10;
pub(crate) const FUSE_RMDIR: u32 = 11;
pub(crate) const FUSE_RENAME: u32 = 12;
pub(crate) const FUSE_LINK: u32 = 13;
pub(crate) const FUSE_OPEN: u32 = 14;
pub(crate) const FUSE_READ: u32 = 15;
pub(crate) const FUSE_WRITE: u32 = 16;
pub(crate) const FUSE_STATFS: u32 = 17;
pub(crate) const FUSE_RELEASE: u32 = 18;
pub(crate) const FUSE_FLUSH: u32 = 25;
pub(crate) const FUSE_INIT: u32 = 26;
pub(crate) const FUSE_OPENDIR: u32 = 27;
pub(crate) const FUSE_READDIR: u32 = 28;
pub(crate) const FUSE_RELEASEDIR: u32 = 29;
pub(crate) const FUSE_DESTROY: u32 = 38;
pub(crate) const FUSE_CREATE: u32 = 35;
pub(crate) const FUSE_IN_HEADER_SIZE: usize = 40;
pub(crate) const FUSE_DIRENT_SIZE: usize = 24;

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
    pub(crate) fn getuid() -> u32;
    pub(crate) fn getgid() -> u32;
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
