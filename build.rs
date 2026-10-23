//! Linker setup for the freestanding server binary.
//!
//! These arguments apply only to the `syswebd` binary
//! (`rustc-link-arg-bin`), never to test or fuzz binaries, which still link
//! the host libc/std and sanitizer runtimes.

fn main() {
    println!("cargo:rerun-if-changed=build.rs");

    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();

    match target_os.as_str() {
        "linux" => {
            // No crt1.o/crti.o, no -lc, no -lgcc: only our objects and the
            // Rust core/compiler_builtins rlibs. _start lives in the binary.
            // `-nostdlib` implies `-nostartfiles` for both gcc and lld.
            for arg in ["-nostdlib", "-static", "-no-pie"] {
                println!("cargo:rustc-link-arg-bin=syswebd={arg}");
            }
        }
        "windows" => {
            let args: &[&str] = match std::env::var("CARGO_CFG_TARGET_ENV").as_deref() {
                Ok("msvc") => &[
                    "/NODEFAULTLIB",
                    "/ENTRY:mainCRTStartup",
                    "/SUBSYSTEM:CONSOLE,6.0",
                    "/MACHINE:X64",
                    "/STACK:8388608,8388608",
                ],
                _ => &[
                    "-nostdlib",
                    "-Wl,--entry=mainCRTStartup",
                    "-Wl,--subsystem,console",
                    "-Wl,--major-subsystem-version,6",
                    "-Wl,--minor-subsystem-version,0",
                ],
            };
            for arg in args {
                println!("cargo:rustc-link-arg-bin=syswebd={arg}");
            }
        }
        "macos" => {
            // ld64 refuses to produce a dynamic executable without a
            // libSystem load command. We import no symbols from it (mem* is
            // provided by the binary), but the LC_LOAD_DYLIB is mandatory.
            println!("cargo:rustc-link-arg-bin=syswebd=-lSystem");
        }
        _ => {}
    }
}
