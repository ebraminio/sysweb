//! macOS arm64 backend using the public libSystem ABI.
//!
//! Apple does not guarantee the raw syscall numbers or the kernel `stat`
//! layout. libSystem is the stable interface, and ld64 requires linking it for
//! any dynamic executable anyway. This backend imports only the libSystem
//! functions below; it does not use the C runtime startup, std, or an
//! allocator. Directory iteration uses `fdopendir`/`readdir`, which may
//! allocate inside libSystem.

use crate::path::MAX_PATH;
use crate::platform::{Error, FileInfo, Interface, Result, SocketAddr};
use core::ffi::c_void;
use core::ptr::null_mut;

const AF_INET: i32 = 2;
const SOCK_STREAM: i32 = 1;
const SOL_SOCKET: i32 = 0xffff;
const SO_REUSEADDR: i32 = 4;
const IPPROTO_TCP: i32 = 6;
const TCP_NODELAY: i32 = 1;
const SO_NOSIGPIPE: i32 = 0x1022;
const AT_FDCWD: i32 = -2;
const AT_SYMLINK_NOFOLLOW: i32 = 0x20;

const O_RDONLY: i32 = 0;
const O_WRONLY: i32 = 1;
const O_NOFOLLOW_ANY: i32 = 0x2000_0000;
const O_CLOEXEC: i32 = 0x0100_0000;
const O_CREAT: i32 = 0x200;
const O_TRUNC: i32 = 0x400;
const O_EXCL: i32 = 0x800;
const O_DIRECTORY: i32 = 0x100000;
const MODE_FILE: u32 = 0o644;
const MODE_DIR: u32 = 0o755;

const S_IFMT: u16 = 0o170000;
const S_IFDIR: u16 = 0o040000;

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct Timespec {
    tv_sec: i64,
    tv_nsec: i64,
}

/// The public 64-bit-inode `struct stat` layout from the macOS SDK.
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct Stat {
    st_dev: i32,
    st_mode: u16,
    st_nlink: u16,
    st_ino: u64,
    st_uid: u32,
    st_gid: u32,
    st_rdev: i32,
    st_atimespec: Timespec,
    st_mtimespec: Timespec,
    st_ctimespec: Timespec,
    st_birthtimespec: Timespec,
    st_size: i64,
    st_blocks: i64,
    st_blksize: i32,
    st_flags: u32,
    st_gen: u32,
    st_lspare: i32,
    st_qspare: [i64; 2],
}

#[repr(C)]
struct SockaddrIn {
    len: u8,
    family: u8,
    port: [u8; 2],
    addr: [u8; 4],
    zero: [u8; 8],
}

#[repr(C)]
#[derive(Clone, Copy)]
struct Kevent {
    ident: usize,
    filter: i16,
    flags: u16,
    fflags: u32,
    data: i64,
    udata: usize,
}

#[repr(C)]
struct IfAddrs {
    next: *mut IfAddrs,
    name: *const u8,
    flags: u32,
    _pad: u32,
    addr: *const SockaddrIn,
    netmask: *const SockaddrIn,
    ifu: *const SockaddrIn,
    data: *mut u8,
}

#[repr(C)]
struct Dirent {
    d_ino: u64,
    d_seekoff: i64,
    d_reclen: u16,
    d_namlen: u16,
    d_type: u8,
    d_name: [u8; 1024],
}

