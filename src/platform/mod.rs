//! OS abstraction layer.
//!
//! The `Platform` trait is the seam between portable HTTP logic and OS
//! facilities. It is deliberately small and object-free: a backend is a
//! zero-sized type implementing the trait, and all buffers are supplied by
//! the caller.
//!
//! The current implementation is blocking. The same trait shape is intended
//! to grow a reactor (`epoll`/`kqueue`/IOCP) for the async milestone; see
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
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FileInfo {
    pub len: u64,
    pub is_dir: bool,
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

    fn bind(&mut self, addr: SocketAddr, backlog: u16) -> Result<Self::Listener>;
    fn accept(&mut self, listener: &Self::Listener) -> Result<Self::Stream>;
    fn read(&mut self, stream: &mut Self::Stream, buf: &mut [u8]) -> Result<usize>;
    fn write_all(&mut self, stream: &mut Self::Stream, buf: &[u8]) -> Result<()>;

    /// Open `path` (a byte path, no NUL required) read-only.
    fn open(&mut self, path: &[u8]) -> Result<Self::File>;
    fn file_info(&mut self, file: &Self::File) -> Result<FileInfo>;
    fn read_file(&mut self, file: &mut Self::File, buf: &mut [u8]) -> Result<usize>;

    fn log(&mut self, message: &[u8]);
    fn exit(&mut self, code: i32) -> !;
}
