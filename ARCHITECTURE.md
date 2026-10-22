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
5. Windows: no CRT (roadmap).
6. `unsafe` concentrated in small, reviewable ABI modules.
7. A blocking implementation that already has the shape of the async one.

Non-goals for now: TLS, dynamic content, CGI, compression, virtual hosts,
directory listings, and hardening for hostile public traffic.

## Layers

```
┌──────────────────────────────────────────────────────────────┐
│ src/bin/syswebd.rs                                           │
│   _start (Linux) / _main (macOS), panic handler, mem* shims  │
├──────────────────────────────────────────────────────────────┤
│ src/server.rs                                                │
│   accept loop, fixed buffers, response writer, logging       │
├──────────────────────────────────────────────────────────────┤
│ src/http.rs  src/path.rs  src/mime.rs  src/config.rs         │
│   pure, safe, allocation-free                                │
├──────────────────────────────────────────────────────────────┤
│ src/platform/mod.rs                                          │
│   trait Platform; Error; SocketAddr; FileInfo                │
├──────────────────────────────────────────────────────────────┤
│ src/platform/unix/mod.rs                                     │
│   RAII Listener/Stream/File; shared Unix operation logic     │
├──────────────────────────────────────────────────────────────┤
│ src/platform/unix/linux.rs  src/platform/unix/darwin.rs      │
│   raw syscall numbers, registers, struct layouts             │
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

    fn bind(&mut self, addr: SocketAddr, backlog: u16) -> Result<Self::Listener>;
    fn accept(&mut self, listener: &Self::Listener) -> Result<Self::Stream>;
    fn read(&mut self, stream: &mut Self::Stream, buf: &mut [u8]) -> Result<usize>;
    fn write_all(&mut self, stream: &mut Self::Stream, buf: &[u8]) -> Result<()>;

    fn open(&mut self, path: &[u8]) -> Result<Self::File>;
    fn file_info(&mut self, file: &Self::File) -> Result<FileInfo>;
    fn read_file(&mut self, file: &mut Self::File, buf: &mut [u8]) -> Result<usize>;

    fn log(&mut self, message: &[u8]);
    fn exit(&mut self, code: i32) -> !;
}
```

Design notes:

- **Generic, not `dyn`.** The server is monomorphized for `platform::Sys`.
  There is no vtable and no allocation.
- **RAII handles.** `Listener`, `Stream`, and `File` own an `i32` fd and close
  it in `Drop`. The server never calls `close` manually, so early returns and
  error paths cannot leak.
- **Caller-provided buffers.** No method allocates. The backend fills the
  caller's slice, and all sizes are bounded by the caller.
- **`&mut self`.** A future reactor keeps its epoll/kqueue/IOCP state inside
  the backend without changing the trait.
- **Normalized errors.** `Error` hides errno differences (`EAGAIN` is 11 on
  Linux and 35 on macOS, `ECONNRESET` is 104 vs 54, etc.).

## Linux backend

### Syscall table

| Operation | x86_64 | aarch64 |
| --- | ---: | ---: |
| `read` | 0 | 63 |
| `write` | 1 | 64 |
| `close` | 3 | 57 |
| `fstat` | 5 | 80 |
| `socket` | 41 | 198 |
| `accept` | 43 | 202 |
| `sendto` | 44 | 206 |
| `bind` | 49 | 200 |
| `listen` | 50 | 201 |
| `setsockopt` | 54 | 208 |
| `exit_group` | 231 | 94 |
| `openat` | 257 | 56 |

Conventions:

- x86_64: number in `rax`, arguments in `rdi, rsi, rdx, r10, r8, r9`,
  `syscall`, result in `rax`. `rcx` and `r11` are clobbered.
- aarch64: number in `x8`, arguments in `x0..x5`, `svc #0`.
- A result in `[-4095, -1]` is `-errno`; everything else is success.

The wrappers use `asm!` with `options(nostack)`, no `nomem`, and explicitly
declare the clobbered registers. They are `unsafe fn`, called only from the
`unix` layer which validates slice lengths and pointer provenance.

### `struct stat`

`fstat` fills a 144-byte buffer. Only two fields are read:

| ABI | `st_mode` | `st_size` |
| --- | ---: | ---: |
| x86_64 Linux | 24 (`u32`) | 48 (`i64`) |
| aarch64 Linux | 16 (`u32`) | 48 (`i64`) |
| macOS arm64 | 8 (`u16`) | 72 (`i64`) |

The buffer is `#[repr(C, align(8))]` to be safe for the kernel's copy-out.
`st_mode & 0170000 == 0040000` detects a directory.

