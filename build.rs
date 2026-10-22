//! Linker setup for the freestanding server binary.
//!
//! These arguments apply to bins only (`rustc-link-arg-bins`), never to the
//! test harness, which still links the host libc/std.

fn main() {
    println!("cargo:rerun-if-changed=build.rs");

    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();

    match target_os.as_str() {
        "linux" => {
            // No crt1.o/crti.o, no -lc, no -lgcc: only our objects and the
            // Rust core/compiler_builtins rlibs. _start lives in the binary.
            // `-nostdlib` implies `-nostartfiles` for both gcc and lld.
            for arg in ["-nostdlib", "-static", "-no-pie"] {
                println!("cargo:rustc-link-arg-bins={arg}");
            }
        }
        "macos" => {
            // ld64 refuses to produce a dynamic executable without a
            // libSystem load command. We import no symbols from it (mem* is
            // provided by the binary), but the LC_LOAD_DYLIB is mandatory.
            println!("cargo:rustc-link-arg-bins=-lSystem");
        }
        _ => {}
    }
}
