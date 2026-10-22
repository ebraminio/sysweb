#!/bin/sh
# The server must not depend on third-party crates. `cargo tree --depth 1`
# should contain exactly the sysweb package itself.
set -eu

tree="$(cargo tree --depth 1 --prefix none | sed '/^[[:space:]]*$/d')"
count="$(printf '%s\n' "$tree" | wc -l | tr -d ' ')"

if [ "$count" -ne 1 ] || ! printf '%s\n' "$tree" | grep -q '^sysweb '; then
    echo "check-no-deps: unexpected dependency tree:" >&2
    printf '%s\n' "$tree" >&2
    exit 1
fi

echo "check-no-deps: OK"
printf '%s\n' "$tree"
