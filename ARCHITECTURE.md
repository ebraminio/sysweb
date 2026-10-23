# Architecture

This document describes how `sysweb` is put together, the low-level contracts
it relies on, and how the async milestone fits without introducing an allocator
or a futures runtime.

## Goals and non-goals

Goals:

1. `#![no_std]`, no `alloc`, no global allocator.
2. No third-party crates.
3. Direct OS interfaces, with a portable abstraction above them.
4. Linux: no libc, static ELF, custom `_start`.
5. Windows: no-CRT spike validated; IOCP backend on the roadmap.
6. `unsafe` concentrated in small, reviewable ABI modules.
7. A blocking implementation that already has the shape of the async one.
8. Latest OS APIs only. No legacy fallbacks.

Non-goals for now: TLS, dynamic content, CGI, compression, virtual hosts,
authentication, and hardening for hostile public traffic.

## Latest-API policy

When two OS interfaces exist, the newer one is used and the older one is not
implemented:

| Area | Chosen API | Minimum |
| --- | --- | --- |
| Linux open/containment | `openat2` + `RESOLVE_NO_SYMLINKS` | Linux 5.6 |
| Linux metadata | `statx` (`STATX_BASIC_STATS`) | Linux 4.11 |
| Linux directories | `getdents64` | Linux 2.6.23 |
| Linux async | `io_uring` (not epoll) | Linux 5.1+ (milestone 2) |
| Linux zero-copy | `sendfile` | Linux 2.6 |
| macOS open/containment | `O_NOFOLLOW_ANY` | macOS 11 |
| macOS directories | `fdopendir`/`readdir` | public libSystem |
| macOS metadata | `fstatat` + SDK `struct stat` | 64-bit-inode macOS |
| macOS async | `kqueue`/`kevent` | macOS 10.5+ |
| macOS ABI | libSystem functions | macOS 11+ |
| Windows async | IOCP + `GetQueuedCompletionStatusEx` | Windows 10 1809+ |
| Windows files | `CreateFile2` | Windows 8+ |

There is no legacy Linux `openat`/`fstat` fallback, no epoll backend, no
`WSAPoll` path, and no pre-Vista Windows story. If the host is older than the
table, the project does not target it.

## Layers

```
┌──────────────────────────────────────────────────────────────┐
│ src/bin/syswebd.rs                                           │
│   _start (Linux) / _main (macOS), panic handler, mem* shims  │
├──────────────────────────────────────────────────────────────┤
│ src/server.rs                                                │
│   accept loop, fixed buffers, uploads, response writer, logs │
├──────────────────────────────────────────────────────────────┤
│ src/listing.rs                                               │
│   self-contained directory page in a 64 KiB fixed buffer     │
├──────────────────────────────────────────────────────────────┤
│ src/http.rs  src/path.rs  src/mime.rs  src/config.rs         │
│   pure, safe, allocation-free                                │
├──────────────────────────────────────────────────────────────┤
│ src/platform/mod.rs                                          │
│   trait Platform; Error; SocketAddr; FileInfo                │
├──────────────────────────────────────────────────────────────┤
│ src/platform/unix/mod.rs                                     │
│   RAII Listener/Stream/File/Dir; shared Unix logic           │
├──────────────────────────────────────────────────────────────┤
│ src/platform/unix/linux.rs  src/platform/unix/darwin.rs      │
│   Linux raw syscalls; macOS libSystem functions              │
└──────────────────────────────────────────────────────────────┘
```

The dependency direction is strictly downward. HTTP code cannot see a raw file
descriptor; it only sees `P::Stream`, `P::File`, and the `Platform` methods.

## The `Platform` trait

