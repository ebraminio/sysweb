# Raspberry Pi idle and large-file measurements

These measurements answer two practical questions that the localhost request
benchmark does not:

1. What does an idle server cost?
2. What does one multi-gigabyte download cost in server CPU?

They are single runs, not statistically controlled experiments. Network,
page-cache, and client CPU matter.

## Setup

- Raspberry Pi 5, 4 cores, 8 GiB RAM, Debian 12, kernel 6.12 aarch64
- server files in `/tmp/pi-bench/root`
- sysweb: current aarch64 release, async default, `--quiet`
- nginx: 1.22.1, one worker, `sendfile on`
- Caddy: 2.11.7, `file-server` defaults
- client: another machine on the LAN running curl; the 2 GiB file was already
  in page cache
- CPU ticks from `/proc/<pid>/stat` (`utime + stime`), `CLK_TCK=100`

`bench/pi/measure-cpu.sh` samples aggregate CPU ticks for one or more PIDs.

## Idle CPU

Each server was left with no traffic for 30 seconds:

| Server | CPU ticks | CPU |
| --- | ---: | ---: |
| sysweb | 0 | < 0.033% of one core |
| nginx | 0 | < 0.033% of one core |
| Caddy | 0 | < 0.033% of one core |

The 1-second sysweb poll tick does not show up at this resolution. An idle
sysweb process is not measurably busier than nginx or Caddy.

## 2 GiB download

A 2,147,483,648-byte file was downloaded from the Pi over the LAN with
`curl -o /dev/null`. Server CPU is the process CPU time accumulated during the
transfer.

| Server | Time | Throughput | Server CPU | CPU, one core |
| --- | ---: | ---: | ---: | ---: |
| sysweb | 48.82 s | 43.99 MB/s | 0.58 s | 1.19% |
| nginx | 49.28 s | 43.57 MB/s | 0.15 s | 0.30% |
| Caddy | 44.58 s | 48.17 MB/s | 3.14 s | 7.04% |

Read this honestly:

- Throughput was network-bound; the three servers are within normal LAN
  variation of each other.
- sysweb used more CPU than nginx for the transfer, but far less than Caddy:
  roughly **0.29 CPU-seconds per GiB** for sysweb, 0.075 for nginx, and 1.57
  for Caddy.
- Caddy's multi-threaded default does more work per byte here; sysweb is
  single-threaded and uses `sendfile`.
- This does not measure TLS, compression, many concurrent clients, or cold
  disk reads.

## Reproduce

Build and copy the aarch64 binary, install nginx/Caddy, then create the file:

```sh
fallocate -l 2G /tmp/pi-bench/root/big.bin
```

Start one server (example: sysweb), sample idle CPU:

```sh
/tmp/pi-bench/syswebd --root /tmp/pi-bench/root --port 19080 --quiet &
pid=$!
./bench/pi/measure-cpu.sh 30 "$pid"
```

For the download, record ticks before and after from the client side:

```sh
pid=$(ssh pi 'cat /tmp/pi-bench/sysweb-dl.pid')
ssh pi "awk '{print \$14+\$15}' /proc/$pid/stat"
curl -o /dev/null -w '%{size_download} %{time_total} %{speed_download}\n' \
    http://pi:19080/big.bin
ssh pi "awk '{print \$14+\$15}' /proc/$pid/stat"
```

Use `bench/pi/measure-cpu.sh` around a concurrent transfer if you prefer,
though it sleeps for a fixed interval rather than stopping exactly at the end
of the download.
