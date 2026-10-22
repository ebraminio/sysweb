# sysweb

**A `no_std`, no-alloc, syscall-first file-transfer server in Rust.**

`sysweb` is a pocket static server and file-transfer appliance with a built-in
browser UI. It is a spiritual continuation of [pad.js](LINEAGE.md): one
self-contained artifact, zero dependencies, a useful directory index, and
browser/terminal uploads — reimplemented as a static Rust binary with no
runtime, no heap, and, on Linux, no libc at all.

- `#![no_std]` — `core` only, no `std`;
- no heap — no `alloc`, no global allocator, all buffers are fixed-size;
- no third-party crates — `cargo tree` shows exactly one node;
- on Linux — **no libc at all**: a static ELF, hand-written `_start`, raw
  syscalls, no dynamic section;
- on macOS — **raw BSD syscalls** and **zero imported symbols**. ld64 requires
  a `libSystem` load command for any dynamic executable, so it is linked, but
  the program never calls into it;
- `unsafe` is confined to the syscall ABI, the entry points, and the few `mem*`
  shims.

> Status: **milestone 0.5** — blocking, single-threaded. Static files,
> built-in directory indexes, opt-in raw `PUT` uploads, classic
> `multipart/form-data` form uploads for old/no-JavaScript browsers, folder
> creation, `sendfile` file serving, single-range HTTP 206/416 responses,
> HTTP/1.0 and 1.1, `Connection: close`.
> The async reactor is the next milestone; see [ROADMAP.md](ROADMAP.md).

## Why

Direct syscalls and no allocation make the moving parts visible. There is no
runtime, no scheduler, no hidden allocations, and no C library between the
request and the kernel. That makes the project a useful testbed for:

- a portable `Platform` trait over Linux `io_uring`, macOS `kqueue`, and
  Windows IOCP;
- fixed-buffer HTTP parsing, directory rendering, and response generation;
- a genuinely libc-free Linux server binary;
- a CRT-free Windows server binary.

It is also a small, fast way to move files between a browser, a terminal, and a
machine that has no Node or Python.

## The pad.js lineage

pad.js was one `.html` file that was simultaneously a Node server and its own
landing page. `sysweb` cannot be an HTML file and an ELF at the same time, so
it inherits the *property* instead of the trick: the UI is compiled into the
binary, needs no assets, and the binary is the distribution.

[LINEAGE.md](LINEAGE.md) explains the original `<!--` / `/* -->` trick, the
feature map, and what is deliberately not carried forward (notably `--cmd`).

## Platform support

| Target | Status | Backend | libc |
| --- | --- | --- | --- |
| Linux x86_64 | ✅ cross-built locally; CI job | raw syscalls, static ELF | **none** |
| Linux aarch64 | ✅ cross-built + checked locally; CI check | raw syscalls, static ELF | **none** |
| macOS arm64 | ✅ tested locally; CI job | raw BSD syscalls | no libc *calls*; `libSystem` load command only |
| macOS x86_64 | ❌ not yet | — | — |
| Windows x86_64 | 🗺️ roadmap | Win32 / IOCP | CRT-free planned |

Linux is the primary target for the libc-free goal. macOS uses Apple's
undocumented raw syscall ABI; Apple does not promise stability across releases.
The `libSystem` load command is mandatory for ld64, but `nm -u` on the release
binary is empty.

### Latest-API policy

There are no compatibility fallbacks for older OS releases. When two interfaces
exist, the newest one wins:

- **Linux 5.6+**: `openat2` with `RESOLVE_NO_SYMLINKS` for every open/create,
  `statx` for metadata, `getdents64` for directory reads. The async milestone
  targets **io_uring**, not epoll.
- **macOS 11+**: `getdirentries64`, `fstatat`, and `O_NOFOLLOW_ANY`; the async
  milestone targets `kqueue`.
- **Windows 10 1809+**: IOCP with `GetQueuedCompletionStatusEx` and
  `CreateFile2`; no `WSAPoll` fallback and no CRT.
- File serving uses `sendfile` (Linux 40/71, macOS 337); there is no
  read/write-loop fallback.

The document root is opened once and all paths are relative to that directory
fd. Symlink denial therefore applies *beneath* the root, while the root path
itself may live under a symlinked system directory such as macOS `/var`.

## Quick start

```sh
cargo build --release

# Serve the repository demo (read-only, loopback):
./target/release/syswebd --root public --port 8080

# Browser:
#   http://127.0.0.1:8080/
#   http://127.0.0.1:8080/files/   <- built-in directory index
```

Enable uploads and folder creation explicitly:

```sh
./target/release/syswebd --root public --port 8080 --upload
```

Then:

- open `http://127.0.0.1:8080/files/` and drag files onto the drop zone;
- or use the terminal:

