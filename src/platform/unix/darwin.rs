//! macOS arm64 raw syscall ABI.
//!
//! Apple does not document this ABI as stable, but it is the same interface
//! libSystem itself uses. The binary imports no libc symbols; ld64 requires a
//! libSystem load command for any dynamic executable, so the build links it
//! but never calls into it.
//!
//! Syscall convention: number in `x16`, arguments in `x0..x5`, `svc #0x80`,
//! carry set on error with the errno in `x0`.

use crate::path::MAX_PATH;
use crate::platform::{Error, FileInfo, Result, SocketAddr};
use core::arch::asm;
use core::ptr::read_unaligned;

const READ: usize = 3;
const WRITE: usize = 4;
const CLOSE: usize = 6;
const FSTAT: usize = 189;
const SOCKET: usize = 97;
const ACCEPT: usize = 30;
const SENDTO: usize = 133;
const SENDFILE: usize = 337;
const BIND: usize = 104;
const LISTEN: usize = 106;
const SETSOCKOPT: usize = 105;
const GETDIRENTRIES64: usize = 344;
const OPENAT: usize = 463;
const FSTATAT: usize = 469;
const UNLINKAT: usize = 472;
const MKDIRAT: usize = 475;
const EXIT: usize = 1;

const AF_INET: usize = 2;
const SOCK_STREAM: usize = 1;
const SOL_SOCKET: usize = 0xffff;
const SO_REUSEADDR: usize = 4;
const SO_NOSIGPIPE: usize = 0x1022;
const MSG_NOSIGNAL: usize = 0x80000;
const AT_FDCWD: usize = (-2isize) as usize;
const AT_SYMLINK_NOFOLLOW: usize = 0x20;

const O_RDONLY: usize = 0;
const O_WRONLY: usize = 1;
const O_NOFOLLOW_ANY: usize = 0x2000_0000;
const O_CLOEXEC: usize = 0x0100_0000;
const O_CREAT: usize = 0x200;
const O_TRUNC: usize = 0x400;
const O_EXCL: usize = 0x800;
const O_DIRECTORY: usize = 0x100000;
const MODE_FILE: usize = 0o644;
const MODE_DIR: usize = 0o755;

/// Kernel `struct stat` offsets for the raw arm64 syscall ABI. These differ
/// from the userspace `struct stat` in the SDK (libSystem translates between
/// the two); verified on macOS 26/27 arm64.
const STAT_MODE_OFFSET: usize = 8;
const STAT_MTIME_OFFSET: usize = 40;
const STAT_SIZE_OFFSET: usize = 72;
const STAT_BUFFER_SIZE: usize = 144;
const S_IFMT: u32 = 0o170000;
const S_IFDIR: u32 = 0o040000;

