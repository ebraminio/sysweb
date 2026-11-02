# macOS `_start` vs `_main` spike

Question: can a custom macOS `_start` reduce the binary compared with the
current LC_MAIN `_main` entry?

## What was tried

The production source was temporarily changed to:

```asm
.globl _start
.p2align 2
_start:
    b _sysweb_start
```

with:

```text
-Wl,-e,_start
```

The entry still called only the existing Rust `sysweb::run_from_argv`, so no
raw syscall numbers or undocumented BSD calls were introduced. libSystem was
still linked.

## Result

| Entry | macOS arm64 release |
| --- | ---: |
| `_main` (current) | 134,144 B |
| custom `_start` | 134,160 B |

The custom entry was **16 bytes larger**. Both binaries run correctly.

The Windows custom-entry win does not transfer here: macOS already has no CRT
startup to remove. dyld was already calling a plain `_main(argc, argv)`, and
the custom stub adds an entry/trampoline rather than replacing a large CRT.

## Verdict

Keep LC_MAIN `_main`. It is smaller, standard, and still free of raw syscalls
and CRT startup. The experiment was reverted.

If size becomes critical, the next macOS target is libSystem's internal
allocation in `fdopendir`/`readdir`, not the entry stub.