```sh
# upload (raw request body; no multipart needed)
curl -T ./report.pdf http://127.0.0.1:8080/files/report.pdf

# classic form upload (works from old browsers / curl -F)
curl -F "file=@./report.pdf" http://127.0.0.1:8080/files/

# single byte range (206 Partial Content)
curl -r 0-1023 http://127.0.0.1:8080/files/report.pdf -o report.head

# create a directory (trailing slash matters)
curl -X PUT http://127.0.0.1:8080/files/notes/

# overwrite an existing file
./target/release/syswebd --root public --upload --replace
curl -T ./new.txt http://127.0.0.1:8080/files/report.txt
```

The built-in index always contains a plain HTML form
(`method=post enctype=multipart/form-data`). Modern JavaScript hides it after
detecting the File API and shows the drag/drop tools; a browser with
JavaScript disabled, or an old browser without those APIs, keeps the classic
form and can still upload one file. Classic form posts take the filename from
the part; a post to `/files/fixed.txt` uses the URL path instead.

The server answers `201 Created` on uploads and folder creation, `409 Conflict`
when a file exists without `--replace`, `411 Length Required` when a file
upload has no `Content-Length`, `415 Unsupported Media Type` for a non-multipart
form post, and `413 Payload Too Large` above the 1 GiB limit.

## CLI

```
USAGE:
    syswebd [OPTIONS]

OPTIONS:
    -r, --root <DIR>       Directory to serve (default: .)
    -p, --port <PORT>      TCP port, 1-65535 (default: 8080; if omitted,
                           a busy default falls back to the next free port)
    -H, --host <ADDR>      IPv4 address to bind (default: 127.0.0.1)
        --backlog <N>      Listen backlog (default: 128)
        --upload           Enable PUT uploads and folder creation
        --replace          Allow uploads to replace existing files
        --no-index         Disable generated directory indexes
    -q, --quiet            Suppress startup and request logs
    -h, --help             Print help
    -V, --version          Print the version
```

The default bind address is loopback. If the default port 8080 is occupied and
no `--port` was given, the server tries the next ports. Use `--host 0.0.0.0`
only when you mean it; this server has no TLS, authentication, or rate limiting.

## Features

- HTTP/1.0 and HTTP/1.1 request lines; `GET`, `HEAD`, and (opt-in) `PUT`;
- static files, `Content-Length`, `Content-Type`, `Connection: close`;
- `index.html` is served when present; otherwise a built-in directory index;
- directory index with names, sizes, modification times, types, breadcrumbs,
  sortable columns, and a parent link;
- a self-contained browser UI with drag-and-drop upload, progress, and folder
  creation, inlined into a fixed 64 KiB buffer;
- raw-body `PUT` uploads with `Content-Length`, `Expect: 100-continue`, a 1 GiB
  cap, exact streaming (no heap), and cleanup of failed partial files;
- classic `multipart/form-data` `POST` uploads for old browsers, parsed by a
  streaming state machine with a 32 KiB buffer and no whole-body buffering;
  the inline JS stays in an ES3-style, feature-detected subset (no `Promise`,
  `querySelectorAll`, `dataset`, `classList`, `fetch`, arrow functions, or
  `const`/`let`), and the classic form remains visible if it cannot initialize;
- file responses use `sendfile`, so the file is never copied through a
  16 KiB userspace loop;
- single `Range: bytes=start-end`, `start-`, and `-suffix` requests with
  `206 Partial Content`, `Content-Range`, `Accept-Ranges: bytes`, and `416`
  for unsatisfiable ranges; multi-ranges are ignored (full 200);
- `PUT /dir/` creates one directory; `--replace` opts into overwriting;
- `--no-index` disables generated listings (403 for directories without
  `index.html`);
- `301` redirect from `/dir` to `/dir/`;
- percent-decoding, query-string stripping, dot-segment normalization, and
  traversal rejection;
- all symlinks beneath the document root are denied (`openat2`
  `RESOLVE_NO_SYMLINKS` on Linux, `O_NOFOLLOW_ANY` on macOS) and shown as
  unlinked “link” entries in the index;
- fixed buffers throughout: request 8 KiB, URL 1 KiB, path 1.5 KiB, header
  4 KiB, directory record 4 KiB, listing 64 KiB, file chunk 16 KiB;
- request logging to stderr, `--quiet` to suppress it;
- if `--port` is omitted and 8080 is busy, the server tries 8081, 8082, and so
  on (pad.js behavior). An explicit `--port` fails instead of moving;
- IPv4 only for now.

## Verification

On macOS, after `cargo build --release`:

