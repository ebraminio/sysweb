//! Minimal CRT-free IOCP spike.
//!
//! It self-tests one loopback connection: blocking `accept`, associate the
//! accepted socket with an IOCP, post overlapped `WSARecv`, complete it,
//! overlapped `WSASend`, complete it, and verify the reply. AcceptEx and the
//! full server state machine remain for the real backend.

#![no_std]
#![no_main]
#![allow(non_snake_case)]

use core::ffi::c_void;
use core::panic::PanicInfo;
use core::ptr::null_mut;

const AF_INET: i32 = 2;
const SOCK_STREAM: i32 = 1;
const IPPROTO_TCP: i32 = 6;
const INVALID_SOCKET: usize = usize::MAX;
const SOCKET_ERROR: i32 = -1;
const WSA_IO_PENDING: i32 = 997;
const INFINITE: u32 = 0xffff_ffff;
const WAIT_MS: u32 = 5000;
const STD_OUTPUT_HANDLE: u32 = -11i32 as u32;
const WSA_FLAG_OVERLAPPED: u32 = 1;
const SIO_GET_EXTENSION_FUNCTION_POINTER: u32 = 0xc8000006;

#[repr(C)]
struct Guid {
    data1: u32,
    data2: u16,
    data3: u16,
    data4: [u8; 8],
}

const WSAID_ACCEPTEX: Guid = Guid {
    data1: 0xb5367df1,
    data2: 0xcbac,
    data3: 0x11cf,
    data4: [0x95, 0xca, 0x00, 0x80, 0x5f, 0x48, 0xa1, 0x92],
};

type AcceptExFn = unsafe extern "system" fn(
    listener: usize,
    accept_socket: usize,
    output: *mut c_void,
    receive_length: u32,
    local_length: u32,
    remote_length: u32,
    bytes: *mut u32,
    overlapped: *mut Overlapped,
) -> i32;

#[repr(C, align(8))]
struct WsaData([u8; 512]);

#[repr(C)]
struct SockaddrIn {
    family: u16,
    port: u16,
    addr: [u8; 4],
    zero: [u8; 8],
}

#[repr(C)]
struct WsaBuf {
    len: u32,
    buf: *mut u8,
}

#[repr(C)]
struct Overlapped {
    internal: usize,
    internal_high: usize,
    offset: u32,
    offset_high: u32,
    event: *mut c_void,
}

#[link(name = "ws2_32")]
unsafe extern "system" {
    fn WSAStartup(version: u16, data: *mut WsaData) -> i32;
    fn WSACleanup() -> i32;
    fn WSAGetLastError() -> i32;
    fn socket(af: i32, ty: i32, protocol: i32) -> usize;
    fn WSASocketW(
        af: i32,
        ty: i32,
        protocol: i32,
        protocol_info: *mut c_void,
        group: u32,
        flags: u32,
    ) -> usize;
    fn WSAIoctl(
        s: usize,
        code: u32,
        in_buffer: *mut c_void,
        in_size: u32,
        out_buffer: *mut c_void,
        out_size: u32,
        bytes: *mut u32,
        overlapped: *mut c_void,
        completion: *mut c_void,
    ) -> i32;
    fn bind(s: usize, name: *const SockaddrIn, len: i32) -> i32;
    fn listen(s: usize, backlog: i32) -> i32;
    fn connect(s: usize, name: *const SockaddrIn, len: i32) -> i32;
    fn getsockname(s: usize, name: *mut SockaddrIn, len: *mut i32) -> i32;
    fn send(s: usize, buf: *const u8, len: i32, flags: i32) -> i32;
    fn recv(s: usize, buf: *mut u8, len: i32, flags: i32) -> i32;
    fn closesocket(s: usize) -> i32;
    fn WSARecv(
        s: usize,
        buffers: *const WsaBuf,
        count: u32,
        bytes: *mut u32,
        flags: *mut u32,
        overlapped: *mut Overlapped,
        completion: *mut c_void,
    ) -> i32;
    fn WSASend(
        s: usize,
        buffers: *const WsaBuf,
        count: u32,
        bytes: *mut u32,
        flags: u32,
        overlapped: *mut Overlapped,
        completion: *mut c_void,
    ) -> i32;
}

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
    fn CreateIoCompletionPort(
        file: *mut c_void,
        existing: *mut c_void,
        key: usize,
        threads: u32,
    ) -> *mut c_void;
    fn GetQueuedCompletionStatus(
        port: *mut c_void,
        bytes: *mut u32,
        key: *mut usize,
        overlapped: *mut *mut Overlapped,
        timeout: u32,
    ) -> i32;
    fn ExitProcess(code: u32) -> !;
}