unsafe extern "C" {
    #[link_name = "openat"]
    fn c_openat(dirfd: i32, path: *const u8, flags: i32, ...) -> i32;
    #[link_name = "fstat"]
    fn c_fstat(fd: i32, stat: *mut Stat) -> i32;
    #[link_name = "fstatat"]
    fn c_fstatat(dirfd: i32, path: *const u8, stat: *mut Stat, flags: i32) -> i32;
    #[link_name = "unlinkat"]
    fn c_unlinkat(dirfd: i32, path: *const u8, flags: i32) -> i32;
    #[link_name = "mkdirat"]
    fn c_mkdirat(dirfd: i32, path: *const u8, mode: u32) -> i32;
    #[link_name = "sendfile"]
    fn c_sendfile(fd: i32, s: i32, offset: i64, len: *mut i64, hdtr: *mut u8, flags: i32) -> i32;
    #[link_name = "setsockopt"]
    fn c_setsockopt(fd: i32, level: i32, name: i32, value: *const u8, len: u32) -> i32;
    #[link_name = "socket"]
    fn c_socket(domain: i32, ty: i32, protocol: i32) -> i32;
    #[link_name = "bind"]
    fn c_bind(fd: i32, addr: *const u8, len: u32) -> i32;
    #[link_name = "listen"]
    fn c_listen(fd: i32, backlog: i32) -> i32;
    #[link_name = "accept"]
    fn c_accept(fd: i32, addr: *mut u8, len: *mut u32) -> i32;
    #[link_name = "getpeername"]
    fn c_getpeername(fd: i32, addr: *mut u8, len: *mut u32) -> i32;
    #[link_name = "read"]
    fn c_read(fd: i32, buf: *mut c_void, count: usize) -> isize;
    #[link_name = "write"]
    fn c_write(fd: i32, buf: *const c_void, count: usize) -> isize;
    #[link_name = "send"]
    fn c_send(fd: i32, buf: *const u8, len: usize, flags: i32) -> isize;
    #[link_name = "close"]
    fn c_close(fd: i32) -> i32;
    #[link_name = "fcntl"]
    fn c_fcntl(fd: i32, cmd: i32, ...) -> i32;
    #[link_name = "kqueue"]
    fn c_kqueue() -> i32;
    #[link_name = "kevent"]
    fn c_kevent(
        kq: i32,
        changes: *const Kevent,
        nchanges: i32,
        events: *mut Kevent,
        nevents: i32,
        timeout: *const Timespec,
    ) -> i32;
    #[link_name = "getifaddrs"]
    fn c_getifaddrs(out: *mut *mut IfAddrs) -> i32;
    #[link_name = "freeifaddrs"]
    fn c_freeifaddrs(list: *mut IfAddrs);
    #[link_name = "fdopendir"]
    fn c_fdopendir(fd: i32) -> *mut u8;
    #[link_name = "readdir"]
    fn c_readdir(dirp: *mut u8) -> *mut Dirent;
    #[link_name = "closedir"]
    fn c_closedir(dirp: *mut u8) -> i32;
    #[link_name = "__error"]
    fn c_error() -> *mut i32;
    #[link_name = "_exit"]
    fn c_exit(code: i32) -> !;
}

fn last_error() -> Error {
    unsafe { map_errno(*c_error()) }
}

fn cvt(result: i32) -> Result<i32> {
    if result < 0 {
        Err(last_error())
    } else {
        Ok(result)
    }
}

fn cvt_isize(result: isize) -> Result<usize> {
    if result < 0 {
        Err(last_error())
    } else {
        Ok(result as usize)
    }
}

fn map_errno(errno: i32) -> Error {
    match errno {
        4 => Error::Interrupted,
        35 => Error::WouldBlock,
        2 | 20 => Error::NotFound,
        13 | 62 => Error::PermissionDenied, // EACCES, ELOOP
        17 => Error::AlreadyExists,
        21 => Error::IsADirectory,
        48 => Error::AddressInUse,
        23 | 24 => Error::TooManyOpenFiles,
        54 => Error::ConnectionReset,
        53 => Error::ConnectionAborted,
        32 => Error::BrokenPipe,
        22 | 63 => Error::InvalidInput,
        other => Error::Other(other),
    }
}

fn c_path(path: &[u8]) -> Result<[u8; MAX_PATH]> {
    if path.len() >= MAX_PATH || path.contains(&0) {
        return Err(Error::InvalidInput);
    }
    let mut buffer = [0u8; MAX_PATH];
    buffer[..path.len()].copy_from_slice(path);
    Ok(buffer)
}

fn open_path(dir_fd: i32, path: &[u8], flags: i32, mode: u32) -> Result<i32> {
    let path = c_path(path)?;
    cvt(unsafe { c_openat(dir_fd, path.as_ptr(), flags, mode) })
}

/// Open the document root, following symlinks in the root's own path. All
/// later operations are relative to this fd and use O_NOFOLLOW_ANY.
pub fn open_root(path: &[u8]) -> Result<i32> {
    let path = c_path(path)?;
    cvt(unsafe {
        c_openat(
            AT_FDCWD,
            path.as_ptr(),
            O_RDONLY | O_DIRECTORY | O_CLOEXEC,
            0u32,
        )
    })
}

/// Split a path into `(parent, basename)`. Parent is `"."` when the path has
/// no separator, and `"/"` for an absolute one-component path.
fn split_parent(path: &[u8]) -> (&[u8], &[u8]) {
    let trimmed = path.strip_suffix(b"/").unwrap_or(path);
    match trimmed.iter().rposition(|&byte| byte == b'/') {
        Some(0) => (b"/", &trimmed[1..]),
        Some(index) => (&trimmed[..index], &trimmed[index + 1..]),
        None => (b".", trimmed),
    }
}

