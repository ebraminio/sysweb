//! Unix backends built directly on syscalls.
//!
//! Shared ownership/lifetime logic lives here; `linux` and `darwin` only
//! provide the raw syscall ABI (numbers, register convention, stat layout).

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
use linux as abi;

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
mod darwin;
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
use darwin as abi;

#[cfg(all(target_os = "macos", not(target_arch = "aarch64")))]
compile_error!(
    "sysweb's macOS backend currently targets Apple Silicon (aarch64) only; \
     Intel macOS support is on the roadmap"
);

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
compile_error!(
    "sysweb currently has Linux and macOS backends only; \
     the Windows CRT-free backend is on the roadmap"
);

use super::{DirEntry, EntryKind, Error, FileInfo, Platform, Result, SocketAddr};

const DIR_BUFFER: usize = 4096;
const DT_DIR: u8 = 4;
const DT_REG: u8 = 8;
const DT_LNK: u8 = 10;

/// The current OS backend. Zero-sized: all state lives in OS handles.
#[derive(Default)]
pub struct Sys;

pub struct Listener {
    fd: i32,
}

pub struct Stream {
    fd: i32,
}

pub struct File {
    fd: i32,
}

pub struct Dir {
    fd: i32,
    buffer: [u8; DIR_BUFFER],
    len: usize,
    pos: usize,
}

impl Drop for Listener {
    fn drop(&mut self) {
        let _ = abi::close(self.fd);
    }
}

impl Drop for Stream {
    fn drop(&mut self) {
        let _ = abi::close(self.fd);
    }
}

impl Drop for File {
    fn drop(&mut self) {
        let _ = abi::close(self.fd);
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = abi::close(self.fd);
    }
}

impl Platform for Sys {
    type Listener = Listener;
    type Stream = Stream;
    type File = File;
    type Dir = Dir;

    fn bind(&mut self, addr: SocketAddr, backlog: u16) -> Result<Self::Listener> {
        let listener = Listener { fd: abi::socket()? };
        abi::set_reuseaddr(listener.fd)?;
        abi::bind(listener.fd, addr)?;
        abi::listen(listener.fd, backlog)?;
        Ok(listener)
    }

    fn accept(&mut self, listener: &Self::Listener) -> Result<Self::Stream> {
        loop {
            match abi::accept(listener.fd) {
                Err(Error::Interrupted) => continue,
                Ok(fd) => return Ok(Stream { fd }),
                Err(error) => return Err(error),
            }
        }
    }

    fn read(&mut self, stream: &mut Self::Stream, buf: &mut [u8]) -> Result<usize> {
        loop {
            match abi::read(stream.fd, buf) {
                Err(Error::Interrupted) => continue,
                other => return other,
            }
        }
    }

    fn write_all(&mut self, stream: &mut Self::Stream, buf: &[u8]) -> Result<()> {
        let mut written = 0usize;
        while written < buf.len() {
            match abi::send(stream.fd, &buf[written..]) {
                Ok(0) => return Err(Error::BrokenPipe),
                Ok(count) => written += count,
                Err(Error::Interrupted) => continue,
                Err(error) => return Err(error),
            }
        }
        Ok(())
    }

    fn open(&mut self, path: &[u8]) -> Result<Self::File> {
        loop {
            match abi::open(path) {
                Err(Error::Interrupted) => continue,
                Ok(fd) => return Ok(File { fd }),
                Err(error) => return Err(error),
            }
        }
    }

    fn file_info(&mut self, file: &Self::File) -> Result<FileInfo> {
        loop {
            match abi::fstat(file.fd) {
                Err(Error::Interrupted) => continue,
                other => return other,
            }
        }
    }

    fn read_file(&mut self, file: &mut Self::File, buf: &mut [u8]) -> Result<usize> {
        loop {
            match abi::read(file.fd, buf) {
                Err(Error::Interrupted) => continue,
                other => return other,
            }
        }
    }

    fn open_dir(&mut self, path: &[u8]) -> Result<Self::Dir> {
        loop {
            match abi::open_dir(path) {
                Err(Error::Interrupted) => continue,
                Ok(fd) => {
                    return Ok(Dir {
                        fd,
                        buffer: [0; DIR_BUFFER],
                        len: 0,
                        pos: 0,
                    })
                }
                Err(error) => return Err(error),
            }
        }
    }

    fn next_dir(&mut self, dir: &mut Self::Dir, name: &mut [u8]) -> Result<Option<DirEntry>> {
        loop {
            if dir.pos == dir.len {
                let count = loop {
                    match abi::getdents(dir.fd, &mut dir.buffer) {
                        Err(Error::Interrupted) => continue,
                        other => break other?,
                    }
                };
                if count == 0 {
                    return Ok(None);
                }
                dir.len = count;
                dir.pos = 0;
            }

            let (name_off, name_len, reclen, dtype) =
                match abi::dirent_at(&dir.buffer[..dir.len], dir.pos) {
                    Some(entry) => entry,
                    None => return Err(Error::InvalidInput),
                };
            dir.pos += reclen;

            if name_len > name.len() {
                return Err(Error::InvalidInput);
            }
            name[..name_len].copy_from_slice(&dir.buffer[name_off..name_off + name_len]);
            let entry_name = &name[..name_len];
            if entry_name == b"." || entry_name == b".." || name_len == 0 {
                continue;
            }

            let mut kind = match dtype {
                DT_DIR => EntryKind::Dir,
                DT_REG => EntryKind::File,
                DT_LNK => EntryKind::Symlink,
                _ => EntryKind::Other,
            };
            let info =
                abi::fstat_at(dir.fd, entry_name, kind == EntryKind::Symlink).unwrap_or(FileInfo {
                    len: 0,
                    is_dir: false,
                    mtime: 0,
                });
            if kind == EntryKind::Other {
                kind = if info.is_dir {
                    EntryKind::Dir
                } else {
                    EntryKind::File
                };
            }

            return Ok(Some(DirEntry {
                name_len,
                kind,
                len: info.len,
                mtime: info.mtime,
            }));
        }
    }

    fn create_file(&mut self, path: &[u8], replace: bool) -> Result<Self::File> {
        loop {
            match abi::create_file(path, replace) {
                Err(Error::Interrupted) => continue,
                Ok(fd) => return Ok(File { fd }),
                Err(error) => return Err(error),
            }
        }
    }

    fn write_file(&mut self, file: &mut Self::File, buf: &[u8]) -> Result<()> {
        let mut written = 0usize;
        while written < buf.len() {
            match abi::write(file.fd, &buf[written..]) {
                Ok(0) => return Err(Error::BrokenPipe),
                Ok(count) => written += count,
                Err(Error::Interrupted) => continue,
                Err(error) => return Err(error),
            }
        }
        Ok(())
    }

    fn create_dir(&mut self, path: &[u8]) -> Result<()> {
        loop {
            match abi::create_dir(path) {
                Err(Error::Interrupted) => continue,
                other => return other,
            }
        }
    }

    fn remove_file(&mut self, path: &[u8]) -> Result<()> {
        loop {
            match abi::remove_file(path) {
                Err(Error::Interrupted) => continue,
                other => return other,
            }
        }
    }

    fn log(&mut self, message: &[u8]) {
        abi::write_fd(2, message);
    }

    fn exit(&mut self, code: i32) -> ! {
        abi::exit(code)
    }
}

pub fn write_stdout(message: &[u8]) {
    abi::write_fd(1, message);
}

pub fn write_stderr(message: &[u8]) {
    abi::write_fd(2, message);
}

pub fn exit(code: i32) -> ! {
    abi::exit(code)
}