### No-libc linking

`build.rs` adds these flags to the **binary only**:

```
-nostdlib -static -no-pie
```

`-nostdlib` suppresses startup files and default libraries, including libc and
libgcc. `compiler_builtins` still provides integer helpers. The binary provides:

- `_start` (assembly) which extracts `argc`/`argv` from the initial stack,
  aligns the stack, and calls `sysweb_entry`;
- `memcpy`, `memmove`, `memset`, `memcmp`, `bcmp`, `bzero`, `strlen`;
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

## macOS backend

### Syscall convention

- number in `x16`, arguments in `x0..x5`, `svc #0x80`;
- on error the kernel sets the carry flag and returns the errno in `x0`;
- the wrapper emits `cset` immediately after `svc` and maps errno.

Numbers used (Apple Silicon): `read` 3, `write` 4, `close` 6, `fstat` 189,
`socket` 97, `accept` 30, `sendto` 133, `bind` 104, `listen` 106,
`setsockopt` 105, `openat` 463, `exit` 1.

`AT_FDCWD` is `-2`, `sockaddr_in` begins with a length byte, `SOL_SOCKET` is
`0xffff`, `SO_REUSEADDR` is `4`, and `MSG_NOSIGNAL` is `0x80000`.

### Kernel `struct stat` vs. userspace `struct stat`

The raw `fstat` syscall fills the kernel's legacy layout, **not** the 144-byte
SDK `struct stat` that libSystem hands to C programs. The raw layout starts
with `st_dev` (u32), `st_ino` (u32), `st_mode` (u16 at offset 8), and puts
`st_size` at offset 72. The SDK layout has `st_mode` at 4 and `st_size` at 96.
`sysweb` reads the raw layout directly and documents it in `darwin.rs`. This is
one reason macOS support is explicit rather than pretending the ABI is stable.

### Why libSystem is still listed

ld64 refuses to link a dynamic executable without a `libSystem` load command:

```
ld: dynamic executables or dylibs must link with libSystem.dylib
```

`build.rs` therefore adds `-lSystem` for the macOS binary. It is a load command,
not a dependency in practice: the final binary has **no undefined symbols**, so
there is nothing to bind from libSystem. `otool -L` shows it; `nm -u` is empty.
Direct syscalls are used for every operation.

Apple's syscall numbers and layouts can change between OS releases. Linux is the
supported libc-free target; macOS is a best-effort experiment. Intel macOS is
not implemented yet.

## HTTP pipeline

1. `read_request` reads into an 8 KiB stack buffer until `\r\n\r\n` is found,
   EOF is reached, or the buffer is full (431).
2. `http::parse` validates the request line: `GET`/`HEAD`, origin-form target,
   `HTTP/1.0` or `HTTP/1.1`. Other methods return 405; other versions 505.
3. `path::normalize` strips the query, percent-decodes once, rejects NUL,
   backslash and control bytes, drops empty/`.` segments, rejects escaping
   `..`, and preserves a trailing slash. Result: at most 1 KiB.
4. `path::join` prefixes the document root and appends `index.html` when the
   URL path ends in `/`. It re-checks for `..` segments as defense in depth.
   Result: at most 1.5 KiB including NUL.
5. `Platform::open` opens the path read-only. `file_info` gets size and mode.
   Directories are 404 in this milestone.
6. The response header is built in a 512-byte `FixedBuf`: status line, `Server`,
   `Content-Type`, `Content-Length`, optional `Allow`, `Connection: close`.
7. The file is streamed in 16 KiB chunks. `HEAD` sends only the header. The
   loop stops after exactly `Content-Length` bytes, so a growing file cannot
   overrun the advertised length.
8. `Connection: close`: the stream is dropped, closing the fd. There is no
   keep-alive state yet.

The HTTP, path, MIME, and config modules are exercised by unit tests with `std`
enabled; the library itself is `no_std` whenever it is not compiled as a test
target.

## No-allocation strategy

| Buffer | Size | Lifetime |
| --- | ---: | --- |
| request headers | 8 KiB | one connection |
| normalized URL path | 1 KiB | one request |
| filesystem path | 1.5 KiB | one request |
| response header | 512 B | one response |
| file chunk | 16 KiB | one file response |
| log line | 320 B | one log event |
| config root | 256 B | process |

There is no `alloc` crate dependency and no `#[global_allocator]`. If a future
change introduces an allocation, the crate stops compiling for a no-std target,
which is the desired failure mode.

## Safety invariants

