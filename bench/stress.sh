#!/bin/sh
# High-concurrency survival/throughput check for the async server.
# usage: CONNECTIONS=64 DURATION=5s ./bench/stress.sh
set -eu

root="$(CDPATH= cd -- "$(dirname "$0")/.." && pwd)"
bin="${SYSWEB_BIN:-$root/target/release/syswebd}"
port="${PORT_STRESS:-19400}"
connections="${CONNECTIONS:-64}"
duration="${DURATION:-5s}"
work="${TMPDIR:-/tmp}/sysweb-stress"

for tool in wrk ab curl; do
    command -v "$tool" >/dev/null 2>&1 || {
        echo "stress: missing $tool" >&2
        exit 2
    }
done
[ -x "$bin" ] || {
    echo "stress: missing $bin (build release first)" >&2
    exit 2
}

rm -rf "$work"
mkdir -p "$work/root"
dd if=/dev/zero of="$work/root/file-1k.bin" bs=1024 count=1 2>/dev/null
dd if=/dev/zero of="$work/root/file-64k.bin" bs=1024 count=64 2>/dev/null
printf '<h1>stress</h1>' >"$work/root/index.html"

"$bin" --root "$work/root" --port "$port" --quiet >"$work/server.log" 2>&1 &
pid=$!
trap 'kill "$pid" 2>/dev/null || true; wait "$pid" 2>/dev/null || true' EXIT INT TERM

n=0
while [ "$n" -lt 50 ]; do
    if curl -fsS -o /dev/null "http://127.0.0.1:$port/file-1k.bin" 2>/dev/null; then
        break
    fi
    n=$((n + 1))
    sleep 0.1
done
[ "$n" -lt 50 ] || {
    echo "stress: server did not start" >&2
    cat "$work/server.log" >&2
    exit 1
}

echo "sysweb stress: ${connections} connections, ${duration}"
for file in file-1k.bin file-64k.bin; do
    echo "--- wrk $file"
    wrk -t4 -c"$connections" -d"$duration" --latency \
        "http://127.0.0.1:$port/$file" |
        grep -E 'Latency|50%|99%|Requests/sec|Transfer/sec'
done

echo "--- ab no-keep-alive, 1000 requests"
ab -n 2000 -c8 -s 30 \
    "http://127.0.0.1:$port/file-1k.bin" |
    grep -E 'Complete requests|Failed requests|Requests per second|Time per request'

if ! kill -0 "$pid" 2>/dev/null; then
    echo "stress: server exited during load" >&2
    cat "$work/server.log" >&2
    exit 1
fi
code=$(curl -sS -o /dev/null -w '%{http_code}' "http://127.0.0.1:$port/file-64k.bin")
[ "$code" = "200" ] || {
    echo "stress: server unhealthy after load (HTTP $code)" >&2
    exit 1
}
echo "stress: server survived; final HTTP $code"
