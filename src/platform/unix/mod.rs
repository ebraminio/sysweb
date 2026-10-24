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

use super::{
    AsyncPlatform, DirEntry, EntryKind, Error, Event, FileInfo, Interest, Interface, Platform,
    Result, SocketAddr,
};

const DIR_BUFFER: usize = 4096;
const DT_DIR: u8 = 4;
const DT_REG: u8 = 8;
const DT_LNK: u8 = 10;

/// The current OS backend. Holds the document-root descriptor.
pub struct Sys {
    root_fd: i32,
}

impl Sys {
    pub const fn new() -> Self {
        Self { root_fd: -1 }
    }

    fn root(&self) -> Result<i32> {
        if self.root_fd < 0 {
            Err(Error::InvalidInput)
        } else {
            Ok(self.root_fd)
        }
    }

    fn dir_entry(&mut self, dir_fd: i32, entry_name: &[u8], dtype: u8) -> Result<Option<DirEntry>> {
        let mut kind = match dtype {
            DT_DIR => EntryKind::Dir,
            DT_REG => EntryKind::File,
            DT_LNK => EntryKind::Symlink,
            _ => EntryKind::Other,
        };
        let info =
            abi::fstat_at(dir_fd, entry_name, kind == EntryKind::Symlink).unwrap_or(FileInfo {
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

        Ok(Some(DirEntry {
            name_len: entry_name.len(),
            kind,
            len: info.len,
            mtime: info.mtime,
        }))
    }
}

impl Default for Sys {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for Sys {
    fn drop(&mut self) {
        if self.root_fd >= 0 {
            let _ = abi::close(self.root_fd);
        }
    }
}

pub struct Listener {
    fd: i32,
}

pub struct Stream {
    fd: i32,
}

pub struct File {
    fd: i32,
}

pub struct Poller {
    fd: i32,
}

impl Drop for Poller {
    fn drop(&mut self) {
        let _ = abi::close(self.fd);
    }
}

pub struct Dir {
    fd: i32,
    #[cfg(target_os = "macos")]
    dirp: *mut u8,
    #[allow(dead_code)]
    buffer: [u8; DIR_BUFFER],
    #[allow(dead_code)]
    len: usize,
    #[allow(dead_code)]
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
        #[cfg(target_os = "macos")]
        {
            if self.dirp.is_null() {
                let _ = abi::close(self.fd);
            } else {
                let _ = abi::dir_close(self.dirp);
            }
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = abi::close(self.fd);
        }
    }
}

impl Platform for Sys {
    type Listener = Listener;
    type Stream = Stream;
    type File = File;
    type Dir = Dir;

    fn set_root(&mut self, root: &[u8]) -> Result<()> {
        abi::block_sigpipe()?;
        let fd = abi::open_root(root)?;
        if self.root_fd >= 0 {
            let _ = abi::close(self.root_fd);
        }
        self.root_fd = fd;
        Ok(())
    }

    fn interfaces(&mut self, out: &mut [Interface]) -> usize {
        abi::interfaces(out).unwrap_or(0)
    }

    fn bind(&mut self, addr: SocketAddr, backlog: u16) -> Result<Self::Listener> {
        let listener = Listener { fd: abi::socket()? };
        abi::set_reuseaddr(listener.fd)?;
        abi::bind(listener.fd, addr)?;
        abi::listen(listener.fd, backlog)?;
        Ok(listener)
    }

    fn open(&mut self, path: &[u8]) -> Result<Self::File> {
        loop {
            match abi::open(self.root()?, path) {
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

    fn open_dir(&mut self, path: &[u8]) -> Result<Self::Dir> {
        loop {
            match abi::open_dir(self.root()?, path) {
                Err(Error::Interrupted) => continue,
                Ok(fd) => {
                    #[cfg(target_os = "macos")]
                    {
                        match abi::dir_open(fd) {
                            Ok(dirp) => {
                                return Ok(Dir {
                                    fd,
                                    dirp,
                                    buffer: [0; DIR_BUFFER],
                                    len: 0,
                                    pos: 0,
                                })
                            }
                            Err(error) => {
                                let _ = abi::close(fd);
                                return Err(error);
                            }
                        }
                    }
                    #[cfg(not(target_os = "macos"))]
                    {
                        return Ok(Dir {
                            fd,
                            buffer: [0; DIR_BUFFER],
                            len: 0,
                            pos: 0,
                        });
                    }
                }
                Err(error) => return Err(error),
            }
        }
    }

    fn next_dir(&mut self, dir: &mut Self::Dir, name: &mut [u8]) -> Result<Option<DirEntry>> {
        #[cfg(target_os = "macos")]
        {
            loop {
                let entry = loop {
                    match abi::next_dir(dir.dirp, name) {
                        Err(Error::Interrupted) => continue,
                        other => break other?,
                    }
                };
                let Some((name_len, dtype)) = entry else {
                    return Ok(None);
                };
                let entry_name = &name[..name_len];
                if name_len == 0 || entry_name == b"." || entry_name == b".." {
                    continue;
                }
                return self.dir_entry(dir.fd, entry_name, dtype);
            }
        }
        #[cfg(not(target_os = "macos"))]
        {
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

                return self.dir_entry(dir.fd, entry_name, dtype);
            }
        }
    }

    fn create_file(&mut self, path: &[u8], replace: bool) -> Result<Self::File> {
        loop {
            match abi::create_file(self.root()?, path, replace) {
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
            match abi::create_dir(self.root()?, path) {
                Err(Error::Interrupted) => continue,
                other => return other,
            }
        }
    }

    fn remove_file(&mut self, path: &[u8]) -> Result<()> {
        loop {
            match abi::remove_file(self.root()?, path) {
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

impl AsyncPlatform for Sys {
    type Poller = Poller;

    fn poller(&mut self) -> Result<Self::Poller> {
        #[cfg(target_os = "linux")]
        let fd = abi::epoll_create()?;
        #[cfg(target_os = "macos")]
        let fd = abi::kqueue()?;
        Ok(Poller { fd })
    }

    fn register_listener(
        &mut self,
        poller: &mut Self::Poller,
        listener: &Self::Listener,
        token: usize,
    ) -> Result<()> {
        self.register_fd(poller, listener.fd, token, false)
    }

    fn register_stream(
        &mut self,
        poller: &mut Self::Poller,
        stream: &Self::Stream,
        token: usize,
    ) -> Result<()> {
        self.register_fd(poller, stream.fd, token, false)
    }

    fn set_interest(
        &mut self,
        poller: &mut Self::Poller,
        stream: &Self::Stream,
        token: usize,
        interest: Interest,
    ) -> Result<()> {
        let write = interest == Interest::Writable;
        #[cfg(target_os = "linux")]
        {
            abi::epoll_modify(poller.fd, stream.fd, token, write)
        }
        #[cfg(target_os = "macos")]
        {
            abi::kqueue_modify(poller.fd, stream.fd, token, write)
        }
    }

    fn deregister_stream(
        &mut self,
        poller: &mut Self::Poller,
        stream: &Self::Stream,
        _token: usize,
    ) -> Result<()> {
        #[cfg(target_os = "linux")]
        {
            abi::epoll_delete(poller.fd, stream.fd)
        }
        #[cfg(target_os = "macos")]
        {
            abi::kqueue_delete(poller.fd, stream.fd)
        }
    }

    fn wait(
        &mut self,
        poller: &mut Self::Poller,
        events: &mut [Event],
        timeout_ms: i32,
    ) -> Result<usize> {
        #[cfg(target_os = "linux")]
        {
            abi::epoll_wait(poller.fd, events, timeout_ms)
        }
        #[cfg(target_os = "macos")]
        {
            abi::kqueue_wait(poller.fd, events, timeout_ms)
        }
    }

    fn set_listener_nonblocking(&mut self, listener: &Self::Listener) -> Result<()> {
        abi::set_nonblocking(listener.fd)
    }

    fn accept_nonblocking(&mut self, listener: &Self::Listener) -> Result<Self::Stream> {
        loop {
            #[cfg(target_os = "linux")]
            match abi::accept_nonblocking(listener.fd) {
                Err(Error::Interrupted) => continue,
                Ok(fd) => return Ok(Stream { fd }),
                Err(error) => return Err(error),
            }
            #[cfg(target_os = "macos")]
            match abi::accept(listener.fd) {
                Err(Error::Interrupted) => continue,
                Ok(fd) => {
                    abi::configure_stream_nonblocking(fd)?;
                    return Ok(Stream { fd });
                }
                Err(error) => return Err(error),
            }
        }
    }

    fn read_nonblocking(&mut self, stream: &mut Self::Stream, buf: &mut [u8]) -> Result<usize> {
        loop {
            match abi::read(stream.fd, buf) {
                Err(Error::Interrupted) => continue,
                other => return other,
            }
        }
    }

    fn write_nonblocking(&mut self, stream: &mut Self::Stream, buf: &[u8]) -> Result<usize> {
        loop {
            match abi::send(stream.fd, buf) {
                Err(Error::Interrupted) => continue,
                other => return other,
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
        let want = core::cmp::min(len, 1 << 30) as usize;
        loop {
            match abi::sendfile(stream.fd, file.fd, offset, want) {
                Err(Error::Interrupted) => continue,
                Ok(count) => return Ok(count as u64),
                Err(error) => return Err(error),
            }
        }
    }
}

impl Sys {
    fn register_fd(
        &mut self,
        poller: &mut Poller,
        fd: i32,
        token: usize,
        write: bool,
    ) -> Result<()> {
        #[cfg(target_os = "linux")]
        {
            abi::epoll_add(poller.fd, fd, token, write)
        }
        #[cfg(target_os = "macos")]
        {
            abi::kqueue_add(poller.fd, fd, token, write)
        }
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

#[cfg(test)]
mod tests {
    #[test]
    fn lists_loopback_interface() {
        use super::{Interface, Platform, Sys};
        let mut sys = Sys::new();
        let mut interfaces = [Interface::default(); 16];
        let count = sys.interfaces(&mut interfaces);
        assert!(count > 0, "no IPv4 interfaces found");
    }
}