unsafe fn syscall6(
    number: usize,
    a1: usize,
    a2: usize,
    a3: usize,
    a4: usize,
    a5: usize,
    a6: usize,
) -> Result<usize> {
    let result: isize;
    let failed: u64;
    unsafe {
        asm!(
            "svc #0x80",
            "cset {failed}, cs",
            failed = out(reg) failed,
            in("x16") number,
            inlateout("x0") a1 => result,
            in("x1") a2,
            in("x2") a3,
            in("x3") a4,
            in("x4") a5,
            in("x5") a6,
            options(nostack),
        );
    }
    if failed != 0 {
        Err(map_errno(result as i32))
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
        23 | 24 => Error::TooManyOpenFiles,
        54 => Error::ConnectionReset,
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

fn open_path(dir_fd: i32, path: &[u8], flags: usize, mode: usize) -> Result<i32> {
    let path = c_path(path)?;
    unsafe {
        syscall6(
            OPENAT,
            dir_fd as usize,
            path.as_ptr() as usize,
            flags,
            mode,
            0,
            0,
        )
    }
    .map(|fd| fd as i32)
}

/// Open the document root, following symlinks in the root's own path. All
/// later operations are relative to this fd and use O_NOFOLLOW_ANY.
pub fn open_root(path: &[u8]) -> Result<i32> {
    let path = c_path(path)?;
    unsafe {
        syscall6(
            OPENAT,
            AT_FDCWD,
            path.as_ptr() as usize,
            O_RDONLY | O_DIRECTORY | O_CLOEXEC,
            0,
            0,
            0,
        )
    }
    .map(|fd| fd as i32)
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

#[repr(C)]
struct SockaddrIn {
    len: u8,
    family: u8,
    port: [u8; 2],
    addr: [u8; 4],
    zero: [u8; 8],
}

pub fn socket() -> Result<i32> {
    let result = unsafe { syscall6(SOCKET, AF_INET, SOCK_STREAM, 0, 0, 0, 0) };
    Ok(result? as i32)
}

pub fn set_reuseaddr(fd: i32) -> Result<()> {
    let one: i32 = 1;
    unsafe {
        syscall6(
            SETSOCKOPT,
            fd as usize,
            SOL_SOCKET,
            SO_REUSEADDR,
            &one as *const i32 as usize,
            4,
            0,
        )
    }
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
    unsafe {
        syscall6(
            BIND,
            fd as usize,
            &sockaddr as *const SockaddrIn as usize,
            core::mem::size_of::<SockaddrIn>(),
            0,
            0,
            0,
        )
    }
    .map(|_| ())
}

pub fn listen(fd: i32, backlog: u16) -> Result<()> {
    unsafe { syscall6(LISTEN, fd as usize, backlog as usize, 0, 0, 0, 0) }.map(|_| ())
}

pub fn accept(fd: i32) -> Result<i32> {
    let result = unsafe { syscall6(ACCEPT, fd as usize, 0, 0, 0, 0, 0) };
    Ok(result? as i32)
}

pub fn read(fd: i32, buf: &mut [u8]) -> Result<usize> {
    unsafe {
        syscall6(
            READ,
            fd as usize,
            buf.as_mut_ptr() as usize,
            buf.len(),
            0,
            0,
            0,
        )
    }
}

pub fn write(fd: i32, buf: &[u8]) -> Result<usize> {
    unsafe {
        syscall6(
            WRITE,
            fd as usize,
            buf.as_ptr() as usize,
            buf.len(),
            0,
            0,
            0,
        )
    }
}

pub fn send(fd: i32, buf: &[u8]) -> Result<usize> {
    unsafe {
        syscall6(
            SENDTO,
            fd as usize,
            buf.as_ptr() as usize,
            buf.len(),
            MSG_NOSIGNAL,
            0,
            0,
        )
    }
}

pub fn sendfile(out_fd: i32, in_fd: i32, offset: u64, count: usize) -> Result<usize> {
    let mut length = count as i64;
    let result = unsafe {
        syscall6(
            SENDFILE,
            in_fd as usize,
            out_fd as usize,
            offset as usize,
            &mut length as *mut i64 as usize,
            0,
            0,
        )
    };
    result?;
    if length < 0 {
        Err(Error::InvalidInput)
    } else {
        Ok(length as usize)
    }
}

/// Darwin's `sendfile` can raise SIGPIPE; set SO_NOSIGPIPE on every accepted
/// socket instead of touching the process signal mask.
pub fn configure_stream(fd: i32) -> Result<()> {
    let one: i32 = 1;
    unsafe {
        syscall6(
            SETSOCKOPT,
            fd as usize,
            SOL_SOCKET,
            SO_NOSIGPIPE,
            &one as *const i32 as usize,
            4,
            0,
        )
    }
    .map(|_| ())
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
    unsafe { syscall6(CLOSE, fd as usize, 0, 0, 0, 0, 0) }.map(|_| ())
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
    let result = unsafe {
        syscall6(
            MKDIRAT,
            parent_fd as usize,
            name.as_ptr() as usize,
            MODE_DIR,
            0,
            0,
            0,
        )
    };
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
    let result = unsafe {
        syscall6(
            UNLINKAT,
            parent_fd as usize,
            name.as_ptr() as usize,
            0,
            0,
            0,
            0,
        )
    };
    let _ = close(parent_fd);
    result.map(|_| ())
}

pub fn getdents(fd: i32, buf: &mut [u8]) -> Result<usize> {
    let mut base: i64 = 0;
    unsafe {
        syscall6(
            GETDIRENTRIES64,
            fd as usize,
            buf.as_mut_ptr() as usize,
            buf.len(),
            &mut base as *mut i64 as usize,
            0,
            0,
        )
    }
}

/// Decode one 64-bit `struct dirent` record at `pos`.
/// Returns `(name_offset, name_length, record_length, d_type)`.
pub fn dirent_at(buf: &[u8], pos: usize) -> Option<(usize, usize, usize, u8)> {
    if pos + 21 > buf.len() {
        return None;
    }
    let record_len = read_u16(buf, pos + 16)? as usize;
    let name_len = read_u16(buf, pos + 18)? as usize;
    if record_len < 21 || pos + record_len > buf.len() || 21 + name_len > record_len {
        return None;
    }
    Some((pos + 21, name_len, record_len, buf[pos + 20]))
}

fn read_u16(buf: &[u8], at: usize) -> Option<u16> {
    if at + 2 > buf.len() {
        return None;
    }
    Some(unsafe { read_unaligned(buf.as_ptr().add(at) as *const u16) })
}

#[repr(C, align(8))]
struct StatBuffer([u8; STAT_BUFFER_SIZE]);

fn parse_stat(buffer: &StatBuffer) -> FileInfo {
    let mode =
        unsafe { read_unaligned(buffer.0.as_ptr().add(STAT_MODE_OFFSET) as *const u16) as u32 };
    let size = unsafe { read_unaligned(buffer.0.as_ptr().add(STAT_SIZE_OFFSET) as *const i64) };
    let mtime = unsafe { read_unaligned(buffer.0.as_ptr().add(STAT_MTIME_OFFSET) as *const i64) };
    FileInfo {
        len: if size < 0 { 0 } else { size as u64 },
        is_dir: mode & S_IFMT == S_IFDIR,
        mtime,
    }
}

pub fn fstat(fd: i32) -> Result<FileInfo> {
    let mut buffer = StatBuffer([0; STAT_BUFFER_SIZE]);
    unsafe {
        syscall6(
            FSTAT,
            fd as usize,
            buffer.0.as_mut_ptr() as usize,
            0,
            0,
            0,
            0,
        )
    }?;
    Ok(parse_stat(&buffer))
}

pub fn fstat_at(dir_fd: i32, name: &[u8], nofollow: bool) -> Result<FileInfo> {
    let path = c_path(name)?;
    let flags = if nofollow { AT_SYMLINK_NOFOLLOW } else { 0 };
    let mut buffer = StatBuffer([0; STAT_BUFFER_SIZE]);
    unsafe {
        syscall6(
            FSTATAT,
            dir_fd as usize,
            path.as_ptr() as usize,
            buffer.0.as_mut_ptr() as usize,
            flags,
            0,
            0,
        )
    }?;
    Ok(parse_stat(&buffer))
}

pub fn exit(code: i32) -> ! {
    unsafe {
        let _ = syscall6(EXIT, code as usize, 0, 0, 0, 0, 0);
    }
    loop {
        core::hint::spin_loop();
    }
}
