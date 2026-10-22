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

    # The libSystem ABI is the stable macOS interface. Allowlist exactly the
    # functions this backend calls (plus dyld's stub binder).
    allowed=" __error _exit accept bind close closedir fdopendir fstat fstatat listen mkdirat nanosleep openat read readdir send sendfile setsockopt socket unlinkat write dyld_stub_binder "
    imports="$(nm -u "$binary" 2>/dev/null || true)"
    [ -n "$imports" ] || fail "no libSystem imports found (unexpected)"
    for symbol in $imports; do
        name="${symbol#_}"
        case " $allowed " in
        *" $name "*) ;;
        *) fail "unexpected macOS import: $symbol" ;;
        esac
    done
    echo "macOS imports (allowlisted libSystem ABI):"
    printf '%s\n' "$imports"
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

echo "check-binary: OK (dependency and ABI checks passed)"
