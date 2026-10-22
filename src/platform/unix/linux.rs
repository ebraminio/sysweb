//! Linux raw syscall ABI (x86_64 and aarch64). No libc is involved.

use crate::path::MAX_PATH;
use crate::platform::{Error, FileInfo, Result, SocketAddr};
use core::arch::asm;
use core::ptr::read_unaligned;

#[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
compile_error!("the Linux backend currently supports x86_64 and aarch64 only");

#[cfg(target_arch = "x86_64")]
mod nr {
    pub const READ: usize = 0;
    pub const WRITE: usize = 1;
    pub const CLOSE: usize = 3;
    pub const STATX: usize = 332;
    pub const SOCKET: usize = 41;
    pub const ACCEPT: usize = 43;
    pub const SENDFILE: usize = 40;
    pub const SENDTO: usize = 44;
    pub const RT_SIGPROCMASK: usize = 14;
    pub const BIND: usize = 49;
    pub const LISTEN: usize = 50;
    pub const SETSOCKOPT: usize = 54;
    pub const GETDENTS64: usize = 217;
    pub const EXIT_GROUP: usize = 231;
    pub const OPENAT2: usize = 437;
    pub const MKDIRAT: usize = 258;
    pub const UNLINKAT: usize = 263;
}

#[cfg(target_arch = "aarch64")]
mod nr {
    pub const READ: usize = 63;
    pub const WRITE: usize = 64;
    pub const CLOSE: usize = 57;
    pub const STATX: usize = 291;
    pub const SOCKET: usize = 198;
    pub const ACCEPT: usize = 202;
    pub const SENDFILE: usize = 71;
    pub const SENDTO: usize = 206;
    pub const RT_SIGPROCMASK: usize = 135;
    pub const BIND: usize = 200;
    pub const LISTEN: usize = 201;
    pub const SETSOCKOPT: usize = 208;
    pub const GETDENTS64: usize = 61;
    pub const EXIT_GROUP: usize = 94;
    pub const OPENAT2: usize = 437;
    pub const MKDIRAT: usize = 34;
    pub const UNLINKAT: usize = 35;
}

use nr::*;

const AF_INET: usize = 2;
const SOCK_STREAM: usize = 1;
const SOL_SOCKET: usize = 1;
const SO_REUSEADDR: usize = 2;
const MSG_NOSIGNAL: usize = 0x4000;
const AT_FDCWD: usize = (-100isize) as usize;
const AT_SYMLINK_NOFOLLOW: usize = 0x100;
const AT_EMPTY_PATH: usize = 0x1000;
const STATX_BASIC_STATS: usize = 0x07ff;

const O_RDONLY: usize = 0;
const O_WRONLY: usize = 1;
const O_CREAT: usize = 0x40;
const O_EXCL: usize = 0x80;
const O_TRUNC: usize = 0x200;
#[cfg(target_arch = "x86_64")]
const O_DIRECTORY: usize = 0x10000;
#[cfg(target_arch = "aarch64")]
const O_DIRECTORY: usize = 0x4000;
const O_CLOEXEC: usize = 0x80000;
const RESOLVE_NO_SYMLINKS: u64 = 0x04;
const MODE_FILE: usize = 0o644;
const MODE_DIR: usize = 0o755;

/// `struct statx` offsets, verified against the Linux UAPI. We request
/// `STATX_BASIC_STATS` only; stx_mode is u16, stx_size and stx_mtime.tv_sec are
/// 64-bit.
const STATX_MODE_OFFSET: usize = 28;
const STATX_SIZE_OFFSET: usize = 40;
const STATX_MTIME_OFFSET: usize = 112;
const STATX_BUFFER_SIZE: usize = 256;
const S_IFMT: u32 = 0o170000;
const S_IFDIR: u32 = 0o040000;

#[cfg(target_arch = "x86_64")]
unsafe fn syscall6(
    number: usize,
    a1: usize,
    a2: usize,
    a3: usize,
    a4: usize,
    a5: usize,
    a6: usize,
) -> isize {
    let result: isize;
    unsafe {
        asm!(
            "syscall",
            inlateout("rax") number as isize => result,
            in("rdi") a1,
            in("rsi") a2,
            in("rdx") a3,
            in("r10") a4,
            in("r8") a5,
            in("r9") a6,
            lateout("rcx") _,
            lateout("r11") _,
            options(nostack),
        );
    }
    result
}