pub fn socket() -> Result<i32> {
    cvt(unsafe { c_socket(AF_INET, SOCK_STREAM, 0) })
}

pub fn set_reuseaddr(fd: i32) -> Result<()> {
    let one: i32 = 1;
    cvt(unsafe {
        c_setsockopt(
            fd,
            SOL_SOCKET,
            SO_REUSEADDR,
            &one as *const i32 as *const u8,
            4,
        )
    })
    .map(|_| ())
}

pub fn bind(fd: i32, addr: SocketAddr) -> Result<()> {
    let sockaddr = SockaddrIn {
        len: core::mem::size_of::<SockaddrIn>() as u8,
        family: AF_INET as u8,
        port: addr.port.to_be_bytes(),
        addr: addr.ip,
        zero: [0; 8],
    };
    cvt(unsafe {
        c_bind(
            fd,
            &sockaddr as *const SockaddrIn as *const u8,
            core::mem::size_of::<SockaddrIn>() as u32,
        )
    })
    .map(|_| ())
}

pub fn listen(fd: i32, backlog: u16) -> Result<()> {
    cvt(unsafe { c_listen(fd, backlog as i32) }).map(|_| ())
}

pub fn accept(fd: i32) -> Result<i32> {
    cvt(unsafe { c_accept(fd, null_mut(), null_mut()) })
}

pub fn peer_addr(fd: i32) -> Result<SocketAddr> {
    let mut addr = SockaddrIn {
        len: core::mem::size_of::<SockaddrIn>() as u8,
        family: AF_INET as u8,
        port: [0; 2],
        addr: [0; 4],
        zero: [0; 8],
    };
    let mut len = core::mem::size_of::<SockaddrIn>() as u32;
    cvt(unsafe { c_getpeername(fd, &mut addr as *mut SockaddrIn as *mut u8, &mut len) })?;
    Ok(SocketAddr {
        ip: addr.addr,
        port: u16::from_be_bytes(addr.port),
    })
}

pub fn read(fd: i32, buf: &mut [u8]) -> Result<usize> {
    cvt_isize(unsafe { c_read(fd, buf.as_mut_ptr() as *mut c_void, buf.len()) })
}

pub fn write(fd: i32, buf: &[u8]) -> Result<usize> {
    cvt_isize(unsafe { c_write(fd, buf.as_ptr() as *const c_void, buf.len()) })
}

pub fn send(fd: i32, buf: &[u8]) -> Result<usize> {
    // SO_NOSIGPIPE is set on accepted streams; no MSG_NOSIGNAL on macOS.
    cvt_isize(unsafe { c_send(fd, buf.as_ptr(), buf.len(), 0) })
}

pub fn sendfile(out_fd: i32, in_fd: i32, offset: u64, count: usize) -> Result<usize> {
    let mut length = count as i64;
    let result = unsafe { c_sendfile(in_fd, out_fd, offset as i64, &mut length, null_mut(), 0) };
    if result < 0 {
        // Darwin may send a partial amount and then return EAGAIN. That
        // progress must be reported, or the caller retries the same offset and
        // duplicates bytes.
        if length > 0 && (length as usize) < count {
            return Ok(length as usize);
        }
        return Err(last_error());
    }
    if length < 0 {
        Err(Error::InvalidInput)
    } else {
        Ok(length as usize)
    }
}

pub fn interfaces(out: &mut [Interface]) -> Result<usize> {
    const IFF_UP: u32 = 0x1;
    let mut list: *mut IfAddrs = core::ptr::null_mut();
    if unsafe { c_getifaddrs(&mut list) } != 0 {
        return Err(last_error());
    }
    let mut count = 0usize;
    let mut current = list;
    while !current.is_null() && count < out.len() {
        let entry = unsafe { &*current };
        current = entry.next;
        if entry.flags & IFF_UP == 0 || entry.addr.is_null() {
            continue;
        }
        let addr = unsafe { &*entry.addr };
        if addr.family != AF_INET as u8 {
            continue;
        }
        let mut name_len = 0usize;
        if !entry.name.is_null() {
            while name_len < 15 && unsafe { *entry.name.add(name_len) } != 0 {
                name_len += 1;
            }
        }
        if !entry.name.is_null() {
            out[count].name[..name_len]
                .copy_from_slice(unsafe { core::slice::from_raw_parts(entry.name, name_len) });
        }
        out[count].name_len = name_len;
        out[count].addr = addr.addr;
        count += 1;
    }
    unsafe { c_freeifaddrs(list) };
    Ok(count)
}

