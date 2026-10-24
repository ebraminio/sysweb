#![feature(coroutines, coroutine_trait, stmt_expr_attributes)]

use std::io::{self, ErrorKind};
use std::ops::{Coroutine, CoroutineState};
use std::pin::pin;

#[derive(Clone, Copy, Debug)]
enum Need {
    Read,
    Write,
}

#[derive(Clone, Copy, Debug)]
enum Step {
    Read,
    Write,
    Done,
}

/// A tiny nonblocking transport.
///
/// The first read/write attempt simulates `WouldBlock`; the next one succeeds.
/// A real reactor would set readiness from `epoll`/`kqueue`/`WSAPoll`.
struct Io {
    input: &'static [u8],
    input_at: usize,
    output: [u8; 128],
    output_len: usize,
    readable: bool,
    writable: bool,
}

impl Io {
    fn new() -> Self {
        Self {
            input: b"GET / HTTP/1.1\r\nHost: x\r\n\r\n",
            input_at: 0,
            output: [0; 128],
            output_len: 0,
            readable: false,
            writable: false,
        }
    }

    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        if !self.readable {
            self.readable = true;
            return Err(io::Error::from(ErrorKind::WouldBlock));
        }
        if self.input_at == self.input.len() {
            return Ok(0);
        }
        let n = core::cmp::min(out.len(), self.input.len() - self.input_at);
        out[..n].copy_from_slice(&self.input[self.input_at..self.input_at + n]);
        self.input_at += n;
        Ok(n)
    }

    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        if !self.writable {
            self.writable = true;
            return Err(io::Error::from(ErrorKind::WouldBlock));
        }
        let space = self.output.len() - self.output_len;
        let n = core::cmp::min(space, data.len());
        self.output[self.output_len..self.output_len + n].copy_from_slice(&data[..n]);
        self.output_len += n;
        Ok(n)
    }
}

const RESPONSE: &[u8] = b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok";

fn main() {
    let mut handler = pin!(
        #[coroutine]
        static move || {
            let mut io = Io::new();
            let mut request = [0u8; 64];
            let mut used = 0;

            loop {
                match io.read(&mut request[used..]) {
                    Ok(0) => return io,
                    Ok(n) => {
                        used += n;
                        if request[..used].windows(4).any(|w| w == b"\r\n\r\n") {
                            break;
                        }
                    }
                    Err(error) if error.kind() == ErrorKind::WouldBlock => {
                        yield Need::Read;
                    }
                    Err(_) => return io,
                }
            }

            let mut sent = 0;
            while sent < RESPONSE.len() {
                match io.write(&RESPONSE[sent..]) {
                    Ok(0) => return io,
                    Ok(n) => sent += n,
                    Err(error) if error.kind() == ErrorKind::WouldBlock => {
                        yield Need::Write;
                    }
                    Err(_) => return io,
                }
            }
            io
        }
    );

    let mut polls = 0;
    let io = loop {
        polls += 1;
        match handler.as_mut().resume(()) {
            CoroutineState::Yielded(Need::Read) => println!("  coroutine wants read"),
            CoroutineState::Yielded(Need::Write) => println!("  coroutine wants write"),
            CoroutineState::Complete(io) => break io,
        }
    };
    let coro_size = core::mem::size_of_val(&*handler);
    drop(handler);

    println!("coroutine: {} polls", polls);
    println!("coroutine object: {} bytes", coro_size);
    println!(
        "response: {}",
        String::from_utf8_lossy(&io.output[..io.output_len])
    );

    let mut io = Io::new();
    let mut explicit = Explicit::new();
    let mut polls = 0;
    loop {
        polls += 1;
        match explicit.resume(&mut io) {
            Some(Need::Read) => println!("  explicit wants read"),
            Some(Need::Write) => println!("  explicit wants write"),
            None => break,
        }
    }
    println!("explicit: {} polls", polls);
    println!(
        "explicit object: {} bytes",
        core::mem::size_of::<Explicit>()
    );
    println!(
        "response: {}",
        String::from_utf8_lossy(&io.output[..io.output_len])
    );
}

/// The same protocol as an explicit state machine.
struct Explicit {
    state: Step,
    request: [u8; 64],
    used: usize,
    sent: usize,
}

impl Explicit {
    fn new() -> Self {
        Self {
            state: Step::Read,
            request: [0; 64],
            used: 0,
            sent: 0,
        }
    }

    fn resume(&mut self, io: &mut Io) -> Option<Need> {
        loop {
            match self.state {
                Step::Read => match io.read(&mut self.request[self.used..]) {
                    Ok(0) => {
                        self.state = Step::Done;
                    }
                    Ok(n) => {
                        self.used += n;
                        if self.request[..self.used]
                            .windows(4)
                            .any(|w| w == b"\r\n\r\n")
                        {
                            self.state = Step::Write;
                        }
                    }
                    Err(error) if error.kind() == ErrorKind::WouldBlock => {
                        return Some(Need::Read);
                    }
                    Err(_) => self.state = Step::Done,
                },
                Step::Write => match io.write(&RESPONSE[self.sent..]) {
                    Ok(0) => self.state = Step::Done,
                    Ok(n) => {
                        self.sent += n;
                        if self.sent == RESPONSE.len() {
                            self.state = Step::Done;
                        }
                    }
                    Err(error) if error.kind() == ErrorKind::WouldBlock => {
                        return Some(Need::Write);
                    }
                    Err(_) => self.state = Step::Done,
                },
                Step::Done => return None,
            }
        }
    }
}
