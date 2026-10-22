# Security policy

`sysweb` is a **static file server in an early milestone**. It is suitable for
local development and trusted LANs. It is **not** hardened for direct exposure
to the public internet.

## Supported versions

| Version | Supported |
| --- | --- |
| 0.1.x | ✅ |

## Reporting a vulnerability

Please do not open a public issue for a suspected vulnerability. Contact the
maintainer privately (for example, via the repository's private security
advisory feature) with:

- the affected version/commit,
- the platform and architecture,
- a minimal request or reproduction,
- the impact you believe it has.

You will get an acknowledgement and a remediation plan. Since the project has
no released users yet, disclosure timing is negotiable.

## Threat model

In scope:

- untrusted HTTP requests from the network;
- path traversal and encoded-path tricks;
- requests that are larger than the fixed buffers;
- malformed request lines and methods;
- client disconnects and partial writes;
- resource exhaustion through slow or many connections.

Out of scope for 0.1:

- TLS termination (there is none);
- authentication or authorization;
- multi-tenant isolation;
- protecting files whose permissions already allow the server user to read
  them;
- kernel or hypervisor compromise.

## Current protections

- **Loopback by default.** The server binds `127.0.0.1:8080` unless `--host` is
  given. Exposing it is an explicit decision.
- **Origin-form targets only.** Absolute URLs, authority-form, and `*` are
  rejected.
- **Single percent-decoding.** `%XX` is decoded exactly once. Invalid escapes
  are 400.
- **Rejected bytes.** NUL, backslash, ASCII controls, and DEL are rejected
  before the filesystem is touched.
- **Dot-segment normalization.** Empty and `.` segments are dropped; `..` pops
  a segment and is rejected if it would escape `/`. A second check in
  `path::join` rejects any residual `..` segment.
- **Fixed buffers.** Oversized headers are 431; oversized URLs are 414. There
  are no dynamic allocations to exhaust.
- **No header reflection.** Response headers contain only static strings and
  decimal numbers; the request target is not echoed into headers.
- **Bounded reads/writes.** The parser stops at `\r\n\r\n` or 8 KiB; file
  streaming stops at `Content-Length`.
- **`MSG_NOSIGNAL`.** A client disconnect during a write does not kill the
  process with `SIGPIPE` on Linux or macOS.
- **Read-only files.** Files are opened `O_RDONLY`. There is no upload, write,
  delete, or CGI path.

## Known limitations

### No TLS

Traffic is plaintext. Do not send credentials or sensitive data. Put a TLS
terminator in front if you need HTTPS.

### Single-threaded blocking I/O

One connection at a time. A client that opens a connection and sends nothing
blocks every other client indefinitely. There are no read/write timeouts yet.
Do not expose the server to untrusted networks until the async reactor and
timeouts land (milestone 1/2). A reverse proxy with timeouts can mitigate this
in the meantime.

### Symlinks are followed

`openat` follows symlinks. A symlink inside the document root can point to a
file outside it. Only serve a directory tree you control and do not create
links to sensitive files. Planned fix: `openat2(RESOLVE_BENEATH)` on Linux and
`O_NOFOLLOW`/`lstat` walks elsewhere.

### `accept` errors terminate the process

`EMFILE`, a broken listener, or another non-`EINTR` accept error causes a clean
exit with status 1. This is a denial-of-service vector under fd exhaustion.

### No rate limiting or connection cap

The kernel's listen backlog is the only queue limit. There is no per-IP limit,
request-rate limit, or maximum concurrent connection setting yet.

### Log injection

Request logging uses the normalized URL path, which excludes control bytes.
The root path supplied on the command line is logged verbatim; do not pass a
root containing terminal escape sequences.

### macOS raw syscall ABI

The macOS backend uses Apple's undocumented syscall numbers and struct layouts.
They can change across OS releases. A wrong layout could read the wrong file
size or mode; it is not a memory-safety issue in the current code because the
stat buffer is fixed-size and fields are read with bounds, but it can cause
incorrect responses. Linux is the primary supported target.

### No privilege dropping

The server runs as the invoking user, with that user's file access. There is no
`chroot`, `setuid`, or seccomp sandbox. Use a dedicated unprivileged user and a
minimal document root.

## Hardening checklist for operators

- Keep the default loopback bind unless you need otherwise.
- Run as an unprivileged user with read-only access to the document root.
- Put a reverse proxy in front for TLS, timeouts, and connection limits.
- Do not place secrets, `.git`, source code, or private keys in the served
  directory.
- Keep the document root free of symlinks to outside paths.
- Watch stderr for `syswebd: ...` errors; `--quiet` suppresses request logs but
  not error lines.

## Security-relevant roadmap

- read/write/idle timeouts;
- connection limits and accept backoff;
- symlink containment;
- fuzzing of the parser and path normalizer;
- TLS boundary documentation (likely a separate project);
- seccomp/pledge-style sandboxing;
- `Date`/`ETag`/range correctness (not security-critical, but HTTP correctness).
