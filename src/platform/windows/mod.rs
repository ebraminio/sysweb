//! CRT-free Win32 async backend over WSAPoll; IOCP remains future work.

#![allow(non_snake_case)]

use crate::platform::{
    DirEntry, EntryKind, Error, Event, FileInfo, Interest, Interface, Platform, Result, SocketAddr,
};
use core::ffi::c_void;
use core::ptr::null_mut;

const MAX_WPATH: usize = 1024;
const INVALID_HANDLE_VALUE: *mut c_void = (-1isize) as *mut c_void;
const INVALID_SOCKET: usize = usize::MAX;
const SOCKET_ERROR: i32 = -1;
const AF_INET: i32 = 2;
const SOCK_STREAM: i32 = 1;
const IPPROTO_TCP: i32 = 6;
const TCP_NODELAY: i32 = 1;
const SOL_SOCKET: i32 = 0xffff;
const SO_EXCLUSIVEADDRUSE: i32 = -5;
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
const INVALID_FILE_ATTRIBUTES: u32 = 0xffff_ffff;
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
const FIONBIO: i32 = 0x8004667eu32 as i32;
const POLLRDNORM: i16 = 0x0100;
const POLLWRNORM: i16 = 0x0010;
const POLLERR: i16 = 0x0001;
const POLLHUP: i16 = 0x0002;
const POLLNVAL: i16 = 0x0004;
const MAX_POLL: usize = 64;
const SEND_BUF: usize = 16 * 1024;

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
    fn GetFileAttributesW(path: *const u16) -> u32;
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
    fn ioctlsocket(s: usize, cmd: i32, value: *mut u32) -> i32;
    fn WSAPoll(fds: *mut WsaPollFd, count: u32, timeout: i32) -> i32;
}

#[repr(C)]
#[derive(Clone, Copy)]
struct WsaPollFd {
    fd: usize,
    events: i16,
    revents: i16,
}

#[derive(Clone, Copy)]
struct PollFd {
    fd: usize,
    token: usize,
    events: i16,
    active: bool,
}

impl PollFd {
    const fn empty() -> Self {
        Self {
            fd: usize::MAX,
            token: 0,
            events: 0,
            active: false,
        }
    }
}

pub struct Poller {
    fds: [PollFd; MAX_POLL],
}

impl Poller {
    fn new() -> Self {
        Self {
            fds: [PollFd::empty(); MAX_POLL],
        }
    }
}

pub struct Sys {
    root: [u16; MAX_WPATH],
    root_len: usize,
    wsa: bool,
}

pub struct Listener(usize);
pub struct Stream(usize);
pub struct File {
    handle: *mut c_void,
    buffer: [u8; SEND_BUF],
    buffer_offset: u64,
    buffer_pos: usize,
    buffer_len: usize,
}

impl File {
    fn new(handle: *mut c_void) -> Self {
        Self {
            handle,
            buffer: [0; SEND_BUF],
            buffer_offset: 0,
            buffer_pos: 0,
            buffer_len: 0,
        }
    }

    fn clear_buffer(&mut self) {
        self.buffer_len = 0;
        self.buffer_pos = 0;
        self.buffer_offset = 0;
    }
}
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
        unsafe { CloseHandle(self.handle) };
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

    fn check_parents(&self, rel: &[u8]) -> Result<()> {
        if rel == b"." || rel.is_empty() {
            return Ok(());
        }
        let trimmed = rel.strip_suffix(b"/").unwrap_or(rel);
        let mut path = [0u16; MAX_WPATH];
        path[..self.root_len].copy_from_slice(&self.root[..self.root_len]);
        let mut len = self.root_len;
        let mut start = 0usize;
        let mut index = 0usize;
        while index <= trimmed.len() {
            if index == trimmed.len() || trimmed[index] == b'/' || trimmed[index] == b'\\' {
                let segment = &trimmed[start..index];
                start = index + 1;
                if index == trimmed.len() || segment.is_empty() {
                    break;
                }
                if len > 0 {
                    path[len] = b'\\' as u16;
                    len += 1;
                }
                let converted = unsafe {
                    MultiByteToWideChar(
                        CP_UTF8,
                        0,
                        segment.as_ptr(),
                        segment.len() as i32,
                        path[len..].as_mut_ptr(),
                        (MAX_WPATH - len - 1) as i32,
                    )
                };
                if converted <= 0 || len + converted as usize + 1 >= path.len() {
                    return Err(Error::InvalidInput);
                }
                len += converted as usize;
                path[len] = 0;
                let attributes = unsafe { GetFileAttributesW(path.as_ptr()) };
                if attributes == INVALID_FILE_ATTRIBUTES {
                    return Err(Self::last_error());
                }
                if attributes & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
                    return Err(Error::PermissionDenied);
                }
            }
            index += 1;
        }
        Ok(())
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

    fn reject_reparse(handle: *mut c_void) -> Result<()> {
        let mut info = ByHandleFileInformation::default();
        if unsafe { GetFileInformationByHandle(handle, &mut info) } == 0 {
            return Err(Sys::last_error());
        }
        if info.attributes & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            Err(Error::PermissionDenied)
        } else {
            Ok(())
        }
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

