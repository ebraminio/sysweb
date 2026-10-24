# io_uring download spike

A Linux-only experiment: a download-only file server with a compact
per-connection record and a single io_uring completion loop.

It is not production code and is not linked into `syswebd`.

## Why this shape

The goal was not another epoll reactor. It was to see what file serving looks
like when the event API is completion-based:

- one submission queue and one completion queue;
- `IORING_OP_ACCEPT`, `IORING_OP_RECV`, `IORING_OP_READ`, `IORING_OP_SEND`;
- no per-connection poll registration;
- one state machine driven by `struct io_uring_cqe`.

Linux io_uring has **no `IORING_OP_SENDFILE`** in the 6.12 headers. This spike
therefore reads the file into an 8 KiB buffer and submits `SEND`. The zero-copy
alternative would be `SPLICE` through a pipe: two operations and two extra fds
per connection. That is a different spike.

## Design

```text
Conn:
    socket fd
    2 KiB request buffer
    256 B response header
    8 KiB body buffer
    file fd, offset, remaining
    state
```

`sizeof(struct conn)` is **10,568 bytes**. At 12,000 records that is about
**126 MiB**; at the benchmarked 10,000 connections it is about **105 MiB**.

No keep-alive: the server sends `Connection: close`, streams one file, and
closes. The reactor remains alive for the next connection.

## Build and run

```sh
# example: build liburing locally, without root
curl -L -o /tmp/liburing.tar.gz \
  https://github.com/axboe/liburing/archive/refs/tags/liburing-2.9.tar.gz
tar -xzf /tmp/liburing.tar.gz -C /tmp
cd /tmp/liburing-liburing-2.9
./configure --prefix=/tmp/liburing
make -j4 && make install

cd /path/to/sysweb
./spikes/io-uring/build.sh
./spikes/io-uring/server --root /tmp/io-uring-root \
    --port 19530 --max 12000 --backlog 8192
```

Benchmark:

```sh
./spikes/io-uring/bench.sh
```

## Results

Debian 13, kernel 6.12, 20 cores, loopback, `ab` without keep-alive:

| Workload | Connections | Requests/sec | Transfer rate |
| --- | ---: | ---: | ---: |
| 1 KiB | 10,000 | 39,287 | 41.6 MB/s |
| 1 KiB | 1,000 | 46,133 | 49.1 MB/s |
| 64 KiB | 1,000 | 16,015 | 1.00 GB/s |
| 64 KiB | 5,000 | 15,149 | 970 MB/s |

All runs reported **0 failed requests**. The 10,000-connection run completed
100,000 requests.

For comparison, the production `syswebd` on the same host has only 32 fixed
slots. At `ab -c1000` it failed 19,872 of 20,000 because the client concurrency
far exceeds its slot table; that is a capacity limit, not an io_uring versus
epoll throughput result.

## API comparison

### io_uring

- userspace submits SQEs and consumes CQEs;
- operations carry `user_data` for connection/state routing;
- socket and file operations share one ring;
- no per-fd poll set;
- no `SENDFILE` opcode in this kernel; use read+send or splice;
- completion order is not connection order; state must be explicit.

### XDP / eBPF

- XDP runs in the NIC/driver receive path and can pass, drop, redirect, or
  modify packets;
- eBPF is verified bytecode with maps/helpers, not a file or socket API;
- neither can serve a file: there is no file descriptor, page cache, or TCP
  stack in the XDP program;
- XDP requires root/CAP_NET_ADMIN/CAP_BPF and a suitable NIC/driver.

### AF_XDP

- delivers raw Ethernet frames through UMEM plus fill/completion/RX/TX rings;
- needs an XDP redirect program and a userspace TCP/IP stack to speak HTTP;
- therefore a file-serving comparison is not a library swap: it is a
  user-space TCP stack plus file I/O;
- needs root and driver support.

The remote host used for this spike is a Wi-Fi-only Debian machine:
`wlp0s20f3`, no root, `unprivileged_bpf_disabled=2`. XDP, eBPF, and AF_XDP
could not be loaded there. The API comparison above is therefore analytical;
only io_uring was measured.

## What this settles

- A completion-based file server can reach 10k simultaneous connections on
  this host with compact state and no keep-alive.
- io_uring keeps the protocol loop readable: the completion handler is a small
  switch over operations.
- The absence of a sendfile opcode matters: read+send needs an 8 KiB body
  buffer per active connection; splice needs pipes and two completions.
- For pure file serving, the kernel network stack plus io_uring is a much
  smaller project than AF_XDP plus a TCP stack.

## What it does not settle

- no keep-alive, ranges, error responses, uploads, listings, or pipelining;
- only `GET`, single file path, no HTTP parser hardening;
- one thread; no multi-ring or registered-buffer optimization;
- io_uring performance depends on kernel, filesystem, page cache, and queue
  depth; these are one-host loopback numbers;
- XDP/AF_XDP performance remains unmeasured on this host.
