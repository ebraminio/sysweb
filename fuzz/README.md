# Fuzzing

`cargo-fuzz`/libFuzzer targets for the allocation-free parsers:

| Target | Input |
| --- | --- |
| `http` | arbitrary request bytes into `http::parse` |
| `path` | arbitrary targets into `path::normalize`/`append_index`, asserting the `..`-free invariant |
| `multipart` | arbitrary `multipart/form-data` bodies in 13-byte chunks into the streaming parser |

The targets use a null sink, so they never touch the filesystem. They catch
panics, integer/bounds bugs, and parser-state mistakes. The black-box
containment/hang/crash test lives in `tests/security.rs` and runs with the
normal suite.

## Local

```sh
cargo install cargo-fuzz
cargo +nightly fuzz build
mkdir -p fuzz/corpus/http fuzz/corpus/path fuzz/corpus/multipart
cp fuzz/seeds/http/* fuzz/corpus/http/
cp fuzz/seeds/path/* fuzz/corpus/path/
cp fuzz/seeds/multipart/* fuzz/corpus/multipart/
cargo +nightly fuzz run http fuzz/corpus/http
cargo +nightly fuzz run path fuzz/corpus/path
cargo +nightly fuzz run multipart fuzz/corpus/multipart
```

Seeds are in `fuzz/seeds/`; copy them into the gitignored `fuzz/corpus/`
before fuzzing so the committed seeds stay pristine.

## CI

The `fuzz-smoke` job in `.github/workflows/ci.yml` builds all three targets on
nightly and runs each for 20 seconds with a 5-second per-input timeout. It pins
`--target` to the triple reported by `rustc -vV` because the `install-action`
`cargo-fuzz` binary is musl-static and would otherwise default to
`x86_64-unknown-linux-musl`, which is not installed and cannot take ASan. The
normal `cargo test` job also runs `tests/security.rs`, which sends hundreds of
deterministic traversal, symlink, malformed, and randomized requests to the real
binary and checks for status bounds, timeouts, crashes, and secret leakage.

## OSS-Fuzz

The layout follows the cargo-fuzz model used by OSS-Fuzz. To onboard the
project into OSS-Fuzz proper, a `google/oss-fuzz` project entry needs a
`project.yaml` and a `build.sh` that runs `cargo fuzz build` in this directory;
after that, the [OSS-Fuzz continuous integration](https://google.github.io/oss-fuzz/getting-started/continuous-integration/)
workflow / CIFuzz can run these targets on pull requests. The local CI smoke
job is intentionally self-contained so it works before that onboarding.
