# Security policy

`sysweb` is a **file-transfer server in an early milestone**. It is suitable
for local development, trusted LANs, and moving files between machines you
control. It is **not** hardened for direct exposure to the public internet.

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
- malicious uploads, stored XSS, disk exhaustion;
- cross-site requests against a `--host 127.0.0.1` server;
- resource exhaustion through slow or many connections.

Out of scope for 0.1:

- TLS termination (there is none);
- authorization beyond optional HTTP Basic authentication;
- multi-tenant isolation;
- protecting files whose permissions already allow the server user to read
  them;
- kernel or hypervisor compromise.

## Current protections

- **All interfaces by default.** The server binds `0.0.0.0:8080` unless
  `--host 127.0.0.1` is passed. Read-only files are reachable from the local
  network; uploads remain off unless `--upload` is passed.
  given. Exposing it is an explicit decision.
- **Origin-form targets only.** Absolute URLs, authority-form, and `*` are
  rejected.
- **Single percent-decoding.** `%XX` is decoded exactly once. Invalid escapes
  are 400.
- **Rejected bytes.** NUL, backslash, ASCII controls, and DEL are rejected
  before the filesystem is touched.
- **Dot-segment normalization.** Empty and `.` segments are dropped; `..` pops
  a segment and is rejected if it would escape `/`. The resulting URL path is
  made relative to the document-root fd and opened with a no-symlink policy.
- **Fixed buffers.** Oversized headers are 431; oversized URLs are 414. There
  are no dynamic allocations to exhaust.
- **No raw header reflection.** Response headers contain static strings,
  decimal numbers, and a percent-encoded `Location` path; raw request bytes are
  never echoed.
- **Bounded reads/writes.** The parser stops at `\r\n\r\n` or 8 KiB; file
  streaming and uploads stop at `Content-Length`. Range offsets are resolved
  against `st_size`; past-EOF requests get `416`, never a read beyond the file.
- **SIGPIPE-safe disconnects.** Normal writes use `MSG_NOSIGNAL`; Linux blocks
  `SIGPIPE` process-wide for `sendfile`, and macOS sets `SO_NOSIGPIPE` on every
  accepted socket. An aborted download yields `EPIPE`, not process death.
- **Bounded multipart forms.** Classic `POST` forms use a streaming parser with
  a 32 KiB buffer; filenames are reduced to a basename and reject separators,
  controls, NUL, `.` and `..`; only one file part is accepted; non-file fields
  are discarded. There is no per-request size cap.
- **Read-only by default.** Files are opened `O_RDONLY`; uploads and folder
  creation require `--upload`, and overwriting requires `--replace`.
- **No symlinks beneath the document root.** Linux uses `openat2` with
  `RESOLVE_NO_SYMLINKS`; macOS uses `O_NOFOLLOW_ANY`. Every operation is
  relative to a document-root fd opened once at startup. Symlinks are rendered
  as unlinked `link` rows in the index.
- **Streaming uploads.** `Content-Length` is mandatory for file uploads and
  chunked bodies are rejected, but there is no fixed per-request size cap.
- **Failed uploads are unlinked.** A partial file is removed after early EOF or
  a write error, so it is not served later.
- **Index output is escaped.** Names are HTML-escaped in text/attributes and
  percent-encoded in URLs; a filename cannot inject markup or a
  `javascript:` href.
- **No CGI/`--cmd`.** There is no command execution endpoint, by design.

## Known limitations

### Latest-API floor

There are no compatibility fallbacks: Linux 5.6+ (`openat2`), macOS 11+
(`O_NOFOLLOW_ANY`), and Windows 10 1809+ (IOCP) are the minimums. Older hosts
are unsupported, not emulated.

### No TLS

Traffic is plaintext. Basic auth credentials are only base64-encoded, not
encrypted. Do not send credentials or sensitive data. Put a TLS
terminator in front if you need HTTPS.

### Single-threaded blocking I/O

One connection at a time. A client that opens a connection and sends nothing
blocks every other client indefinitely. There are no read/write timeouts yet.
Do not expose the server to untrusted networks until the async reactor and
timeouts land (milestone 1/2). A reverse proxy with timeouts can mitigate this
in the meantime.

### Symlinks

All symlinks beneath the document root are refused, including intermediate
directories, for reads, listings, uploads, folder creation, and cleanup. The
root path itself may contain symlinks (for example macOS `/var`): it is opened
once at startup. Hard links and bind mounts are not detected; only serve a tree
you control.

