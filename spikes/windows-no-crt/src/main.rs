//! Minimal CRT-free Windows entry-point/link spike.
//!
//! This is not the Windows server backend. It proves that a `no_std` Rust
//! binary can provide its own entry point, link only `kernel32`, and exit
//! without msvcrt/ucrt or Rust startup code. The real backend remains IOCP
//! after the async reactor.

#![no_std]
#![no_main]
#![allow(non_snake_case)]

use core::ffi::c_void;
use core::panic::PanicInfo;
use core::ptr::null_mut;

const STD_OUTPUT_HANDLE: u32 = -11i32 as u32;

#[link(name = "kernel32")]
unsafe extern "system" {
    fn GetStdHandle(which: u32) -> *mut c_void;
    fn WriteFile(
        handle: *mut c_void,
        buffer: *const u8,
        length: u32,
        written: *mut u32,
        overlapped: *mut c_void,
    ) -> i32;
    fn ExitProcess(code: u32) -> !;
}

#[no_mangle]
pub extern "system" fn mainCRTStartup() -> ! {
    let message = b"sysweb windows no-CRT spike\n";
    let handle = unsafe { GetStdHandle(STD_OUTPUT_HANDLE) };
    let mut written = 0u32;
    let ok = unsafe {
        WriteFile(
            handle,
            message.as_ptr(),
            message.len() as u32,
            &mut written,
            null_mut(),
        )
    };
    unsafe { ExitProcess(if ok == 0 { 1 } else { 0 }) }
}

#[panic_handler]
fn panic(_: &PanicInfo) -> ! {
    unsafe { ExitProcess(101) }
}
