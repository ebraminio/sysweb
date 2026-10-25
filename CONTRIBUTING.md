# Contributing

Thanks for taking a look. This project is intentionally small and strict about
its constraints; please read this before sending a patch.

## Ground rules

1. **No heap.** Do not add `alloc`, a global allocator, `Box`, `Vec`, `String`,
   or a dependency that requires them in the server binary.
2. **No third-party runtime crates.** The binary should keep `cargo tree` at a
   single node. Test-only tooling is a discussion, not a default.
3. **Latest readiness APIs.** Do not add compatibility fallbacks for older
   kernels or OS releases. Linux uses `openat2`/`statx`/epoll, macOS uses
   `O_NOFOLLOW_ANY`/kqueue, and Windows uses `WSAPoll`. See the policy in
   [ARCHITECTURE.md](ARCHITECTURE.md).
4. **`unsafe` stays at the edges.** New `unsafe` belongs in a syscall/ABI or
   entry module, with a comment stating the invariant. Safe HTTP/path code
   should remain safe.
5. **Every platform claim needs a command.** If a change says “no libc” or
   “CRT-free”, CI must be able to prove it (`nm -u`, `readelf -d`, `otool -L`,
   `dumpbin /dependents`).
6. **Release builds are part of testing.** LTO has produced real bugs here
   (`bzero` recursively calling itself). Run both `cargo test` and
   `cargo test --release`.

## Before you send a change

```sh
cargo fmt --all
cargo clippy --all-targets -- -D warnings
cargo test
cargo test --release
cargo build --release
```

If you touched Linux linking, also verify:

```sh
readelf -h target/release/syswebd   # Type: EXEC
readelf -d target/release/syswebd   # no NEEDED
nm -u target/release/syswebd        # empty (no libc)
```

On macOS:

```sh
otool -L target/release/syswebd     # libSystem only (ld64 requirement)
nm -u target/release/syswebd        # allowlisted libSystem imports only
```

## Style

- Keep modules small and focused; prefer a free function over a trait if there
  is only one implementation.
- Fixed buffer sizes are named constants; do not scatter magic numbers.
- Error paths return `platform::Result`; logs are short and never allocate.
- Anything that writes to disk is opt-in and must be reflected in
  [SECURITY.md](SECURITY.md). Uploads should stay streamed and bounded; never
  buffer a whole request body.
- Directory/listing output is untrusted input: HTML-escape names, URL-encode
  hrefs, and never build markup from raw request bytes.
- Comments should explain *why* a syscall or layout is used, not restate the
  code.
- Avoid `unwrap`/`expect` in the library and binary. Tests may use them.
- No `println!`/`eprintln!` in the server; use `Platform::log` or raw writes.

## Adding a platform

See the “Porting checklist” in [ARCHITECTURE.md](ARCHITECTURE.md). A platform
PR should include:

- the raw ABI module,
- RAII handle types and a `Platform` impl,
- a binary entry point and `mem*` shims,
- linker arguments in `build.rs`,
- an end-to-end test,
- CI proof of the runtime it does not link.

## Adding HTTP features

Add pure logic to `src/http.rs`, `src/path.rs`, or `src/listing.rs` with unit
tests first. The server should stay a thin state machine over `Platform`; avoid
putting parsing or path policy in the I/O loop. If a feature needs a new
response header, update the 4 KiB header budget and the e2e assertions.
Directory pages are rendered in head/entry/tail fragments and streamed:
chunked for HTTP/1.1, close-delimited for HTTP/1.0. Keep both paths tested.

Uploads deserve extra care: require `Content-Length`, stream in fixed chunks, unlink partial files, and keep the feature behind `--upload`.
The multipart parser must stay incremental: never buffer a whole body, add
chunk-split tests for every state transition, keep the 8 KiB buffer bound, and
sanitize filenames before touching the filesystem.

For fuzz changes:

```sh
cargo check --manifest-path fuzz/Cargo.toml
cargo +nightly fuzz run http
cargo +nightly fuzz run path
cargo +nightly fuzz run multipart
```

Keep `tests/security.rs` deterministic; it must always leave the server alive
and must never observe outside-file content. For release checks, run:

