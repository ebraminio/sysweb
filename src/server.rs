//! The portable server loop and HTTP response writer.

use crate::config::Config;
use crate::fixedbuf::FixedBuf;
use crate::http::{self, Method, Status};
use crate::mime;
use crate::path::{self, MAX_PATH, MAX_URL_PATH};
use crate::platform::{Error, FileInfo, Platform};

const REQUEST_BUF: usize = 8192;
const FILE_BUF: usize = 16 * 1024;
const HEADER_BUF: usize = 512;
const LOG_BUF: usize = 1200;

const FALLBACK_500: &[u8] = b"HTTP/1.1 500 Internal Server Error\r\n\
Content-Type: text/plain; charset=utf-8\r\n\
Content-Length: 26\r\n\
Connection: close\r\n\r\n\
500 Internal Server Error\n";

pub struct Server<P: Platform> {
    platform: P,
    config: Config,
}

impl<P: Platform> Server<P> {
    pub fn new(platform: P, config: Config) -> Self {
        Self { platform, config }
    }

    /// Bind, then serve connections forever. Fatal errors terminate the
    /// process; this server intentionally has no supervision layer yet.
    pub fn run(&mut self) -> ! {
        let listener = match self
            .platform
            .bind(self.config.addr(), self.config.backlog())
        {
            Ok(listener) => listener,
            Err(_) => {
                self.platform.log(b"syswebd: bind failed\n");
                self.platform.exit(1)
            }
        };

        if !self.config.quiet() {
            self.log_startup();
        }

        loop {
            match self.platform.accept(&listener) {
                Ok(mut stream) => self.handle(&mut stream),
                Err(Error::Interrupted) => continue,
                Err(_) => {
                    self.platform.log(b"syswebd: accept failed\n");
                    self.platform.exit(1)
                }
            }
        }
    }

    fn handle(&mut self, stream: &mut P::Stream) {
        let mut request_buf = [0u8; REQUEST_BUF];
        let used = match self.read_request(stream, &mut request_buf) {
            ReadOutcome::Complete(used) => used,
            ReadOutcome::Closed | ReadOutcome::Io => return,
            ReadOutcome::TooLarge => {
                self.send_error(stream, Status::RequestHeaderFieldsTooLarge, false, false);
                return;
            }
        };

        let request = match http::parse(&request_buf[..used]) {
            Ok(request) => request,
            Err(http::ParseError::UnsupportedMethod) => {
                self.send_error(stream, Status::MethodNotAllowed, false, true);
                return;
            }
            Err(http::ParseError::UnsupportedVersion) => {
                self.send_error(stream, Status::VersionNotSupported, false, false);
                return;
            }
            Err(http::ParseError::Malformed) => {
                self.send_error(stream, Status::BadRequest, false, false);
                return;
            }
        };

        let head_only = request.method == Method::Head;
        let mut url_path = [0u8; MAX_URL_PATH];
        let url_len = match path::normalize(request.target, &mut url_path) {
            Ok(len) => len,
            Err(path::PathError::TooLong) => {
                self.send_error(stream, Status::UriTooLong, head_only, false);
                return;
            }
            Err(_) => {
                self.send_error(stream, Status::BadRequest, head_only, false);
                return;
            }
        };

        let mut fs_path = [0u8; MAX_PATH];
        let fs_len = match path::join(self.config.root(), &url_path[..url_len], &mut fs_path) {
            Ok(len) => len,
            Err(path::PathError::TooLong) => {
                self.send_error(stream, Status::UriTooLong, head_only, false);
                return;
            }
            Err(_) => {
                self.send_error(stream, Status::BadRequest, head_only, false);
                return;
            }
        };

        let mut file = match self.platform.open(&fs_path[..fs_len]) {
            Ok(file) => file,
            Err(Error::NotFound) | Err(Error::IsADirectory) => {
                self.send_error(stream, Status::NotFound, head_only, false);
                return;
            }
            Err(Error::PermissionDenied) => {
                self.send_error(stream, Status::Forbidden, head_only, false);
                return;
            }
            Err(_) => {
                self.send_error(stream, Status::InternalServerError, head_only, false);
                return;
            }
        };

        let info = match self.platform.file_info(&file) {
            Ok(info) => info,
            Err(_) => {
                self.send_error(stream, Status::InternalServerError, head_only, false);
                return;
            }
        };
        if info.is_dir {
            self.send_error(stream, Status::NotFound, head_only, false);
            return;
        }

        if self.send_file(stream, &mut file, &fs_path[..fs_len], info, head_only) {
            self.log_request(request.method, &url_path[..url_len], Status::Ok, info.len);
        }
    }

    fn read_request(&mut self, stream: &mut P::Stream, buf: &mut [u8]) -> ReadOutcome {
        let mut used = 0usize;
        loop {
            if http::header_end(&buf[..used]).is_some() {
                return ReadOutcome::Complete(used);
            }
            if used == buf.len() {
                return ReadOutcome::TooLarge;
            }
            match self.platform.read(stream, &mut buf[used..]) {
                Ok(0) => {
                    return if used == 0 {
                        ReadOutcome::Closed
                    } else {
                        ReadOutcome::Complete(used)
                    };
                }
                Ok(count) => used += count,
                Err(_) => return ReadOutcome::Io,
            }
        }
    }

