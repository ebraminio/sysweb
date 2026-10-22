#!/bin/sh
set -eu

target=x86_64-pc-windows-gnu
root="$(CDPATH= cd -- "$(dirname "$0")" && pwd)"
export CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER="${CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER:-x86_64-w64-mingw32-gcc}"

cargo build --release --manifest-path "$root/Cargo.toml" --target "$target"
binary="$root/target/$target/release/windows-no-crt-spike.exe"
if command -v llvm-strip >/dev/null 2>&1; then
    llvm-strip --strip-all "$binary"
fi
file "$binary"

if command -v llvm-readobj >/dev/null 2>&1; then
    llvm-readobj --file-headers --coff-imports "$binary"
fi
