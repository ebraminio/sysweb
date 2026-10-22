# Roadmap

The project is built in small, verifiable milestones. Each milestone keeps the
core constraints: `no_std`, no allocator, no third-party crates, and `unsafe`
confined to ABI layers.

Legend: ✅ done · 🚧 next · 🗺️ planned · 🔬 research

---

## M0 — Blocking, syscall-first server ✅

The current release (milestone 0.5).

- [x] `#![no_std]`, no `alloc`, no global allocator.
- [x] Zero third-party dependencies.
- [x] `Platform` trait with RAII `Listener`/`Stream`/`File`/`Dir`.
- [x] Linux x86_64 and aarch64 raw syscall backends
      (x86_64 runtime-tested; aarch64 cross-built and checked).
- [x] Linux binary: static ELF, custom `_start`, no libc, no dynamic section.
- [x] macOS arm64 raw BSD syscall backend.
- [x] macOS binary: zero imported symbols; `libSystem` only as an ld64
      requirement.
- [x] HTTP/1.0 and HTTP/1.1 request parsing for `GET`/`HEAD`/`PUT`.
- [x] `Content-Length` and `Expect: 100-continue`; chunked bodies rejected.
- [x] Static files, `index.html`, MIME table, 16 KiB streaming.
- [x] Built-in directory index with metadata, breadcrumbs, sorting, and a
      self-contained 64 KiB HTML/CSS/JS page.
- [x] Opt-in raw `PUT` uploads, folder creation, `--replace`, 1 GiB cap, and
      failed-upload cleanup.
- [x] `301` from `/dir` to `/dir/`; `--no-index` mode.
- [x] HTML escaping and URL encoding for listing entries; all symlinks
      beneath the root denied.
- [x] Percent-decoding, dot-segment normalization, traversal rejection.
- [x] 201/301/400/403/404/405/409/411/413/414/431/500/505 responses.
- [x] `Content-Length`, `Content-Type`, `Server`, `Connection: close`.
- [x] IPv4 bind, CLI (`--root`, `--port`, `--host`, `--backlog`, `--upload`,
      `--replace`, `--no-index`, `--quiet`).
- [x] Unit tests + four real-binary end-to-end tests.
- [x] CI workflow for Linux and macOS, including `cargo test --release` to
      catch LTO-only libcall bugs.
- [x] Latest-API policy: `openat2` + `RESOLVE_NO_SYMLINKS` and `statx` on
      Linux, `O_NOFOLLOW_ANY` on macOS, all relative to a document-root fd.
- [x] All symlinks beneath the root denied (final and intermediate).
- [x] Demonstrated: 66,480-byte macOS binary with `nm -u` empty; 43,408-byte
      (x86_64) and 41,904-byte (aarch64) static Linux ELFs with no dynamic
      section.

**Known gaps:** single-threaded blocking, no timeouts, no keep-alive, no `Date`,
no `Range`, no auth/delete/rename, macOS Intel unsupported, no Windows backend.
Minimums are Linux 5.6+, macOS 11+, and Windows 10 1809+; there are no legacy
fallbacks.

---

## M1 — Hardening, still blocking 🚧

Make the blocking server pleasant and safe enough to run on a LAN.

### HTTP correctness

- [ ] `Date` header in IMF-fixdate format. Requires a clock syscall
      (`clock_gettime` on Linux, `gettimeofday`/`clock_gettime` on macOS) and a
      civil-from-days conversion, all on the stack.
- [ ] `Last-Modified` and `ETag` from `st_mtime` + `st_size`.
- [ ] `If-Modified-Since` / `If-None-Match` → 304.
- [ ] `Range: bytes=...` with a single range → 206 + `Content-Range`; reject
      multi-range with 416 or ignore per policy.
- [ ] `Keep-Alive` with a bounded request count and idle timeout.
- [ ] `HEAD` for every error path (already done, keep under test).
- [ ] Optional `OPTIONS`/`TRACE` policy: 405 with `Allow` is fine.
- [ ] Optional multipart/form-data compatibility for clients that cannot send a
      raw `PUT` body.
- [ ] Listing pagination or chunked rendering for directories whose page
      exceeds 64 KiB.

### Parity and hardening

- [ ] Opt-in `--delete` with the same path containment checks and explicit
      confirmation in the UI.
