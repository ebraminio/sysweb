#!/bin/sh
# Verify that the release binary has no unexpected runtime dependencies or
# undefined symbols. Intended for Linux and macOS CI, and for local release
# checks.
set -eu

binary="${1:-target/release/syswebd}"

if [ ! -f "$binary" ]; then
    echo "check-binary: missing binary: $binary" >&2
    exit 1
fi

fail() {
    echo "check-binary: $*" >&2
    exit 1
}

echo "== checking $binary"
ls -l "$binary"
file "$binary"

os="$(uname -s)"
case "$os" in
Darwin)
    echo "--- load commands"
    otool -L "$binary"
    deps="$(otool -L "$binary" | tail -n +2 | awk '{print $1}' | sed '/^$/d')"
    [ "$deps" = "/usr/lib/libSystem.B.dylib" ] || fail "unexpected dylib dependencies: $deps"
    if nm -u "$binary" 2>/dev/null | grep -q .; then
        fail "undefined symbols found (macOS)"
    fi
    ;;
Linux)
    echo "--- ELF"
    readelf -h "$binary" | grep -q 'Type:.*EXEC' || fail "not a static EXEC ELF"
    if readelf -l "$binary" | grep -q 'INTERP'; then
        fail "binary has a PT_INTERP (dynamic loader)"
    fi
    if readelf -d "$binary" 2>/dev/null | grep -q 'NEEDED'; then
        fail "binary has DT_NEEDED entries"
    fi
    if nm -u "$binary" 2>/dev/null | grep -q .; then
        fail "undefined symbols found (Linux)"
    fi
    ;;
*)
    fail "unsupported OS for this check: $os"
    ;;
esac

echo "check-binary: OK (no external runtime dependencies, no undefined symbols)"