```sh
./scripts/check-binary.sh target/release/syswebd
./scripts/check-size.sh target/release/syswebd
./scripts/check-no-deps.sh
./scripts/check-no-globals.sh
```

## Security

Please read [SECURITY.md](SECURITY.md). Do not open a public issue for a likely
vulnerability; use the private advisory channel.

## License

By contributing, you agree that your contribution is licensed under the MIT
license in [LICENSE](LICENSE).


## Repository map

```text
src/bin/syswebd.rs   entry points, panic handler, mem* shims
src/async_server.rs  fixed-slot async connection state machine
src/chat_server.rs   fixed-ring --chat mode
src/server.rs        shared response/path helpers
src/http.rs          request parsing
src/path.rs          target decoding and normalization
src/listing.rs       compiled-in HTML/CSS/JS directory page
src/multipart.rs     streaming multipart parser
src/mime.rs          static extension table
src/config.rs        no-alloc command-line parser
src/platform/        merged Platform trait; OS backends
tests/e2e.rs         real-process HTTP tests
tests/ports.rs       port selection tests
tests/security.rs    adversarial black-box tests (Unix)
fuzz/                cargo-fuzz targets
spikes/              isolated ABI experiments
bench/               localhost, Pi, and stress measurements
```

## Size budgets

Current hard caps:

| Target | Budget |
| --- | ---: |
| macOS arm64 | 140 KiB |
| Linux x86_64 | 108 KiB |
| Linux aarch64 | 104 KiB |
| Windows x86_64 | 116 KiB |

`scripts/check-size.sh` enforces the Unix budgets. The Windows release job
checks its own cap. A budget change must be deliberate and documented.

## Async model

There is one server path: a single-threaded readiness reactor over epoll,
kqueue, or WSAPoll. Connections occupy fixed slots; there is no executor and no
`async`/`await`. That is intentional: it maps directly to the syscalls, avoids
future boxing/pinning, and keeps every per-connection buffer fixed.

Keep-alive, sequential pipelining, uploads, and classic-form redirects live in
this state machine. `TCP_NODELAY` is set on accepted streams; Linux uses
`accept4(SOCK_NONBLOCK)`. The blocking server, `Platform::sleep_ms`, and the
`--blocking`/`--async` flags were removed.

io_uring and IOCP are not implemented. They are completion-based and would
require a separate reactor abstraction rather than the current readiness
interface. See [ROADMAP.md](ROADMAP.md); do not claim them because a spike
exists.

## HTTP and auth

- HTTP/1.0 defaults to close; HTTP/1.1 defaults to keep-alive.
- A connection closes after 1000 requests, on error, or on idle timeout.
- `--userpass user:pass` enables Basic auth on every route. Credentials are
  decoded on the stack and compared in constant time. It is not TLS.
- Classic forms return `303 See Other` after success so the listing reloads.
- Older browsers can use the GET text form; its size is limited by the
  browser's URL length.

## Releases

Every push creates a per-commit GitHub Release:

- tag `build-<short-sha>`;
- raw assets with stable names such as `sysweb-linux-x86_64`,
  `sysweb-linux-aarch64`, `sysweb-macos-arm64`, and
  `sysweb-windows-x86_64.exe`;
- the release is marked latest, so `/releases/latest/download/<asset>` works.

CI runs Linux and macOS tests, Linux aarch64 checks, fuzz smoke, Windows build,
Windows full e2e/port tests, import checks, and the Windows smoke script.

## Benchmarks, spikes, and stress

- `bench/run.sh`: sysweb/nginx/Caddy localhost comparison;
- `bench/pi/`: Raspberry Pi idle/2 GiB results and CPU sampler;
- `bench/stress.sh`: high-concurrency sysweb survival/throughput run;
- `spikes/`: isolated ABI experiments, documented in
  [spikes/README.md](spikes/README.md).

Benchmarks are single runs unless repeated; state the caveats. Do not turn a
small-file difference into a feature-parity claim.

## Commits

Incremental commits are preferred. Include:

```text
Assisted-by: DeepSeek <noreply@deepseek.com>
```

when an agent contributes. Push as work progresses. Do not wait for CI unless
the user asks; report the commit and let them report failures.
