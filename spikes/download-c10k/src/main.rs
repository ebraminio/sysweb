//! macOS-only download spike: compact per-connection state, no keep-alive.
//!
//! This is an experiment, not production code. It intentionally ignores
//! listings, uploads, ranges, authentication, and keep-alive. Each accepted
//! connection reads one request into a small buffer, opens one file, sends a
//! header, streams the file with `sendfile`, and closes.
//!
//! The point is to measure how far the compact-state model can go on one
//! kqueue reactor, not to be a complete HTTP server.

#[cfg(not(target_os = "macos"))]
compile_error!("download-c10k spike is macOS-only");

use std::env;
use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;

use sysweb::fixedbuf::FixedBuf;
use sysweb::platform::{Error, Event, Interest, Platform, SocketAddr, Sys};

type Stream = <Sys as Platform>::Stream;
type File = <Sys as Platform>::File;
type Poller = <Sys as Platform>::Poller;
type Listener = <Sys as Platform>::Listener;

const REQUEST_BUF: usize = 2048;
const HEADER_BUF: usize = 256;
const ACCEPT_BATCH: usize = 256;

struct Conn {
    stream: Stream,
    keep_alive: bool,
    state: State,
    request: [u8; REQUEST_BUF],
    request_len: usize,
    file: Option<File>,
    remaining: u64,
    offset: u64,
    header: [u8; HEADER_BUF],
    header_len: usize,
    header_pos: usize,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum State {
    Reading,
    WritingHeader,
    SendingFile,
}

impl Conn {
    fn new(stream: Stream, keep_alive: bool) -> Self {
        Self {
            stream,
            keep_alive,
            state: State::Reading,
            request: [0; REQUEST_BUF],
            request_len: 0,
            file: None,
            remaining: 0,
            offset: 0,
            header: [0; HEADER_BUF],
            header_len: 0,
            header_pos: 0,
        }
    }
}

fn main() {
    let mut root = None;
    let mut port = 19500u16;
    let mut max = 10000usize;
    let mut backlog = 8192u16;
    let mut keep_alive = false;
    let mut args = env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--root" => root = args.next(),
            "--port" => port = args.next().and_then(|v| v.parse().ok()).unwrap_or(port),
            "--max" => max = args.next().and_then(|v| v.parse().ok()).unwrap_or(max),
            "--backlog" => backlog = args.next().and_then(|v| v.parse().ok()).unwrap_or(backlog),
            "--keepalive" => keep_alive = true,
            other => {
                eprintln!("unknown argument: {other}");
                std::process::exit(2);
            }
        }
    }
    let Some(root) = root else {
        eprintln!(
            "usage: download-c10k --root DIR [--port N] [--max N] [--backlog N] [--keepalive]"
        );
        std::process::exit(2);
    };

    let mut sys = Sys::new();
    sys.set_root(OsStr::new(&root).as_bytes())
        .expect("set root");
    let listener = sys.bind(SocketAddr::loopback(port), backlog).expect("bind");
    sys.set_listener_nonblocking(&listener)
        .expect("nonblocking");
    let mut poller = sys.poller().expect("poller");
    sys.register_listener(&mut poller, &listener, 0)
        .expect("register listener");

    let mut slots: Vec<Option<Conn>> = Vec::with_capacity(max);
    for _ in 0..max {
        slots.push(None);
    }
    let mut events = [Event::default(); 256];

    println!(
        "download-c10k: root={root} port={port} max={max} backlog={backlog} keepalive={keep_alive} conn_size={} pid={}",
        core::mem::size_of::<Conn>(),
        std::process::id()
    );

    loop {
        let count = match sys.wait(&mut poller, &mut events, -1) {
            Ok(count) => count,
            Err(Error::Interrupted) => continue,
            Err(error) => {
                eprintln!("wait: {error:?}");
                return;
            }
        };

        for event in &events[..count] {
            if event.token == 0 {
                accept_ready(&mut sys, &mut poller, &listener, &mut slots, keep_alive);
            } else {
                let index = event.token - 1;
                if index >= slots.len() {
                    continue;
                }
                if event.error {
                    close(&mut sys, &mut poller, &mut slots, index);
                    continue;
                }
                let keep = if let Some(conn) = slots[index].as_mut() {
                    match conn.state {
                        State::Reading => read_request(&mut sys, &mut poller, conn, index),
                        State::WritingHeader => write_header(&mut sys, &mut poller, conn, index),
                        State::SendingFile => send_file(&mut sys, &mut poller, conn, index),
                    }
                } else {
                    false
                };
                if !keep {
                    close(&mut sys, &mut poller, &mut slots, index);
                }
            }
        }
    }
}

