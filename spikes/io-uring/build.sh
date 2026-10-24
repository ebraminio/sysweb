#!/bin/sh
set -eu
here="$(CDPATH= cd -- "$(dirname "$0")" && pwd)"
cc="${CC:-gcc}"
liburing="${LIBURING_PREFIX:-/tmp/liburing}"
"$cc" -O2 -Wall -Wextra -o "$here/server" "$here/server.c" \
    -I"$liburing/include" -L"$liburing/lib" -luring \
    -Wl,-rpath,"$liburing/lib"
