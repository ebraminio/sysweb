# sysweb

**A `no_std`, no-alloc, syscall-first file-transfer server in Rust.**

`sysweb` is a pocket static server and file-transfer appliance with a built-in
browser UI. It is a spiritual continuation of [pad.js](LINEAGE.md): one
self-contained artifact, zero dependencies, a useful directory index, and
browser/terminal uploads — reimplemented as a static Rust binary with no
runtime, no heap, and, on Linux, no libc at all.

- `#![no_std]` — `core` only, no `std`;
- no heap in our code — no `alloc`, no global allocator, all buffers are
  fixed-size; macOS directory iteration may allocate inside libSystem;
- no third-party crates — `cargo tree` shows exactly one node;
- no writable global state — service-side `static`/`static mut`/
  `thread_local!` items are rejected by `scripts/check-no-globals.sh`;
- on Linux — **no libc at all**: a static ELF, hand-written `_start`, raw
  syscalls, no dynamic section;
- on macOS — the **stable libSystem ABI**, not private syscall numbers or
  kernel `struct stat` layouts. The binary imports only an allowlisted set of
  libSystem functions; there is no CRT or `std`;
- `unsafe` is confined to the syscall/FFI ABI, the entry points, and the few
  `mem*` shims;
- **binary size is a first-class design goal** — release budgets are checked in
  CI, not just measured after the fact.

> Status: async default on Linux, macOS, and Windows. Static files, built-in
> directory indexes, opt-in raw `PUT` uploads, classic `multipart/form-data`
> form uploads for old/no-JavaScript browsers, folder creation, `sendfile` file
> serving, single-range HTTP 206/416 responses, HTTP/1.0/1.1 keep-alive and
> close. The Windows backend uses `WSAPoll`; IOCP and io_uring remain roadmap.

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
| macOS arm64 | ✅ tested locally; CI job | libSystem ABI | libSystem imports only; no CRT/std |
| Windows x86_64 | ✅ CRT-free Win32 async (`WSAPoll`); CI e2e + smoke + release | Win32 / shell32 / ws2_32 / iphlpapi | **none** |

Linux is the primary target for the libc-free goal. macOS uses the public
libSystem ABI: Apple does not promise stability for raw syscall numbers or
kernel `struct stat` layouts, and ld64 requires `libSystem` anyway. The release
binary imports only the functions in the allowlist checked by
`scripts/check-binary.sh`.

A CRT-free Win32 backend runs on Windows 11 with the same async state machine:
GET, ranges, PUT, multipart POST, and directory listings work. Socket readiness
currently uses `WSAPoll`; IOCP is the next Windows step. The backend imports
only `KERNEL32.dll`, `SHELL32.dll`, `WS2_32.dll`, and `IPHLPAPI.dll`.

Two supporting spikes are also validated:

- [`spikes/windows-no-crt`](spikes/windows-no-crt/) builds a 3,584-byte x86_64
  PE importing only `KERNEL32.dll`;
- [`spikes/windows-iocp`](spikes/windows-iocp/) uses a completion port with
  overlapped `AcceptEx`, `WSARecv`, and `WSASend` over a loopback connection;
  its stripped PE imports only `KERNEL32.dll` and `WS2_32.dll`.

### Latest-API policy

There are no compatibility fallbacks for older OS releases. When two interfaces
exist, the newest one wins:

- **Linux 5.6+**: `openat2` with `RESOLVE_NO_SYMLINKS` for every open/create,
  `statx` for metadata, `getdents64` for directory reads. The async milestone
  targets **io_uring**, not epoll.
- **macOS 11+**: libSystem `fstatat`, `O_NOFOLLOW_ANY`, and
  `fdopendir`/`readdir`; the async milestone targets `kqueue`.
- **Windows 10 1809+**: IOCP with `GetQueuedCompletionStatusEx` and
  `CreateFile2`; no `WSAPoll` fallback and no CRT.
- File serving uses `sendfile` (Linux 40/71, macOS 337); there is no
  read/write-loop fallback.

The document root is opened once and all paths are relative to that directory
fd. Symlink denial therefore applies *beneath* the root, while the root path
itself may live under a symlinked system directory such as macOS `/var`.

## Install and run

### From source

Cargo has no `cargo run --git`; `cargo install --git` is the closest single
command. This builds and installs the binary, then runs it:

