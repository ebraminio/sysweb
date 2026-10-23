fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let args: &[&str] = match std::env::var("CARGO_CFG_TARGET_ENV").as_deref() {
        Ok("msvc") => &[
            "/NODEFAULTLIB",
            "/ENTRY:mainCRTStartup",
            "/SUBSYSTEM:CONSOLE,6.0",
            "/MACHINE:X64",
            "/OPT:REF",
            "/OPT:ICF",
        ],
        _ => &[
            "-nostdlib",
            "-Wl,--entry=mainCRTStartup",
            "-Wl,--subsystem,console",
            "-Wl,--major-subsystem-version,6",
            "-Wl,--minor-subsystem-version,0",
            "-Wl,--gc-sections",
            "-Wl,--no-insert-timestamp",
        ],
    };
    for arg in args {
        println!("cargo:rustc-link-arg-bins={arg}");
    }
}
