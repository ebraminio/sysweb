# Coroutine connection spike

An isolated experiment: what would a coroutine-per-connection handler look like,
and is it a better fit than the explicit state machine?

It is not production code and is not linked into `syswebd`.

## What it does

- one `#[coroutine]` handler with `yield Need::Read` / `yield Need::Write`;
- one explicit `enum Step { Read, Write, Done }` handler that does the same work;
- a tiny fake `Io` whose first read/write returns `WouldBlock`, so both paths
  actually suspend;
- a reactor loop that resumes the coroutine with `CoroutineState`;
- prints poll count and `size_of` for both handlers.

The fake `Io` toggles readiness itself to keep the harness small. A real reactor
would set readiness from epoll/kqueue/WSAPoll; that is exactly where the
borrowing question becomes harder.

## Run

Stable Rust does not expose the feature, so either use nightly:

```sh
cargo +nightly run --manifest-path spikes/coroutine/Cargo.toml
```

or, with a compiler that supports it behind the bootstrap gate:

```sh
RUSTC_BOOTSTRAP=1 cargo run --manifest-path spikes/coroutine/Cargo.toml
```

## Output

```text
  coroutine wants read
  coroutine wants write
coroutine: 3 polls
coroutine object: 280 bytes
response: HTTP/1.1 200 OK
Content-Length: 2

ok
  explicit wants read
  explicit wants write
explicit: 3 polls
explicit object: 88 bytes
response: HTTP/1.1 200 OK
Content-Length: 2

ok
```

The coroutine object includes the fake `Io` it owns (128-byte output plus
bookkeeping), so this is not a pure state-size comparison. Even so, the
generated object is **larger** than the explicit state in this toy.

## What it shows

The middle of the handler reads linearly:

```text
read request until CRLFCRLF
write response until done
```

There is no `State` enum and no per-transition helper.

## What it does not solve

### Normal helpers cannot yield

`read_exact(stream, buf)?` cannot suspend. Every operation that may return
`WouldBlock` needs its loop rewritten with `yield`, or wrapped in another
coroutine. That is the main readability cost, and it grows with uploads,
multipart, ranges, and listings.

### The coroutine type is unnameable

`pin!` keeps one coroutine on the stack, but a fixed `Slot` needs a field type.
The options are:

- make `Slot` generic over the coroutine type;
- use `Pin<Box<dyn Coroutine>>`, which needs a heap;
- pin-project a field with unsafe code and never move the slot.

This toy only exercises the first case (one stack-pinned local).

### Borrowing the reactor is the hard part

In a real handler the coroutine needs the platform, socket, and buffers. If it
captures `&mut platform` or `&mut io`, the outer reactor cannot also poll or set
interests. Passing them as resume arguments makes the coroutine lifetime tied to
the borrow, which is why this toy lets `Io` own its own readiness.

### State is hidden in the object

The compiler stores locals across yields, including buffers and offsets. The
generated size is not obvious and debugging shows compiler-generated coroutine
state rather than named states.

## Verdict

This is a useful readability prototype, not a production candidate yet. The
central protocol reads better, but helper composition, pinning, type erasure,
and reactor borrowing move the complexity elsewhere. The current explicit state
machine remains the better no-heap design.

`gen` blocks are a different feature: they are `Iterator`s and cannot receive
resume values, so they are a better fit for yielding listing fragments than for
I/O readiness.