```sh
cargo install --git https://github.com/ebraminio/sysweb --bin syswebd --locked && syswebd --root ./public
```

The download default is all interfaces, port 8080. `cargo install` builds the
source locally; it does not trust a prebuilt binary.

### Prebuilt binary, one line

Linux x86_64:

```sh
curl -fsSL -o /tmp/sysweb https://github.com/ebraminio/sysweb/releases/latest/download/sysweb-linux-x86_64 && chmod +x /tmp/sysweb && /tmp/sysweb --root ./public
```

Linux aarch64:

```sh
curl -fsSL -o /tmp/sysweb https://github.com/ebraminio/sysweb/releases/latest/download/sysweb-linux-aarch64 && chmod +x /tmp/sysweb && /tmp/sysweb --root ./public
```

macOS arm64:

```sh
curl -fsSL -o /tmp/sysweb https://github.com/ebraminio/sysweb/releases/latest/download/sysweb-macos-arm64 && chmod +x /tmp/sysweb && /tmp/sysweb --root ./public
```

Windows PowerShell:

```powershell
Invoke-WebRequest https://github.com/ebraminio/sysweb/releases/latest/download/sysweb-windows-x86_64.exe -OutFile sysweb.exe; .\sysweb.exe --root .
```

Or let the wrapper choose the platform:

```sh
curl -fsSL https://raw.githubusercontent.com/ebraminio/sysweb/main/scripts/run-latest.sh | sh -s -- --root ./public
```

The `/releases/latest/download/<name>` URLs are stable because the release
assets no longer contain the commit hash. These commands run a binary fetched
from GitHub over TLS; if you do not trust that, use the `cargo install --git`
command above.

## Quick start

```sh
cargo build --release

# Serve the repository demo (read-only; default all interfaces):
./target/release/syswebd --root public --port 8080

# Startup lists reachable interface URLs, for example:
#   syswebd 0.1.0 (async) listening on port 8080 of all interfaces
#     lo0: http://127.0.0.1:8080
#     en0: http://192.168.1.130:8080
#     root: public, uploads: off

# Browser:
#   http://127.0.0.1:8080/
#   http://127.0.0.1:8080/files/   <- built-in directory index
```

Enable uploads and folder creation explicitly:

```sh
./target/release/syswebd --root public --port 8080 --upload
```

Then:

- open `http://127.0.0.1:8080/files/` and drag files or folders onto the drop zone;
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

For older Windows/PowerShell environments, `System.Net.WebClient` also works.
`UploadFile` sends multipart `POST`, so `--upload` must be enabled:

```powershell
$f = "C:\filename.bin"
$w = New-Object System.Net.WebClient
$w.UploadFile("http://127.0.0.1:8080/files/filename.bin", $f)
```

The `X-Filename` header sometimes used with this pattern is not required:
sysweb takes the destination from the URL path, or the multipart filename
when the URL ends in `/`.

The built-in index always contains a plain multipart file form plus a plain
text form (`method=get action="?sysweb-text=1"` with `name` and `text`
fields). Modern JavaScript hides both after detecting the File API and shows
the dialog/tools; a browser with JavaScript disabled, or an old browser without
those APIs, keeps the classic forms. Classic file posts take the filename from
the part; a post to `/files/fixed.txt` uses the URL path instead. The text form
needs no XHR or PUT and writes the text as a file. After either classic form
succeeds the server returns `303 See Other` to the current directory, so the
browser reloads the listing/table. The text form is limited by the browser's
GET URL length; the modern dialog has no such limit.

API uploads and folder creation answer `201 Created`; a classic form receives
the redirect above. The server answers `409 Conflict`
when a file exists without `--replace`, `411 Length Required` when a file
upload has no `Content-Length`, and `415 Unsupported Media Type` for a
non-multipart form post. There is no per-request upload size cap.

## CLI

```
USAGE:
    syswebd [OPTIONS]

OPTIONS:
    -r, --root <DIR>       Directory to serve (default: .)
    -p, --port <PORT>      TCP port, 1-65535 (default: 8080; if omitted,
                           a busy default falls back to the next free port)
    -H, --host <ADDR>      IPv4 address to bind (default: 0.0.0.0)
        --backlog <N>      Listen backlog (default: 128)
        --upload           Enable PUT uploads and folder creation
        --replace          Allow uploads to replace existing files
        --no-index         Disable generated directory indexes
        --async            Use the async reactor path (default)
        --blocking         Use the old blocking server path
        --idle-timeout <S> Async idle timeout in seconds (default: 30)
    -q, --quiet            Suppress startup and request logs
    -h, --help             Print help
    -V, --version          Print the version
```

