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

For a single-section MSVC layout, the battery-warn recipe can be extended with
`/FIXED /DYNAMICBASE:NO /MERGE:.rdata=.text` once the image has no writable
global state. This spike defaults to a relocatable image; it has no `static`
items, so the read-only sections are mergeable. The real Windows backend
remains the IOCP/async backend; this spike only de-risks the entry point, linker
flags, and import table.