#[link(name = "iphlpapi")]
unsafe extern "system" {
    fn GetIpAddrTable(table: *mut u8, size: *mut u32, order: i32) -> u32;
}

fn interface_name(index: u32, out: &mut Interface) {
    let mut digits = [0u8; 10];
    let mut count = 0usize;
    let mut value = index;
    if value == 0 {
        digits[0] = b'0';
        count = 1;
    } else {
        while value > 0 {
            digits[count] = b'0' + (value % 10) as u8;
            value /= 10;
            count += 1;
        }
    }
    out.name[..2].copy_from_slice(b"if");
    let mut pos = 2usize;
    while count > 0 && pos < out.name.len() {
        count -= 1;
        out.name[pos] = digits[count];
        pos += 1;
    }
    out.name_len = pos;
}

fn set_nodelay(fd: usize) -> Result<()> {
    let one: i32 = 1;
    if unsafe {
        setsockopt(
            fd,
            IPPROTO_TCP,
            TCP_NODELAY,
            &one as *const i32 as *const u8,
            4,
        )
    } == SOCKET_ERROR
    {
        return Err(Sys::last_wsa());
    }
    Ok(())
}

#[link(name = "kernel32")]
unsafe extern "system" {
    fn GetStdHandle(which: u32) -> *mut c_void;
}

impl Platform for Sys {
    type Listener = Listener;
    type Stream = Stream;
    type File = File;
    type Dir = Dir;

    fn interfaces(&mut self, out: &mut [Interface]) -> usize {
        let mut buffer = [0u8; 4096];
        let mut size = buffer.len() as u32;
        let result = unsafe { GetIpAddrTable(buffer.as_mut_ptr(), &mut size, 1) };
        if result != 0 || size < 4 {
            return 0;
        }
        let count = u32::from_ne_bytes([buffer[0], buffer[1], buffer[2], buffer[3]]) as usize;
        let mut written = 0usize;
        for index in 0..count {
            if written >= out.len() {
                break;
            }
            let offset = 4 + index * 24;
            if offset + 24 > buffer.len() {
                break;
            }
            let addr = u32::from_ne_bytes([
                buffer[offset],
                buffer[offset + 1],
                buffer[offset + 2],
                buffer[offset + 3],
            ])
            .to_ne_bytes();
            let interface_index = u32::from_ne_bytes([
                buffer[offset + 4],
                buffer[offset + 5],
                buffer[offset + 6],
                buffer[offset + 7],
            ]);
            if addr == [0, 0, 0, 0] {
                continue;
            }
            interface_name(interface_index, &mut out[written]);
            out[written].addr = addr;
            written += 1;
        }
        written
    }

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
        if unsafe {
            setsockopt(
                fd,
                SOL_SOCKET,
                SO_EXCLUSIVEADDRUSE,
                &one as *const i32 as *const u8,
                4,
            )
        } == SOCKET_ERROR
        {
            let error = Self::last_wsa();
            unsafe { closesocket(fd) };
            return Err(error);
        }
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

