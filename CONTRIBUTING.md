# Contributing

Thanks for taking a look. This project is intentionally small and strict about
its constraints; please read this before sending a patch.

## Ground rules

1. **No heap.** Do not add `alloc`, a global allocator, `Box`, `Vec`, `String`,
   or a dependency that requires them in the server binary.
2. **No third-party runtime crates.** The binary should keep `cargo tree` at a
   single node. Test-only tooling is a discussion, not a default.
3. **Latest APIs only.** Do not add compatibility fallbacks for older kernels
   or OS releases. Linux targets `openat2`/`statx`/io_uring, macOS targets
   `O_NOFOLLOW_ANY`/kqueue, Windows targets IOCP. See the policy in
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
Directory pages fit in 64 KiB; if a change can exceed that, add truncation
behavior and a test.

Uploads deserve extra care: require `Content-Length`, enforce the cap, stream
in fixed chunks, unlink partial files, and keep the feature behind `--upload`.
The multipart parser must stay incremental: never buffer a whole body, add
chunk-split tests for every state transition, keep the 32 KiB buffer bound, and
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
```

## Security

Please read [SECURITY.md](SECURITY.md). Do not open a public issue for a likely
vulnerability; use the private advisory channel.

## License

By contributing, you agree that your contribution is licensed under the MIT
license in [LICENSE](LICENSE).
