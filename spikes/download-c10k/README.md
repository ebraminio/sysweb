# Download-only C10k spike

A macOS-only experiment: what does a static download server look like if
uploads, listings, ranges, auth, and keep-alive are not promises?

It is not production code and is not linked into `syswebd`.

## Design

Each accepted connection is a compact record:

```text
stream fd
request buffer        2 KiB
response header        256 B
state (read/header/send)
file fd, offset, remaining
```

Measured `size_of::<Conn>()`: **2,360 bytes**.

The event loop is one kqueue reactor with a fixed `Vec<Option<Conn>>` of slot
records. There are no per-connection file copies:

- request headers are parsed into the 2 KiB buffer;
- the target file is opened;
- response headers are written;
- `sendfile` streams the file;
- the connection is closed.

With `--keepalive`, the same record is reset for the next request. Without it,
the connection closes after one response. The default is no keep-alive.

## Run

```sh
cargo build --release --manifest-path spikes/download-c10k/Cargo.toml
spikes/download-c10k/target/release/download-c10k \
    --root /tmp/download-c10k-root --port 19520 --max 12000 --backlog 8192 \
    --keepalive
```

The benchmark helper:

```sh
./spikes/download-c10k/bench.sh
```

It runs 64 KiB downloads at 1,000 connections, with and without keep-alive,
then opens 10,000 idle connections and counts accepted sockets.

## Results

macOS arm64, loopback, one 64 KiB file, `wrk -t4 -c1000 -d5s`:

| Server / mode | Requests/sec | Transfer/sec | p99 |
| --- | ---: | ---: | ---: |
| spike, close | 11,240 | 705 MB/s | 186 ms |
| spike, keep-alive | 23,628 | 1.44 GB/s | 58 ms |
| sysweb 32 slots, close | 9,617 | 604 MB/s | 211 ms |
| sysweb 32 slots, keep-alive | 1,554 | 97 MB/s | 47 ms |

The sysweb keep-alive result is the fixed-slot idle-retention effect: 1,000
clients share 32 slots, and idle keep-alive connections hold slots until the
30-second timeout. The close benchmark is closer to an apples-to-apples
use of `sendfile`.

At 5,000 connections the results were not usable as a server benchmark:

- spike, close: 4,727 req/s with socket errors;
- spike, keep-alive: 1,446 req/s with many client read errors;
- 10,000 connections produced no usable request rate.

That is a client/macOS port and scheduling limit in this setup, not evidence
that the reactor cannot hold 10,000 sockets. The idle test did accept **10,000
connections** with the 12,000-slot table.

## What it shows

- A download connection does not need a 40 KiB request/upload/listing slot.
- The state that must survive a download is small: fd, file fd, offset,
  remaining, and a little header state.
- `sendfile` cannot be fire-and-forget: the reactor still needs writable
  events, completion, errors, and connection lifetime.
- Keep-alive is a major throughput win and a major slot-retention cost. This
  spike makes the trade explicit.

## What it does not settle

- no real HTTP parser: it ignores methods other than `GET`/`HEAD`, ranges,
  `Connection: close` parsing, pipelining, and error responses;
- `HEAD` sends a body in this spike;
- std and `Vec` are used for the harness; production would need an arena or
  fixed table;
- no Windows/IOCP or cross-platform comparison;
- the 5k/10k request-rate numbers are not trustworthy on this client.

The next production-shaped step would be a segmented design: compact
connection records, a bounded upload/list pool, and an arena for the slot
records. That is a larger change than this spike.