- [ ] Opt-in rename/move with two-step containment checks.
- [ ] `--userpass=user:pass` Basic auth (base64 decode on the stack, constant
      time compare) like pad.js.
- [ ] CORS / cross-origin isolation headers, off by default.
- [ ] Optional path prefix and inactivity timeout.
- [ ] Port fallback when the requested port is busy (pad.js behavior).
- [ ] Atomic uploads: write a temp file, fsync, rename; keep `--replace`
      non-destructive on failure.
- [ ] Upload quota / max total bytes, and a dedicated upload directory option.
- [ ] CSRF/Origin/Host checks before any write endpoint.
- [ ] Read/write timeouts (`SO_RCVTIMEO` / `SO_SNDTIMEO`) before the async
      reactor lands.
- [ ] Graceful shutdown on `SIGINT`/`SIGTERM` (signal syscalls; no libc).
- [ ] `EMFILE` backoff instead of terminating the process.
- [ ] Configurable maximum request-header size and connection limit.
- [ ] Never log raw control bytes; keep the current normalized-path logging.
- [ ] Fuzz targets for `http::parse`, `path::normalize`,
      `path::append_index`, and the listing escaper (`cargo-fuzz` is test-only
      tooling, not a runtime dependency).
- [ ] Property test: no input to `normalize` can produce a `..` segment.
- [ ] Miri/UB review of the unsafe modules where feasible.

### Verification

- [ ] `strace`/`dtruss` audit: the only syscalls are the expected ones, and no
      libc symbol is called on Linux/macOS.
- [ ] Compare behavior against a reference server for the supported subset.
- [ ] Add a benchmark script and record baseline throughput/latency.

---

## M2 — Async reactor abstraction 🚧

The core goal: non-blocking I/O through each OS's native readiness API, with no
executor and no allocation.

### Abstraction

- [ ] Add `Reactor` with `register`, `deregister`, and `wait`.
- [ ] Add `Interest`/`Event` types and a stable per-connection token.
- [ ] Keep `Platform` for files and socket creation; add non-blocking variants
      (`accept_nonblocking`, `read_nonblocking`, `write_nonblocking`).
- [ ] Unified `WouldBlock` handling; no busy loops.
- [ ] A `Reactor` implementation for testing in memory (scripted events).

### Backends

- [ ] Linux **io_uring** only: `io_uring_setup` (425), `io_uring_enter` (426),
      `io_uring_register` (427); `IORING_OP_ACCEPT/RECV/SEND/OPENAT2/STATX/
      CLOSE/SPLICE`. No epoll backend.
- [ ] Registered buffers and, where useful, `IORING_SETUP_SQPOLL`;
      multishot accept/recv where it simplifies the state machine.
- [ ] macOS `kqueue` (362) / `kevent` (363) with `EVFILT_READ`/`EVFILT_WRITE`.
- [ ] macOS non-blocking sockets via `fcntl(F_SETFL, O_NONBLOCK)` (92).
- [ ] Document the io_uring submission/completion model and the kqueue
      mapping side by side.

### Server state machine

- [ ] Fixed `Connection` slots (`MAX_CONNECTIONS`, no heap).
- [ ] States: `ReadingHeaders → Opening → WritingHeaders → Streaming`,
      plus `ReadingBody → WritingFile` for uploads, `Listing`, and `Closing`.
- [ ] Per-slot deadlines for header read, idle keep-alive, and write progress.
- [ ] Bounded accept loop per event to avoid starving existing connections.
- [ ] Backpressure: stop reading when the response socket is not writable;
      apply the same discipline to upload bodies and disk writes.
- [ ] Keep-alive integrated into the state machine.
- [ ] `Connection: close` remains the default on errors.

### Performance and validation

- [ ] Benchmark 1/10/100/1,000/10,000 concurrent connections.
- [ ] Confirm RSS is flat in the number of connections (fixed slots).
- [ ] Load test with slow clients, partial headers, and slow uploads
      (slowloris resistance).
- [ ] No `std`, no `alloc`, still zero third-party deps.
- [ ] CI runs the async e2e suite on Linux and macOS.

---

## M3 — Windows CRT-free backend 🗺️

- [ ] `#![no_main]`, export `mainCRTStartup`, `/NODEFAULTLIB`.
- [ ] Provide `mem*` shims and the panic handler for the MSVC target.
- [ ] Link `kernel32`/`ws2_32` explicitly; document the exact MSVC/link.exe
      recipe and the LLVM/lld alternative.
