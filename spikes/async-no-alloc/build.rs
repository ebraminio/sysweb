fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    match std::env::var("CARGO_CFG_TARGET_OS").as_deref() {
        Ok("linux") => {
            for arg in ["-nostdlib", "-static", "-no-pie"] {
                println!("cargo:rustc-link-arg-bins={arg}");
            }
        }
        Ok("macos") => {
            println!("cargo:rustc-link-arg-bins=-lSystem");
        }
        Ok("windows") => {
            let args: &[&str] = match std::env::var("CARGO_CFG_TARGET_ENV").as_deref() {
                Ok("msvc") => &[
                    "/NODEFAULTLIB",
                    "/ENTRY:mainCRTStartup",
                    "/SUBSYSTEM:CONSOLE,6.0",
                    "/MACHINE:X64",
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
                println!("cargo:rustc-link-arg-bins={arg}");
            }
        }
        _ => {}
    }
}
