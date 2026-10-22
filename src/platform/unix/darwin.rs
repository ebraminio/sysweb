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
const BIND: usize = 104;
const LISTEN: usize = 106;
const SETSOCKOPT: usize = 105;
const OPENAT: usize = 463;
const EXIT: usize = 1;

const AF_INET: usize = 2;
const SOCK_STREAM: usize = 1;
const SOL_SOCKET: usize = 0xffff;
const SO_REUSEADDR: usize = 4;
const MSG_NOSIGNAL: usize = 0x80000;
const AT_FDCWD: usize = (-2isize) as usize;
const O_RDONLY: usize = 0;

/// Kernel `struct stat` offsets for the raw arm64 syscall ABI. These differ
/// from the userspace `struct stat` in the SDK (libSystem translates between
/// the two); verified on macOS 26/27 arm64.
const STAT_MODE_OFFSET: usize = 8;
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
        13 => Error::PermissionDenied,
        21 => Error::IsADirectory,
        23 | 24 => Error::TooManyOpenFiles,
        54 => Error::ConnectionReset,
        32 => Error::BrokenPipe,
        22 => Error::InvalidInput,
        other => Error::Other(other),
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

pub fn write_fd(fd: i32, buf: &[u8]) {
    let mut written = 0usize;
    while written < buf.len() {
        let result = unsafe {
            syscall6(
                WRITE,
                fd as usize,
                buf.as_ptr().add(written) as usize,
                buf.len() - written,
                0,
                0,
                0,
            )
        };
        match result {
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

pub fn open(path: &[u8]) -> Result<i32> {
    if path.len() >= MAX_PATH || path.contains(&0) {
        return Err(Error::InvalidInput);
    }
    let mut c_path = [0u8; MAX_PATH];
    c_path[..path.len()].copy_from_slice(path);
    c_path[path.len()] = 0;

    let result = unsafe {
        syscall6(
            OPENAT,
            AT_FDCWD,
            c_path.as_ptr() as usize,
            O_RDONLY,
            0,
            0,
            0,
        )
    };
    Ok(result? as i32)
}

#[repr(C, align(8))]
struct StatBuffer([u8; STAT_BUFFER_SIZE]);

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

    let mode =
        unsafe { read_unaligned(buffer.0.as_ptr().add(STAT_MODE_OFFSET) as *const u16) as u32 };
    let size = unsafe { read_unaligned(buffer.0.as_ptr().add(STAT_SIZE_OFFSET) as *const i64) };

    Ok(FileInfo {
        len: if size < 0 { 0 } else { size as u64 },
        is_dir: mode & S_IFMT == S_IFDIR,
    })
}

pub fn exit(code: i32) -> ! {
    unsafe {
        let _ = syscall6(EXIT, code as usize, 0, 0, 0, 0, 0);
    }
    loop {
        core::hint::spin_loop();
    }
}