```rust
pub trait Platform {
    type Listener;
    type Stream;
    type File;
    type Dir;

    fn set_root(&mut self, root: &[u8]) -> Result<()>;
    fn bind(&mut self, addr: SocketAddr, backlog: u16) -> Result<Self::Listener>;
    fn accept(&mut self, listener: &Self::Listener) -> Result<Self::Stream>;
    fn read(&mut self, stream: &mut Self::Stream, buf: &mut [u8]) -> Result<usize>;
    fn write_all(&mut self, stream: &mut Self::Stream, buf: &[u8]) -> Result<()>;
    fn sleep_ms(&mut self, ms: u64);

    fn open(&mut self, path: &[u8]) -> Result<Self::File>;
    fn file_info(&mut self, file: &Self::File) -> Result<FileInfo>;
    fn read_file(&mut self, file: &mut Self::File, buf: &mut [u8]) -> Result<usize>;
    fn send_file(
        &mut self,
        stream: &mut Self::Stream,
        file: &mut Self::File,
        offset: u64,
        len: u64,
    ) -> Result<u64>;

    fn open_dir(&mut self, path: &[u8]) -> Result<Self::Dir>;
    fn next_dir(&mut self, dir: &mut Self::Dir, name: &mut [u8])
        -> Result<Option<DirEntry>>;

    fn create_file(&mut self, path: &[u8], replace: bool) -> Result<Self::File>;
    fn write_file(&mut self, file: &mut Self::File, buf: &[u8]) -> Result<()>;
    fn create_dir(&mut self, path: &[u8]) -> Result<()>;
    fn remove_file(&mut self, path: &[u8]) -> Result<()>;

    fn log(&mut self, message: &[u8]);
    fn exit(&mut self, code: i32) -> !;
}
```

Design notes:

- **Generic, not `dyn`.** The server is monomorphized for `platform::Sys`.
  There is no vtable and no allocation.
- **RAII handles.** `Listener`, `Stream`, `File`, and `Dir` own an `i32` fd
  and close it in `Drop`. The server never calls `close` manually, so early
  returns and error paths cannot leak.
- **Caller-provided buffers.** No method allocates. The backend fills the
  caller's slice, and all sizes are bounded by the caller.
- **`&mut self`.** A future reactor keeps its io_uring/kqueue/IOCP state inside
  the backend without changing the trait.
- **`set_root`.** The document root is opened once. Every later path is
  relative to that directory fd, so no-symlink rules apply beneath the root
  without rejecting a symlinked system prefix such as macOS `/var`.
- **Normalized errors.** `Error` hides errno differences (`EAGAIN` is 11 on
  Linux and 35 on macOS, `ECONNRESET` is 104 vs 54, etc.).
- **Liveness over throughput.** A stalled blocking server is acceptable; a
  permanent exit on transient pressure is not. `accept` retries
  `EMFILE`/`ENFILE`, `ECONNABORTED`, `ECONNRESET`, and `EAGAIN` after a 100 ms
  `sleep_ms` (libSystem `nanosleep` on macOS) instead of exiting.
  `sleep_ms` is explicitly blocking-only: the async reactor must replace this
  with a timer/deadline and remove it if no blocking backend remains.
  Fatal listener errors still terminate; there is no supervisor yet.
- **Size budget.** Binary size is a design constraint, not an afterthought.
  CI and the release workflow enforce 112 KiB on macOS arm64, 64 KiB on Linux
  x86_64, and 72 KiB on Linux aarch64 via `scripts/check-size.sh`; changing a
  budget is a deliberate change, not a silent regression.

## Linux backend

### Syscall table

| Operation | x86_64 | aarch64 |
| --- | ---: | ---: |
| `read` | 0 | 63 |
| `write` | 1 | 64 |
| `close` | 3 | 57 |
| `socket` | 41 | 198 |
| `accept` | 43 | 202 |
| `sendto` | 44 | 206 |
| `bind` | 49 | 200 |
| `listen` | 50 | 201 |
| `setsockopt` | 54 | 208 |
| `getdents64` | 217 | 61 |
| `sendfile` | 40 | 71 |
| `rt_sigprocmask` | 14 | 135 |
| `exit_group` | 231 | 94 |
| `mkdirat` | 258 | 34 |
| `unlinkat` | 263 | 35 |
| `statx` | 332 | 291 |
| `openat2` | 437 | 437 |

Conventions:

- x86_64: number in `rax`, arguments in `rdi, rsi, rdx, r10, r8, r9`,
  `syscall`, result in `rax`. `rcx` and `r11` are clobbered.
