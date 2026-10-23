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
- [x] macOS arm64 libSystem ABI backend (stable public functions).
- [x] macOS binary: no CRT/std; only allowlisted libSystem imports.
- [x] HTTP/1.0 and HTTP/1.1 request parsing for `GET`/`HEAD`/`PUT`.
- [x] `Content-Length` and `Expect: 100-continue`; chunked bodies rejected.
- [x] Static files, `index.html`, MIME table, `sendfile`, single-range 206/416.
- [x] Built-in directory index with metadata, breadcrumbs, sorting, and a
      self-contained 64 KiB HTML/CSS/JS page.
- [x] Opt-in raw `PUT` uploads, folder creation, `--replace`, no per-request
      size cap, and failed-upload cleanup.
- [x] `301` from `/dir` to `/dir/`; `--no-index` mode.
- [x] HTML escaping and URL encoding for listing entries; all symlinks
      beneath the root denied.
- [x] Percent-decoding, dot-segment normalization, traversal rejection.
- [x] 201/301/400/403/404/405/409/411/414/431/500/505 responses.
- [x] `Content-Length`, `Content-Type`, `Server`, `Connection: close`.
- [x] IPv4 bind, CLI (`--root`, `--port`, `--host`, `--backlog`, `--upload`,
      `--replace`, `--no-index`, `--quiet`).
- [x] Unit tests + five real-binary end-to-end tests.
- [x] CI workflow for Linux and `macos-latest` arm64, including `cargo test
      --release` to
      catch LTO-only libcall bugs.
- [x] Latest-API policy: `openat2` + `RESOLVE_NO_SYMLINKS` and `statx` on
      Linux, `O_NOFOLLOW_ANY` on macOS, all relative to a document-root fd.
- [x] All symlinks beneath the root denied (final and intermediate).
- [x] `sendfile` file serving, with SIGPIPE-safe aborted downloads.
- [x] Streaming `multipart/form-data` form uploads for old/no-JS browsers,
      plus a no-JS form in the directory index.
- [x] Reusable `scripts/check-binary.sh` and `scripts/check-no-deps.sh` used
      by CI.
- [x] Per-commit GitHub Releases for Linux x86_64/aarch64 and macOS arm64,
      with tarballs and SHA-256 checksums.
- [x] Transient `accept` failures retry with a 100 ms backoff instead of
      exiting; a stalled service is acceptable, a permanent exit is not.
- [x] Enforced binary-size budgets: 140 KiB macOS arm64, 108 KiB x86_64, 104 KiB aarch64
      target.
- [x] `cargo-fuzz` targets for HTTP/path/multipart plus a black-box
      containment/hang/crash test in `tests/security.rs`.
- [x] Default-port fallback when the port is implicit and 8080 is busy.
- [x] Runtime-tested on a Raspberry Pi 5 (kernel 6.12 aarch64): listings,
      uploads, forms, `sendfile`, symlink denial, aborted transfers.
- [x] Demonstrated: 100,608-byte macOS binary with only allowlisted libSystem
      imports; 57,488-byte (x86_64) and 66,248-byte (aarch64) static Linux ELFs
      with no dynamic section.

**Known gaps:** no `Date`, no multi-range/`If-Range`, no auth/delete/rename,
classic forms accept one file.
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
- [ ] `Keep-Alive` with a bounded request count and idle timeout.
- [ ] `HEAD` for every error path (already done, keep under test).
- [ ] Optional `OPTIONS`/`TRACE` policy: 405 with `Allow` is fine.
- [ ] Multi-file multipart forms and RFC 5987 `filename*` names.
- [ ] Optional: keep the legacy page usable on very old layout engines with a
      simpler stylesheet.
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
- [ ] Atomic uploads: write a temp file, fsync, rename; keep `--replace`
      non-destructive on failure.
- [ ] Upload quota / max total bytes, and a dedicated upload directory option.
- [ ] CSRF/Origin/Host checks before any write endpoint.
- [ ] Read/write timeouts (`SO_RCVTIMEO` / `SO_SNDTIMEO`) before the async
      reactor lands.
- [ ] Graceful shutdown on `SIGINT`/`SIGTERM` (signal syscalls; no libc).
- [x] `EMFILE` backoff instead of terminating the process.
- [ ] Configurable maximum request-header size and connection limit.
- [ ] Never log raw control bytes; keep the current normalized-path logging.
- [x] Property test: no input to `normalize` can produce a `..` segment.
- [ ] Miri/UB review of the unsafe modules where feasible.

### Verification

- [ ] `strace`/`dtruss` audit: Linux makes only the expected syscalls and calls
      no libc symbol; macOS imports only the allowlisted libSystem functions.
- [ ] Compare behavior against a reference server for the supported subset.
- [ ] Add a benchmark script and record baseline throughput/latency.

---

## M2 — Async reactor abstraction 🚧

The core goal: non-blocking I/O through each OS's native readiness API, with no
executor and no allocation. The readiness path is default on Unix; io_uring
replacement and Windows IOCP remain.

### Abstraction

- [x] Add a readiness reactor with register/deregister/wait (epoll first,
      kqueue next; io_uring remains the target Linux backend).
- [x] Add `Interest`/`Event` types and a stable per-connection token.
- [x] Keep `Platform` for files; add non-blocking listener/stream variants.
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

- [x] Fixed `Connection` slots for the GET/HEAD async path (no heap).
- [x] States for `ReadingHeaders → WritingHeaders → Streaming`,
      `ReadingBody → WritingFile`, listing, and closing.
