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

## `download-c10k`

A macOS-only download server with compact state (~2.3 KiB/connection),
`sendfile`, optional keep-alive, and a 12,000-record table. It benchmarks the
cost of separating download state from upload/listing buffers.

## `io-uring`

A Linux-only download server using io_uring accept/recv/read/send with compact
state and no keep-alive. It reaches 10k connections on loopback and includes an
API comparison with XDP/eBPF/AF_XDP, which could not be loaded on the available
Wi-Fi-only host.

## `chat`

A no-JavaScript old-browser chat experiment: form POST, 303 redirect, and an
iframe refreshed by `<meta http-equiv="refresh">`. It demonstrates the polling
shape without WebSocket, plus the fixed-ring state a sysweb chat mode would
need.

Each directory has its own build/run notes.