- aarch64: number in `x8`, arguments in `x0..x5`, `svc #0`.
- A result in `[-4095, -1]` is `-errno`; everything else is success.

The wrappers use `asm!` with `options(nostack)`, no `nomem`, and explicitly
declare the clobbered registers. They are `unsafe fn`, called only from the
`unix` layer which validates slice lengths and pointer provenance.

### Document-root fd and symlink containment

`Platform::set_root` opens the document root once at startup, following
symlinks in the root's own path (so `--root /var/...` works on macOS, where
`/var` is a symlink). Every later operation is relative to that directory fd:

- Linux: `openat2(root_fd, "a/b", RESOLVE_NO_SYMLINKS)`; the root is opened
  with `resolve = 0`.
- macOS: `openat(root_fd, "a/b", O_NOFOLLOW_ANY)`; the root is opened without
  `O_NOFOLLOW_ANY`.

`create_dir` and `remove_file` split the path into parent and basename, open the
parent with the same no-symlink policy, then call `mkdirat`/`unlinkat` relative
to it. A symlink anywhere beneath the root — final component or intermediate
directory — is rejected with `ELOOP` and mapped to 403.

This is why the server keeps a `Sys` value with a root fd instead of the old
zero-sized backend.

### `struct statx` and macOS `struct stat`

Linux uses `statx` with a 256-byte buffer and `STATX_BASIC_STATS`:

| Field | Offset | Type |
| --- | ---: | --- |
| `stx_mode` | 28 | `u16` |
| `stx_size` | 40 | `u64` |
| `stx_mtime.tv_sec` | 112 | `i64` |

`fstat(fd)` is `statx(fd, "", AT_EMPTY_PATH, ...)`; `fstat_at` uses the root
directory fd and relative name, with `AT_SYMLINK_NOFOLLOW` for symlink entries.

macOS uses libSystem `fstat`/`fstatat` and the SDK's 144-byte 64-bit-inode
`struct stat`: `st_mode` at 4 (`u16`), `st_size` at 96 (`i64`), and
`st_mtimespec.tv_sec` at 48 (`i64`). The struct is `#[repr(C)]`; no kernel
layout is assumed.

The Linux stat buffer is `#[repr(C, align(8))]`. `st_mode & 0170000 == 0040000`
detects a directory. `st_mtime` drives the index's modification column and is formatted
with a stack-only civil-from-days conversion (UTC; the page upgrades it to local
time in JS).

### No-libc linking

`build.rs` adds these flags to the **binary only**:

```
-nostdlib -static -no-pie
```

`-nostdlib` suppresses startup files and default libraries, including libc and
libgcc. `compiler_builtins` still provides integer helpers. The binary provides:

- `_start` (assembly) which extracts `argc`/`argv` from the initial stack,
  aligns the stack, and calls `sysweb_entry`;
- `memcpy`, `memmove`, `memset`, `memcmp`, `bcmp`, `bzero`;
- `rust_eh_personality` (a no-op) and `__stack_chk_fail`;
- a `panic_handler` that writes a fixed message and calls `exit_group(101)`.

`-no-pie` matters: Rust's Linux targets default to PIE, and a PIE link would
bring in a dynamic loader. The result is `ET_EXEC`, statically linked, with no
`PT_INTERP` and no `.dynamic` section. `nm -u` is empty.

The `x86_64` cross-build in this repository was linked with `ld.lld`; native
Linux CI uses the system `cc` driver. Both accept the same Rust link args.

### The `mem*` trap under LTO

LLVM knows the C library semantics of `bzero` and `memset`. The first version
of `bzero` was:

```rust
pub unsafe extern "C" fn bzero(dest: *mut c_void, count: usize) {
    memset(dest, 0, count);
}
```

With `lto = true`, LLVM canonicalized that body back into a call to `bzero`,
producing:

```
_bzero:
    b _bzero
```

— an infinite loop that only appears in release builds. `bzero` is now its own
volatile store loop. CI runs `cargo test --release` specifically to exercise
the LTO binary, not just the debug build.

