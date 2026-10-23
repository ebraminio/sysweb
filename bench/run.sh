#!/bin/sh
# Reproducible localhost static-file comparison. Not a feature-parity test.
set -eu

root="$(CDPATH= cd -- "$(dirname "$0")/.." && pwd)"
work="${TMPDIR:-/tmp}/sysweb-bench"
bin="${SYSWEB_BIN:-$root/target/release/syswebd}"
port_sysweb="${PORT_SYSWEB:-19080}"
port_nginx="${PORT_NGINX:-19081}"
port_caddy="${PORT_CADDY:-19082}"

for tool in wrk ab nginx caddy curl; do
    if ! command -v "$tool" >/dev/null 2>&1; then
        echo "bench: missing $tool" >&2
        exit 2
    fi
done

if [ ! -x "$bin" ]; then
    (cd "$root" && cargo build --release)
    bin="$root/target/release/syswebd"
fi

rm -rf "$work"
mkdir -p "$work/root" "$work/nginx/client_temp" "$work/nginx/proxy_temp" \
    "$work/nginx/fastcgi_temp" "$work/nginx/uwsgi_temp" "$work/nginx/scgi_temp"
dd if=/dev/zero of="$work/root/file-1k.bin" bs=1024 count=1 2>/dev/null
dd if=/dev/zero of="$work/root/file-64k.bin" bs=1024 count=64 2>/dev/null
dd if=/dev/zero of="$work/root/file-1m.bin" bs=1024 count=1024 2>/dev/null

cat > "$work/nginx/nginx.conf" <<CONF
daemon off;
worker_processes 1;
pid $work/nginx/nginx.pid;
error_log $work/nginx/error.log;
events { worker_connections 1024; }
http {
  access_log off;
  sendfile on;
  client_body_temp_path $work/nginx/client_temp;
  proxy_temp_path $work/nginx/proxy_temp;
  fastcgi_temp_path $work/nginx/fastcgi_temp;
  uwsgi_temp_path $work/nginx/uwsgi_temp;
  scgi_temp_path $work/nginx/scgi_temp;
  server { listen 127.0.0.1:$port_nginx; root $work/root; }
}
CONF

pids=""
cleanup() {
    for pid in $pids; do kill "$pid" 2>/dev/null || true; done
    wait 2>/dev/null || true
}
trap cleanup EXIT INT TERM

"$bin" --root "$work/root" --port "$port_sysweb" --quiet >"$work/sysweb.log" 2>&1 &
pids="$pids $!"
nginx -e "$work/nginx/error.log" -c "$work/nginx/nginx.conf" -p "$work/nginx" \
    >"$work/nginx/stdout.log" 2>&1 &
pids="$pids $!"
caddy file-server --root "$work/root" --listen "127.0.0.1:$port_caddy" \
    >"$work/caddy.log" 2>&1 &
pids="$pids $!"

wait_for() {
    port=$1
    n=0
    while [ "$n" -lt 50 ]; do
        if curl -fsS -o /dev/null "http://127.0.0.1:$port/file-1k.bin" 2>/dev/null; then
            return 0
        fi
        n=$((n + 1))
        sleep 0.1
    done
    echo "bench: server on port $port did not start" >&2
    cat "$work/sysweb.log" "$work/nginx/error.log" "$work/caddy.log" 2>/dev/null || true
    return 1
}
wait_for "$port_sysweb"
wait_for "$port_nginx"
wait_for "$port_caddy"

echo "sysweb benchmark (localhost, page cache, no TLS)"
echo "client: $(uname -sm), $(sysctl -n hw.ncpu 2>/dev/null || nproc) CPUs"
echo

echo "--- keep-alive, wrk -t2 -c8 -d3s (all servers reuse connections) ---"
for size in 1k 64k 1m; do
    for item in "sysweb:$port_sysweb" "nginx:$port_nginx" "caddy:$port_caddy"; do
        name=${item%%:*}
        port=${item#*:}
        printf '%-7s %-4s  ' "$name" "$size"
        wrk -t2 -c8 -d3s "http://127.0.0.1:$port/file-$size.bin" 2>&1 |
            sed -n 's/^Requests\/sec:[[:space:]]*//p'
    done
done

echo
echo "--- no keep-alive, ab -n N -c8 (Connection: close) ---"
for size in 1k 64k 1m; do
    case "$size" in
        1m) n=200 ;;
        *) n=2000 ;;
    esac
    for item in "sysweb:$port_sysweb" "nginx:$port_nginx" "caddy:$port_caddy"; do
        name=${item%%:*}
        port=${item#*:}
        printf '%-7s %-4s  ' "$name" "$size"
        ab -n "$n" -c 8 -s 30 "http://127.0.0.1:$port/file-$size.bin" 2>&1 |
            sed -n 's/^Requests per second:[[:space:]]*//p'
    done
done