#[cfg(target_arch = "aarch64")]
unsafe fn syscall6(
    number: usize,
    a1: usize,
    a2: usize,
    a3: usize,
    a4: usize,
    a5: usize,
    a6: usize,
) -> isize {
    let result: isize;
    unsafe {
        asm!(
            "svc #0",
            in("x8") number,
            inlateout("x0") a1 => result,
            in("x1") a2,
            in("x2") a3,
            in("x3") a4,
            in("x4") a5,
            in("x5") a6,
            options(nostack),
        );
    }
    result
}

fn decode(result: isize) -> Result<usize> {
    if result < 0 {
        Err(map_errno((-result) as i32))
    } else {
        Ok(result as usize)
    }
}

fn map_errno(errno: i32) -> Error {
    match errno {
        4 => Error::Interrupted,
        11 => Error::WouldBlock,
        2 | 20 => Error::NotFound,
        13 => Error::PermissionDenied,
        40 => Error::PermissionDenied, // ELOOP: symlink with O_NOFOLLOW
        17 => Error::AlreadyExists,
        21 => Error::IsADirectory,
        23 | 24 => Error::TooManyOpenFiles,
        104 => Error::ConnectionReset,
        32 => Error::BrokenPipe,
        22 | 36 => Error::InvalidInput,
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

#[repr(C)]
struct OpenHow {
    flags: u64,
    mode: u64,
    resolve: u64,
}

fn open_path(dir_fd: i32, path: &[u8], flags: usize, mode: usize) -> Result<i32> {
    let path = c_path(path)?;
    let how = OpenHow {
        flags: flags as u64,
        mode: mode as u64,
        resolve: RESOLVE_NO_SYMLINKS,
    };
    let result = unsafe {
        syscall6(
            OPENAT2,
            dir_fd as usize,
            path.as_ptr() as usize,
            &how as *const OpenHow as usize,
            core::mem::size_of::<OpenHow>(),
            0,
            0,
        )
    };
    Ok(decode(result)? as i32)
}

/// Open the document root, following symlinks in the root's own path. All
/// later operations are relative to this fd and forbid symlinks.
pub fn open_root(path: &[u8]) -> Result<i32> {
    let path = c_path(path)?;
    let how = OpenHow {
        flags: (O_RDONLY | O_DIRECTORY | O_CLOEXEC) as u64,
        mode: 0,
        resolve: 0,
    };
    let result = unsafe {
        syscall6(
            OPENAT2,
            AT_FDCWD,
            path.as_ptr() as usize,
            &how as *const OpenHow as usize,
            core::mem::size_of::<OpenHow>(),
            0,
            0,
        )
    };
    Ok(decode(result)? as i32)
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
    family: u16,
    port: [u8; 2],
    addr: [u8; 4],
    zero: [u8; 8],
}

pub fn socket() -> Result<i32> {
    let result = unsafe { syscall6(SOCKET, AF_INET, SOCK_STREAM, 0, 0, 0, 0) };
    Ok(decode(result)? as i32)
}

pub fn set_reuseaddr(fd: i32) -> Result<()> {
    let one: i32 = 1;
    let result = unsafe {
        syscall6(
            SETSOCKOPT,
            fd as usize,
            SOL_SOCKET,
            SO_REUSEADDR,
            &one as *const i32 as usize,
            4,
            0,
        )
    };
    decode(result).map(|_| ())
}

pub fn bind(fd: i32, addr: SocketAddr) -> Result<()> {
    let sockaddr = SockaddrIn {
        family: AF_INET as u16,
        port: addr.port.to_be_bytes(),
        addr: addr.ip,
        zero: [0; 8],
    };
    let result = unsafe {
        syscall6(
            BIND,
            fd as usize,
            &sockaddr as *const SockaddrIn as usize,
            core::mem::size_of::<SockaddrIn>(),
            0,
            0,
            0,
        )
    };
    decode(result).map(|_| ())
}

pub fn listen(fd: i32, backlog: u16) -> Result<()> {
    let result = unsafe { syscall6(LISTEN, fd as usize, backlog as usize, 0, 0, 0, 0) };
    decode(result).map(|_| ())
}

pub fn accept(fd: i32) -> Result<i32> {
    let result = unsafe { syscall6(ACCEPT, fd as usize, 0, 0, 0, 0, 0) };
    Ok(decode(result)? as i32)
}

pub fn read(fd: i32, buf: &mut [u8]) -> Result<usize> {
    let result = unsafe {
        syscall6(
            READ,
            fd as usize,
            buf.as_mut_ptr() as usize,
            buf.len(),
            0,
            0,
            0,
        )
    };
    decode(result)
}

pub fn write(fd: i32, buf: &[u8]) -> Result<usize> {
    let result = unsafe {
        syscall6(
            WRITE,
            fd as usize,
            buf.as_ptr() as usize,
            buf.len(),
            0,
            0,
            0,
        )
    };
    decode(result)
}

pub fn send(fd: i32, buf: &[u8]) -> Result<usize> {
    let result = unsafe {
        syscall6(
            SENDTO,
            fd as usize,
            buf.as_ptr() as usize,
            buf.len(),
            MSG_NOSIGNAL,
            0,
            0,
        )
    };
    decode(result)
}

pub fn sendfile(out_fd: i32, in_fd: i32, offset: u64, count: usize) -> Result<usize> {
    let mut offset = offset as i64;
    let result = unsafe {
        syscall6(
            SENDFILE,
            out_fd as usize,
            in_fd as usize,
            &mut offset as *mut i64 as usize,
            count,
            0,
            0,
        )
    };
    decode(result)
}

/// Block SIGPIPE process-wide. `sendfile` has no per-call MSG_NOSIGNAL flag,
/// so a client disconnect must produce EPIPE instead of killing the process.
pub fn block_sigpipe() -> Result<()> {
    const SIGPIPE_BIT: u64 = 1 << (13 - 1);
    const SIG_BLOCK: usize = 0;
    let set = SIGPIPE_BIT;
    let result = unsafe {
        syscall6(
            RT_SIGPROCMASK,
            SIG_BLOCK,
            &set as *const u64 as usize,
            0,
            8,
            0,
            0,
        )
    };
    decode(result).map(|_| ())
}

pub fn configure_stream(_fd: i32) -> Result<()> {
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
    let result = unsafe { syscall6(CLOSE, fd as usize, 0, 0, 0, 0, 0) };
    decode(result).map(|_| ())
}

pub fn open(dir_fd: i32, path: &[u8]) -> Result<i32> {
    open_path(dir_fd, path, O_RDONLY | O_CLOEXEC, 0)
}

pub fn open_dir(dir_fd: i32, path: &[u8]) -> Result<i32> {
    open_path(dir_fd, path, O_RDONLY | O_DIRECTORY | O_CLOEXEC, 0)
}

pub fn create_file(dir_fd: i32, path: &[u8], replace: bool) -> Result<i32> {
    let flags = O_WRONLY | O_CREAT | O_CLOEXEC | if replace { O_TRUNC } else { O_EXCL };
    open_path(dir_fd, path, flags, MODE_FILE)
}

pub fn create_dir(dir_fd: i32, path: &[u8]) -> Result<()> {
    let (parent, name) = split_parent(path);
    if name.is_empty() {
        return Err(Error::InvalidInput);
    }
    let parent_fd = open_path(dir_fd, parent, O_RDONLY | O_DIRECTORY | O_CLOEXEC, 0)?;
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
    decode(result).map(|_| ())
}

pub fn remove_file(dir_fd: i32, path: &[u8]) -> Result<()> {
    let (parent, name) = split_parent(path);
    if name.is_empty() {
        return Err(Error::InvalidInput);
    }
    let parent_fd = open_path(dir_fd, parent, O_RDONLY | O_DIRECTORY | O_CLOEXEC, 0)?;
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
    decode(result).map(|_| ())
}

pub fn getdents(fd: i32, buf: &mut [u8]) -> Result<usize> {
    let result = unsafe {
        syscall6(
            GETDENTS64,
            fd as usize,
            buf.as_mut_ptr() as usize,
            buf.len(),
            0,
            0,
            0,
        )
    };
    decode(result)
}

/// Decode one `linux_dirent64` record at `pos`.
/// Returns `(name_offset, name_length, record_length, d_type)`.
pub fn dirent_at(buf: &[u8], pos: usize) -> Option<(usize, usize, usize, u8)> {
    if pos + 19 > buf.len() {
        return None;
    }
    let record_len = read_u16(buf, pos + 16)? as usize;
    if record_len < 19 || pos + record_len > buf.len() {
        return None;
    }
    let name_offset = pos + 19;
    let record_end = pos + record_len;
    let mut name_end = name_offset;
    while name_end < record_end && buf[name_end] != 0 {
        name_end += 1;
    }
    Some((
        name_offset,
        name_end - name_offset,
        record_len,
        buf[pos + 18],
    ))
}

fn read_u16(buf: &[u8], at: usize) -> Option<u16> {
    if at + 2 > buf.len() {
        return None;
    }
    Some(unsafe { read_unaligned(buf.as_ptr().add(at) as *const u16) })
}

#[repr(C, align(8))]
struct StatxBuffer([u8; STATX_BUFFER_SIZE]);

fn parse_statx(buffer: &StatxBuffer) -> FileInfo {
    let mode =
        unsafe { read_unaligned(buffer.0.as_ptr().add(STATX_MODE_OFFSET) as *const u16) as u32 };
    let size = unsafe { read_unaligned(buffer.0.as_ptr().add(STATX_SIZE_OFFSET) as *const u64) };
    let mtime = unsafe { read_unaligned(buffer.0.as_ptr().add(STATX_MTIME_OFFSET) as *const i64) };
    FileInfo {
        len: size,
        is_dir: mode & S_IFMT == S_IFDIR,
        mtime,
    }
}

pub fn fstat(fd: i32) -> Result<FileInfo> {
    let mut buffer = StatxBuffer([0; STATX_BUFFER_SIZE]);
    let result = unsafe {
        syscall6(
            STATX,
            fd as usize,
            c"".as_ptr() as usize,
            AT_EMPTY_PATH,
            STATX_BASIC_STATS,
            buffer.0.as_mut_ptr() as usize,
            0,
        )
    };
    decode(result)?;
    Ok(parse_statx(&buffer))
}

pub fn fstat_at(dir_fd: i32, name: &[u8], nofollow: bool) -> Result<FileInfo> {
    let path = c_path(name)?;
    let flags = if nofollow { AT_SYMLINK_NOFOLLOW } else { 0 };
    let mut buffer = StatxBuffer([0; STATX_BUFFER_SIZE]);
    let result = unsafe {
        syscall6(
            STATX,
            dir_fd as usize,
            path.as_ptr() as usize,
            flags,
            STATX_BASIC_STATS,
            buffer.0.as_mut_ptr() as usize,
            0,
        )
    };
    decode(result)?;
    Ok(parse_statx(&buffer))
}

pub fn exit(code: i32) -> ! {
    unsafe {
        syscall6(EXIT_GROUP, code as usize, 0, 0, 0, 0, 0);
    }
    loop {
        core::hint::spin_loop();
    }
}

#[cfg(test)]
mod tests {
    use super::{parse_statx, StatxBuffer, STATX_BUFFER_SIZE};

    #[test]
    fn parses_statx_fields() {
        let mut buffer = StatxBuffer([0; STATX_BUFFER_SIZE]);
        buffer.0[28..30].copy_from_slice(&0o100644u16.to_ne_bytes());
        buffer.0[40..48].copy_from_slice(&12_345u64.to_ne_bytes());
        buffer.0[112..120].copy_from_slice(&1_700_000_000i64.to_ne_bytes());

        let info = parse_statx(&buffer);
        assert_eq!(info.len, 12_345);
        assert!(!info.is_dir);
        assert_eq!(info.mtime, 1_700_000_000);

        buffer.0[28..30].copy_from_slice(&0o040755u16.to_ne_bytes());
        assert!(parse_statx(&buffer).is_dir);
    }
}