## macOS backend (libSystem ABI)

Apple does not document raw syscall numbers or the kernel `struct stat` layout
as stable. They are private XNU ABI and can change across OS releases. Since
ld64 requires `libSystem` for every dynamic executable anyway, the macOS
backend calls the stable libSystem ABI directly instead of guessing syscall
numbers:

- `openat`, `fstat`, `fstatat`, `unlinkat`, `mkdirat`;
- `socket`, `bind`, `listen`, `accept`, `setsockopt`;
- `read`, `write`, `send`, `sendfile`, `close`, `poll`;
- `fdopendir`, `readdir`, `closedir`;
- `__error` for errno and `_exit` for termination.

`build.rs` adds `-lSystem`; the binary still has no CRT, no `std`, no
allocator, and no third-party crates. The tradeoff is explicit: `nm -u` now
lists a small allowlisted set of libSystem functions, checked by
`scripts/check-binary.sh`. Directory iteration uses `fdopendir`/`readdir`,
which may allocate inside libSystem; the binary itself never links an
allocator.

`AT_FDCWD` is `-2`, `sockaddr_in` begins with a length byte, `SOL_SOCKET` is
`0xffff`, and `SO_REUSEADDR` is `4`. `O_NOFOLLOW_ANY` is available from macOS
11. Intel macOS is not implemented yet.

## HTTP pipeline

1. `read_request` reads into an 8 KiB stack buffer until `\r\n\r\n` is found,
   EOF is reached, or the buffer is full (431).
2. `http::parse` validates the request line (`GET`, `HEAD`, or `PUT`), the
   origin-form target, and `HTTP/1.0`/`HTTP/1.1`. It also extracts
   `Content-Length` and `Expect: 100-continue`. Chunked bodies are rejected;
   conflicting `Content-Length` values are 400.
3. `path::normalize` strips the query, percent-decodes once, rejects NUL,
   backslash and control bytes, drops empty/`.` segments, rejects escaping
   `..`, and preserves a trailing slash. Result: at most 1 KiB.
4. `root_relative` turns `/a/b` into `a/b` (root itself is `.`). No document
   root prefix is concatenated into the path; the platform backend resolves it
   against the root fd. `append_index` is applied only for directory URLs.
5. `Platform::open` opens the relative path read-only with no-symlink
   resolution. `file_info` gets size, mode, and mtime.
6. A regular file is sent with `sendfile`. A single `Range` header resolves to
   an offset and length before headers are written; `206` carries
   `Content-Range`, and an unsatisfiable range gets `416` with
   `Content-Range: bytes */total`. `HEAD` sends only the header. The loop stops
   after exactly `Content-Length` bytes, so a growing file cannot overrun the
   advertised length. Linux blocks `SIGPIPE` process-wide; macOS
   sets `SO_NOSIGPIPE` on every accepted socket, so a client disconnect yields
   `EPIPE` instead of killing the server.
7. A directory URL without a trailing slash gets a 301 to the slashed form. A
   directory with `/` gets `index.html` if it exists; otherwise the built-in
   index is rendered, unless `--no-index` is set (403).
8. A `PUT` body is streamed to a file or creates one directory. `Expect` is
   answered with an interim `100 Continue` before the socket is read.
9. The response header is built in a 4 KiB `FixedBuf`: status line, `Server`,
   `Content-Type`, `Content-Length`, optional `Allow`/`Location`, and
   `Connection: close`.
10. `Connection: close`: the stream is dropped, closing the fd. There is no
    keep-alive state yet. If a rejected request announced a small body, it is
    drained (up to 64 KiB) first: closing with unread input can turn the final
    FIN into an RST on macOS and discard the response the client is reading.

The HTTP, path, MIME, config, and listing modules are exercised by unit tests
with `std` enabled; the library itself is `no_std` whenever it is not compiled
as a test target.

## Classic multipart form compatibility

Raw `PUT` is the primary upload path, but old browsers and no-JavaScript forms
need `multipart/form-data`. `src/multipart.rs` implements a bounded streaming
parser:

