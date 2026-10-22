# sysweb

**A `no_std`, no-alloc, syscall-first static web server written in Rust.**

`sysweb` is an experiment in building a small HTTP file server with as little
runtime underneath it as possible:

- `#![no_std]` — `core` only, no `std`;
- no heap — no `alloc`, no global allocator, all buffers are fixed-size;
- no third-party crates — `cargo tree` shows exactly one node;
- on Linux — **no libc at all**: a statically linked ELF, a hand-written
  `_start`, raw syscalls, no dynamic section;
- on macOS — **raw BSD syscalls** and **zero imported symbols**. ld64 requires
  a `libSystem` load command for any dynamic executable, so it is linked, but
  the program never calls into it;
- `unsafe` is confined to the syscall ABI, the entry points, and the few `mem*`
  shims.

> Status: **milestone 0** — blocking, single-threaded, HTTP/1.0 and 1.1,
> `GET`/`HEAD`, static files, `Connection: close`. The async reactor is the next
> milestone; see [ROADMAP.md](ROADMAP.md).

## Why

Direct syscalls and no allocation make the moving parts visible. There is no
runtime, no scheduler, no hidden allocations, and no C library between the
request and the kernel. That makes the project a useful testbed for:

- a portable `Platform` trait over Linux `epoll`, macOS `kqueue`, and Windows
  IOCP;
- fixed-buffer HTTP parsing and response generation;
- a genuinely libc-free Linux server binary;
- a CRT-free Windows server binary.

It is also just a small, fast static file server.

## Platform support

| Target | Status | Backend | libc |
| --- | --- | --- | --- |
| Linux x86_64 | ✅ cross-built locally; CI job | raw syscalls, static ELF | **none** |
| Linux aarch64 | ✅ cross-built + checked locally; CI check | raw syscalls, static ELF | **none** |
| macOS arm64 | ✅ tested locally; CI job | raw BSD syscalls | no libc *calls*; `libSystem` load command only |
| macOS x86_64 | ❌ not yet | — | — |
| Windows x86_64 | 🗺️ roadmap | Win32 / IOCP | CRT-free planned |

Linux is the primary target for the libc-free goal. macOS uses Apple's
undocumented raw syscall ABI, which is the same interface libSystem uses
internally; it works on current Apple Silicon macOS but Apple does not promise
stability across releases. The `libSystem` load command is mandatory for ld64
to produce a dynamic executable, but `nm -u` on the release binary is empty.

## Quick start

```sh
cargo build --release

mkdir -p public
printf '<h1>hello from sysweb</h1>\n' > public/index.html

./target/release/syswebd --root public --port 8080
```

In another terminal:

```sh
curl -i http://127.0.0.1:8080/
curl -I http://127.0.0.1:8080/index.html
curl -i http://127.0.0.1:8080/does-not-exist
```

The repository also ships the demo page in [`public/`](public/):

```sh
./target/release/syswebd --root public
```

### CLI

```
USAGE:
    syswebd [OPTIONS]

OPTIONS:
    -r, --root <DIR>       Directory to serve (default: .)
    -p, --port <PORT>      TCP port, 1-65535 (default: 8080)
    -H, --host <ADDR>      IPv4 address to bind (default: 127.0.0.1)
        --backlog <N>      Listen backlog (default: 128)
    -q, --quiet            Suppress startup and request logs
    -h, --help             Print help
    -V, --version          Print the version
```

The default bind address is loopback. Use `--host 0.0.0.0` only when you mean
it; this server has no TLS, authentication, or rate limiting.

## Features

- HTTP/1.0 and HTTP/1.1 request lines; `GET` and `HEAD`;
- static files, `index.html` for paths ending in `/`;
- MIME types for common web assets, with `application/octet-stream` fallback;
- `Content-Length`, `Content-Type`, `Server`, `Connection: close`;
- 400, 403, 404, 405, 414, 431, 500, 505 error responses;
- percent-decoding, query-string stripping, dot-segment normalization, and
  traversal rejection;
- files streamed in 16 KiB chunks, never loaded whole;
- one request per connection, then close;
- request logging to stderr, `--quiet` to suppress it;
- IPv4 only for now.

## Verification

On macOS, after `cargo build --release`:

```console
$ ls -l target/release/syswebd
-rwxr-xr-x  33392  target/release/syswebd

$ otool -L target/release/syswebd
target/release/syswebd:
	/usr/lib/libSystem.B.dylib (compatibility version 1.0.0, ...)

$ nm -u target/release/syswebd
$ echo $?
0
```

The Linux binary is a static ELF with no interpreter and no undefined symbols:

```console
$ file target/release/syswebd
target/release/syswebd: ELF 64-bit LSB executable, x86-64, version 1 (SYSV),
statically linked, stripped

$ readelf -d target/release/syswebd
# no dynamic section

$ nm -u target/release/syswebd
# empty
```

This repository was cross-linked for `x86_64-unknown-linux-gnu` with `ld.lld`
and verified with `file`, `readelf`, and `nm`. Linux runtime tests are wired into CI.

## How it works

```
bin/syswebd        entry (_start / _main), panic handler, mem* shims
      │
server             blocking accept loop, HTTP response writer
      │
http / path / mime / config / fixedbuf     pure, safe, no allocation
      │
Platform trait     OS seam; associated Listener/Stream/File types
      │
unix               RAII file descriptors, shared Unix logic
      │
linux.rs / darwin.rs                       raw syscall ABI
```

- The `Platform` trait is generic, not `dyn`; the server is monomorphized for
  the current OS. The types returned by a backend own their file descriptors
  and close them on `Drop`.
- The HTTP, path, MIME, and configuration modules are safe Rust. Only the
  syscall ABI modules and the binary entry points use `unsafe`.
- No request data is allocated. The request buffer (8 KiB), URL path (1 KiB),
  filesystem path (1.5 KiB), header buffer (512 B), and file chunk buffer
  (16 KiB) are fixed-size stack arrays.
- The parser only accepts origin-form targets. `..`, encoded slashes,
  backslashes, NULs, and control bytes are handled by
  [`src/path.rs`](src/path.rs); `join` re-checks for parent segments as defense
  in depth.

Read [ARCHITECTURE.md](ARCHITECTURE.md) for the syscall details, stat layouts,
linker setup, and the async design.

## Limits

This is milestone 0, not a hardened internet-facing server:

- single-threaded and blocking; one slow client stalls everyone;
- no read/write timeouts yet (slowloris is possible);
- `Connection: close` only; no keep-alive or pipelining;
- no `Date`, `Range`, `ETag`, or `Last-Modified` headers yet;
- symlinks inside the document root are followed, including ones that point
  outside the root;
- no TLS, authentication, compression, or directory listing;
- an `accept` error other than `EINTR` terminates the process;
- macOS support is Apple Silicon only.

See [SECURITY.md](SECURITY.md) before exposing it to a network.

## Roadmap

Milestone 1 is the async reactor abstraction:

- Linux `epoll`, macOS `kqueue`, Windows IOCP behind one `Reactor` trait;
- non-blocking sockets, a hand-written connection state machine, fixed
  per-connection buffers, no futures executor;
- timeouts, keep-alive, connection limits;
- Windows CRT-free entry and build recipe;
- `sendfile`/`splice`/`io_uring` zero-copy paths;
- range requests, conditional GET, directory redirects;
- fuzzing and a larger CI matrix.

The full plan is in [ROADMAP.md](ROADMAP.md).

## Name

The crate is `sysweb` and the daemon is `syswebd`. The name is intentionally
plain and easy to change before publishing.

## License

MIT. See [LICENSE](LICENSE).
