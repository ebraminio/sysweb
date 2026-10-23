//! OS abstraction layer.
//!
//! The `Platform` trait is the seam between portable HTTP logic and OS
//! facilities. It is deliberately small and object-free: a backend is a
//! zero-sized type implementing the trait, and all buffers are supplied by
//! the caller.
//!
//! The current implementation is blocking. The same trait shape is intended
//! to grow a reactor (`io_uring`/`kqueue`/IOCP) for the async milestone; see
//! `ARCHITECTURE.md`.

#[cfg(unix)]
pub mod unix;
#[cfg(windows)]
pub mod windows;

#[cfg(unix)]
pub use unix::{exit, write_stderr, write_stdout, Sys};
#[cfg(windows)]
pub use windows::{exit, write_stderr, write_stdout, Sys};

/// A normalized error from the platform layer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// The call was interrupted by a signal; safe to retry.
    Interrupted,
    /// The operation would block (used by the future non-blocking backend).
    WouldBlock,
    NotFound,
    PermissionDenied,
    IsADirectory,
    AlreadyExists,
    AddressInUse,
    TooManyOpenFiles,
    ConnectionReset,
    ConnectionAborted,
    BrokenPipe,
    InvalidInput,
    /// Any errno the abstraction does not know about.
    Other(i32),
}

pub type Result<T> = core::result::Result<T, Error>;

/// IPv4 socket address. IPv6 is on the roadmap.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SocketAddr {
    pub ip: [u8; 4],
    pub port: u16,
}

impl SocketAddr {
    pub const fn new(ip: [u8; 4], port: u16) -> Self {
        Self { ip, port }
    }

    pub const fn loopback(port: u16) -> Self {
        Self::new([127, 0, 0, 1], port)
    }

    pub const fn any(port: u16) -> Self {
        Self::new([0, 0, 0, 0], port)
    }
}

/// Metadata needed to serve a file.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FileInfo {
    pub len: u64,
    pub is_dir: bool,
    /// Seconds since the Unix epoch, as reported by `fstat`.
    pub mtime: i64,
}

/// The kind of a directory entry, from `d_type` or a follow-up stat.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EntryKind {
    File,
    Dir,
    Symlink,
    Other,
}

/// One directory entry. The name itself is copied into the caller's buffer;
/// `name_len` says how much of that buffer is valid.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DirEntry {
    pub name_len: usize,
    pub kind: EntryKind,
    pub len: u64,
    pub mtime: i64,
}

/// Readiness interest for the reactor backend.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Interest {
    Readable,
    Writable,
}

/// One readiness event returned by `AsyncPlatform::wait`.
#[derive(Clone, Copy, Debug, Default)]
pub struct Event {
    pub token: usize,
    pub readable: bool,
    pub writable: bool,
    pub error: bool,
}

/// Non-blocking OS services used by the async server.
///
/// The blocking `Platform` trait remains for file/metadata operations and for
/// the existing server path. Only socket progress is required to be async in
/// the first milestone.
pub trait AsyncPlatform: Platform {
    type Poller;

    fn poller(&mut self) -> Result<Self::Poller>;
    fn register_listener(
        &mut self,
        poller: &mut Self::Poller,
        listener: &Self::Listener,
        token: usize,
    ) -> Result<()>;
    fn register_stream(
        &mut self,
        poller: &mut Self::Poller,
        stream: &Self::Stream,
        token: usize,
    ) -> Result<()>;
    fn set_interest(
        &mut self,
        poller: &mut Self::Poller,
        stream: &Self::Stream,
        token: usize,
        interest: Interest,
    ) -> Result<()>;
    fn deregister_stream(
        &mut self,
        poller: &mut Self::Poller,
        stream: &Self::Stream,
        token: usize,
    ) -> Result<()>;
    fn wait(
        &mut self,
        poller: &mut Self::Poller,
        events: &mut [Event],
        timeout_ms: i32,
    ) -> Result<usize>;