- one 32 KiB buffer plus a boundary-sized tail; the body is never buffered
  whole;
- quote-aware parsing of `Content-Type: ...; boundary=...` and
  `Content-Disposition: ...; filename=...`;
- Windows `C:\fakepath\name` paths are reduced to a basename; NUL, controls,
  `/`, `\`, `.` and `..` are rejected; names are capped at 255 bytes;
- non-file fields are streamed and discarded; exactly one file part is
  accepted per request (the classic form fallback has one input);
- file bytes are handed to the `Sink` as they are confirmed, so there is no
  whole-body buffer and no fixed size cap;
- a failed or truncated part is unlinked by the sink;
- the directory page always includes a form with
  `method=post enctype=multipart/form-data`; modern JS detects the File API,
  hides the form, and uses `XMLHttpRequest`/`PUT` with progress. If the File
  API or modern APIs are absent, the form remains and still works. The inline
  JS is intentionally ES3-style and feature-detected: no `Promise`,
  `querySelectorAll`, `dataset`, `classList`, `fetch`, arrow functions, or
  `const`/`let`. Folder upload uses `webkitdirectory`/`webkitRelativePath`,
  creates each parent with `PUT .../` (409 is treated as existing), and then
  PUTs the files in order; unsupported browsers simply hide that control. The CSS has hard-coded light base rules and a single
  `prefers-color-scheme: dark` override block at the end; there are no custom
  properties, so old engines still get readable colors.

`POST` is only accepted when `--upload` is set; non-multipart posts get 415, a
missing file part 400, and conflicts follow the same `--replace` rules as PUT.

## Directory index

`listing::render` builds a complete HTML page in a 64 KiB `FixedBuf`:

- breadcrumb links are URL-encoded segment by segment and HTML-escaped;
- entries come from a 4 KiB `getdents` buffer; names are copied into a 256-byte
  caller buffer and never allocated;
- each entry is `statx`-ed on Linux / `fstatat`-ed on macOS for size and
  mtime; symlinks use `AT_SYMLINK_NOFOLLOW` and are rendered as unlinked
  `link` rows;
- names are escaped for HTML and percent-encoded for `href`, so `&`, `<`, `>`,
  `"`, `'`, `%`, spaces, and non-ASCII bytes cannot break the markup;
- sizes use SI units and timestamps use UTC `YYYY-MM-DD HH:MM`; inline JS
  upgrades the timestamps to the viewer's local time and sorts columns;
- the page's CSS and JS are `const` byte strings compiled into the binary —
  there are no external assets;
- if the buffer fills, the listing is truncated with a visible notice rather
  than growing or panicking.

The page is served with `Content-Length` computed from the buffer, so `HEAD`
works without special-casing.

## Uploads

`PUT` is deliberately a raw-body protocol; classic forms use the separate
bounded multipart `POST` path:

- `Content-Length` is mandatory for file uploads (411 otherwise); the body is
  streamed until exactly that many bytes have been read, with no fixed cap;
- bytes already buffered after the header block are written first, then the
  socket is streamed in 16 KiB chunks;
- `Expect: 100-continue` is answered only when the body is not already
  buffered;
- the body is written with `write` to a file opened `O_WRONLY|O_CREAT` plus
  `O_EXCL` (409 on conflict) or `O_TRUNC` with `--replace`;
- `PUT /dir/` strips the trailing slash and calls `mkdirat` once; parent
  directories are not created automatically;
- a failed upload (early EOF, write error) closes the file, unlinks it, and
  returns 400/500, so a partial file does not linger;
- uploads are disabled unless `--upload` is passed.

The browser UI uses `XMLHttpRequest` with `PUT` for progress; `curl -T FILE URL`
works unchanged. Raw `PUT` remains the primary upload path; the bounded
multipart parser is only the compatibility path for classic form `POST`s.

## No-allocation strategy