/// Darwin's `sendfile` can raise SIGPIPE; set SO_NOSIGPIPE on every accepted
/// socket instead of touching the process signal mask.
pub fn configure_stream(fd: i32) -> Result<()> {
    let one: i32 = 1;
    cvt(unsafe {
        c_setsockopt(
            fd,
            IPPROTO_TCP,
            TCP_NODELAY,
            &one as *const i32 as *const u8,
            4,
        )
    })?;
    let one: i32 = 1;
    cvt(unsafe {
        c_setsockopt(
            fd,
            SOL_SOCKET,
            SO_NOSIGPIPE,
            &one as *const i32 as *const u8,
            4,
        )
    })
    .map(|_| ())
}

pub fn configure_stream_nonblocking(fd: i32) -> Result<()> {
    configure_stream(fd)?;
    set_nonblocking(fd)
}

pub fn set_nonblocking(fd: i32) -> Result<()> {
    const F_GETFL: i32 = 3;
    const F_SETFL: i32 = 4;
    const O_NONBLOCK: i32 = 0x4;
    let flags = cvt(unsafe { c_fcntl(fd, F_GETFL) })?;
    cvt(unsafe { c_fcntl(fd, F_SETFL, flags | O_NONBLOCK) }).map(|_| ())
}

const EV_ADD: u16 = 0x1;
const EV_DELETE: u16 = 0x2;
const EV_ENABLE: u16 = 0x4;
const EV_EOF: u16 = 0x8000;
const EV_ERROR: u16 = 0x4000;
const EVFILT_READ: i16 = -1;
const EVFILT_WRITE: i16 = -2;

pub fn kqueue() -> Result<i32> {
    cvt(unsafe { c_kqueue() })
}

fn kevent_change(kq: i32, change: Kevent) -> Result<()> {
    let result = unsafe { c_kevent(kq, &change, 1, null_mut(), 0, null_mut()) };
    cvt(result).map(|_| ())
}

fn change(fd: i32, filter: i16, flags: u16, token: usize) -> Kevent {
    Kevent {
        ident: fd as usize,
        filter,
        flags,
        fflags: 0,
        data: 0,
        udata: token,
    }
}

pub fn kqueue_add(kq: i32, fd: i32, token: usize, write: bool) -> Result<()> {
    kevent_change(
        kq,
        change(
            fd,
            if write { EVFILT_WRITE } else { EVFILT_READ },
            EV_ADD | EV_ENABLE,
            token,
        ),
    )
}

pub fn kqueue_delete(kq: i32, fd: i32) -> Result<()> {
    let _ = kevent_change(kq, change(fd, EVFILT_READ, EV_DELETE, 0));
    let _ = kevent_change(kq, change(fd, EVFILT_WRITE, EV_DELETE, 0));
    Ok(())
}

pub fn kqueue_modify(kq: i32, fd: i32, token: usize, write: bool) -> Result<()> {
    kqueue_delete(kq, fd)?;
    kqueue_add(kq, fd, token, write)
}

pub fn kqueue_wait(kq: i32, events: &mut [super::super::Event], timeout_ms: i32) -> Result<usize> {
    let mut raw = [Kevent {
        ident: 0,
        filter: 0,
        flags: 0,
        fflags: 0,
        data: 0,
        udata: 0,
    }; 64];
    let count = core::cmp::min(events.len(), raw.len());
    let mut timeout_value = Timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    let timeout = if timeout_ms < 0 {
        null_mut()
    } else {
        timeout_value.tv_sec = (timeout_ms / 1000) as i64;
        timeout_value.tv_nsec = ((timeout_ms % 1000) * 1_000_000) as i64;
        &timeout_value as *const Timespec as *mut Timespec
    };
    let ready = unsafe { c_kevent(kq, null_mut(), 0, raw.as_mut_ptr(), count as i32, timeout) };
    let ready = cvt(ready)? as usize;
    for index in 0..ready {
        let event = &raw[index];
        let error = event.flags & EV_ERROR != 0;
        let readable = event.filter == EVFILT_READ || (event.flags & EV_EOF != 0);
        let writable = event.filter == EVFILT_WRITE;
        events[index] = super::super::Event {
            token: event.udata,
            readable,
            writable,
            error,
        };
    }
    Ok(ready)
}

pub fn block_sigpipe() -> Result<()> {
    Ok(())
}

pub fn write_fd(fd: i32, buf: &[u8]) {
    let mut written = 0usize;
    while written < buf.len() {
        match write(fd, &buf[written..]) {
            Ok(0) => return,
            Ok(count) => written += count,
            Err(Error::Interrupted) => continue,
            Err(_) => return,
        }
    }
}

