//! CRT-free Win32 backend (blocking first; IOCP reactor next).

#![allow(non_snake_case)]

use crate::platform::{DirEntry, EntryKind, Error, FileInfo, Platform, Result, SocketAddr};
use core::ffi::c_void;
use core::ptr::null_mut;

const MAX_WPATH: usize = 1024;
const INVALID_HANDLE_VALUE: *mut c_void = (-1isize) as *mut c_void;
const INVALID_SOCKET: usize = usize::MAX;
const SOCKET_ERROR: i32 = -1;
const AF_INET: i32 = 2;
const SOCK_STREAM: i32 = 1;
const IPPROTO_TCP: i32 = 6;
const SOL_SOCKET: i32 = 0xffff;
const SO_REUSEADDR: i32 = 4;
const GENERIC_READ: u32 = 0x8000_0000;
const GENERIC_WRITE: u32 = 0x4000_0000;
const FILE_SHARE_READ: u32 = 1;
const FILE_SHARE_WRITE: u32 = 2;
const FILE_SHARE_DELETE: u32 = 4;
const OPEN_EXISTING: u32 = 3;
const CREATE_NEW: u32 = 1;
const CREATE_ALWAYS: u32 = 2;
const FILE_ATTRIBUTE_NORMAL: u32 = 0x80;
const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
const FILE_ATTRIBUTE_DIRECTORY: u32 = 0x10;
const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
const ERROR_FILE_NOT_FOUND: u32 = 2;
const ERROR_PATH_NOT_FOUND: u32 = 3;
const ERROR_ACCESS_DENIED: u32 = 5;
const ERROR_ALREADY_EXISTS: u32 = 183;
const ERROR_FILE_EXISTS: u32 = 80;
const ERROR_DIRECTORY: u32 = 267;
const WSAEWOULDBLOCK: i32 = 10035;
const WSAEINTR: i32 = 10004;
const WSAEADDRINUSE: i32 = 10048;
const WSAECONNRESET: i32 = 10054;
const WSAECONNABORTED: i32 = 10053;
const WSAENOTSOCK: i32 = 10038;
const CP_UTF8: u32 = 65001;
const STDERR_HANDLE: u32 = -12i32 as u32;

#[repr(C)]
struct WsaData([u8; 512]);

#[repr(C)]
struct SockaddrIn {
    family: u16,
    port: [u8; 2],
    addr: [u8; 4],
    zero: [u8; 8],
}

#[repr(C)]
#[derive(Clone, Copy)]
struct Win32FindDataW {
    attributes: u32,
    creation_low: u32,
    creation_high: u32,
    access_low: u32,
    access_high: u32,
    write_low: u32,
    write_high: u32,
    size_high: u32,
    size_low: u32,
    reserved0: u32,
    reserved1: u32,
    name: [u16; 260],
    alternate: [u16; 14],
}

