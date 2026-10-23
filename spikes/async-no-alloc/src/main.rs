#![no_std]
#![no_main]
#![allow(non_snake_case)]

use core::ffi::c_void;
use core::panic::PanicInfo;

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
core::arch::global_asm!(
    r#"
    .globl _start
    .type _start,@function
_start:
    and rsp, -16
    call async_entry
    mov edi, eax
    mov eax, 60
    syscall
    ud2
    .size _start, .-_start
    .section .note.GNU-stack,"",@progbits
    "#
);

#[cfg(all(target_os = "linux", target_arch = "aarch64"))]
core::arch::global_asm!(
    r#"
    .globl _start
    .type _start,%function
_start:
    bl async_entry
    mov x8, #93
    svc #0
    brk #0
    .size _start, .-_start
    .section .note.GNU-stack,"",%progbits
    "#
);

#[cfg(target_os = "linux")]
#[no_mangle]
pub extern "C" fn async_entry() -> i32 {
    async_no_alloc::run()
}

#[cfg(target_os = "macos")]
#[no_mangle]
pub extern "C" fn main() -> i32 {
    async_no_alloc::run()
}

#[cfg(target_os = "windows")]
#[link(name = "kernel32")]
unsafe extern "system" {
    fn ExitProcess(code: u32) -> !;
}

#[cfg(target_os = "windows")]
#[no_mangle]
pub extern "system" fn mainCRTStartup() -> ! {
    unsafe { ExitProcess(async_no_alloc::run() as u32) }
}

#[panic_handler]
fn panic(_: &PanicInfo) -> ! {
    #[cfg(target_os = "windows")]
    unsafe {
        ExitProcess(101)
    }
    #[cfg(not(target_os = "windows"))]
    loop {
        core::hint::spin_loop();
    }
}

#[no_mangle]
pub unsafe extern "C" fn memset(dest: *mut c_void, value: i32, count: usize) -> *mut c_void {
    let bytes = dest as *mut u8;
    let mut i = 0usize;
    while i < count {
        unsafe { core::ptr::write_volatile(bytes.add(i), value as u8) };
        i += 1;
    }
    dest
}

#[no_mangle]
pub unsafe extern "C" fn memcpy(
    dest: *mut c_void,
    src: *const c_void,
    count: usize,
) -> *mut c_void {
    let dst = dest as *mut u8;
    let src = src as *const u8;
    let mut i = 0usize;
    while i < count {
        unsafe { core::ptr::write_volatile(dst.add(i), core::ptr::read_volatile(src.add(i))) };
        i += 1;
    }
    dest
}
