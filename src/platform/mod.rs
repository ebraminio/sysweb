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

pub mod unix;

pub use unix::{exit, write_stderr, write_stdout, Sys};

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
    TooManyOpenFiles,
    ConnectionReset,
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
    fn accept(&mut self, listener: &Self::Listener) -> Result<Self::Stream>;
    fn read(&mut self, stream: &mut Self::Stream, buf: &mut [u8]) -> Result<usize>;
    fn write_all(&mut self, stream: &mut Self::Stream, buf: &[u8]) -> Result<()>;

    /// Open `path` (relative to the document root) read-only.
    fn open(&mut self, path: &[u8]) -> Result<Self::File>;
    fn file_info(&mut self, file: &Self::File) -> Result<FileInfo>;
    fn read_file(&mut self, file: &mut Self::File, buf: &mut [u8]) -> Result<usize>;

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
