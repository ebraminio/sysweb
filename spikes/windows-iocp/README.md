# Windows IOCP spike

A minimal CRT-free IOCP proof, built from the same Windows GNU cross toolchain
as the no-CRT entry/link spike.

It self-tests one loopback connection:

1. `WSAStartup`, listener/client sockets, `bind`, `listen`, `connect`;
2. create an IOCP and associate both the listener and the accept socket;
3. post an overlapped `AcceptEx`, connect the client, and wait for completion;
4. post an overlapped `WSARecv`, send `ping` from the client, wait for the
   completion, and verify the bytes;
5. post an overlapped `WSASend`, wait for the completion, and verify `pong` on
   the client.

This validates the Windows completion model, `AcceptEx`, and the
`WSARecv`/`WSASend` overlapped ABI without the CRT. Socket state and the real
connection state machine remain for the backend after the Unix async reactor.

Build and inspect the PE:

```sh
rustup target add x86_64-pc-windows-gnu
./spikes/windows-iocp/build.sh
```

The stripped PE imports only `KERNEL32.dll` and `WS2_32.dll`. On the Windows
test machine it prints:

```text
iocp: AcceptEx/WSARecv/WSASend completion ok
```

The toolchain used here is the persistent user-level `~/.rustup`/`~/.cargo`
installation, not a workspace `.tmp`.