The default bind is all IPv4 interfaces (`0.0.0.0`). Pass `--host 127.0.0.1`
for a localhost-only server. At startup the server lists each interface URL,
e.g. `lo0: http://127.0.0.1:8080` and `en0: http://192.168.1.130:8080`.
If the default port 8080 is occupied and
no `--port` was given, the server tries the next ports. Use `--host 0.0.0.0`
only when you mean it; this server has no TLS, authentication, or rate limiting.

## Features

- HTTP/1.0 and HTTP/1.1 request lines; `GET`, `HEAD`, and (opt-in) `PUT`;
- static files, `Content-Length`, `Content-Type`, HTTP/1.1 keep-alive
  (including sequential pipelining), and `Connection: close`;
- `index.html` is served when present; otherwise a built-in directory index;
- directory index with names, sizes, modification times, types, breadcrumbs,
  sortable columns, and a parent link;
- a self-contained browser UI with file/folder upload (`webkitdirectory`,
  preserving relative paths), recursive drag-and-drop of files or folders,
  clipboard files (including images) and text paste, a paste-text dialog
  with an editable filename, progress, and folder creation, inlined into a
  fixed 64 KiB buffer;
- raw-body `PUT` uploads with `Content-Length`, `Expect: 100-continue`, no
  per-request size cap, exact streaming (no heap), and cleanup of failed
  partial files;
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
-rwxr-xr-x  100608  target/release/syswebd

$ otool -L target/release/syswebd
target/release/syswebd:
	/usr/lib/libSystem.B.dylib (compatibility version 1.0.0, ...)

$ nm -u target/release/syswebd
___error
__exit
_accept
_bind
_close
... (allowlisted libSystem imports)
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

| Target | Size | Budget |
| --- | ---: | ---: |
| macOS arm64 | 117,680 B | 140 KiB |
| Linux x86_64 | 96,080 B | 108 KiB |
| Linux aarch64 | 93,288 B | 104 KiB |
| Windows x86_64 | 91,136 B | 116 KiB |

CI runs on `ubuntu-latest` and `macos-latest` (Apple Silicon). The release
workflow builds and tests natively on `ubuntu-latest`, `ubuntu-24.04-arm`, and
`macos-latest`. The aarch64 build was also exercised on a Raspberry Pi 5
(kernel 6.12): directory listings, `openat2`/`statx`, `sendfile`, raw `PUT`,
classic multipart forms, symlink denial, and aborted transfers.

The same checks are available locally and are used by CI:

```sh
./scripts/check-binary.sh target/release/syswebd
./scripts/check-size.sh target/release/syswebd
./scripts/check-no-deps.sh
./scripts/check-no-globals.sh
```

`check-binary.sh` verifies the static/EXEC/no-interpreter/no-`NEEDED` shape on
Linux and the single-`libSystem`/allowlisted-imports shape on macOS.
`check-size.sh` enforces the per-platform budget above in CI and in the release
workflow. Binary size is treated as a design constraint: new features must fit
the budget or the budget must be changed deliberately.

Fuzzing and adversarial checks:

- `fuzz/` contains `cargo-fuzz` targets for the HTTP parser, path normalizer,
  and multipart state machine.
- `tests/security.rs` fuzzes the **real server** with traversal, symlink,
  malformed-path, and hundreds of randomized requests, asserting bounded
  statuses, timeouts, crashes, and no outside-file leakage.
- CI runs a nightly `cargo fuzz` smoke job and the black-box security test.

## Per-commit releases

Every push to `main` (and manual dispatches) creates a GitHub Release for that
exact commit:

- tag: `build-<short-sha>`
- title: `Build <short-sha>`
- assets: raw binaries with stable names, e.g. `sysweb-linux-x86_64`
- Windows asset: `sysweb-windows-x86_64.exe`

Supported release targets:

| Platform | Runner | Binary |
| --- | --- | --- |
| `linux-x86_64` | `ubuntu-latest` | static ELF |
| `linux-aarch64` | `ubuntu-24.04-arm` | static ELF |
| `macos-arm64` | `macos-latest` | Mach-O, allowlisted libSystem ABI imports |
| `windows-x86_64` | `windows-latest` | CRT-free PE, MSVC |