```console
$ ls -l target/release/syswebd
-rwxr-xr-x  66480  target/release/syswebd

$ otool -L target/release/syswebd
target/release/syswebd:
	/usr/lib/libSystem.B.dylib (compatibility version 1.0.0, ...)

$ nm -u target/release/syswebd
# empty
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

Current stripped sizes from this workspace:

| Target | Size |
| --- | ---: |
| macOS arm64 | 83,024 B |
| Linux x86_64 | 56,232 B |
| Linux aarch64 | 54,664 B |

CI runs on `ubuntu-latest` and `macos-latest` (Apple Silicon). Linux runtime
tests run there; both Linux targets
were cross-built locally, and the aarch64 build was also exercised on a
Raspberry Pi 5 (kernel 6.12): directory listings, `openat2`/`statx`, `sendfile`, raw `PUT`,
classic multipart forms, symlink denial, and aborted transfers.

The same checks are available locally and are used by CI:

```sh
./scripts/check-binary.sh target/release/syswebd
./scripts/check-no-deps.sh
```

`check-binary.sh` verifies the static/EXEC/no-interpreter/no-`NEEDED` shape on
Linux and the single-`libSystem`/no-undefined-symbols shape on macOS.

Fuzzing and adversarial checks:

- `fuzz/` contains `cargo-fuzz` targets for the HTTP parser, path normalizer,
  and multipart state machine.
- `tests/security.rs` fuzzes the **real server** with traversal, symlink,
  malformed-path, and hundreds of randomized requests, asserting bounded
  statuses, timeouts, crashes, and no outside-file leakage.
- CI runs a nightly `cargo fuzz` smoke job and the black-box security test.

## How it works

```
bin/syswebd        entry (_start / _main), panic handler, mem* shims
      │
server             accept loop, HTTP bytes, upload/listing orchestration
      │
listing            self-contained directory page (64 KiB fixed buffer)
      │
http / path / mime / config / fixedbuf     pure, safe, no allocation
      │
Platform trait     OS seam; Listener/Stream/File/Dir associated types
      │
unix               RAII descriptors, directory iteration, shared logic
      │
linux.rs / darwin.rs                       raw syscall ABI
```

- The `Platform` trait is generic, not `dyn`; the server is monomorphized for
  the current OS. All OS handles own their descriptor and close on `Drop`.
- The HTTP, path, MIME, config, and listing modules are safe Rust. Only the
  syscall ABI modules and the binary entry points use `unsafe`.
- No request data is allocated. Directory pages are built with a
  `FixedBuf<65536>` and truncated cleanly if a directory is huge.
- Names are HTML-escaped and URL-encoded; user-controlled bytes never enter
  response headers unencoded. Symlink entries are not rendered as links.

Read [ARCHITECTURE.md](ARCHITECTURE.md) for syscall tables, stat layouts,
linker setup, and the async design.

## Limits

This is a milestone, not a hardened internet-facing server:

- single-threaded and blocking; one slow client stalls everyone;
- no read/write timeouts yet (slowloris is possible, including slow uploads);
- `Connection: close` only; no keep-alive or pipelining;
- uploads are off by default, capped at 1 GiB, and have no auth or quota;
- classic form posts upload one file per request; `filename*` (RFC 5987) and
  multi-file form fields are not parsed yet;
- the old-browser page is functional but the CSS assumes a reasonably modern
  layout engine; JS uploads need `File`/XHR, otherwise the classic form is the
  fallback;
- no delete, rename, multi-range responses, `If-Range`, `ETag`,
  `Last-Modified`, or `Date` yet;
- no CORS, cross-origin isolation, prefix, or inactivity timeout yet;
- all symlinks beneath the root are denied; the root path itself may be a
  symlink (it is opened once at startup);
- no TLS, authentication, compression, or directory deletion;
- an `accept` error other than `EINTR` terminates the process;
- macOS support is Apple Silicon only.

See [SECURITY.md](SECURITY.md) before enabling writes or exposing the server.

## Roadmap

The next milestone is the async reactor abstraction:

- Linux **io_uring**, macOS `kqueue`, Windows IOCP behind one `Reactor` trait;
  no epoll/WSAPoll fallback;
- non-blocking sockets, a hand-written connection state machine, fixed
  per-connection buffers, no futures executor;
- uploads integrated into the state machine; directory listing stays bounded;
- timeouts, keep-alive, connection limits;
- Windows CRT-free entry and build recipe;
- conditional requests, multi-range, delete/rename, auth, CORS, prefix;
- `splice`/io_uring zero-copy paths beyond `sendfile`;
- multi-file multipart forms and `filename*` support;
- broader OSS-Fuzz onboarding and a larger CI matrix.

The full plan is in [ROADMAP.md](ROADMAP.md).

## Name

The crate is `sysweb` and the daemon is `syswebd`. The name is intentionally
plain and easy to change before publishing.

## License

MIT. See [LICENSE](LICENSE).