    fn set_listener_nonblocking(&mut self, listener: &Self::Listener) -> Result<()>;
    fn accept_nonblocking(&mut self, listener: &Self::Listener) -> Result<Self::Stream>;
    fn read_nonblocking(&mut self, stream: &mut Self::Stream, buf: &mut [u8]) -> Result<usize>;
    fn write_nonblocking(&mut self, stream: &mut Self::Stream, buf: &[u8]) -> Result<usize>;
    fn send_file_nonblocking(
        &mut self,
        stream: &mut Self::Stream,
        file: &mut Self::File,
        offset: u64,
        len: u64,
    ) -> Result<u64>;
}

/// One local IPv4 interface address for startup logging.
#[derive(Clone, Copy)]
pub struct Interface {
    pub name: [u8; 16],
    pub name_len: usize,
    pub addr: [u8; 4],
}

impl Interface {
    pub const fn new() -> Self {
        Self {
            name: [0; 16],
            name_len: 0,
            addr: [0; 4],
        }
    }
}

impl Default for Interface {
    fn default() -> Self {
        Self::new()
    }
}

/// The OS services used by the server.
///
/// Types returned by the backend own their OS handles and close them on drop.
/// Methods take `&mut self` so a reactor-based backend can keep mutable state
/// without changing the trait later.
pub trait Platform {
    type Listener;
    type Stream;
    type File;
    type Dir;

    /// Open the document root once. All later paths are relative to this
    /// directory, and symlink rules apply only beneath it.
    fn set_root(&mut self, root: &[u8]) -> Result<()>;
    fn bind(&mut self, addr: SocketAddr, backlog: u16) -> Result<Self::Listener>;
    /// Fill `out` with up-facing IPv4 interface addresses. Returns the count.
    fn interfaces(&mut self, out: &mut [Interface]) -> usize;
    fn accept(&mut self, listener: &Self::Listener) -> Result<Self::Stream>;
    fn read(&mut self, stream: &mut Self::Stream, buf: &mut [u8]) -> Result<usize>;
    fn write_all(&mut self, stream: &mut Self::Stream, buf: &[u8]) -> Result<()>;
    /// Blocking-only backoff for transient accept failures.
    ///
    /// TODO(async): the reactor should schedule the retry through its timer or
    /// deadline instead. Remove this method once the accept loop no longer
    /// calls it and no blocking backend needs it.
    fn sleep_ms(&mut self, ms: u64);

    /// Open `path` (relative to the document root) read-only.
    fn open(&mut self, path: &[u8]) -> Result<Self::File>;
    fn file_info(&mut self, file: &Self::File) -> Result<FileInfo>;
    fn read_file(&mut self, file: &mut Self::File, buf: &mut [u8]) -> Result<usize>;
    /// Send up to `len` bytes from `file` starting at `offset` to `stream`
    /// using the OS zero-copy path. Returns the number of bytes sent.
    fn send_file(
        &mut self,
        stream: &mut Self::Stream,
        file: &mut Self::File,
        offset: u64,
        len: u64,
    ) -> Result<u64>;

    /// Open a directory (relative to the document root) for iteration.
    /// Symlinks anywhere beneath the root are refused.
    fn open_dir(&mut self, path: &[u8]) -> Result<Self::Dir>;
    /// Copy the next entry name into `name` and return its metadata. Returns
    /// `None` at end-of-directory.
    fn next_dir(&mut self, dir: &mut Self::Dir, name: &mut [u8]) -> Result<Option<DirEntry>>;

    /// Create a file (relative to the document root) for streaming writes.
    /// Existing files are refused unless `replace` is true, in which case they
    /// are truncated.
    fn create_file(&mut self, path: &[u8], replace: bool) -> Result<Self::File>;
    /// Write the whole buffer to a writable file.
    fn write_file(&mut self, file: &mut Self::File, buf: &[u8]) -> Result<()>;
    /// Create one directory. Parent directories must already exist.
    fn create_dir(&mut self, path: &[u8]) -> Result<()>;
    /// Remove a file (used to clean up a failed upload).
    fn remove_file(&mut self, path: &[u8]) -> Result<()>;

    fn log(&mut self, message: &[u8]);
    fn exit(&mut self, code: i32) -> !;
}
