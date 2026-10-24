# Spikes

These are isolated experiments, not part of the shipped server. They document
which OS paths are viable without the production state machine.

## `windows-no-crt`

A 3,584-byte x86_64 PE with `#![no_main]`, `mainCRTStartup`, `/NODEFAULTLIB`,
and only `KERNEL32.dll` imports. Proves the CRT-free link/entry recipe.

## `windows-iocp`

A completion-port spike using overlapped `AcceptEx`, `WSARecv`, and `WSASend`
over loopback. It imports only `KERNEL32.dll` and `WS2_32.dll`.

The production Windows server still uses `WSAPoll`; replacing it with IOCP
would require a completion-based server state machine.

## `async-no-alloc`

A small future-size experiment showing that `core::future` is technically
available without allocation. It is not used by the server; the explicit
state machine is kept for readability and syscall visibility.

## `coroutine`

A nightly coroutine-per-connection prototype. It compares a linear
`yield Need::Read/Write` handler with an explicit `Step` state machine, and
records the pin/type/borrow problems that appear before it can live in a fixed
slot.

Each directory has its own build/run notes.
