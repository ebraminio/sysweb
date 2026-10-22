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
    pub const FSTAT: usize = 5;
    pub const SOCKET: usize = 41;
    pub const ACCEPT: usize = 43;
    pub const SENDTO: usize = 44;
    pub const BIND: usize = 49;
    pub const LISTEN: usize = 50;
    pub const SETSOCKOPT: usize = 54;
    pub const EXIT_GROUP: usize = 231;
    pub const OPENAT: usize = 257;
}

#[cfg(target_arch = "aarch64")]
mod nr {
    pub const READ: usize = 63;
    pub const WRITE: usize = 64;
    pub const CLOSE: usize = 57;
    pub const FSTAT: usize = 80;
    pub const SOCKET: usize = 198;
    pub const ACCEPT: usize = 202;
    pub const SENDTO: usize = 206;
    pub const BIND: usize = 200;
    pub const LISTEN: usize = 201;
    pub const SETSOCKOPT: usize = 208;
    pub const EXIT_GROUP: usize = 94;
    pub const OPENAT: usize = 56;
}

use nr::*;

const AF_INET: usize = 2;
const SOCK_STREAM: usize = 1;
const SOL_SOCKET: usize = 1;
const SO_REUSEADDR: usize = 2;
const MSG_NOSIGNAL: usize = 0x4000;
const AT_FDCWD: usize = (-100isize) as usize;
const O_RDONLY: usize = 0;

/// `struct stat` offsets, verified against the Linux UAPI for both ABIs.
/// st_mode is a u32 on Linux and st_size is a signed 64-bit off_t.
#[cfg(target_arch = "x86_64")]
const STAT_MODE_OFFSET: usize = 24;
#[cfg(target_arch = "aarch64")]
const STAT_MODE_OFFSET: usize = 16;
const STAT_SIZE_OFFSET: usize = 48;
const STAT_BUFFER_SIZE: usize = 144;
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
        21 => Error::IsADirectory,
        23 | 24 => Error::TooManyOpenFiles,
        104 => Error::ConnectionReset,
        32 => Error::BrokenPipe,
        22 => Error::InvalidInput,
        other => Error::Other(other),
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
        match decode(result) {
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
    Ok(decode(result)? as i32)
}

#[repr(C, align(8))]
struct StatBuffer([u8; STAT_BUFFER_SIZE]);

pub fn fstat(fd: i32) -> Result<FileInfo> {
    let mut buffer = StatBuffer([0; STAT_BUFFER_SIZE]);
    let result = unsafe {
        syscall6(
            FSTAT,
            fd as usize,
            buffer.0.as_mut_ptr() as usize,
            0,
            0,
            0,
            0,
        )
    };
    decode(result)?;

    let mode = unsafe { read_unaligned(buffer.0.as_ptr().add(STAT_MODE_OFFSET) as *const u32) };
    let size = unsafe { read_unaligned(buffer.0.as_ptr().add(STAT_SIZE_OFFSET) as *const i64) };

    Ok(FileInfo {
        len: if size < 0 { 0 } else { size as u64 },
        is_dir: mode & S_IFMT == S_IFDIR,
    })
}

pub fn exit(code: i32) -> ! {
    unsafe {
        syscall6(EXIT_GROUP, code as usize, 0, 0, 0, 0, 0);
    }
    loop {
        core::hint::spin_loop();
    }
}