fn out(message: &[u8]) {
    let mut written = 0u32;
    unsafe {
        let handle = GetStdHandle(STD_OUTPUT_HANDLE);
        let _ = WriteFile(
            handle,
            message.as_ptr(),
            message.len() as u32,
            &mut written,
            null_mut(),
        );
    }
}

fn fail(message: &[u8]) -> ! {
    out(message);
    unsafe { ExitProcess(1) }
}

fn fail_code(message: &[u8], code: i32) -> ! {
    out(message);
    unsafe { ExitProcess(code as u32) }
}

fn loopback(port: u16) -> SockaddrIn {
    SockaddrIn {
        family: AF_INET as u16,
        port: port.to_be(),
        addr: [127, 0, 0, 1],
        zero: [0; 8],
    }
}

fn wait(port: *mut c_void, expected: *mut Overlapped) -> u32 {
    let mut bytes = 0u32;
    let mut key = 0usize;
    let mut overlapped: *mut Overlapped = null_mut();
    if unsafe {
        GetQueuedCompletionStatus(port, &mut bytes, &mut key, &mut overlapped, WAIT_MS)
    } == 0
    {
        fail(b"iocp: GetQueuedCompletionStatus failed\n");
    }
    if overlapped != expected {
        fail(b"iocp: unexpected completion packet\n");
    }
    bytes
}

