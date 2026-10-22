#!/bin/sh
# The service binary must not have writable global state. Rust does have
# `static`, `static mut`, function-local `static`, and `thread_local!`; a
# writable `.data`/`.bss` also prevents merging the remaining read-only
# sections into `.text` on Windows.
#
# `const` is allowed (immutable, usually placed in read-only data), and
# `&'static` lifetimes/string literals are not variables.
set -eu

roots="src spikes/windows-no-crt/src"
hits="$(grep -RInE '^[[:space:]]*(pub([[:space:]]*\([^)]*\))?[[:space:]]+)?(unsafe[[:space:]]+)?static[[:space:]]+(mut[[:space:]]+)?[A-Za-z_]' $roots 2>/dev/null || true)"
threads="$(grep -RInE 'thread_local![[:space:]]*\(' $roots 2>/dev/null || true)"
if [ -n "$threads" ]; then
    hits="${hits}${hits:+
}${threads}"
fi

if [ -n "$hits" ]; then
    echo "check-no-globals: writable/global static items found:" >&2
    printf '%s\n' "$hits" >&2
    echo "Use a caller-provided buffer, a const, or pass state explicitly." >&2
    exit 1
fi

echo "check-no-globals: OK (no service-side static/global variables)"