    /// Write headers and, unless this is a HEAD request, stream the body.
    /// Returns `true` once the response headers were accepted by the socket.
    fn send_file(
        &mut self,
        stream: &mut P::Stream,
        file: &mut P::File,
        mime_path: &[u8],
        info: FileInfo,
        head_only: bool,
    ) -> bool {
        let content_type = mime::content_type(mime_path);
        let mut header = FixedBuf::<HEADER_BUF>::new();
        if !build_header(&mut header, Status::Ok, info.len, content_type, false) {
            let _ = self.send_fallback(stream);
            return false;
        }
        if self.platform.write_all(stream, header.as_slice()).is_err() {
            return false;
        }
        if head_only {
            return true;
        }

        let mut buffer = [0u8; FILE_BUF];
        let mut remaining = info.len;
        while remaining > 0 {
            let want = core::cmp::min(remaining, FILE_BUF as u64) as usize;
            let count = match self.platform.read_file(file, &mut buffer[..want]) {
                Ok(0) => break,
                Ok(count) => count,
                Err(_) => return false,
            };
            if self.platform.write_all(stream, &buffer[..count]).is_err() {
                return false;
            }
            remaining -= count as u64;
        }
        true
    }

    fn send_error(&mut self, stream: &mut P::Stream, status: Status, head_only: bool, allow: bool) {
        let body = status.error_body();
        let mut header = FixedBuf::<HEADER_BUF>::new();
        if !build_header(
            &mut header,
            status,
            body.len() as u64,
            b"text/plain; charset=utf-8",
            allow,
        ) {
            let _ = self.send_fallback(stream);
            return;
        }
        if self.platform.write_all(stream, header.as_slice()).is_err() {
            return;
        }
        if !head_only && !body.is_empty() {
            let _ = self.platform.write_all(stream, body);
        }
        self.log_error(status);
    }

    fn send_fallback(&mut self, stream: &mut P::Stream) -> bool {
        self.platform.write_all(stream, FALLBACK_500).is_ok()
    }

    fn log_startup(&mut self) {
        let addr = self.config.addr();
        let mut line = FixedBuf::<LOG_BUF>::new();
        let _ = line.push(b"syswebd ");
        let _ = line.push(crate::VERSION.as_bytes());
        let _ = line.push(b" listening on http://");
        let _ = line.push_ipv4(addr.ip);
        let _ = line.push_byte(b':');
        let _ = line.push_u64(addr.port as u64);
        let _ = line.push(b" (root: ");
        let _ = line.push(self.config.root());
        let _ = line.push(b")\n");
        self.platform.log(line.as_slice());
    }

    fn log_request(&mut self, method: Method, path: &[u8], status: Status, len: u64) {
        if self.config.quiet() {
            return;
        }
        let mut line = FixedBuf::<LOG_BUF>::new();
        let _ = line.push(method.as_bytes());
        let _ = line.push_byte(b' ');
        let _ = line.push(path);
        let _ = line.push_byte(b' ');
        let _ = line.push_u64(status.code() as u64);
        let _ = line.push_byte(b' ');
        let _ = line.push_u64(len);
        let _ = line.push_byte(b'\n');
        self.platform.log(line.as_slice());
    }

    fn log_error(&mut self, status: Status) {
        let mut line = FixedBuf::<64>::new();
        let _ = line.push(b"syswebd: ");
        let _ = line.push_u64(status.code() as u64);
        let _ = line.push_byte(b' ');
        let _ = line.push(status.reason());
        let _ = line.push_byte(b'\n');
        self.platform.log(line.as_slice());
    }
}

enum ReadOutcome {
    Complete(usize),
    Closed,
    TooLarge,
    Io,
}

fn build_header(
    buffer: &mut FixedBuf<HEADER_BUF>,
    status: Status,
    content_length: u64,
    content_type: &[u8],
    allow: bool,
) -> bool {
    buffer.clear();
    if !buffer.push(b"HTTP/1.1 ") {
        return false;
    }
    if !buffer.push_u64(status.code() as u64)
        || !buffer.push_byte(b' ')
        || !buffer.push(status.reason())
        || !buffer.push(b"\r\n")
        || !buffer.push(b"Server: sysweb/")
        || !buffer.push(crate::VERSION.as_bytes())
        || !buffer.push(b"\r\n")
        || !buffer.push(b"Content-Type: ")
        || !buffer.push(content_type)
        || !buffer.push(b"\r\n")
        || !buffer.push(b"Content-Length: ")
        || !buffer.push_u64(content_length)
        || !buffer.push(b"\r\n")
    {
        return false;
    }
    if allow && !buffer.push(b"Allow: GET, HEAD\r\n") {
        return false;
    }
    buffer.push(b"Connection: close\r\n\r\n")
}