impl Default for Win32FindDataW {
    fn default() -> Self {
        Self {
            attributes: 0,
            creation_low: 0,
            creation_high: 0,
            access_low: 0,
            access_high: 0,
            write_low: 0,
            write_high: 0,
            size_high: 0,
            size_low: 0,
            reserved0: 0,
            reserved1: 0,
            name: [0; 260],
            alternate: [0; 14],
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct ByHandleFileInformation {
    attributes: u32,
    creation_low: u32,
    creation_high: u32,
    access_low: u32,
    access_high: u32,
    write_low: u32,
    write_high: u32,
    volume: u32,
    size_high: u32,
    size_low: u32,
    links: u32,
    index_high: u32,
    index_low: u32,
}

#[link(name = "kernel32")]
unsafe extern "system" {
    fn CreateFileW(
        name: *const u16,
        access: u32,
        share: u32,
        security: *mut c_void,
        creation: u32,
        flags: u32,
        template: *mut c_void,
    ) -> *mut c_void;
    fn ReadFile(
        file: *mut c_void,
        buffer: *mut c_void,
        len: u32,
        read: *mut u32,
        overlapped: *mut c_void,
    ) -> i32;
    fn WriteFile(
        file: *mut c_void,
        buffer: *const c_void,
        len: u32,
        written: *mut u32,
        overlapped: *mut c_void,
    ) -> i32;
    fn SetFilePointerEx(file: *mut c_void, distance: i64, new: *mut i64, method: u32) -> i32;
    fn GetFileInformationByHandle(file: *mut c_void, info: *mut ByHandleFileInformation) -> i32;
    fn CloseHandle(handle: *mut c_void) -> i32;
    fn CreateDirectoryW(path: *const u16, security: *mut c_void) -> i32;
    fn DeleteFileW(path: *const u16) -> i32;
    fn FindFirstFileW(path: *const u16, data: *mut Win32FindDataW) -> *mut c_void;
    fn FindNextFileW(handle: *mut c_void, data: *mut Win32FindDataW) -> i32;
    fn FindClose(handle: *mut c_void) -> i32;
    fn GetLastError() -> u32;
    fn Sleep(ms: u32);
    fn ExitProcess(code: u32) -> !;
    fn MultiByteToWideChar(
        page: u32,
        flags: u32,
        input: *const u8,
        input_len: i32,
        output: *mut u16,
        output_len: i32,
    ) -> i32;
    fn WideCharToMultiByte(
        page: u32,
        flags: u32,
        input: *const u16,
        input_len: i32,
        output: *mut u8,
        output_len: i32,
        default: *const u8,
        used: *mut i32,
    ) -> i32;
}

#[link(name = "ws2_32")]
unsafe extern "system" {
    fn WSAStartup(version: u16, data: *mut WsaData) -> i32;
    fn WSACleanup() -> i32;
    fn WSAGetLastError() -> i32;
    fn socket(af: i32, ty: i32, protocol: i32) -> usize;
    fn bind(s: usize, addr: *const SockaddrIn, len: i32) -> i32;
    fn listen(s: usize, backlog: i32) -> i32;
    fn accept(s: usize, addr: *mut SockaddrIn, len: *mut i32) -> usize;
    fn recv(s: usize, buffer: *mut u8, len: i32, flags: i32) -> i32;
    fn send(s: usize, buffer: *const u8, len: i32, flags: i32) -> i32;
    fn closesocket(s: usize) -> i32;
    fn setsockopt(s: usize, level: i32, name: i32, value: *const u8, len: i32) -> i32;
}

pub struct Sys {
    root: [u16; MAX_WPATH],
    root_len: usize,
    wsa: bool,
}

pub struct Listener(usize);
pub struct Stream(usize);
pub struct File(*mut c_void);
pub struct Dir {
    handle: *mut c_void,
    data: Win32FindDataW,
    first: bool,
}

impl Drop for Listener {
    fn drop(&mut self) {
        unsafe { closesocket(self.0) };
    }
}
impl Drop for Stream {
    fn drop(&mut self) {
        unsafe { closesocket(self.0) };
    }
}
impl Drop for File {
    fn drop(&mut self) {
        unsafe { CloseHandle(self.0) };
    }
}
impl Drop for Dir {
    fn drop(&mut self) {
        unsafe { FindClose(self.handle) };
    }
}

impl Sys {
    pub const fn new() -> Self {
        Self {
            root: [0; MAX_WPATH],
            root_len: 0,
            wsa: false,
        }
    }

    fn last_error() -> Error {
        map_win_error(unsafe { GetLastError() })
    }

    fn last_wsa() -> Error {
        map_wsa_error(unsafe { WSAGetLastError() })
    }

    fn root_path(&self, rel: &[u8], out: &mut [u16; MAX_WPATH]) -> Result<usize> {
        if self.root_len == 0 {
            return Err(Error::InvalidInput);
        }
        let mut len = self.root_len;
        out[..len].copy_from_slice(&self.root[..len]);
        if rel != b"." {
            if rel.is_empty() || rel.len() + len + 1 >= out.len() {
                return Err(Error::InvalidInput);
            }
            out[len] = b'\\' as u16;
            len += 1;
            let converted = unsafe {
                MultiByteToWideChar(
                    CP_UTF8,
                    0,
                    rel.as_ptr(),
                    rel.len() as i32,
                    out[len..].as_mut_ptr(),
                    (out.len() - len - 1) as i32,
                )
            };
            if converted <= 0 {
                return Err(Error::InvalidInput);
            }
            len += converted as usize;
        }
        out[len] = 0;
        Ok(len)
    }

    fn open_handle(
        &self,
        rel: &[u8],
        access: u32,
        creation: u32,
        flags: u32,
    ) -> Result<*mut c_void> {
        let mut path = [0u16; MAX_WPATH];
        self.root_path(rel, &mut path)?;
        let handle = unsafe {
            CreateFileW(
                path.as_ptr(),
                access,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                null_mut(),
                creation,
                flags,
                null_mut(),
            )
        };
        if handle == INVALID_HANDLE_VALUE {
            Err(Self::last_error())
        } else {
            Ok(handle)
        }
    }

    fn info_for(handle: *mut c_void) -> Result<FileInfo> {
        let mut info = ByHandleFileInformation::default();
        if unsafe { GetFileInformationByHandle(handle, &mut info) } == 0 {
            return Err(Sys::last_error());
        }
        let filetime = ((info.write_high as u64) << 32) | info.write_low as u64;
        let unix = filetime / 10_000_000;
        Ok(FileInfo {
            len: ((info.size_high as u64) << 32) | info.size_low as u64,
            is_dir: info.attributes & FILE_ATTRIBUTE_DIRECTORY != 0,
            mtime: unix.saturating_sub(11_644_473_600) as i64,
        })
    }

    fn configure(&mut self) -> Result<()> {
        if self.wsa {
            return Ok(());
        }
        let mut data = WsaData([0; 512]);
        if unsafe { WSAStartup(0x0202, &mut data) } != 0 {
            return Err(Error::Other(0));
        }
        self.wsa = true;
        Ok(())
    }
}

impl Default for Sys {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for Sys {
    fn drop(&mut self) {
        if self.wsa {
            unsafe { WSACleanup() };
        }
    }
}

fn map_win_error(code: u32) -> Error {
    match code {
        ERROR_FILE_NOT_FOUND | ERROR_PATH_NOT_FOUND => Error::NotFound,
        ERROR_ACCESS_DENIED => Error::PermissionDenied,
        ERROR_ALREADY_EXISTS | ERROR_FILE_EXISTS => Error::AlreadyExists,
        ERROR_DIRECTORY => Error::IsADirectory,
        _ => Error::Other(code as i32),
    }
}

fn map_wsa_error(code: i32) -> Error {
    match code {
        WSAEWOULDBLOCK => Error::WouldBlock,
        WSAEINTR => Error::Interrupted,
        WSAEADDRINUSE => Error::AddressInUse,
        WSAECONNRESET => Error::ConnectionReset,
        WSAECONNABORTED => Error::ConnectionAborted,
        WSAENOTSOCK => Error::InvalidInput,
        _ => Error::Other(code),
    }
}

fn sockaddr(addr: SocketAddr) -> SockaddrIn {
    SockaddrIn {
        family: AF_INET as u16,
        port: addr.port.to_be_bytes(),
        addr: addr.ip,
        zero: [0; 8],
    }
}

impl Platform for Sys {
    type Listener = Listener;
    type Stream = Stream;
    type File = File;
    type Dir = Dir;

    fn set_root(&mut self, root: &[u8]) -> Result<()> {
        if root.is_empty() || root.len() >= MAX_WPATH {
            return Err(Error::InvalidInput);
        }
        let converted = unsafe {
            MultiByteToWideChar(
                CP_UTF8,
                0,
                root.as_ptr(),
                root.len() as i32,
                self.root.as_mut_ptr(),
                (MAX_WPATH - 1) as i32,
            )
        };
        if converted <= 0 {
            return Err(Error::InvalidInput);
        }
        self.root_len = converted as usize;
        self.root[self.root_len] = 0;
        Ok(())
    }

    fn bind(&mut self, addr: SocketAddr, backlog: u16) -> Result<Self::Listener> {
        self.configure()?;
        let fd = unsafe { socket(AF_INET, SOCK_STREAM, IPPROTO_TCP) };
        if fd == INVALID_SOCKET {
            return Err(Self::last_wsa());
        }
        let one: i32 = 1;
        unsafe {
            setsockopt(
                fd,
                SOL_SOCKET,
                SO_REUSEADDR,
                &one as *const i32 as *const u8,
                4,
            )
        };
        let address = sockaddr(addr);
        if unsafe { bind(fd, &address, core::mem::size_of::<SockaddrIn>() as i32) } == SOCKET_ERROR
        {
            let error = Self::last_wsa();
            unsafe { closesocket(fd) };
            return Err(error);
        }
        if unsafe { listen(fd, backlog as i32) } == SOCKET_ERROR {
            let error = Self::last_wsa();
            unsafe { closesocket(fd) };
            return Err(error);
        }
        Ok(Listener(fd))
    }

    fn accept(&mut self, listener: &Self::Listener) -> Result<Self::Stream> {
        loop {
            let fd = unsafe { accept(listener.0, null_mut(), null_mut()) };
            if fd != INVALID_SOCKET {
                return Ok(Stream(fd));
            }
            match Self::last_wsa() {
                Error::Interrupted => continue,
                error => return Err(error),
            }
        }
    }

    fn read(&mut self, stream: &mut Self::Stream, buf: &mut [u8]) -> Result<usize> {
        loop {
            let count = unsafe { recv(stream.0, buf.as_mut_ptr(), buf.len() as i32, 0) };
            if count >= 0 {
                return Ok(count as usize);
            }
            match Self::last_wsa() {
                Error::Interrupted => continue,
                error => return Err(error),
            }
        }
    }

    fn write_all(&mut self, stream: &mut Self::Stream, buf: &[u8]) -> Result<()> {
        let mut written = 0usize;
        while written < buf.len() {
            let count = unsafe {
                send(
                    stream.0,
                    buf[written..].as_ptr(),
                    (buf.len() - written) as i32,
                    0,
                )
            };
            if count > 0 {
                written += count as usize;
                continue;
            }
            match Self::last_wsa() {
                Error::Interrupted => continue,
                error => return Err(error),
            }
        }
        Ok(())
    }

    fn sleep_ms(&mut self, ms: u64) {
        unsafe { Sleep(ms as u32) };
    }

    fn open(&mut self, path: &[u8]) -> Result<Self::File> {
        let handle = self.open_handle(
            path,
            GENERIC_READ,
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
        )?;
        let _info = Self::info_for(handle)?;
        Ok(File(handle))
    }

    fn file_info(&mut self, file: &Self::File) -> Result<FileInfo> {
        Self::info_for(file.0)
    }

    fn read_file(&mut self, file: &mut Self::File, buf: &mut [u8]) -> Result<usize> {
        let mut read = 0u32;
        if unsafe {
            ReadFile(
                file.0,
                buf.as_mut_ptr() as *mut c_void,
                buf.len() as u32,
                &mut read,
                null_mut(),
            )
        } == 0
        {
            return Err(Self::last_error());
        }
        Ok(read as usize)
    }

    fn send_file(
        &mut self,
        stream: &mut Self::Stream,
        file: &mut Self::File,
        offset: u64,
        len: u64,
    ) -> Result<u64> {
        let mut position = offset as i64;
        if unsafe { SetFilePointerEx(file.0, position, &mut position, 0) } == 0 {
            return Err(Self::last_error());
        }
        let mut remaining = len;
        let mut buffer = [0u8; 16 * 1024];
        while remaining > 0 {
            let want = core::cmp::min(remaining, buffer.len() as u64) as usize;
            let mut read = 0u32;
            if unsafe {
                ReadFile(
                    file.0,
                    buffer.as_mut_ptr() as *mut c_void,
                    want as u32,
                    &mut read,
                    null_mut(),
                )
            } == 0
                || read == 0
            {
                return Err(Self::last_error());
            }
            self.write_all(stream, &buffer[..read as usize])?;
            remaining -= read as u64;
        }
        Ok(len)
    }

    fn open_dir(&mut self, path: &[u8]) -> Result<Self::Dir> {
        let mut full = [0u16; MAX_WPATH];
        let len = self.root_path(path, &mut full)?;
        if len + 2 >= full.len() {
            return Err(Error::InvalidInput);
        }
        full[len] = b'\\' as u16;
        full[len + 1] = b'*' as u16;
        full[len + 2] = 0;
        let mut data = Win32FindDataW::default();
        let handle = unsafe { FindFirstFileW(full.as_ptr(), &mut data) };
        if handle == INVALID_HANDLE_VALUE {
            return Err(Self::last_error());
        }
        Ok(Dir {
            handle,
            data,
            first: true,
        })
    }

    fn next_dir(&mut self, dir: &mut Self::Dir, name: &mut [u8]) -> Result<Option<DirEntry>> {
        loop {
            if !dir.first {
                if unsafe { FindNextFileW(dir.handle, &mut dir.data) } == 0 {
                    return Ok(None);
                }
            }
            dir.first = false;
            let mut len = 0usize;
            while len < dir.data.name.len() && dir.data.name[len] != 0 {
                len += 1;
            }
            if len == 0 {
                continue;
            }
            let name16 = &dir.data.name[..len];
            if name16 == [b'.' as u16] || name16 == [b'.' as u16, b'.' as u16] {
                continue;
            }
            let converted = unsafe {
                WideCharToMultiByte(
                    CP_UTF8,
                    0,
                    name16.as_ptr(),
                    len as i32,
                    name.as_mut_ptr(),
                    name.len() as i32,
                    null_mut(),
                    null_mut(),
                )
            };
            if converted <= 0 || converted as usize > name.len() {
                return Err(Error::InvalidInput);
            }
            let info = if dir.data.attributes & FILE_ATTRIBUTE_DIRECTORY != 0 {
                EntryKind::Dir
            } else if dir.data.attributes & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
                EntryKind::Symlink
            } else {
                EntryKind::File
            };
            let filetime = ((dir.data.write_high as u64) << 32) | dir.data.write_low as u64;
            let unix = (filetime / 10_000_000).saturating_sub(11_644_473_600);
            return Ok(Some(DirEntry {
                name_len: converted as usize,
                kind: info,
                len: ((dir.data.size_high as u64) << 32) | dir.data.size_low as u64,
                mtime: unix as i64,
            }));
        }
    }

    fn create_file(&mut self, path: &[u8], replace: bool) -> Result<Self::File> {
        let creation = if replace { CREATE_ALWAYS } else { CREATE_NEW };
        let handle = self.open_handle(path, GENERIC_WRITE, creation, FILE_ATTRIBUTE_NORMAL)?;
        Ok(File(handle))
    }

    fn write_file(&mut self, file: &mut Self::File, buf: &[u8]) -> Result<()> {
        let mut written = 0u32;
        if unsafe {
            WriteFile(
                file.0,
                buf.as_ptr() as *const c_void,
                buf.len() as u32,
                &mut written,
                null_mut(),
            )
        } == 0
            || written as usize != buf.len()
        {
            return Err(Self::last_error());
        }
        Ok(())
    }

    fn create_dir(&mut self, path: &[u8]) -> Result<()> {
        let mut full = [0u16; MAX_WPATH];
        self.root_path(path, &mut full)?;
        if unsafe { CreateDirectoryW(full.as_ptr(), null_mut()) } == 0 {
            return Err(Self::last_error());
        }
        Ok(())
    }

    fn remove_file(&mut self, path: &[u8]) -> Result<()> {
        let mut full = [0u16; MAX_WPATH];
        self.root_path(path, &mut full)?;
        if unsafe { DeleteFileW(full.as_ptr()) } == 0 {
            return Err(Self::last_error());
        }
        Ok(())
    }

    fn log(&mut self, message: &[u8]) {
        unsafe {
            let handle = GetStdHandle(STDERR_HANDLE);
            let mut written = 0u32;
            let _ = WriteFile(
                handle,
                message.as_ptr() as *const c_void,
                message.len() as u32,
                &mut written,
                null_mut(),
            );
        }
    }

    fn exit(&mut self, code: i32) -> ! {
        unsafe { ExitProcess(code as u32) }
    }
}

#[link(name = "kernel32")]
unsafe extern "system" {
    fn GetStdHandle(which: u32) -> *mut c_void;
}

pub fn write_stdout(message: &[u8]) {
    unsafe {
        let handle = GetStdHandle(-11i32 as u32);
        let mut written = 0u32;
        let _ = WriteFile(
            handle,
            message.as_ptr() as *const c_void,
            message.len() as u32,
            &mut written,
            null_mut(),
        );
    }
}

pub fn write_stderr(message: &[u8]) {
    unsafe {
        let handle = GetStdHandle(STDERR_HANDLE);
        let mut written = 0u32;
        let _ = WriteFile(
            handle,
            message.as_ptr() as *const c_void,
            message.len() as u32,
            &mut written,
            null_mut(),
        );
    }
}

pub fn exit(code: i32) -> ! {
    unsafe { ExitProcess(code as u32) }
}