#[no_mangle]
pub extern "system" fn mainCRTStartup() -> ! {
    let mut wsa = WsaData([0; 512]);
    if unsafe { WSAStartup(0x0202, &mut wsa) } != 0 {
        fail(b"iocp: WSAStartup failed\n");
    }

    let listener = unsafe {
        WSASocketW(
            AF_INET,
            SOCK_STREAM,
            IPPROTO_TCP,
            null_mut(),
            0,
            WSA_FLAG_OVERLAPPED,
        )
    };
    if listener == INVALID_SOCKET {
        fail(b"iocp: listener socket failed\n");
    }
    let bind_addr = loopback(0);
    if unsafe { bind(listener, &bind_addr, 16) } == SOCKET_ERROR {
        fail(b"iocp: bind failed\n");
    }
    if unsafe { listen(listener, 4) } == SOCKET_ERROR {
        fail(b"iocp: listen failed\n");
    }

    let mut local = loopback(0);
    let mut local_len = 16i32;
    if unsafe { getsockname(listener, &mut local, &mut local_len) } == SOCKET_ERROR {
        fail(b"iocp: getsockname failed\n");
    }
    let port = u16::from_be(local.port);

    let port_handle = unsafe { CreateIoCompletionPort((-1isize) as *mut c_void, null_mut(), 0, 0) };
    if port_handle.is_null() {
        fail(b"iocp: completion port creation failed\n");
    }

    if unsafe { CreateIoCompletionPort(listener as *mut c_void, port_handle, 0, 0) }.is_null() {
        fail(b"iocp: listener association failed\n");
    }

    let mut accept_ex: AcceptExFn = unsafe { core::mem::transmute(core::ptr::null::<c_void>()) };
    let mut accept_ex_bytes = 0u32;
    if unsafe {
        WSAIoctl(
            listener,
            SIO_GET_EXTENSION_FUNCTION_POINTER,
            &WSAID_ACCEPTEX as *const Guid as *mut c_void,
            16,
            &mut accept_ex as *mut AcceptExFn as *mut c_void,
            8,
            &mut accept_ex_bytes,
            null_mut(),
            null_mut(),
        )
    } == SOCKET_ERROR
        || accept_ex_bytes != 8
    {
        fail(b"iocp: AcceptEx lookup failed\n");
    }

    let server = unsafe { WSASocketW(AF_INET, SOCK_STREAM, IPPROTO_TCP, null_mut(), 0, WSA_FLAG_OVERLAPPED) };
    if server == INVALID_SOCKET {
        fail(b"iocp: accept socket failed\n");
    }
    if unsafe { CreateIoCompletionPort(server as *mut c_void, port_handle, 1, 0) }.is_null() {
        fail(b"iocp: socket association failed\n");
    }

    let client = unsafe { socket(AF_INET, SOCK_STREAM, IPPROTO_TCP) };
    if client == INVALID_SOCKET {
        fail(b"iocp: client socket failed\n");
    }
    let target = loopback(port);

    let mut accept_buffer = [0u8; 64];
    let mut accept_overlapped = Overlapped {
        internal: 0,
        internal_high: 0,
        offset: 0,
        offset_high: 0,
        event: null_mut(),
    };
    let accept_result = unsafe {
        accept_ex(
            listener,
            server,
            accept_buffer.as_mut_ptr() as *mut c_void,
            0,
            32,
            32,
            null_mut(),
            &mut accept_overlapped,
        )
    };
    if accept_result == 0 && unsafe { WSAGetLastError() } != WSA_IO_PENDING {
        fail(b"iocp: AcceptEx post failed\n");
    }
    if unsafe { connect(client, &target, 16) } == SOCKET_ERROR {
        fail(b"iocp: connect failed\n");
    }
    out(b"iocp: waiting for AcceptEx\n");
    let accepted = wait(port_handle, &mut accept_overlapped);
    let _ = accepted;

    let mut recv_buffer = [0u8; 64];
    let recv_wsa = WsaBuf {
        len: recv_buffer.len() as u32,
        buf: recv_buffer.as_mut_ptr(),
    };
    let mut recv_overlapped = Overlapped {
        internal: 0,
        internal_high: 0,
        offset: 0,
        offset_high: 0,
        event: null_mut(),
    };
    let mut recv_flags = 0u32;
    let recv_result = unsafe {
        WSARecv(
            server,
            &recv_wsa,
            1,
            null_mut(),
            &mut recv_flags,
            &mut recv_overlapped,
            null_mut(),
        )
    };
    if recv_result == SOCKET_ERROR {
        let code = unsafe { WSAGetLastError() };
        if code != WSA_IO_PENDING {
            fail_code(b"iocp: WSARecv post failed, WSA error\n", code);
        }
    }

    let ping = b"ping";
    if unsafe { send(client, ping.as_ptr(), ping.len() as i32, 0) } != ping.len() as i32 {
        fail(b"iocp: client send failed\n");
    }
    out(b"iocp: waiting for WSARecv\n");
    let received = wait(port_handle, &mut recv_overlapped);
    if received != ping.len() as u32 || &recv_buffer[..received as usize] != ping {
        fail(b"iocp: WSARecv data mismatch\n");
    }

    let pong = b"pong";
    let send_wsa = WsaBuf {
        len: pong.len() as u32,
        buf: pong.as_ptr() as *mut u8,
    };
    let mut send_overlapped = Overlapped {
        internal: 0,
        internal_high: 0,
        offset: 0,
        offset_high: 0,
        event: null_mut(),
    };
    let send_result = unsafe {
        WSASend(
            server,
            &send_wsa,
            1,
            null_mut(),
            0,
            &mut send_overlapped,
            null_mut(),
        )
    };
    if send_result == SOCKET_ERROR {
        let code = unsafe { WSAGetLastError() };
        if code != WSA_IO_PENDING {
            fail_code(b"iocp: WSASend post failed, WSA error\n", code);
        }
    }
    let sent = wait(port_handle, &mut send_overlapped);
    if sent != pong.len() as u32 {
        fail(b"iocp: WSASend length mismatch\n");
    }

    let mut reply = [0u8; 4];
    let mut got = 0usize;
    while got < reply.len() {
        let count = unsafe {
            recv(
                client,
                reply.as_mut_ptr().add(got),
                (reply.len() - got) as i32,
                0,
            )
        };
        if count <= 0 {
            fail(b"iocp: client recv failed\n");
        }
        got += count as usize;
    }
    if &reply != pong {
        fail(b"iocp: WSASend data mismatch\n");
    }

    unsafe {
        let _ = closesocket(server);
        let _ = closesocket(client);
        let _ = closesocket(listener);
        let _ = WSACleanup();
    }
    out(b"iocp: AcceptEx/WSARecv/WSASend completion ok\n");
    unsafe { ExitProcess(0) }
}

#[panic_handler]
fn panic(_: &PanicInfo) -> ! {
    unsafe { ExitProcess(101) }
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