- `argv` is read only for `argc` entries, capped at 32, with each string capped
  at 4096 bytes. The kernel/dyld guarantees NUL termination and validity.
- Syscall buffers are local, initialized (or written by the kernel before being
  read), and bounds-checked before the call.
- `fstat` buffers are larger than every supported ABI's struct (144 bytes).
- File descriptors have exactly one owner. `Drop` closes; there is no manual
  close and no double-close.
- `write_all` advances by the kernel-returned count and rejects a zero-length
  write as `BrokenPipe`, so it cannot spin.
- Path normalization is done before the filesystem is touched; `open` is never
  called with a path containing a `..` segment.
- Response headers contain only static strings and decimal numbers. No
  user-controlled bytes are reflected into headers.

Known unsafe edges: the raw syscall ABI itself, the `argv` dereference, the
`mem*` shims, and Apple's undocumented syscall ABI. All are isolated and
reviewable.

## Async design (milestone 1)

The blocking `Platform` trait is deliberately shaped so the async version is an
extension, not a rewrite. The async backend will not use Rust `async`/`await`:
a futures executor and its state machines tend to want allocation and add a lot
of machinery for a static file server. A hand-written state machine over
readiness events is smaller and fully no-alloc.

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
| Linux | `epoll` | `epoll_create1` 291, `epoll_ctl` 233, `epoll_wait` 232 |
| macOS | `kqueue` | `kqueue` 362, `kevent` 363 |
| Windows | IOCP | `CreateIoCompletionPort`, `GetQueuedCompletionStatus`, `AcceptEx` |

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

A fixed array of `MAX_CONNECTIONS` slots (for example 1024) is allocated once
in the server's stack or a static. Each event carries a slot token. There is no
per-connection heap allocation and no unbounded queue. Idle/read/write timeouts
come from the reactor wait timeout plus a per-slot deadline.

The HTTP parser already returns a borrowed `Request<'a>`, so the async state
machine can keep using the same code; only the I/O driver changes.

## Windows plan (milestone 2)

- `#![no_main]`, export `mainCRTStartup` as the entry point.
- Link with `/NODEFAULTLIB` and explicit `kernel32.lib` / `ws2_32.lib`; provide
  the same `mem*` symbols.
- Winsock initialization via `WSAStartup` (not needed on modern Windows for
  some APIs, but required for Winsock).
- First cut: `WSAPoll` or `select` for readiness, reusing the `Reactor` shape.
- Then IOCP with overlapped `AcceptEx`/`WSARecv`/`WSASend`, which is the native
  async API and matches the no-thread-per-connection goal.
- File I/O via `CreateFileA`/`GetFileSizeEx`/`ReadFile`.

The `Platform` trait stays the same; `unix` becomes one implementation among
several.

## Porting checklist

1. Add `src/platform/<os>.rs` with the raw ABI (syscall/API numbers, structs,
   error mapping).
2. Add safe RAII handle types and `impl Platform`.
3. Add the binary entry point and `mem*` shims to `src/bin/syswebd.rs`.
4. Add link arguments to `build.rs`.
5. Add a CI job that proves the absence of the runtime you claim to avoid
   (`nm -u`, `readelf -d`, `otool -L`, `dumpbin /dependents`).
6. Add an end-to-end test that spawns the real binary.

## Testing strategy

- Unit tests for request parsing, path normalization, MIME, config, and
  `FixedBuf`.
- One end-to-end test spawns the release/debug binary, serves real files over
  loopback, and checks GET, HEAD, binary bodies, large files, 404, 405, 505,
  bad request lines, percent-decoding, and traversal attempts.
- CI runs `cargo test` and `cargo test --release` on Linux and macOS. The
  release run is mandatory because the `bzero`/LTO failure mode did not appear
  in debug builds.
- Future: fuzz the parser and path normalizer; property-test the path
  invariant (`..` cannot survive normalization); soak-test the async reactor.

## Known risks

- **macOS raw ABI**: syscall numbers and the kernel stat layout are not a
  supported public ABI. A future macOS release can break them.
- **Single-threaded blocking**: no timeouts yet; a slow client can pin the
  server. This is the main reason async is milestone 1.
- **Symlinks**: `open` follows symlinks. A link inside the root can expose a
  file outside it. `openat2(RESOLVE_BENEATH)` on Linux or `O_NOFOLLOW` walks
  are the planned fixes.
- **No `Date` header**: HTTP/1.1 says servers SHOULD send one. It is a roadmap
  item because it needs a clock syscall and civil-date conversion.
- **No keep-alive**: every response costs a TCP handshake. Acceptable for a
  small dev server, not for a CDN edge.
