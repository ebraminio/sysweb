#!/bin/sh
# macOS-only benchmark for the compact download spike.
# usage: ./spikes/download-c10k/bench.sh
set -eu

root_dir="$(CDPATH= cd -- "$(dirname "$0")/.." && pwd)"
root="${TMPDIR:-/tmp}/download-c10k-root"
port="${PORT:-19520}"
bin="$root_dir/download-c10k/target/release/download-c10k"

[ "$(uname -s)" = Darwin ] || {
    echo "bench: macOS only" >&2
    exit 2
}
for tool in wrk python3; do
    command -v "$tool" >/dev/null 2>&1 || {
        echo "bench: missing $tool" >&2
        exit 2
    }
done

ulimit -n 65535 2>/dev/null || true
cargo build --release --manifest-path "$root_dir/download-c10k/Cargo.toml" >/dev/null

rm -rf "$root"
mkdir -p "$root"
dd if=/dev/zero of="$root/file.bin" bs=1024 count=64 2>/dev/null

run_server() {
    "$bin" --root "$root" --port "$port" --max 12000 --backlog 8192 "$@" \
        >"$root/server.log" 2>&1 &
    server=$!
    trap 'kill "$server" 2>/dev/null || true; wait "$server" 2>/dev/null || true' EXIT INT TERM
    sleep 0.4
    head -1 "$root/server.log"
}

run_close() {
    echo "--- close, c1000, 64 KiB"
    wrk -t4 -c1000 -d5s -H 'Connection: close' --latency \
        "http://127.0.0.1:$port/file.bin" | tail -12
}

run_keepalive() {
    echo "--- keep-alive, c1000, 64 KiB"
    wrk -t4 -c1000 -d5s --latency \
        "http://127.0.0.1:$port/file.bin" | tail -12
}

run_server
run_close
kill "$server" 2>/dev/null || true
wait "$server" 2>/dev/null || true

run_server --keepalive
run_keepalive
kill "$server" 2>/dev/null || true
wait "$server" 2>/dev/null || true

echo "--- 10k idle connections"
"$bin" --root "$root" --port "$port" --max 12000 --backlog 8192 \
    >"$root/idle.log" 2>&1 &
server=$!
trap 'kill "$server" 2>/dev/null || true; wait "$server" 2>/dev/null || true' EXIT INT TERM
sleep 0.4
python3 - "$port" <<'PY'
import socket, sys, time
port = int(sys.argv[1])
sockets = []
for i in range(10000):
    s = socket.socket()
    s.settimeout(2)
    try:
        s.connect(("127.0.0.1", port))
    except Exception as error:
        print(f"connect failed at {i}: {error}")
        break
    sockets.append(s)
print(f"idle connections accepted: {len(sockets)}")
time.sleep(0.5)
for s in sockets:
    s.close()
PY
kill "$server" 2>/dev/null || true
wait "$server" 2>/dev/null || true