| Buffer | Size | Lifetime |
| --- | ---: | --- |
| request headers | 8 KiB | one connection |
| normalized URL path | 1 KiB | one request |
| relative filesystem path | 1.5 KiB | one request |
| response header | 4 KiB | one response |
| redirect `Location` | ~3 KiB | one redirect |
| directory entries | 4 KiB | one listing |
| directory page | 64 KiB | one listing |
| multipart parser | 32 KiB | one form upload |
| multipart headers | 4 KiB (inside parser buffer) | one part |
| upload/file chunk | 16 KiB | one PUT/upload read |
| log line | 1.2 KiB | one log event |
| config root | 256 B | process |

There is no `alloc` crate dependency and no `#[global_allocator]`. If a future
change introduces an allocation, the crate stops compiling for a no-std target,
which is the desired failure mode.

Rust does have `static`, `static mut`, function-local `static`, and
`thread_local!`; the service sources deliberately have none. `const` values and
`&'static` string literals are fine. `scripts/check-no-globals.sh` rejects
service-side globals so state stays in caller buffers and writable sections do
not appear.

On macOS, `fdopendir`/`readdir` may allocate inside libSystem; that allocator
is not linked into `syswebd`, and it is the only known hidden allocation on the
macOS path.

## Safety invariants

- `argv` is read only for `argc` entries, capped at 32, with each string capped
  at 4096 bytes. The kernel/dyld guarantees NUL termination and validity.
- Syscall buffers are local, initialized (or written by the kernel before being
  read), and bounds-checked before the call.
- `statx` uses a 256-byte buffer; the macOS SDK `struct stat` is 144 bytes.
  Both exceed their ABI struct sizes.
- Directory records are bounds-checked (`reclen`, `namlen`, name range) before
  any field is read; names are copied into a 256-byte caller buffer.
- File descriptors have exactly one owner. `Listener`, `Stream`, `File`, and
  `Dir` close in `Drop`; there is no manual close and no double-close.
- `write_all`/`write_file` advance by the kernel-returned count and reject a
  zero-length write as `BrokenPipe`, so they cannot spin.
- Path normalization is done before the filesystem is touched; `open` is never
  called with a path containing a `..` segment. All symlinks beneath the root
  are refused by `openat2`/`O_NOFOLLOW_ANY`; only the root's own path may
  contain symlinks.
- Uploads require a `Content-Length` and stop after exactly that many bytes;
  there is no per-request cap. A failed upload is unlinked.
- Directory HTML is escaped for text/attributes and percent-encoded for URLs;
  names cannot inject markup or `javascript:` links.
- Response headers contain static strings, decimal numbers, and a
  percent-encoded `Location` path. Raw request bytes are never reflected.
- `sendfile` runs with `SIGPIPE` blocked on Linux and `SO_NOSIGPIPE` on macOS,
  so an aborted download cannot terminate the server.
- Multipart filenames are reduced to a basename and reject separators,
  controls, NUL, `.` and `..`; the parser never allocates or buffers a whole
  body.

Known unsafe edges: the raw syscall/libSystem FFI declarations, the `argv`
dereference, and the `mem*` shims. All are isolated and
reviewable.

## Async design (milestone 1)

Backend status: Linux uses epoll, macOS uses kqueue, and both run the same
fixed-slot state machine. io_uring has a prototype but is not enabled; the
Windows IOCP spike proves `AcceptEx`/`WSARecv`/`WSASend`, but the full Win32
backend is not implemented.

The async path is now the default on Unix: epoll (Linux) or kqueue (macOS)
readiness, non-blocking accept/read/write/sendfile, and a fixed-slot
GET/HEAD/PUT/POST state machine. Metadata and listings remain synchronous.
A 1-second poll tick drives the per-slot idle deadline (about 30 seconds).
`--blocking` keeps the old path for comparison. This is not the final io_uring
backend, but it establishes the state-machine and liveness properties.

The blocking `Platform` trait is deliberately shaped so the async version is an
extension, not a rewrite. The async backend will not use Rust `async`/`await`:
a futures executor and its state machines tend to want allocation and add a lot
of machinery for a static file server. A hand-written state machine over
readiness events is smaller and fully no-alloc.