- [ ] Per-slot deadlines for header read, idle keep-alive, and write progress.
- [ ] Bounded accept loop per event to avoid starving existing connections.
- [ ] Replace the blocking accept backoff with a reactor timer/deadline and
      remove `Platform::sleep_ms` if no blocking backend remains.
- [ ] Backpressure: stop reading when the response socket is not writable;
      apply the same discipline to upload bodies and disk writes.
- [ ] Keep-alive integrated into the state machine.
- [ ] `Connection: close` remains the default on errors.

### Performance and validation

- [ ] **After the reactor:** stress/load testing. Run 1/10/100/1,000/10,000
      concurrent connections, partial headers, slow uploads, disconnects, and
      fd pressure. The acceptance rule is liveness, not latency: the process
      may stall, but must not exit permanently.
- [ ] Confirm RSS is flat in the number of connections (fixed slots).
- [ ] Load test with slow clients, partial headers, and slow uploads
      (slowloris resistance).
- [ ] No `std`, no `alloc`, still zero third-party deps.
- [ ] CI runs the async e2e suite on Linux and macOS.

---

## M3 — Windows CRT-free backend 🗺️

**Ordering:** after M2. A CRT-free Win32 async backend using `WSAPoll` now
exists; the final backend is IOCP, not a blocking
implementation that would be rewritten by the reactor. The no-CRT
link/entry-point spike is done in `spikes/windows-no-crt`; the real backend
lands against the async `Reactor` trait.

- [x] No-CRT entry/link spike: `#![no_main]`, `mainCRTStartup`,
      `/NODEFAULTLIB`, 3,584-byte PE, only `KERNEL32.dll`, runs on Windows 11.
- [x] IOCP completion spike: `CreateIoCompletionPort`, overlapped `AcceptEx`,
      and overlapped `WSARecv`/`WSASend` over loopback; only
      `KERNEL32.dll`/`WS2_32.dll`.
- [x] Provide the `mem*` shims for the full MSVC target.
- [x] Link `kernel32`/`ws2_32`/`shell32` explicitly; document the MSVC and
      LLVM/lld alternatives.
- [x] `WSAStartup`, `socket`, `bind`, `listen`, `accept`, `recv`, `send`.
- [x] File I/O with `CreateFileW`, `GetFileInformationByHandle`, `ReadFile`, `CloseHandle`.
- [ ] IOCP directly: `CreateIoCompletionPort`,
      `GetQueuedCompletionStatusEx`, `AcceptEx`, `WSARecv`, `WSASend`,
      `CancelIoEx`. No `WSAPoll`/`select` fallback.
- [ ] File I/O with `CreateFile2`/`GetFileSizeEx`/`ReadFile`.
- [x] CI job on `windows-latest`; `dumpbin /dependents` proves no CRT DLLs and
      `dumpbin /imports` lists only the intended Win32 DLLs.
- [ ] E2E test on Windows.

Acceptance: a `.exe` that serves the same test suite, imports no
`vcruntime`/`msvcrt`/`ucrtbase`, and uses IOCP for socket readiness.

---

## M4 — Zero-copy and Linux acceleration 🗺️

- [x] Linux/macOS `sendfile(2)` for file bodies. (Moved to M0.)
- [ ] Linux `splice(2)`/io_uring `SPLICE` for paths where `sendfile` does not
      apply.
- [ ] macOS `SF_NODISKIO` behavior documented.
- [ ] `io_uring` backend as an alternative Linux reactor:
      `io_uring_setup` (425), `io_uring_enter` (426), `io_uring_register` (427).
- [ ] Decide whether `mmap` is ever worth it for static files; likely not with
      `sendfile`.
- [ ] Benchmark against `nginx`/`caddy` for the static-file subset (report
      numbers, not claims).

---

## M5 — Protocol and operations 🗺️

- [ ] Conditional requests (`ETag`/`If-Range`) integrated with async.
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
- [ ] Bit-for-bit reproducible release binaries (the per-commit release
      workflow builds the three supported targets but does not yet verify
      reproducibility).
- [ ] `Dockerfile` using `FROM scratch` for the static Linux binary.
- [ ] Man page and shell completions.
- [ ] Broader OSS-Fuzz onboarding and corpus maintenance.
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
6. **No writable global state.** Service-side `static`/`static mut`/
   `thread_local!` items are rejected; the fixed-buffer design passes state
   explicitly. This also keeps read-only sections mergeable into `.text`.
7. **Small binaries.** Release binaries stay within the checked budgets
   (140 KiB macOS arm64, 108 KiB x86_64, 104 KiB aarch64); growth is a deliberate
   decision, not a silent regression.

## Open questions

- Should macOS directory enumeration move from `readdir` to
  `getattrlistbulk` to avoid libSystem-internal allocation while keeping the
  public ABI?
- How much of io_uring should be used directly (`IORING_OP_*`) versus keeping
  the portable readiness layer for macOS/Windows parity?
- Does `splice` or io_uring `SEND_ZC` measurably beat `sendfile` for files that
  are already in page cache?
- How should Windows express `Platform` file handles when IOCP needs
  `OVERLAPPED` state? Likely a separate `AsyncFile` associated type.
- Can `async fn` in traits be used without an allocator/executor, or is the
  hand-written state machine permanently simpler? Current bias: state machine.
  A no-alloc async/await spike (`spikes/async-no-alloc`) found a 16-byte future
  and 1.2–1.3 KiB static Linux binaries, but has not tested self-borrowing
  per-connection futures or a reactor waker.
