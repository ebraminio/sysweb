//! The freestanding server executable.
//!
//! There is no CRT, no libc, no `main` from std, and no global allocator.
//! The entry point is OS-specific:
//! * Linux: hand-written `_start` assembly, then `sysweb_entry(argc, argv)`;
//! * macOS: `_main` (the LC_MAIN entry point), called by dyld with the usual
//!   `argc, argv, envp, apple` arguments.
//!
//! `mem*` symbols are provided here because the binary does not link a C
//! runtime. They are deliberately simple; optimizing them is future work.

#![no_std]
#![no_main]
// The mem* functions below are C runtime shims whose contracts are the C
// library contracts; clippy's safety-doc lint is not useful for them.
#![allow(clippy::missing_safety_doc)]

use core::ffi::c_void;
use core::panic::PanicInfo;

#[cfg(target_os = "macos")]
#[no_mangle]
pub unsafe extern "C" fn main(argc: i32, argv: *const *const u8) -> ! {
    unsafe { sysweb::run_from_argv(argc, argv) }
}

#[cfg(target_os = "linux")]
#[cfg(target_arch = "x86_64")]
core::arch::global_asm!(
    r#"
    .globl _start
    .type _start,@function
_start:
    xor ebp, ebp
    mov rdi, [rsp]
    lea rsi, [rsp + 8]
    and rsp, -16
    call sysweb_entry
    ud2
    .size _start, .-_start
    .section .note.GNU-stack,"",@progbits
    "#
);

#[cfg(target_os = "linux")]
#[cfg(target_arch = "aarch64")]
core::arch::global_asm!(
    r#"
    .globl _start
    .type _start,%function
_start:
    ldr x0, [sp]
    add x1, sp, #8
    bl sysweb_entry
    brk #0
    .size _start, .-_start
    .section .note.GNU-stack,"",@progbits
    "#
);

#[cfg(target_os = "linux")]
#[no_mangle]
pub unsafe extern "C" fn sysweb_entry(argc: i32, argv: *const *const u8) -> ! {
    unsafe { sysweb::run_from_argv(argc, argv) }
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    sysweb::platform::write_stderr(b"syswebd: panic\n");
    sysweb::platform::exit(101)
}

/// Required by `core` even with `panic = "abort"` on some targets.
#[no_mangle]
pub extern "C" fn rust_eh_personality() {}

/// Referenced if the compiler enables stack-protector instrumentation.
#[no_mangle]
pub extern "C" fn __stack_chk_fail() -> ! {
    sysweb::platform::write_stderr(b"syswebd: stack smashing detected\n");
    sysweb::platform::exit(134)
}

#[no_mangle]
#[inline(never)]
pub unsafe extern "C" fn memcpy(
    dest: *mut c_void,
    src: *const c_void,
    count: usize,
) -> *mut c_void {
    let dest = dest as *mut u8;
    let src = src as *const u8;
    let mut i = 0usize;
    while i < count {
        unsafe {
            core::ptr::write_volatile(dest.add(i), core::ptr::read_volatile(src.add(i)));
        }
        i += 1;
    }
    dest as *mut c_void
}

#[no_mangle]
#[inline(never)]
pub unsafe extern "C" fn memmove(
    dest: *mut c_void,
    src: *const c_void,
    count: usize,
) -> *mut c_void {
    let dest = dest as *mut u8;
    let src = src as *const u8;
    if (dest as usize) <= (src as usize) {
        let mut i = 0usize;
        while i < count {
            unsafe {
                core::ptr::write_volatile(dest.add(i), core::ptr::read_volatile(src.add(i)));
            }
            i += 1;
        }
    } else {
        let mut i = count;
        while i > 0 {
            i -= 1;
            unsafe {
                core::ptr::write_volatile(dest.add(i), core::ptr::read_volatile(src.add(i)));
            }
        }
    }
    dest as *mut c_void
}

#[no_mangle]
#[inline(never)]
pub unsafe extern "C" fn memset(dest: *mut c_void, value: i32, count: usize) -> *mut c_void {
    let dest = dest as *mut u8;
    let byte = value as u8;
    let mut i = 0usize;
    while i < count {
        unsafe {
            core::ptr::write_volatile(dest.add(i), byte);
        }
        i += 1;
    }
    dest as *mut c_void
}

#[no_mangle]
#[inline(never)]
pub unsafe extern "C" fn bzero(dest: *mut c_void, count: usize) {
    // Implemented as its own loop rather than `memset(dest, 0, count)`:
    // LLVM canonicalizes that call back into `bzero`, which becomes infinite
    // self-recursion under LTO (`_bzero: b _bzero`).
    let dest = dest as *mut u8;
    let mut i = 0usize;
    while i < count {
        unsafe {
            core::ptr::write_volatile(dest.add(i), 0);
        }
        i += 1;
    }
}

#[no_mangle]
#[inline(never)]
pub unsafe extern "C" fn memcmp(a: *const c_void, b: *const c_void, count: usize) -> i32 {
    let a = a as *const u8;
    let b = b as *const u8;
    let mut i = 0usize;
    while i < count {
        let left = unsafe { core::ptr::read_volatile(a.add(i)) };
        let right = unsafe { core::ptr::read_volatile(b.add(i)) };
        if left != right {
            return left as i32 - right as i32;
        }
        i += 1;
    }
    0
}

#[no_mangle]
#[inline(never)]
pub unsafe extern "C" fn bcmp(a: *const c_void, b: *const c_void, count: usize) -> i32 {
    unsafe { memcmp(a, b, count) }
}