A no-alloc async/await spike exists at `spikes/async-no-alloc`; it records a
16-byte demo future and 1.2–1.3 KiB static Linux binaries. That answers the
size question, not the self-referential fixed-slot question, so the state
machine remains the production plan.

### Reactor trait sketch

```rust
pub enum Interest {
    Readable,
    Writable,
}

pub struct Event {
    pub token: u32,
    pub readable: bool,
    pub writable: bool,
}

pub trait Reactor {
    type Handle;

    fn register(
        &mut self,
        handle: &Self::Handle,
        interest: Interest,
        token: u32,
    ) -> Result<()>;

    fn deregister(&mut self, handle: &Self::Handle) -> Result<()>;

    fn wait(
        &mut self,
        events: &mut [Event],
        timeout_ms: Option<u32>,
    ) -> Result<usize>;
}
```

Backends:

| OS | Primitive | Syscalls / APIs |
| --- | --- | --- |
| Linux | `io_uring` | `io_uring_setup` 425, `io_uring_enter` 426, `io_uring_register` 427; `IORING_OP_ACCEPT/RECV/SEND/OPENAT2/STATX/SPLICE` |
| macOS | `kqueue` | `kqueue` 362, `kevent` 363 |
| Windows | IOCP | `CreateIoCompletionPort`, `GetQueuedCompletionStatusEx`, `AcceptEx`, `WSARecv`/`WSASend` |

Sockets become non-blocking (`accept4(SOCK_NONBLOCK)` on Linux, `fcntl` on
macOS). Regular file reads stay blocking; disk reads are not the bottleneck for
a static server on page cache. `io_uring` can replace that later.

### Connection state machine

```rust
enum State {
    ReadingHeaders,
    Opening,
    WritingHeaders,
    Streaming,
    ReadingBody,   // upload: body bytes from the socket
    WritingFile,   // upload: body bytes to disk
    Listing,       // bounded 64 KiB render + send
    Closing,
}

struct Connection {
    state: State,
    stream: Stream,
    file: Option<File>,
    request: [u8; REQUEST_BUF],
    request_len: usize,
    remaining: u64,
    // ...
}
```

A fixed array of `MAX_CONNECTIONS` slots (8 on Unix, 4 on Windows to fit the default stack) is allocated once
in the server's stack or a static. Each event carries a slot token. There is no
per-connection heap allocation and no unbounded queue. Idle/read/write timeouts
come from the reactor wait timeout plus a per-slot deadline.

The HTTP parser already returns a borrowed `Request<'a>`, uploads are a bounded
read/write loop, and the multipart parser is already incremental, so the async
state machine can keep using the same code; only the I/O driver changes.
Directory rendering stays a bounded in-memory step (64 KiB), so it does not
need incremental state.

## Windows plan (milestone 2)

A CRT-free Win32 **blocking** backend now exists for bring-up. It covers paths,
files, directories, uploads, and sockets and runs on Windows 11. The final
Windows backend is still IOCP; the blocking path is a validation step, not the
async endpoint.

A no-CRT entry/link spike lives in `spikes/windows-no-crt`: a 3,584-byte x86_64
PE with a custom `mainCRTStartup`, no CRT, and only `KERNEL32.dll` imports
(`ExitProcess`, `GetStdHandle`, `WriteFile`). It was run on Windows 11. The
spike also established that this loader needs a 6.0 subsystem version; `10.0`
failed with `STATUS_INVALID_IMAGE_FORMAT`. With 6.0, relocatable, `-static`, and
fixed/no-reloc images all run, and a remote MSVC `/FIXED /DYNAMICBASE:NO` build
was verified. `/FIXED` only fixes the base; a one-section image additionally
needs lld-link `/MERGE` (verified with a 1,024-byte C probe). The Rust spike
keeps relocations for robustness.

`spikes/windows-iocp` goes one step further: it creates an IOCP, associates
the listener and accept socket, posts an overlapped `AcceptEx`, waits for the
connection, posts an overlapped `WSARecv`, waits, posts an overlapped
`WSASend`, waits, and verifies the reply. Its stripped PE imports only
`KERNEL32.dll` and `WS2_32.dll`. Socket state and file I/O remain for the real
backend.

