#!/bin/sh
# Download the latest stable release asset for this platform and run it.
# Usage: curl -fsSL .../scripts/run-latest.sh | sh -s -- --root ./public
set -eu

repo="ebraminio/sysweb"
os=$(uname -s)
arch=$(uname -m)
case "$os:$arch" in
    Linux:x86_64|Linux:amd64)
        asset="sysweb-linux-x86_64"
        ;;
    Linux:aarch64|Linux:arm64)
        asset="sysweb-linux-aarch64"
        ;;
    Darwin:arm64)
        asset="sysweb-macos-arm64"
        ;;
    *)
        echo "sysweb: unsupported platform $os/$arch" >&2
        exit 1
        ;;
esac

url="https://github.com/$repo/releases/latest/download/$asset"
tmp="${TMPDIR:-/tmp}/sysweb-latest.$$"
trap 'rm -f "$tmp"' EXIT INT TERM
curl -fsSL "$url" -o "$tmp"
chmod +x "$tmp"
"$tmp" "$@"
