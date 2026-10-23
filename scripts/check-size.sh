#!/bin/sh
# Binary size is a design goal, not an accident. Enforce a per-platform budget
# for native release binaries. Cross-compiled binaries are checked by the job
# that actually runs on the target.
set -eu

binary="${1:-target/release/syswebd}"

if [ ! -f "$binary" ]; then
    echo "check-size: missing binary: $binary" >&2
    exit 1
fi

os="$(uname -s)"
arch="$(uname -m)"
case "$os/$arch" in
Darwin/arm64)
    budget=114688 # 112 KiB
    ;;
Linux/x86_64)
    budget=73728 # 72 KiB; the async reactor is included in the main binary
    ;;
Linux/aarch64 | Linux/arm64)
    budget=73728 # 72 KiB
    ;;
*)
    echo "check-size: no budget for $os/$arch (skipped)"
    exit 0
    ;;
esac

size="$(wc -c < "$binary" | tr -d ' ')"
if [ "$size" -gt "$budget" ]; then
    echo "check-size: $binary is $size bytes, over the $budget-byte budget for $os/$arch" >&2
    exit 1
fi

echo "check-size: OK ($size B <= $budget B for $os/$arch)"