fn accept_ready(
    sys: &mut Sys,
    poller: &mut Poller,
    listener: &Listener,
    slots: &mut [Option<Conn>],
    keep_alive: bool,
) {
    for _ in 0..ACCEPT_BATCH {
        match sys.accept_nonblocking(listener) {
            Ok(stream) => {
                let Some(index) = slots.iter().position(|slot| slot.is_none()) else {
                    drop(stream);
                    continue;
                };
                let conn = Conn::new(stream, keep_alive);
                let token = index + 1;
                if sys.register_stream(poller, &conn.stream, token).is_err() {
                    continue;
                }
                slots[index] = Some(conn);
            }
            Err(Error::WouldBlock) | Err(Error::Interrupted) => break,
            Err(_) => break,
        }
    }
}

fn read_request(sys: &mut Sys, poller: &mut Poller, conn: &mut Conn, index: usize) -> bool {
    loop {
        if conn.request_len == conn.request.len() {
            return false;
        }
        match sys.read_nonblocking(&mut conn.stream, &mut conn.request[conn.request_len..]) {
            Ok(0) => return false,
            Ok(n) => conn.request_len += n,
            Err(Error::Interrupted) => continue,
            Err(Error::WouldBlock) => return true,
            Err(_) => return false,
        }
        let Some(end) = header_end(&conn.request[..conn.request_len]) else {
            continue;
        };

        let Some(target) = request_target(&conn.request[..end]) else {
            return false;
        };
        let path = target.strip_prefix(b"/").unwrap_or(target);
        let path = match path.iter().position(|byte| *byte == b'?') {
            Some(at) => &path[..at],
            None => path,
        };
        let Ok(file) = sys.open(path) else {
            return false;
        };
        let Ok(info) = sys.file_info(&file) else {
            return false;
        };
        if info.is_dir {
            return false;
        }

        let mut header = FixedBuf::<HEADER_BUF>::new();
        if !header.push(b"HTTP/1.1 200 OK\r\nContent-Length: ")
            || !header.push_u64(info.len)
            || !header.push(if conn.keep_alive {
                b"\r\nConnection: keep-alive\r\n\r\n"
            } else {
                b"\r\nConnection: close\r\n\r\n"
            })
        {
            return false;
        }
        conn.header[..header.len()].copy_from_slice(header.as_slice());
        conn.header_len = header.len();
        conn.header_pos = 0;
        conn.file = Some(file);
        conn.remaining = info.len;
        conn.state = State::WritingHeader;
        return set_write_interest(sys, poller, conn, index);
    }
}

fn write_header(sys: &mut Sys, poller: &mut Poller, conn: &mut Conn, index: usize) -> bool {
    while conn.header_pos < conn.header_len {
        match sys.write_nonblocking(
            &mut conn.stream,
            &conn.header[conn.header_pos..conn.header_len],
        ) {
            Ok(0) => return false,
            Ok(n) => conn.header_pos += n,
            Err(Error::Interrupted) => continue,
            Err(Error::WouldBlock) => return set_write_interest(sys, poller, conn, index),
            Err(_) => return false,
        }
    }
    conn.state = State::SendingFile;
    set_write_interest(sys, poller, conn, index)
}

fn send_file(sys: &mut Sys, poller: &mut Poller, conn: &mut Conn, index: usize) -> bool {
    while conn.remaining > 0 {
        let Some(file) = conn.file.as_mut() else {
            return false;
        };
        match sys.send_file_nonblocking(&mut conn.stream, file, conn.offset, conn.remaining) {
            Ok(0) => return false,
            Ok(n) => {
                conn.offset += n;
                conn.remaining -= n;
            }
            Err(Error::Interrupted) => continue,
            Err(Error::WouldBlock) => return set_write_interest(sys, poller, conn, index),
            Err(_) => return false,
        }
    }
    if conn.keep_alive {
        conn.file = None;
        conn.offset = 0;
        conn.remaining = 0;
        conn.request_len = 0;
        conn.header_len = 0;
        conn.header_pos = 0;
        conn.state = State::Reading;
        sys.set_interest(poller, &conn.stream, index + 1, Interest::Readable)
            .is_ok()
    } else {
        false
    }
}

fn set_write_interest(sys: &mut Sys, poller: &mut Poller, conn: &mut Conn, index: usize) -> bool {
    sys.set_interest(poller, &conn.stream, index + 1, Interest::Writable)
        .is_ok()
}

fn close(sys: &mut Sys, poller: &mut Poller, slots: &mut [Option<Conn>], index: usize) {
    if let Some(conn) = slots[index].as_ref() {
        let _ = sys.deregister_stream(poller, &conn.stream, index + 1);
    }
    slots[index] = None;
}

fn header_end(buf: &[u8]) -> Option<usize> {
    buf.windows(4)
        .position(|window| window == b"\r\n\r\n")
        .map(|at| at + 4)
}

fn request_target(buf: &[u8]) -> Option<&[u8]> {
    let line_end = buf.windows(2).position(|window| window == b"\r\n")?;
    let line = &buf[..line_end];
    let mut parts = line.split(|byte| *byte == b' ');
    let method = parts.next()?;
    if method != b"GET" && method != b"HEAD" {
        return None;
    }
    parts.next()
}
