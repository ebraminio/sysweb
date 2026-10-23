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

- sysweb: release build, async default, 8 fixed slots, `Connection: close`
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
| 1 KiB | 18,311 | 36,246 | 37,024 |
| 64 KiB | 16,258 | 29,981 | 23,830 |
| 1 MiB | 4,674 | 6,814 | 4,011 |

sysweb cannot reuse connections, so this table is not a fair keep-alive
comparison. It is included because it is how a browser or proxy normally talks
to nginx/Caddy.

No-keep-alive pass (`ab -n 2000/200 -c8`, no `-k`), requests/second:

| File | sysweb | nginx | Caddy |
| ---: | ---: | ---: | ---: |
| 1 KiB | 17,366 | 24,583 | 15,729 |
| 64 KiB | 16,394 | 18,312 | 13,436 |
| 1 MiB | 4,252 | 3,152 | 3,535 |

The 1 MiB `ab` numbers are noisy and should not be read as a sysweb win.
Loopback, client CPU, page-cache state, and `ab`'s own per-request overhead
matter more at that size.

These are single representative runs; loopback results vary between runs.

## Honest reading

- sysweb is roughly within **2x** of nginx on the keep-alive pass and
  competitive when every request needs a fresh connection.
- It is one thread, eight slots, no TLS, no compression, no keep-alive, and no
  caching layer. The result is acceptable for a small localhost file server,
  not evidence that it replaces nginx or Caddy.
- nginx has one worker here; Caddy uses its default threading. That is another
  difference, not hidden by the numbers.
- These are loopback/page-cache numbers. Real disks, TLS, and many concurrent
  clients would change them.

## What to prioritize

1. **Keep-alive.** It is the largest missing performance feature and explains
   much of the gap on small files.
2. **Repeatable load scenarios** with a slow client and concurrent
   uploads/downloads; this is the next meaningful async validation.
3. **TLS/compression/caching** only if the project wants to compete on
   feature breadth; they are separate goals.