- `#![no_main]`, export `mainCRTStartup` as the entry point.
- Link with `/NODEFAULTLIB` and explicit `kernel32.lib` / `ws2_32.lib`; provide
  the same `mem*` symbols.
- Winsock initialization via `WSAStartup` (not needed on modern Windows for
  some APIs, but required for Winsock).
- IOCP directly with overlapped `AcceptEx`/`WSARecv`/`WSASend`. There is no
  `WSAPoll`/`select` stepping stone; IOCP is the only Windows reactor.
- File I/O via `CreateFile2`/`GetFileSizeEx`/`ReadFile`.

The `Platform` trait stays the same; `unix` becomes one implementation among
several.

## Porting checklist

1. Add `src/platform/<os>.rs` with the chosen ABI (raw syscalls or public
   library functions, structs, error mapping).
2. Add safe RAII handle types and `impl Platform`.
3. Add the binary entry point and `mem*` shims to `src/bin/syswebd.rs`.
4. Add link arguments to `build.rs`.
5. Add a CI job that proves the absence of the runtime you claim to avoid
   (`nm -u`, `readelf -d`, `otool -L`, `dumpbin /dependents`).
6. Add an end-to-end test that spawns the real binary.

## Testing strategy

- Unit tests for request parsing (including `Content-Length`/`Expect`), path
  normalization/joining, MIME, config, listing formatting, and `FixedBuf`.
- Five end-to-end tests spawn the real binary over loopback and cover GET,
  HEAD, binary bodies, `sendfile` for an 8 MiB file, `Range`/`206`/`416`,
  directory indexes,
  breadcrumbs/metadata, HTML escaping, symlink denial, 301 redirects,
  404/405/505/400/409/411/415, `--no-index`, raw uploads, folder creation,
  `--replace`, missing length, `Expect: 100-continue`, traversal attempts, and
  classic multipart form uploads (including a 100 KiB chunked body).
- CI runs `cargo test` and `cargo test --release` on Linux and macOS. The
  release run is mandatory because the `bzero`/LTO failure mode did not appear
  in debug builds.
- `fuzz/` has libFuzzer targets for `http::parse`, `path::normalize`, and the
  multipart state machine; `tests/security.rs` sends adversarial and
  randomized requests to the real binary and checks status bounds, timeouts,
  process survival, and outside-file leakage.
- Future: fuzz the listing escaper and the range resolver too; property-test the path invariant
  (`..` cannot survive normalization) more deeply; soak-test the async reactor
  and slow uploads.

## Known risks

- **macOS libSystem allocation**: `fdopendir`/`readdir` may allocate inside
  libSystem even though the binary has no allocator. Moving to
  `getattrlistbulk` would remove that allocation without returning to private
  syscall numbers.
- **Single-threaded blocking**: no timeouts yet; a slow client can pin the
  server. This is the main reason async is milestone 1.
- **Symlinks**: all links beneath the root are refused. The root's own path
  may contain symlinks because it is opened once at startup; hard links and
  bind mounts are outside the threat model.
- **Latest-API floor**: Linux 5.6+, macOS 11+, Windows 10 1809+. There are no
  fallbacks for older kernels/OS releases; this is deliberate.
- **Uploads are powerful**: `--upload` lets any client that can reach the port
  write files; `--replace` can overwrite them. There is no authentication,
  quota, or CSRF/Host validation yet. Keep the default loopback bind.
- **Partial files**: a failed upload is unlinked, but a successful upload is
  written in place, not via temp+rename. `--replace` can therefore truncate an
  existing file before a failure. Atomic uploads are roadmap.
- **Listing truncation**: directories whose rendered page exceeds 64 KiB are
  truncated visibly. A pagination or chunked-listing design is future work.
- **No `Date` header**: HTTP/1.1 says servers SHOULD send one. It is a roadmap
  item because it needs a clock syscall and civil-date conversion.
- **No keep-alive**: every response costs a TCP handshake. Acceptable for a
  small dev server, not for a CDN edge.
