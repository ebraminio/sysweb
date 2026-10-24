#!/bin/sh
# Linux-only io_uring download benchmark.
# Requires liburing headers/libs; set LIBURING_PREFIX if not /tmp/liburing.
set -eu

here="$(CDPATH= cd -- "$(dirname "$0")" && pwd)"
port="${PORT:-19540}"
bin="$here/server"
root="${TMPDIR:-/tmp}/io-uring-root"
liburing="${LIBURING_PREFIX:-/tmp/liburing}"

[ "$(uname -s)" = Linux ] || {
    echo "bench: Linux only" >&2
    exit 2
}
command -v ab >/dev/null 2>&1 || {
    echo "bench: ab missing" >&2
    exit 2
}

ulimit -n 65535 2>/dev/null || true
cc="${CC:-gcc}"
"$cc" -O2 -Wall -Wextra -o "$bin" "$here/server.c" \
    -I"$liburing/include" -L"$liburing/lib" -luring \
    -Wl,-rpath,"$liburing/lib"

rm -rf "$root"
mkdir -p "$root"
dd if=/dev/zero of="$root/file.bin" bs=1024 count=1 2>/dev/null

"$bin" --root "$root" --port "$port" --max 12000 --backlog 8192 \
    >"$root/server.log" 2>&1 &
server=$!
trap 'kill "$server" 2>/dev/null || true; wait "$server" 2>/dev/null || true' EXIT INT TERM
sleep 0.5
head -1 "$root/server.log"

echo "--- 1 KiB, 10,000 connections"
ab -n 100000 -c 10000 -s 30 "http://127.0.0.1:$port/file.bin" |
    grep -E 'Complete requests|Failed requests|Requests per second|Time per request|Transfer rate'

dd if=/dev/zero of="$root/file.bin" bs=1024 count=64 2>/dev/null
echo "--- 64 KiB, 1,000 connections"
ab -n 20000 -c 1000 -s 30 "http://127.0.0.1:$port/file.bin" |
    grep -E 'Complete requests|Failed requests|Requests per second|Time per request|Transfer rate'

echo "--- 64 KiB, 5,000 connections"
ab -n 30000 -c 5000 -s 30 "http://127.0.0.1:$port/file.bin" |
    grep -E 'Complete requests|Failed requests|Requests per second|Time per request|Transfer rate'
