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

- Do **not** use `-static` together with `-nostdlib` for the GNU PE link; the
  resulting image failed with `STATUS_INVALID_IMAGE_FORMAT`.
- A PE subsystem version of `10.0` also failed on the test machine; `6.0` is
  the tested loader floor. The Windows 10 API floor is unaffected.
- Keep the relocation section. Fixed-address/no-reloc experiments were the
  other source of `STATUS_INVALID_IMAGE_FORMAT`.

The real Windows backend remains the IOCP/async backend. This spike only
de-risks the entry point, linker flags, and import table.
