//! The portable server loop and HTTP response writer.

use crate::config::Config;
use crate::fixedbuf::FixedBuf;
use crate::http::{self, Method, Status};
use crate::listing::{self, LISTING_BUF};
use crate::mime;
use crate::path::{self, MAX_PATH, MAX_URL_PATH};
use crate::platform::{Error, FileInfo, Platform};

const REQUEST_BUF: usize = 8192;
const FILE_BUF: usize = 16 * 1024;
const HEADER_BUF: usize = 4096;
const LOG_BUF: usize = 1200;
const LOCATION_BUF: usize = MAX_URL_PATH * 3 + 2;
const MAX_UPLOAD: u64 = 1 << 30; // 1 GiB

const ALLOW_READ: &[u8] = b"GET, HEAD";
const ALLOW_UPLOAD: &[u8] = b"GET, HEAD, PUT";
const CONTINUE: &[u8] = b"HTTP/1.1 100 Continue\r\n\r\n";
const UPLOADED: &[u8] = b"UPLOADED\n";
const CREATED: &[u8] = b"CREATED\n";

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
        if self.platform.set_root(self.config.root()).is_err() {
            self.platform.log(b"syswebd: root open failed\n");
            self.platform.exit(1)
        }

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
                self.send_error(stream, Status::RequestHeaderFieldsTooLarge, false, b"");
                return;
            }
        };

        let request = match http::parse(&request_buf[..used]) {
            Ok(request) => request,
            Err(http::ParseError::UnsupportedMethod) => {
                let allow = if self.config.upload() {
                    ALLOW_UPLOAD
                } else {
                    ALLOW_READ
                };
                self.send_error(stream, Status::MethodNotAllowed, false, allow);
                return;
            }
            Err(http::ParseError::UnsupportedVersion) => {
                self.send_error(stream, Status::VersionNotSupported, false, b"");
                return;
            }
            Err(http::ParseError::Malformed) => {
                self.send_error(stream, Status::BadRequest, false, b"");
                return;
            }
        };

        match request.method {
            Method::Put => self.handle_put(stream, &request, &request_buf[..used]),
            Method::Get | Method::Head => self.handle_get(stream, &request),
        }
    }

    fn handle_get(&mut self, stream: &mut P::Stream, request: &http::Request<'_>) {
        let head_only = request.method == Method::Head;

        let mut url_path = [0u8; MAX_URL_PATH];
        let url_len = match path::normalize(request.target, &mut url_path) {
            Ok(len) => len,
            Err(path::PathError::TooLong) => {
                self.send_error(stream, Status::UriTooLong, head_only, b"");
                return;
            }
            Err(_) => {
                self.send_error(stream, Status::BadRequest, head_only, b"");
                return;
            }
        };

        let rel_path = root_relative(&url_path[..url_len]);
        let mut file = match self.platform.open(rel_path) {
            Ok(file) => file,
            Err(Error::NotFound) | Err(Error::IsADirectory) => {
                self.send_error(stream, Status::NotFound, head_only, b"");
                return;
            }
            Err(Error::PermissionDenied) => {
                self.send_error(stream, Status::Forbidden, head_only, b"");
                return;
            }
            Err(_) => {
                self.send_error(stream, Status::InternalServerError, head_only, b"");
                return;
            }
        };

        let info = match self.platform.file_info(&file) {
            Ok(info) => info,
            Err(_) => {
                self.send_error(stream, Status::InternalServerError, head_only, b"");
                return;
            }
        };

        if !info.is_dir {
            if self.send_file(stream, &mut file, rel_path, info, head_only) {
                self.log_request(request.method, &url_path[..url_len], Status::Ok, info.len);
            }
            return;
        }

        if !url_path[..url_len].ends_with(b"/") {
            if self.send_redirect(stream, &url_path[..url_len], head_only) {
                self.log_request(
                    request.method,
                    &url_path[..url_len],
                    Status::MovedPermanently,
                    0,
                );
            }
            return;
        }

        let mut index_path = [0u8; MAX_PATH];
        let index_len = if url_len == 1 {
            index_path[..10].copy_from_slice(b"index.html");
            index_path[10] = 0;
            10
        } else {
            match path::append_index(rel_path, &mut index_path) {
                Ok(len) => len,
                Err(path::PathError::TooLong) => {
                    self.send_error(stream, Status::UriTooLong, head_only, b"");
                    return;
                }
                Err(_) => {
                    self.send_error(stream, Status::BadRequest, head_only, b"");
                    return;
                }
            }
        };

        match self.platform.open(&index_path[..index_len]) {
            Ok(mut index_file) => {
                let index_info = match self.platform.file_info(&index_file) {
                    Ok(info) => info,
                    Err(_) => {
                        self.send_error(stream, Status::InternalServerError, head_only, b"");
                        return;
                    }
                };
                if index_info.is_dir {
                    self.send_error(stream, Status::NotFound, head_only, b"");
                    return;
                }
                if self.send_file(
                    stream,
                    &mut index_file,
                    &index_path[..index_len],
                    index_info,
                    head_only,
                ) {
                    self.log_request(
                        request.method,
                        &url_path[..url_len],
                        Status::Ok,
                        index_info.len,
                    );
                }
            }
            Err(Error::NotFound) | Err(Error::IsADirectory) => {
                if self.config.index() {
                    self.serve_listing(stream, &url_path[..url_len], rel_path, head_only);
                } else {
                    self.send_error(stream, Status::Forbidden, head_only, b"");
                }
            }
            Err(Error::PermissionDenied) => {
                self.send_error(stream, Status::Forbidden, head_only, b"");
            }
            Err(_) => {
                self.send_error(stream, Status::InternalServerError, head_only, b"");
            }
        }
    }

    fn serve_listing(
        &mut self,
        stream: &mut P::Stream,
        url_path: &[u8],
        fs_path: &[u8],
        head_only: bool,
    ) {
        let mut dir = match self.platform.open_dir(fs_path) {
            Ok(dir) => dir,
            Err(Error::NotFound) | Err(Error::IsADirectory) => {
                self.send_error(stream, Status::NotFound, head_only, b"");
                return;
            }
            Err(Error::PermissionDenied) => {
                self.send_error(stream, Status::Forbidden, head_only, b"");
                return;
            }
            Err(_) => {
                self.send_error(stream, Status::InternalServerError, head_only, b"");
                return;
            }
        };

        let mut html = FixedBuf::<LISTING_BUF>::new();
        if !listing::render(
            &mut self.platform,
            &mut dir,
            url_path,
            self.config.upload(),
            &mut html,
        ) {
            self.send_error(stream, Status::InternalServerError, head_only, b"");
            return;
        }

        if self.send_body(
            stream,
            Status::Ok,
            b"text/html; charset=utf-8",
            html.as_slice(),
            head_only,
            b"",
            None,
        ) {
            self.log_request(Method::Get, url_path, Status::Ok, html.len() as u64);
        }
    }

    fn handle_put(&mut self, stream: &mut P::Stream, request: &http::Request<'_>, raw: &[u8]) {
        if !self.config.upload() {
            self.send_error(stream, Status::MethodNotAllowed, false, ALLOW_READ);
            return;
        }

        let mut url_path = [0u8; MAX_URL_PATH];
        let url_len = match path::normalize(request.target, &mut url_path) {
            Ok(len) => len,
            Err(path::PathError::TooLong) => {
                self.send_error(stream, Status::UriTooLong, false, b"");
                return;
            }
            Err(_) => {
                self.send_error(stream, Status::BadRequest, false, b"");
                return;
            }
        };
        if url_len == 1 {
            self.send_error(stream, Status::BadRequest, false, b"");
            return;
        }

        let rel_path = root_relative(&url_path[..url_len]);

        if url_path[..url_len].ends_with(b"/") {
            match self.platform.create_dir(rel_path) {
                Ok(()) => {
                    if self.send_body(
                        stream,
                        Status::Created,
                        b"text/plain; charset=utf-8",
                        CREATED,
                        false,
                        b"",
                        None,
                    ) {
                        self.log_request(
                            Method::Put,
                            &url_path[..url_len],
                            Status::Created,
                            CREATED.len() as u64,
                        );
                    }
                }
                Err(Error::AlreadyExists) => {
                    self.send_error(stream, Status::Conflict, false, b"");
                }
                Err(Error::NotFound) => {
                    self.send_error(stream, Status::NotFound, false, b"");
                }
                Err(Error::PermissionDenied) => {
                    self.send_error(stream, Status::Forbidden, false, b"");
                }
                Err(_) => {
                    self.send_error(stream, Status::InternalServerError, false, b"");
                }
            }
            return;
        }

        let content_length = match request.content_length {
            Some(length) => length,
            None => {
                self.send_error(stream, Status::LengthRequired, false, b"");
                return;
            }
        };
        if content_length > MAX_UPLOAD {
            self.send_error(stream, Status::PayloadTooLarge, false, b"");
            return;
        }

        let mut file = match self.platform.create_file(rel_path, self.config.replace()) {
            Ok(file) => file,
            Err(Error::AlreadyExists) | Err(Error::IsADirectory) => {
                self.send_error(stream, Status::Conflict, false, b"");
                return;
            }
            Err(Error::NotFound) => {
                self.send_error(stream, Status::NotFound, false, b"");
                return;
            }
            Err(Error::PermissionDenied) => {
                self.send_error(stream, Status::Forbidden, false, b"");
                return;
            }
            Err(_) => {
                self.send_error(stream, Status::InternalServerError, false, b"");
                return;
            }
        };

        match self.write_upload_body(stream, request, raw, &mut file, content_length) {
            Ok(()) => {
                drop(file);
                if self.send_body(
                    stream,
                    Status::Created,
                    b"text/plain; charset=utf-8",
                    UPLOADED,
                    false,
                    b"",
                    None,
                ) {
                    self.log_request(
                        Method::Put,
                        &url_path[..url_len],
                        Status::Created,
                        UPLOADED.len() as u64,
                    );
                }
            }
            Err(status) => {
                drop(file);
                let _ = self.platform.remove_file(rel_path);
                self.send_error(stream, status, false, b"");
            }
        }
    }

    fn write_upload_body(
        &mut self,
        stream: &mut P::Stream,
        request: &http::Request<'_>,
        raw: &[u8],
        file: &mut P::File,
        total: u64,
    ) -> core::result::Result<(), Status> {
        let mut remaining = total;
        let leftover = &raw[request.body_offset..];
        let take = core::cmp::min(leftover.len() as u64, remaining) as usize;
        if take > 0 {
            self.platform
                .write_file(file, &leftover[..take])
                .map_err(|_| Status::InternalServerError)?;
            remaining -= take as u64;
        }

        if remaining > 0 && request.expect_continue && take == 0 {
            self.platform
                .write_all(stream, CONTINUE)
                .map_err(|_| Status::BadRequest)?;
        }

        let mut buffer = [0u8; FILE_BUF];
        while remaining > 0 {
            let want = core::cmp::min(remaining, FILE_BUF as u64) as usize;
            let count = match self.platform.read(stream, &mut buffer[..want]) {
                Ok(0) => return Err(Status::BadRequest),
                Ok(count) => count,
                Err(_) => return Err(Status::BadRequest),
            };
            self.platform
                .write_file(file, &buffer[..count])
                .map_err(|_| Status::InternalServerError)?;
            remaining -= count as u64;
        }
        Ok(())
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
    /// Returns `true` once the response was accepted by the socket.
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
        if !build_header(&mut header, Status::Ok, info.len, content_type, b"", None) {
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

    fn send_redirect(&mut self, stream: &mut P::Stream, path: &[u8], head_only: bool) -> bool {
        let mut location = FixedBuf::<LOCATION_BUF>::new();
        if !listing::url_encode_path(&mut location, path) || !location.push_byte(b'/') {
            self.send_error(stream, Status::UriTooLong, head_only, b"");
            return false;
        }
        self.send_body(
            stream,
            Status::MovedPermanently,
            b"text/plain; charset=utf-8",
            b"",
            head_only,
            b"",
            Some(location.as_slice()),
        )
    }

    // The arguments are all distinct pieces of one response; grouping them
    // into a struct would only move the same data around on the stack.
    #[allow(clippy::too_many_arguments)]
    fn send_body(
        &mut self,
        stream: &mut P::Stream,
        status: Status,
        content_type: &[u8],
        body: &[u8],
        head_only: bool,
        allow: &[u8],
        location: Option<&[u8]>,
    ) -> bool {
        let mut header = FixedBuf::<HEADER_BUF>::new();
        if !build_header(
            &mut header,
            status,
            body.len() as u64,
            content_type,
            allow,
            location,
        ) {
            let _ = self.send_fallback(stream);
            return false;
        }
        if self.platform.write_all(stream, header.as_slice()).is_err() {
            return false;
        }
        if !head_only && !body.is_empty() && self.platform.write_all(stream, body).is_err() {
            return false;
        }
        true
    }

    fn send_error(
        &mut self,
        stream: &mut P::Stream,
        status: Status,
        head_only: bool,
        allow: &[u8],
    ) {
        let body = status.error_body();
        if self.send_body(
            stream,
            status,
            b"text/plain; charset=utf-8",
            body,
            head_only,
            allow,
            None,
        ) {
            self.log_error(status);
        }
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
        let _ = line.push(b", uploads: ");
        let _ = line.push(if self.config.upload() { b"on" } else { b"off" });
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

/// Convert a normalized URL path (`/a/b`) into a path relative to the
/// document-root fd (`a/b`). The root itself is `"."`.
fn root_relative(url_path: &[u8]) -> &[u8] {
    if url_path.len() <= 1 {
        b"."
    } else {
        &url_path[1..]
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
    allow: &[u8],
    location: Option<&[u8]>,
) -> bool {
    buffer.clear();
    if !buffer.push(b"HTTP/1.1 ")
        || !buffer.push_u64(status.code() as u64)
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

    if !allow.is_empty()
        && (!buffer.push(b"Allow: ") || !buffer.push(allow) || !buffer.push(b"\r\n"))
    {
        return false;
    }
    if let Some(location) = location {
        if !buffer.push(b"Location: ") || !buffer.push(location) || !buffer.push(b"\r\n") {
            return false;
        }
    }

    buffer.push(b"Connection: close\r\n\r\n")
}