### Uploads are unauthenticated

With `--upload`, anyone who can reach the port can write files, create
directories, and (with `--replace`) overwrite them. Optional `--userpass`
protects every route, including uploads; without it there is no authentication,
quota, or per-client limit. Prefer `--host 127.0.0.1`, or put an authenticated
reverse proxy in front.

The classic multipart form uses a normal HTML `POST`, which browsers do not
CORS-preflight. A hostile page cannot populate a file input with an arbitrary
local file, but do not rely on browser file-input rules as the only boundary:
keep uploads off on untrusted networks.

Uploaded HTML/SVG can be served back with an active content type. If you enable
uploads on an origin that also hosts sensitive pages or cookies, this becomes a
stored-XSS risk. Treat an upload directory as untrusted content, serve it from a
separate origin, or keep uploads disabled.

### No CSRF/Host validation

There is no `Origin`/`Host` allowlist and no CSRF token. A browser on the same
machine can attempt requests to `127.0.0.1:8080`; modern cross-origin `PUT`
requests trigger CORS preflight, which this server does not answer, but do not
rely on that as a security boundary. Prefer `--host 127.0.0.1` and do not
enable uploads on a machine where arbitrary local web content is a concern.

### Disk exhaustion

There is no per-request upload cap. A single upload or repeated uploads can fill
the filesystem. Use a dedicated filesystem/quota and monitor free space.

### `--replace` is destructive

`--replace` truncates an existing file in place before writing the new body. A
failed replacement can lose the original. Use it only where that is acceptable;
atomic temp+rename uploads are roadmap.

### `accept` errors terminate the process

`EMFILE`, a broken listener, or another non-`EINTR` accept error causes a clean
exit with status 1. This is a denial-of-service vector under fd exhaustion.

### `--no-index` is not access control

`--no-index` only disables generated directory pages. Any file whose URL you
know is still served. Use filesystem permissions, a separate root, or a reverse
proxy for access control.

### No rate limiting or connection cap

The kernel's listen backlog is the only queue limit. There is no per-IP limit,
request-rate limit, or maximum concurrent connection setting yet.

### Log injection

Request logging uses the normalized URL path, which excludes control bytes.
The root path supplied on the command line is logged verbatim; do not pass a
root containing terminal escape sequences.

### macOS libSystem ABI

The macOS backend calls the public libSystem ABI rather than private XNU
syscall numbers or kernel `struct stat` layouts. The release binary links
`libSystem` (which ld64 requires anyway) and imports only an allowlist of
functions checked by `scripts/check-binary.sh`. There is no CRT or `std`.

`fdopendir`/`readdir` may allocate inside libSystem; the binary itself has no
allocator. Linux remains the libc-free target. A wrong SDK `struct stat` layout
would still produce incorrect responses, but the public libSystem ABI is the
stable interface Apple supports.

### No privilege dropping

The server runs as the invoking user, with that user's file access. There is no
`chroot`, `setuid`, or seccomp sandbox. Use a dedicated unprivileged user and a
minimal document root.

## Hardening checklist for operators

- Keep `--host 127.0.0.1` unless LAN exposure is intended.
- Run as an unprivileged user with read-only access to the document root.
- Leave `--upload` off unless you need it; never combine it with a public bind.
- If uploads are needed, use a separate directory/origin and an authenticated
  reverse proxy with timeouts and a body-size limit.
- Put a reverse proxy in front for TLS, timeouts, and connection limits.
- Do not place secrets, `.git`, source code, or private keys in the served
  directory.
- Do not place symlinks beneath the document root; the root path itself may be a symlink.
- Watch stderr for `syswebd: ...` errors; `--quiet` suppresses request logs but
  not error lines.

## Security-relevant roadmap

- read/write/idle timeouts;
- connection limits;
- authentication and opt-in delete/rename;
- CSRF/Origin/Host validation;
- atomic temp+rename uploads and disk quotas;
- fuzzing of the parser, path normalizer, multipart parser, and listing
  escaper; the CI fuzz job and `tests/security.rs` cover the first three;
- a latest-API audit (io_uring, IOCP, `statx`/`openat2` extensions);
- TLS boundary documentation (likely a separate project);
- seccomp/pledge-style sandboxing;
- `Date`/`ETag`/range correctness (not security-critical, but HTTP correctness).