- [ ] `WSAStartup`, `socket`, `bind`, `listen`, `accept`, `recv`, `send`.
- [ ] File I/O with `CreateFileA`, `GetFileSizeEx`, `ReadFile`, `CloseHandle`.
- [ ] IOCP directly: `CreateIoCompletionPort`,
      `GetQueuedCompletionStatusEx`, `AcceptEx`, `WSARecv`, `WSASend`,
      `CancelIoEx`. No `WSAPoll`/`select` fallback.
- [ ] File I/O with `CreateFile2`/`GetFileSizeEx`/`ReadFile`.
- [ ] CI job on `windows-latest`; `dumpbin /dependents` proves no CRT DLLs and
      `dumpbin /imports` lists only the intended Win32 DLLs.
- [ ] E2E test on Windows.

Acceptance: a `.exe` that serves the same test suite, imports no
`vcruntime`/`msvcrt`/`ucrtbase`, and uses IOCP for socket readiness.

---

## M4 — Zero-copy and Linux acceleration 🗺️

- [ ] Linux `sendfile(2)` for the header + file body (or header, then
      `sendfile` for the body).
- [ ] Linux `splice(2)` for file→socket paths where `sendfile` does not apply.
- [ ] macOS `sendfile(2)` (raw syscall number 337) with `SF_NODISKIO` behavior
      documented.
- [ ] `io_uring` backend as an alternative Linux reactor:
      `io_uring_setup` (425), `io_uring_enter` (426), `io_uring_register` (427).
- [ ] Decide whether `mmap` is ever worth it for static files; likely not with
      `sendfile`.
- [ ] Benchmark against `nginx`/`caddy` for the static-file subset (report
      numbers, not claims).

---

## M5 — Protocol and operations 🗺️

- [ ] Conditional requests and ranges from M1 fully integrated with async.
- [ ] Configurable MIME overrides and custom headers.
- [ ] Directory index policy (index file list, no generated listing).
- [ ] Optional access log format (still fixed-buffer).
- [ ] Graceful reload of root/port without a restart (hard without exec; maybe
      `SIGHUP` + config copy).
- [ ] systemd socket activation (Linux).
- [ ] `SO_REUSEPORT` for multi-process scaling.
- [ ] TLS research: a `no_std` TLS stack is a separate, much larger project.
      Likely out of scope; document the boundary.

---

## M6 — Project and ecosystem 🗺️

- [ ] Publish crates (`sysweb` library, `syswebd` binary).
- [ ] Reproducible release binaries for Linux x86_64/aarch64 and macOS arm64.
- [ ] `Dockerfile` using `FROM scratch` for the static Linux binary.
- [ ] Man page and shell completions.
- [ ] Fuzzing corpus and a continuous fuzz job.
- [ ] Security policy and disclosure process.
- [ ] Architecture decision records (ADRs) for syscall choices and the macOS
      ABI risk.
- [ ] Porting guide and a `Platform` conformance test suite.

---

## Guiding constraints

Every milestone must preserve:

1. **No heap.** No `alloc`, no global allocator. Fixed arrays, caller buffers,
   or static arenas only.
2. **No third-party runtime crates.** Test-only tooling is acceptable if it
   never enters the binary.
3. **Unsafe budget.** New `unsafe` lives in an ABI/entry module with a written
   invariant and a test where possible.
4. **OS-native async.** No fake async over threads; no hidden thread pools.
5. **Measurable claims.** Every “no libc”/“CRT-free” claim comes with a command
   (`nm -u`, `readelf -d`, `otool -L`, `dumpbin /dependents`) and CI output.
6. **Small binaries.** Regressions are visible in CI size output.

## Open questions

- Should the macOS backend keep raw syscalls, or offer a `libsystem` feature for
  stability? Raw is the experiment; a feature-gated fallback may be wise before
  publishing.
- How much of io_uring should be used directly (`IORING_OP_*`) versus keeping
  the portable readiness layer for macOS/Windows parity?
- Does `sendfile` measurably beat a 16 KiB read/write loop for files that are
  already in page cache?
- How should Windows express `Platform` file handles when IOCP needs
  `OVERLAPPED` state? Likely a separate `AsyncFile` associated type.
- Can `async fn` in traits be used without an allocator/executor, or is the
  hand-written state machine permanently simpler? Current bias: state machine.
