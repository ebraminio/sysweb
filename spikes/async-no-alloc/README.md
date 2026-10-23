# No-alloc async/await spike

An isolated experiment: can Rust `async`/`await` run with **no allocator, no
libc, no CRT**, and is its state-machine cost acceptable for the server?

It is not production code and is not linked into `syswebd`.

## What it does

- `#![no_std]` binary with a hand-written entry point;
- a fixed-slot executor (`run_slots`) that polls an array of futures;
- a `block_on` path for one future;
- a no-op `RawWaker` built without `Arc` or allocation;
- an `async fn` with two await points;
- a host test that measures the future size.

The executor is deliberately eager-polling. It has no I/O and no reactor, and
the no-op waker means it does not test event-driven wakeups. That is the next
question, not this spike's claim.

## Results

Host tests:

```text
async demo future size: 16 bytes
3 tests passed
```

Stripped/static builds:

| Target | Size | Imports |
| --- | ---: | --- |
| Linux x86_64 | 1,296 B | none (static, no symbols) |
| Linux aarch64 | 1,216 B | none (static, no symbols) |
| Windows x86_64 | 3,584 B | `KERNEL32.dll!ExitProcess` only |
| macOS arm64 | 17,584 B | no symbols; libSystem load command only |

The macOS size is mostly the mandatory libSystem load command and Mach-O
overhead, not the executor.

`build.sh` uses the persistent `~/.rustup`/`~/.cargo` toolchain and cross
targets. The Windows and macOS binaries exit 0; the Linux binaries are static
and have no undefined symbols.

## What this does and does not settle

It settles the **size** question: async/await itself is not inherently large.
A 16-byte future and a small executor are entirely compatible with the binary
budget.

It does **not** settle the hard part: a real request future needs to borrow
per-connection buffers across await points, while future and connection live
in fixed slots. That is where self-reference, pinning, lifetimes, and unsafe
projection can become worse than the hand-written state machine. It also does
not test a token waker or a reactor.

Current production bias remains the explicit state machine. This spike makes
async/await a credible fallback if a real connection-future prototype stays
simple and small.

## Build

```sh
rustup target add x86_64-unknown-linux-gnu aarch64-unknown-linux-gnu x86_64-pc-windows-gnu
./spikes/async-no-alloc/build.sh
cargo test --manifest-path spikes/async-no-alloc/Cargo.toml
```
