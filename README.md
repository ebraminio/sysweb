# sysweb

**A `no_std`, no-heap, syscall-first file-transfer server in Rust.**

`sysweb` is a small static file server, browser upload tool, and optional
polling chat in one binary. It has no runtime, no heap allocator, no third-party
crates, no libc on Linux, and no C runtime (CRT) on Windows. The directory
listing and upload UI are compiled into the binary, so there are no assets to
install.

- `#![no_std]`: `core` only, no `std`;
- no allocator: fixed buffers for requests, paths, headers, and listings;
- no third-party crates;
- direct OS APIs: raw Linux syscalls, the macOS libSystem ABI, and CRT-free
  Win32;
- Linux builds as a static ELF with no interpreter or dynamic section;
- single-threaded async I/O with fixed connection slots.

## Features

- HTTP/1.0 and HTTP/1.1, keep-alive, sequential pipelining, and ranges;
- static files, `index.html`, and a built-in sortable directory index;
- directory listings are streamed: chunked for HTTP/1.1, close-delimited for
  HTTP/1.0;
- opt-in raw `PUT` uploads, multipart `POST` uploads, and folder creation;
- upload from the browser: files, folders, clipboard images/text, drag-and-drop,
  and a paste-text dialog;
- classic multipart and GET text forms for old browsers with no File API;
- `sendfile` file serving and streaming uploads;
- optional HTTP Basic auth with `--userpass`;
- `--chat` built-in local chat: no WebSocket, no history, XHR polling;
- startup listing of all reachable interface URLs;
- IPv4 only.

## Platform support

| Target | Backend |
| --- | --- |
| Linux x86_64 | raw syscalls, static ELF |
| Linux aarch64 | raw syscalls, static ELF |
| macOS arm64 | public libSystem ABI |
| Windows x86_64 | CRT-free Win32, `WSAPoll` |

Linux is the primary target. macOS uses only an allowlisted set of libSystem
imports. Windows imports only `KERNEL32`, `SHELL32`, `WS2_32`, and `IPHLPAPI`.
See [ARCHITECTURE.md](ARCHITECTURE.md) for ABI and linker details.

## Install and run

### From source

Cargo has no `cargo run --git`; this builds the source and then runs it:

```sh
cargo install --git https://github.com/ebraminio/sysweb --bin syswebd --locked \
  && syswebd --root ./public
```

The default bind is all interfaces on port 8080.

### Prebuilt binary

Linux x86_64:

```sh
curl -fsSL -o /tmp/sysweb \
  https://github.com/ebraminio/sysweb/releases/latest/download/sysweb-linux-x86_64 \
  && chmod +x /tmp/sysweb \
  && /tmp/sysweb --root ./public
```

Linux aarch64:

```sh
curl -fsSL -o /tmp/sysweb \
  https://github.com/ebraminio/sysweb/releases/latest/download/sysweb-linux-aarch64 \
  && chmod +x /tmp/sysweb \
  && /tmp/sysweb --root ./public
```

macOS arm64:

```sh
curl -fsSL -o /tmp/sysweb \
  https://github.com/ebraminio/sysweb/releases/latest/download/sysweb-macos-arm64 \
  && chmod +x /tmp/sysweb \
  && /tmp/sysweb --root ./public
```

Windows PowerShell:

```powershell
Invoke-WebRequest https://github.com/ebraminio/sysweb/releases/latest/download/sysweb-windows-x86_64.exe -OutFile sysweb.exe
.\sysweb.exe --root .
```

Or detect the platform automatically:

```sh
curl -fsSL https://raw.githubusercontent.com/ebraminio/sysweb/main/scripts/run-latest.sh \
  | sh -s -- --root ./public
```

The release filenames are stable. These commands execute a GitHub-hosted
binary over TLS; use the `cargo install --git` form if you prefer to build from
source.

## Quick start

```sh
# Serve the repository demo, read-only:
syswebd --root public --port 8080
```

At startup the server prints each reachable interface URL, for example:

```text
syswebd 0.1.0 (async) listening on port 8080 of all interfaces
  lo0: http://127.0.0.1:8080
  en0: http://192.168.1.130:8080
  root: public, uploads: off
```

Uploads are disabled by default. Enable them explicitly:

```sh
syswebd --root public --port 8080 --upload
```

Then open `http://127.0.0.1:8080/` and use the page, or use the terminal:

```sh
# raw PUT
curl -T ./report.pdf http://127.0.0.1:8080/report.pdf

# multipart form upload
curl -F "file=@./report.pdf" http://127.0.0.1:8080/

# byte range
curl -r 0-1023 http://127.0.0.1:8080/report.pdf -o report.head

# create a directory
curl -X PUT http://127.0.0.1:8080/notes/
```

A small local chat is available as a separate mode. It is polling-based, keeps
only the last 16 messages in memory, and does not require JavaScript for basic
reading:

```sh
syswebd --chat --port 8080
```

Messages are shown with the sender's IP. Modern browsers use XHR to refresh
just the message list; without JavaScript there is a manual Refresh link. There
is no history and no WebSocket. `--root` is ignored in this mode. Basic auth
applies if configured:

```sh
syswebd --chat --userpass 'user:pass'
```

Protect every route with Basic auth:

```sh
syswebd --root public --userpass 'user:pass'
```

Basic auth is encoded, not encrypted. Without TLS it is only a casual access
barrier.

## CLI

```text
syswebd [OPTIONS]

-r, --root <DIR>       Directory to serve (default: .)
-p, --port <PORT>      TCP port, 1-65535 (default: 8080; implicit fallback)
-H, --host <ADDR>      IPv4 address to bind (default: 0.0.0.0)
    --backlog <N>      Listen backlog (default: 128)
    --userpass <U:P>   Require HTTP Basic auth
    --chat             Serve the built-in chat instead of files
    --upload           Enable PUT uploads and folder creation
    --replace          Allow uploads to replace existing files
    --no-index         Disable generated directory indexes
    --idle-timeout <S> Idle timeout in seconds (default: 30)
-q, --quiet            Suppress startup and request logs
-h, --help             Print help
-V, --version          Print the version
```

If `--port` is omitted and 8080 is busy, the server tries the next port.
An explicit `--port` fails instead.

## Security

This is a small, non-hardened server:

- no TLS; Basic auth credentials are plaintext over HTTP;
- chat messages are unencrypted and visible to anyone who can reach the port;
- uploads are off by default and have no quota;
- one slow client occupies one of the fixed connection slots;
- there are no write timeouts, CSRF checks, or rate limits.

Read [SECURITY.md](SECURITY.md) before exposing it. Prefer `--host 127.0.0.1`
unless LAN access is intentional.

## Development

Build, test, verification commands, size budgets, release details, and known
limitations are in [CONTRIBUTING.md](CONTRIBUTING.md). Architecture notes are in
[ARCHITECTURE.md](ARCHITECTURE.md), the roadmap in [ROADMAP.md](ROADMAP.md), and
benchmarks in [bench/](bench/).

## License

MIT. See [LICENSE](LICENSE).
