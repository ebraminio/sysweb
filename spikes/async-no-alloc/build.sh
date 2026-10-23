#!/bin/sh
set -eu

root="$(CDPATH= cd -- "$(dirname "$0")" && pwd)"
export RUSTUP_HOME="${RUSTUP_HOME:-$HOME/.rustup}"
export CARGO_HOME="${CARGO_HOME:-$HOME/.cargo}"
if command -v brew >/dev/null 2>&1; then
    llvm_prefix="$(brew --prefix llvm 2>/dev/null || true)"
    if [ -n "$llvm_prefix" ]; then
        PATH="$llvm_prefix/bin:$PATH"
        export PATH
    fi
fi
rustc_bin="$(rustup which rustc --toolchain stable)"
PATH="$(dirname "$rustc_bin"):$PATH"
export PATH

for target in x86_64-unknown-linux-gnu aarch64-unknown-linux-gnu x86_64-pc-windows-gnu; do
    case "$target" in
        *linux*) linker=ld.lld ;;
        *windows*) linker=x86_64-w64-mingw32-gcc ;;
        *) linker=cc ;;
    esac
    case "$target" in
        x86_64-unknown-linux-gnu) env_name=X86_64_UNKNOWN_LINUX_GNU ;;
        aarch64-unknown-linux-gnu) env_name=AARCH64_UNKNOWN_LINUX_GNU ;;
        x86_64-pc-windows-gnu) env_name=X86_64_PC_WINDOWS_GNU ;;
    esac
    echo "=== $target"
    env "CARGO_TARGET_${env_name}_LINKER=$linker" \
        cargo build --release --manifest-path "$root/Cargo.toml" --target "$target"
    binary="$root/target/$target/release/async-no-alloc.exe"
    [ -f "$binary" ] || binary="$root/target/$target/release/async-no-alloc"
    if command -v llvm-strip >/dev/null 2>&1; then
        llvm-strip --strip-all "$binary"
    fi
    ls -l "$binary"
    file "$binary"
done

echo "=== host"
cargo build --release --manifest-path "$root/Cargo.toml"
ls -l "$root/target/release/async-no-alloc"
