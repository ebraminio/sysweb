//! sysweb: a no_std, no-alloc, syscall-first static web server.
//!
//! Design constraints:
//! * `core` only: no `std`, no `alloc`, no third-party crates;
//! * on Linux the binary links no libc at all and enters via a hand-written
//!   `_start`;
//! * on macOS the binary makes raw BSD syscalls and imports no libc symbols;
//!   libSystem is linked only because ld64 requires the load command;
//! * `unsafe` is confined to the syscall ABI modules;
//! * every buffer is fixed-size and caller-provided.
//!
//! The server is deliberately small: HTTP/1.0 and 1.1, keep-alive, `GET`/`HEAD`,
//! uploads, and static files over a fixed-slot readiness reactor.

#![cfg_attr(not(test), no_std)]

pub mod async_server;
pub mod auth;
pub mod chat_server;
pub mod config;
pub mod fixedbuf;
pub mod http;
pub mod listing;
pub mod mime;
pub mod multipart;
pub mod path;
pub mod platform;
pub mod server;

pub use config::{Config, ParseOutcome};

/// Cargo package version, embedded at compile time.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

const MAX_ARGS: usize = 32;
const MAX_ARG_LEN: usize = 4096;

/// Parse argv and run the server. Never returns.
pub fn run(args: &[&[u8]]) -> ! {
    match Config::parse(args) {
        Ok(ParseOutcome::Help) => {
            platform::write_stdout(config::USAGE.as_bytes());
            platform::exit(0)
        }
        Ok(ParseOutcome::Version) => {
            platform::write_stdout(b"syswebd ");
            platform::write_stdout(VERSION.as_bytes());
            platform::write_stdout(b"\n");
            platform::exit(0)
        }
        Ok(ParseOutcome::Run(config)) => {
            if config.chat() {
                chat_server::run(platform::Sys::new(), config)
            } else {
                async_server::run(platform::Sys::new(), config)
            }
        }
        Err(error) => {
            platform::write_stderr(b"syswebd: ");
            platform::write_stderr(error.message());
            platform::write_stderr(b"\n\n");
            platform::write_stderr(config::USAGE.as_bytes());
            platform::exit(2)
        }
    }
}

/// Convert the C `argc`/`argv` pair into borrowed byte slices and run.
///
/// # Safety
///
/// `argv` must point to `argc` valid NUL-terminated strings, as guaranteed by
/// the kernel (Linux `_start`) or dyld (macOS `main`).
pub unsafe fn run_from_argv(argc: i32, argv: *const *const u8) -> ! {
    let mut args: [&[u8]; MAX_ARGS] = [&[]; MAX_ARGS];
    let mut count = 0usize;

    if argc > 0 && !argv.is_null() {
        let limit = core::cmp::min(argc as usize, MAX_ARGS);
        while count < limit {
            let pointer = unsafe { *argv.add(count) };
            if pointer.is_null() {
                break;
            }
            let mut len = 0usize;
            while len < MAX_ARG_LEN && unsafe { *pointer.add(len) } != 0 {
                len += 1;
            }
            args[count] = unsafe { core::slice::from_raw_parts(pointer, len) };
            count += 1;
        }
    }

    run(&args[..count])
}
