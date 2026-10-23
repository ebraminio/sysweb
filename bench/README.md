# Localhost static-file benchmark

This is a small sanity check, not a feature-parity benchmark. sysweb is a
single-threaded, no_std, no-alloc static server; nginx and Caddy are mature
servers with keep-alive, TLS, compression, caching, worker pools, and many
features sysweb does not have.

The point is to show where a deliberately constrained server stands on the
simplest case: serving a file from localhost with the file already in page
cache.

## Method

Measured on macOS 27.0.1, 10 CPUs, with:

- sysweb: release build, async default, 8 fixed slots, HTTP/1.1 keep-alive
- nginx: 1.31.8, one worker, `sendfile on`
- Caddy: 2.11.7, `file-server`, defaults
- clients: `wrk` 4.2.0 for the keep-alive pass, `ab` for the
  no-keep-alive pass
- all servers on `127.0.0.1`, same 1 KiB / 64 KiB / 1 MiB files, no TLS

Run:

```sh
./bench/run.sh
```

The harness creates a temporary root, generates an nginx config with temporary
paths, starts sysweb/nginx/Caddy, and tears everything down.

## Results

Keep-alive pass (`wrk -t2 -c8 -d3s`), requests/second:

| File | sysweb | nginx | Caddy |
| ---: | ---: | ---: | ---: |
| 1 KiB | 26,415 | 32,684 | 34,796 |
| 64 KiB | 24,248 | 27,276 | 23,869 |
| 1 MiB | 7,099 | 7,136 | 4,465 |

All three servers can now reuse connections in this pass. sysweb is within
about 20% of nginx on 1 KiB, effectively level on 64 KiB/1 MiB, and ahead of
Caddy on the larger files in this run.

No-keep-alive pass (`ab -n 2000/200 -c8`, no `-k`), requests/second:

| File | sysweb | nginx | Caddy |
| ---: | ---: | ---: | ---: |
| 1 KiB | 20,008 | 22,950 | 17,990 |
| 64 KiB | 18,246 | 16,323 | 12,936 |
| 1 MiB | 4,105 | 4,077 | 3,381 |

The 1 MiB numbers are noisy and should not be read as a sysweb win.
Loopback, client CPU, page-cache state, and `ab`'s own per-request overhead
matter more at that size.

These are single representative runs; loopback results vary between runs.

## Honest reading

- sysweb is now close to nginx on the keep-alive pass and competitive when
  every request needs a fresh connection; on this run it is level on 64 KiB
  and 1 MiB.
- It is one thread, eight slots, no TLS, no compression, no keep-alive, and no
  caching layer. The result is acceptable for a small localhost file server,
  not evidence that it replaces nginx or Caddy.
- nginx has one worker here; Caddy uses its default threading. That is another
  difference, not hidden by the numbers.
- These are loopback/page-cache numbers. Real disks, TLS, and many concurrent
  clients would change them.

## Raspberry Pi idle and 2 GiB download

See [pi/README.md](pi/README.md). Short version: idle CPU is below the
0.033%-of-one-core sampling resolution for all three servers. During a 2 GiB
LAN download, sysweb used 0.58 CPU-seconds (1.19% of one core), nginx 0.15 s,
and Caddy 3.14 s; throughput was network-bound.

## What to prioritize

1. **Repeatable load scenarios** with slow clients and concurrent
   uploads/downloads; this is the next meaningful async validation.
2. **Keep-alive edge behavior** (pipelining limits, idle expiry, uploads) under
   longer soak tests.
3. **TLS/compression/caching** only if the project wants to compete on
   feature breadth; they are separate goals.