pub fn close(fd: i32) -> Result<()> {
    cvt(unsafe { c_close(fd) }).map(|_| ())
}

pub fn open(dir_fd: i32, path: &[u8]) -> Result<i32> {
    open_path(dir_fd, path, O_RDONLY | O_CLOEXEC | O_NOFOLLOW_ANY, 0)
}

pub fn open_dir(dir_fd: i32, path: &[u8]) -> Result<i32> {
    open_path(
        dir_fd,
        path,
        O_RDONLY | O_DIRECTORY | O_CLOEXEC | O_NOFOLLOW_ANY,
        0,
    )
}

pub fn create_file(dir_fd: i32, path: &[u8], replace: bool) -> Result<i32> {
    let flags =
        O_WRONLY | O_CREAT | O_CLOEXEC | O_NOFOLLOW_ANY | if replace { O_TRUNC } else { O_EXCL };
    open_path(dir_fd, path, flags, MODE_FILE)
}

pub fn create_dir(dir_fd: i32, path: &[u8]) -> Result<()> {
    let (parent, name) = split_parent(path);
    if name.is_empty() {
        return Err(Error::InvalidInput);
    }
    let parent_fd = open_path(
        dir_fd,
        parent,
        O_RDONLY | O_DIRECTORY | O_CLOEXEC | O_NOFOLLOW_ANY,
        0,
    )?;
    let name = c_path(name)?;
    let result = cvt(unsafe { c_mkdirat(parent_fd, name.as_ptr(), MODE_DIR) });
    let _ = close(parent_fd);
    result.map(|_| ())
}

pub fn remove_file(dir_fd: i32, path: &[u8]) -> Result<()> {
    let (parent, name) = split_parent(path);
    if name.is_empty() {
        return Err(Error::InvalidInput);
    }
    let parent_fd = open_path(
        dir_fd,
        parent,
        O_RDONLY | O_DIRECTORY | O_CLOEXEC | O_NOFOLLOW_ANY,
        0,
    )?;
    let name = c_path(name)?;
    let result = cvt(unsafe { c_unlinkat(parent_fd, name.as_ptr(), 0) });
    let _ = close(parent_fd);
    result.map(|_| ())
}

pub fn dir_open(fd: i32) -> Result<*mut u8> {
    let dirp = unsafe { c_fdopendir(fd) };
    if dirp.is_null() {
        Err(last_error())
    } else {
        Ok(dirp)
    }
}

pub fn dir_close(dirp: *mut u8) -> Result<()> {
    cvt(unsafe { c_closedir(dirp) }).map(|_| ())
}

/// Read one entry into `name`; returns `(name_len, d_type)`.
pub fn next_dir(dirp: *mut u8, name: &mut [u8]) -> Result<Option<(usize, u8)>> {
    loop {
        // readdir signals end-of-directory with a null return and errno 0,
        // so clear errno first instead of trusting a stale value.
        unsafe { *c_error() = 0 };
        let entry = unsafe { c_readdir(dirp) };
        if entry.is_null() {
            let errno = unsafe { *c_error() };
            if errno == 0 {
                return Ok(None);
            }
            let error = map_errno(errno);
            if error == Error::Interrupted {
                continue;
            }
            return Err(error);
        }
        let entry = unsafe { &*entry };
        let name_len = entry.d_namlen as usize;
        if name_len > name.len() {
            return Err(Error::InvalidInput);
        }
        name[..name_len].copy_from_slice(&entry.d_name[..name_len]);
        return Ok(Some((name_len, entry.d_type)));
    }
}

fn stat_to_info(stat: &Stat) -> FileInfo {
    FileInfo {
        len: if stat.st_size < 0 {
            0
        } else {
            stat.st_size as u64
        },
        is_dir: stat.st_mode & S_IFMT == S_IFDIR,
        mtime: stat.st_mtimespec.tv_sec,
    }
}

pub fn fstat(fd: i32) -> Result<FileInfo> {
    let mut stat = Stat::default();
    cvt(unsafe { c_fstat(fd, &mut stat) })?;
    Ok(stat_to_info(&stat))
}

pub fn fstat_at(dir_fd: i32, name: &[u8], nofollow: bool) -> Result<FileInfo> {
    let path = c_path(name)?;
    let flags = if nofollow { AT_SYMLINK_NOFOLLOW } else { 0 };
    let mut stat = Stat::default();
    cvt(unsafe { c_fstatat(dir_fd, path.as_ptr(), &mut stat, flags) })?;
    Ok(stat_to_info(&stat))
}

pub fn exit(code: i32) -> ! {
    unsafe { c_exit(code) }
}