    fn open(&mut self, path: &[u8]) -> Result<Self::File> {
        self.check_parents(path)?;
        let handle = self.open_handle(
            path,
            GENERIC_READ,
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
        )?;
        if let Err(error) = Self::reject_reparse(handle) {
            unsafe { CloseHandle(handle) };
            return Err(error);
        }
        let _info = Self::info_for(handle)?;
        Ok(File::new(handle))
    }

    fn file_info(&mut self, file: &Self::File) -> Result<FileInfo> {
        Self::info_for(file.handle)
    }

    fn open_dir(&mut self, path: &[u8]) -> Result<Self::Dir> {
        self.check_parents(path)?;
        let check = self.open_handle(
            path,
            GENERIC_READ,
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
        )?;
        let reparse = Self::reject_reparse(check);
        unsafe { CloseHandle(check) };
        reparse?;

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
        self.check_parents(path)?;
        let creation = if replace { CREATE_ALWAYS } else { CREATE_NEW };
        let handle = self.open_handle(path, GENERIC_WRITE, creation, FILE_ATTRIBUTE_NORMAL)?;
        Ok(File::new(handle))
    }

    fn write_file(&mut self, file: &mut Self::File, buf: &[u8]) -> Result<()> {
        let mut written = 0u32;
        if unsafe {
            WriteFile(
                file.handle,
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
        self.check_parents(path)?;
        let mut full = [0u16; MAX_WPATH];
        self.root_path(path, &mut full)?;
        if unsafe { CreateDirectoryW(full.as_ptr(), null_mut()) } == 0 {
            return Err(Self::last_error());
        }
        Ok(())
    }

    fn remove_file(&mut self, path: &[u8]) -> Result<()> {
        self.check_parents(path)?;
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

    type Poller = Poller;

    fn poller(&mut self) -> Result<Self::Poller> {
        Ok(Poller::new())
    }

    fn register_listener(
        &mut self,
        poller: &mut Self::Poller,
        listener: &Self::Listener,
        token: usize,
    ) -> Result<()> {
        poller_register(poller, listener.0, token, false)
    }

    fn register_stream(
        &mut self,
        poller: &mut Self::Poller,
        stream: &Self::Stream,
        token: usize,
    ) -> Result<()> {
        poller_register(poller, stream.0, token, false)
    }

    fn set_interest(
        &mut self,
        poller: &mut Self::Poller,
        stream: &Self::Stream,
        token: usize,
        interest: Interest,
    ) -> Result<()> {
        if let Some(entry) = poller
            .fds
            .iter_mut()
            .find(|entry| entry.active && entry.fd == stream.0 && entry.token == token)
        {
            entry.events = if interest == Interest::Writable {
                POLLWRNORM
            } else {
                POLLRDNORM
            };
            Ok(())
        } else {
            Err(Error::InvalidInput)
        }
    }

    fn deregister_stream(
        &mut self,
        poller: &mut Self::Poller,
        stream: &Self::Stream,
        token: usize,
    ) -> Result<()> {
        if let Some(entry) = poller
            .fds
            .iter_mut()
            .find(|entry| entry.active && entry.fd == stream.0 && entry.token == token)
        {
            entry.active = false;
        }
        Ok(())
    }

    fn wait(
        &mut self,
        poller: &mut Self::Poller,
        events: &mut [Event],
        timeout_ms: i32,
    ) -> Result<usize> {
        let mut raw = [WsaPollFd {
            fd: usize::MAX,
            events: 0,
            revents: 0,
        }; MAX_POLL];
        let mut active = 0usize;
        for entry in &poller.fds {
            if !entry.active {
                continue;
            }
            raw[active] = WsaPollFd {
                fd: entry.fd,
                events: entry.events,
                revents: 0,
            };
            active += 1;
        }
        if active == 0 {
            return Ok(0);
        }
        let result = unsafe { WSAPoll(raw.as_mut_ptr(), active as u32, timeout_ms) };
        if result == SOCKET_ERROR {
            return Err(Self::last_wsa());
        }
        let mut count = 0usize;
        for index in 0..active {
            let revents = raw[index].revents;
            if revents == 0 {
                continue;
            }
            let fd = raw[index].fd;
            let Some(entry) = poller
                .fds
                .iter()
                .find(|entry| entry.active && entry.fd == fd)
            else {
                continue;
            };
            if count == events.len() {
                break;
            }
            events[count] = Event {
                token: entry.token,
                readable: revents & (POLLRDNORM | POLLHUP | POLLERR | POLLNVAL) != 0,
                writable: revents & POLLWRNORM != 0,
                error: revents & (POLLERR | POLLNVAL) != 0,
            };
            count += 1;
        }
        Ok(count)
    }

    fn set_listener_nonblocking(&mut self, listener: &Self::Listener) -> Result<()> {
        set_nonblocking(listener.0)
    }

    fn accept_nonblocking(&mut self, listener: &Self::Listener) -> Result<Self::Stream> {
        loop {
            let fd = unsafe { accept(listener.0, null_mut(), null_mut()) };
            if fd != INVALID_SOCKET {
                if let Err(error) = set_nodelay(fd) {
                    unsafe { closesocket(fd) };
                    return Err(error);
                }
                if let Err(error) = set_nonblocking(fd) {
                    unsafe { closesocket(fd) };
                    return Err(error);
                }
                return Ok(Stream(fd));
            }
            match Self::last_wsa() {
                Error::Interrupted => continue,
                error => return Err(error),
            }
        }
    }

    fn read_nonblocking(&mut self, stream: &mut Self::Stream, buf: &mut [u8]) -> Result<usize> {
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

    fn write_nonblocking(&mut self, stream: &mut Self::Stream, buf: &[u8]) -> Result<usize> {
        loop {
            let count = unsafe { send(stream.0, buf.as_ptr(), buf.len() as i32, 0) };
            if count >= 0 {
                return Ok(count as usize);
            }
            match Self::last_wsa() {
                Error::Interrupted => continue,
                error => return Err(error),
            }
        }
    }

    fn send_file_nonblocking(
        &mut self,
        stream: &mut Self::Stream,
        file: &mut Self::File,
        offset: u64,
        len: u64,
    ) -> Result<u64> {
        if file.buffer_len > 0 && offset != file.buffer_offset + file.buffer_pos as u64 {
            file.clear_buffer();
        }
        if file.buffer_len == 0 {
            let mut position = offset as i64;
            if unsafe { SetFilePointerEx(file.handle, position, &mut position, 0) } == 0 {
                return Err(Self::last_error());
            }
            let mut read = 0u32;
            let want = core::cmp::min(len, SEND_BUF as u64) as u32;
            if unsafe {
                ReadFile(
                    file.handle,
                    file.buffer.as_mut_ptr() as *mut c_void,
                    want,
                    &mut read,
                    null_mut(),
                )
            } == 0
                || read == 0
            {
                return Ok(0);
            }
            file.buffer_len = read as usize;
            file.buffer_pos = 0;
            file.buffer_offset = offset;
        }
        let sent = unsafe {
            send(
                stream.0,
                file.buffer[file.buffer_pos..file.buffer_len].as_ptr(),
                (file.buffer_len - file.buffer_pos) as i32,
                0,
            )
        };
        if sent > 0 {
            file.buffer_pos += sent as usize;
            if file.buffer_pos == file.buffer_len {
                file.clear_buffer();
            }
            return Ok(sent as u64);
        }
        match Self::last_wsa() {
            Error::WouldBlock | Error::Interrupted => Err(Self::last_wsa()),
            error => Err(error),
        }
    }
}

fn set_nonblocking(fd: usize) -> Result<()> {
    let mut one: u32 = 1;
    if unsafe { ioctlsocket(fd, FIONBIO, &mut one) } == SOCKET_ERROR {
        Err(Sys::last_wsa())
    } else {
        Ok(())
    }
}

fn poller_register(poller: &mut Poller, fd: usize, token: usize, write: bool) -> Result<()> {
    let index = poller
        .fds
        .iter()
        .position(|entry| !entry.active || (entry.fd == fd && entry.token == token))
        .ok_or(Error::TooManyOpenFiles)?;
    poller.fds[index] = PollFd {
        fd,
        token,
        events: if write { POLLWRNORM } else { POLLRDNORM },
        active: true,
    };
    Ok(())
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
