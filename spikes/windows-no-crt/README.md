# Windows no-CRT link spike

This is a proof, not the Windows server backend. It shows that the same
technique used by [battery-warn](https://github.com/ebraminio/battery-warn) can
produce a tiny CRT-free Rust PE:

- `#![no_std]` and `#![no_main]`;
- custom `mainCRTStartup` entry point;
- `/NODEFAULTLIB` on MSVC, or `-nostdlib` on the GNU target;
- direct `kernel32` imports only;
- no msvcrt, ucrt, Rust startup objects, or allocator.

Building the GNU target from macOS/Linux:

```sh
rustup target add x86_64-pc-windows-gnu
./spikes/windows-no-crt/build.sh
```

The script uses `x86_64-w64-mingw32-gcc` as the linker and strips the result.
On this machine the stripped binary is **3,584 bytes**, and its imports are:

```text
KERNEL32.dll: ExitProcess, GetStdHandle, WriteFile
```

It was executed on a real Windows 11 x64 machine:

```text
sysweb windows no-CRT spike
EXIT=0
```

Notes from making it load:

- The actual `STATUS_INVALID_IMAGE_FORMAT` cause was the PE subsystem version:
  `10.0` failed on the test machine, while `6.0` works. The Windows 10 API floor
  is unaffected.
- With subsystem `6.0`, all of these were retested and run:
  - normal relocation section
  - `-static` + `-nostdlib`
  - fixed base + no relocation section
  - `-static` + fixed/no-reloc
- A remote MSVC build of the same C probe also ran with:

  ```bat
  link /NODEFAULTLIB /SUBSYSTEM:CONSOLE,6.0 /ENTRY:start ^
       /FIXED /DYNAMICBASE:NO /OPT:REF /OPT:ICF ^
       msvcspike.obj kernel32.lib
  ```

  So `/FIXED` is not a toolchain limitation. The Rust spike keeps the default
  relocation section because it is more robust; fixed-base is optional.

### Fixed base vs. one section

`/FIXED` alone only fixes the image base; it does not collapse sections. The
one-section battery-warn result comes from the **LLVM/ld-link** recipe:

```text
-fuse-ld=lld-link -Wl,/fixed -Wl,/dynamicbase:no
-Wl,/merge:.rdata=.text -Wl,/merge:.pdata=.text
```

A C probe linked that way produced a **1,024-byte PE with a single `.text`
section** and ran on Windows 11. The current battery-warn `build.bat` (MSVC) is
not one-section. This Rust spike is built with GNU GCC and remains
multi-section. A future MSVC/lld-link Windows build can apply the same merge
flags; no writable globals are a prerequisite, because `.data`/`.bss` cannot be
merged into read-only `.text`.

The real Windows backend remains the IOCP/async backend; this spike only
de-risks the entry point, linker flags, and import table.

### `-ffreestanding` and `mem*`

`-ffreestanding` is a C/Clang flag, not a Rust one. `#![no_std]` is the
semantic equivalent here, but it does not stop LLVM from emitting calls to
`memcpy`/`memset` for idioms. It also does not provide those functions. The
server already supplies its own `memcpy`, `memmove`, `memset`, `memcmp`,
`bcmp`, and `bzero` shims in `src/bin/syswebd.rs`; that is the correct layer for
this. `-ffreestanding` is therefore not the tool for one-section linking.