Downloads are raw binaries, not archives.
Each per-commit release is marked latest, so `/releases/latest` points at the
newest build.

The workflow is [.github/workflows/release.yml](.github/workflows/release.yml).
To run a downloaded Unix binary:

```sh
curl -fsSL -o sysweb https://github.com/ebraminio/sysweb/releases/latest/download/sysweb-linux-x86_64
chmod +x sysweb
./sysweb --help
```

On Windows, download `sysweb-windows-x86_64.exe` and run it directly.

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
linux.rs                           raw syscall ABI
darwin.rs                          libSystem ABI
```

- The `Platform` trait is generic, not `dyn`; the server is monomorphized for
  the current OS. All OS handles own their descriptor and close on `Drop`.
- The HTTP, path, MIME, config, and listing modules are safe Rust. Only the
  syscall/FFI ABI modules and the binary entry points use `unsafe`.
- No request data is allocated. Directory pages are built with a
  `FixedBuf<65536>` and truncated cleanly if a directory is huge.
- Names are HTML-escaped and URL-encoded; user-controlled bytes never enter
  response headers unencoded. Symlink entries are not rendered as links.

Read [ARCHITECTURE.md](ARCHITECTURE.md) for ABI/import details, linker setup,
and the async design.

## Limits

This is a milestone, not a hardened internet-facing server:

- single-threaded and blocking; one slow client stalls everyone. That is
  acceptable for this milestone: a stalled service is preferable to exiting.
  Transient `accept` failures (for example `EMFILE` or `ECONNABORTED`) retry
  after a 100 ms backoff; fatal listener errors still terminate the process;
- no read/write timeouts yet (slowloris is possible, including slow uploads);
- stress/load testing is deliberately scheduled after the async reactor. The
  current suites target crashes, resets, containment, and liveness, not
  throughput or latency under concurrency;
- async keep-alive handles sequential requests and pipelined bytes; the
  `--blocking` fallback still serves one request per connection;
- the async reactor is the default on Unix; `--blocking` keeps the old path.
  GET/HEAD/PUT/POST use non-blocking sockets, while metadata and listing work
  remain synchronous. Idle connections are closed after roughly 30 seconds;
- uploads are off by default and have no auth, quota, or per-request size cap;
  disk exhaustion is the operator's responsibility;
- classic form posts upload one file per request; `filename*` (RFC 5987) and
  multi-file form fields are not parsed yet;
- old browsers get a hard-coded light CSS base; modern
  `prefers-color-scheme: dark` overrides the colors at the end of the sheet.
  There are no CSS custom properties. Layout is basic in engines without
  flexbox. JS uploads still need `File`/XHR, otherwise the classic form is the
  fallback;
- the Windows backend uses `WSAPoll`, not IOCP yet. Reparse points are rejected
  on open, but broader Windows path-hardening review is still ongoing;
- folder upload preserves relative paths but relies on the browser's
  `webkitdirectory`/`webkitRelativePath` or `webkitGetAsEntry`; empty
  directories are not reported by the browser. Clipboard folder paste depends
  on whether the browser exposes directory entries; regular file and text
  paste is supported. Text paste opens a filename/text dialog and also works
  from the **Paste text** button on phones. Old browsers without File/XHR use
  the GET text form instead, which is limited by the browser's URL length;
- no delete, rename, multi-range responses, `If-Range`, `ETag`,
  `Last-Modified`, or `Date` yet;
- no CORS, cross-origin isolation, prefix, or inactivity timeout yet;
- all symlinks beneath the root are denied; the root path itself may be a
  symlink (it is opened once at startup);
- no TLS, authentication, compression, or directory deletion;
- an `accept` error other than `EINTR` terminates the process;

See [SECURITY.md](SECURITY.md) before enabling writes or exposing the server.

## Roadmap

The next milestone is the async reactor abstraction:

- Linux **io_uring**, macOS `kqueue`, Windows IOCP behind one `Reactor` trait;
  no epoll/WSAPoll fallback;
- non-blocking sockets, a hand-written connection state machine, fixed
  per-connection buffers, no futures executor;
- uploads integrated into the state machine; directory listing stays bounded;
- connection limits and per-connection request caps;
- Windows IOCP backend; the no-CRT entry/link spike is already done;
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
