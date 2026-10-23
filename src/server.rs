//! The portable server loop and HTTP response writer.

use crate::config::Config;
use crate::fixedbuf::FixedBuf;
use crate::http::{self, Method, Status};
use crate::listing::{self, LISTING_BUF};
use crate::mime;
use crate::multipart::{self, Sink, SinkError};
use crate::path::{self, MAX_PATH, MAX_URL_PATH};
use crate::platform::{AsyncPlatform, Error, FileInfo, Interface, Platform, SocketAddr};

pub(crate) const REQUEST_BUF: usize = 8192;
const FILE_BUF: usize = 16 * 1024;
pub(crate) const HEADER_BUF: usize = 4096;
const LOG_BUF: usize = 1200;
const LOCATION_BUF: usize = MAX_URL_PATH * 3 + 2;

const ALLOW_READ: &[u8] = b"GET, HEAD";
const ALLOW_UPLOAD: &[u8] = b"GET, HEAD, PUT, POST";
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

    /// Run the experimental non-blocking GET/HEAD server.
    pub fn run_async(self) -> !
    where
        P: AsyncPlatform,
    {
        crate::async_server::run(self.platform, self.config)
    }

    /// Bind, then serve connections forever. Fatal errors terminate the
    /// process; this server intentionally has no supervision layer yet.
    pub fn run(&mut self) -> ! {
        if self.platform.set_root(self.config.root()).is_err() {
            self.platform.log(b"syswebd: root open failed\n");
            self.platform.exit(1)
        }

        let mut addr = self.config.addr();
        let mut attempts = 0u16;
        let listener = loop {
            match self.platform.bind(addr, self.config.backlog()) {
                Ok(listener) => break listener,
                Err(Error::AddressInUse)
                    if !self.config.port_explicit() && attempts < 100 && addr.port < u16::MAX =>
                {
                    if !self.config.quiet() {
                        self.log_port_busy(addr.port);
                    }
                    addr.port += 1;
                    attempts += 1;
                }
                Err(_) => {
                    self.platform.log(b"syswebd: bind failed\n");
                    self.platform.exit(1)
                }
            }
        };

        if !self.config.quiet() {
            self.log_startup(addr);
        }

        let mut transient = false;
        loop {
            match self.platform.accept(&listener) {
                Ok(mut stream) => {
                    transient = false;
                    self.handle(&mut stream);
                }
                Err(Error::Interrupted) => continue,
                Err(Error::TooManyOpenFiles)
                | Err(Error::ConnectionAborted)
                | Err(Error::ConnectionReset)
                | Err(Error::WouldBlock) => {
                    if !self.config.quiet() && !transient {
                        self.platform
                            .log(b"syswebd: accept temporarily failed; retrying\n");
                        transient = true;
                    }
                    // TODO(async): schedule this retry through the reactor
                    // timer/deadline and delete Platform::sleep_ms; blocking
                    // here would stall the reactor.
                    self.platform.sleep_ms(100);
                }
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
            Method::Post => self.handle_post(stream, &request, &request_buf[..used]),
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
            if let Some((status, length)) =
                self.send_file(stream, &mut file, rel_path, info, request.range, head_only)
            {
                self.log_request(request.method, &url_path[..url_len], status, length);
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
                if let Some((status, length)) = self.send_file(
                    stream,
                    &mut index_file,
                    &index_path[..index_len],
                    index_info,
                    request.range,
                    head_only,
                ) {
                    self.log_request(request.method, &url_path[..url_len], status, length);
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

    fn handle_post(&mut self, stream: &mut P::Stream, request: &http::Request<'_>, raw: &[u8]) {
        if !self.config.upload() {
            self.drain_request_body(stream, request, raw);
            self.send_error(stream, Status::MethodNotAllowed, false, ALLOW_READ);
            return;
        }
        let form_redirect = request
            .target
            .windows(b"?sysweb-form=1".len())
            .any(|window| window == b"?sysweb-form=1");

        let content_length = match request.content_length {
            Some(length) => length,
            None => {
                self.send_error(stream, Status::LengthRequired, false, b"");
                return;
            }
        };
        let content_type = match request.content_type {
            Some(value) => value,
            None => {
                self.drain_request_body(stream, request, raw);
                self.send_error(stream, Status::UnsupportedMediaType, false, b"");
                return;
            }
        };
        let mut parser = match multipart::Parser::new(content_type) {
            Ok(parser) => parser,
            Err(multipart::ParseError::UnsupportedContentType) => {
                self.drain_request_body(stream, request, raw);
                self.send_error(stream, Status::UnsupportedMediaType, false, b"");
                return;
            }
            Err(_) => {
                self.drain_request_body(stream, request, raw);
                self.send_error(stream, Status::BadRequest, false, b"");
                return;
            }
        };

        let mut url_path = [0u8; MAX_URL_PATH];
        let url_len = match path::normalize(request.target, &mut url_path) {
            Ok(len) => len,
            Err(path::PathError::TooLong) => {
                self.drain_request_body(stream, request, raw);
                self.send_error(stream, Status::UriTooLong, false, b"");
                return;
            }
            Err(_) => {
                self.drain_request_body(stream, request, raw);
                self.send_error(stream, Status::BadRequest, false, b"");
                return;
            }
        };
        let rel_path = root_relative(&url_path[..url_len]);
        let target = if url_path[..url_len].ends_with(b"/") {
            MultipartTarget::Directory(rel_path)
        } else {
            MultipartTarget::File(rel_path)
        };

        let mut sink = MultipartSink::new(&mut self.platform, target, self.config.replace());
        let mut remaining = content_length;
        let leftover = &raw[request.body_offset..];
        let take = core::cmp::min(leftover.len() as u64, remaining) as usize;

        if take == 0
            && remaining > 0
            && request.expect_continue
            && sink.platform.write_all(stream, CONTINUE).is_err()
        {
            drop(sink);
            self.send_error(stream, Status::BadRequest, false, b"");
            return;
        }

        if take > 0 {
            if let Err(error) = parser.feed(&leftover[..take], &mut sink) {
                sink.abort();
                drop(sink);
                self.send_multipart_error(stream, error);
                return;
            }
            remaining -= take as u64;
        }

        let mut buffer = [0u8; FILE_BUF];
        while remaining > 0 {
            let want = core::cmp::min(remaining, FILE_BUF as u64) as usize;
            let count = match sink.platform.read(stream, &mut buffer[..want]) {
                Ok(0) => {
                    sink.abort();
                    drop(sink);
                    self.send_error(stream, Status::BadRequest, false, b"");
                    return;
                }
                Ok(count) => count,
                Err(_) => {
                    sink.abort();
                    drop(sink);
                    self.send_error(stream, Status::BadRequest, false, b"");
                    return;
                }
            };
            if let Err(error) = parser.feed(&buffer[..count], &mut sink) {
                sink.abort();
                drop(sink);
                self.send_multipart_error(stream, error);
                return;
            }
            remaining -= count as u64;
        }

        if let Err(error) = parser.finish(&mut sink) {
            sink.abort();
            drop(sink);
            self.send_multipart_error(stream, error);
            return;
        }
        drop(sink);

        let (status, body, location) = if form_redirect {
            (Status::SeeOther, b"".as_slice(), Some(&url_path[..url_len]))
        } else {
            (Status::Created, UPLOADED, None)
        };
        if self.send_body(
            stream,
            status,
            b"text/plain; charset=utf-8",
            body,
            false,
            b"",
            location,
        ) {
            self.log_request(
                Method::Post,
                &url_path[..url_len],
                status,
                body.len() as u64,
            );
        }
    }

    fn send_multipart_error(&mut self, stream: &mut P::Stream, error: multipart::ParseError) {
        let status = match error {
            multipart::ParseError::UnsupportedContentType => Status::UnsupportedMediaType,
            multipart::ParseError::NoFile | multipart::ParseError::Malformed => Status::BadRequest,
            multipart::ParseError::Sink(SinkError::Conflict) => Status::Conflict,
            multipart::ParseError::Sink(SinkError::Forbidden) => Status::Forbidden,
            multipart::ParseError::Sink(SinkError::NotFound) => Status::NotFound,
            multipart::ParseError::Sink(SinkError::TooManyFiles) => Status::BadRequest,
            multipart::ParseError::Sink(SinkError::Io) => Status::InternalServerError,
        };
        self.send_error(stream, status, false, b"");
    }

    fn handle_put(&mut self, stream: &mut P::Stream, request: &http::Request<'_>, raw: &[u8]) {
        if !self.config.upload() {
            self.drain_request_body(stream, request, raw);
            self.send_error(stream, Status::MethodNotAllowed, false, ALLOW_READ);
            return;
        }

        let mut url_path = [0u8; MAX_URL_PATH];
        let url_len = match path::normalize(request.target, &mut url_path) {
            Ok(len) => len,
            Err(path::PathError::TooLong) => {
                self.drain_request_body(stream, request, raw);
                self.send_error(stream, Status::UriTooLong, false, b"");
                return;
            }
            Err(_) => {
                self.drain_request_body(stream, request, raw);
                self.send_error(stream, Status::BadRequest, false, b"");
                return;
            }
        };
        if url_len == 1 {
            self.drain_request_body(stream, request, raw);
            self.send_error(stream, Status::BadRequest, false, b"");
            return;
        }

        let rel_path = root_relative(&url_path[..url_len]);

        if url_path[..url_len].ends_with(b"/") {
            self.drain_request_body(stream, request, raw);
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
        let mut file = match self.platform.create_file(rel_path, self.config.replace()) {
            Ok(file) => file,
            Err(Error::AlreadyExists) | Err(Error::IsADirectory) => {
                self.drain_request_body(stream, request, raw);
                self.send_error(stream, Status::Conflict, false, b"");
                return;
            }
            Err(Error::NotFound) => {
                self.drain_request_body(stream, request, raw);
                self.send_error(stream, Status::NotFound, false, b"");
                return;
            }
            Err(Error::PermissionDenied) => {
                self.drain_request_body(stream, request, raw);
                self.send_error(stream, Status::Forbidden, false, b"");
                return;
            }
            Err(_) => {
                self.drain_request_body(stream, request, raw);
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

    /// Write headers and, unless this is a HEAD request, stream the selected
    /// byte range. Returns the status and body length on success.
    fn send_file(
        &mut self,
        stream: &mut P::Stream,
        file: &mut P::File,
        mime_path: &[u8],
        info: FileInfo,
        range: Option<http::RangeSpec>,
        head_only: bool,
    ) -> Option<(Status, u64)> {
        let selected = match range {
            Some(spec) => match resolve_range(spec, info.len) {
                Some(range) => Some(range),
                None => {
                    self.send_range_error(stream, info.len, head_only);
                    return None;
                }
            },
            None => None,
        };
        let (status, start, length) = match selected {
            Some((start, end)) => (Status::PartialContent, start, end - start + 1),
            None => (Status::Ok, 0, info.len),
        };

        let mut content_range = FixedBuf::<96>::new();
        if let Some((start, end)) = selected {
            let _ = content_range.push(b"bytes ");
            let _ = content_range.push_u64(start);
            let _ = content_range.push_byte(b'-');
            let _ = content_range.push_u64(end);
            let _ = content_range.push_byte(b'/');
            let _ = content_range.push_u64(info.len);
        }

        let content_type = mime::content_type(mime_path);
        let mut header = FixedBuf::<HEADER_BUF>::new();
        if !build_header(
            &mut header,
            status,
            length,
            content_type,
            b"",
            None,
            if selected.is_some() {
                Some(content_range.as_slice())
            } else {
                None
            },
            true,
            false,
        ) {
            let _ = self.send_fallback(stream);
            return None;
        }
        if self.platform.write_all(stream, header.as_slice()).is_err() {
            return None;
        }
        if head_only {
            return Some((status, length));
        }

        match self.platform.send_file(stream, file, start, length) {
            Ok(sent) if sent == length => Some((status, length)),
            _ => None,
        }
    }

    fn send_range_error(&mut self, stream: &mut P::Stream, total: u64, head_only: bool) {
        let body = Status::RangeNotSatisfiable.error_body();
        let mut content_range = FixedBuf::<64>::new();
        let _ = content_range.push(b"bytes */");
        let _ = content_range.push_u64(total);

        let mut header = FixedBuf::<HEADER_BUF>::new();
        if !build_header(
            &mut header,
            Status::RangeNotSatisfiable,
            body.len() as u64,
            b"text/plain; charset=utf-8",
            b"",
            None,
            Some(content_range.as_slice()),
            true,
            false,
        ) {
            let _ = self.send_fallback(stream);
            return;
        }
        if self.platform.write_all(stream, header.as_slice()).is_err() {
            return;
        }
        if !head_only && !body.is_empty() && self.platform.write_all(stream, body).is_err() {
            return;
        }
        self.log_error(Status::RangeNotSatisfiable);
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
            None,
            false,
            false,
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

    /// Drain a small, already-announced request body before closing. Closing
    /// with unread input can turn the final FIN into a reset on some stacks
    /// (notably macOS), which can discard the response the client is reading.
    fn drain_request_body(
        &mut self,
        stream: &mut P::Stream,
        request: &http::Request<'_>,
        raw: &[u8],
    ) {
        if request.expect_continue {
            return;
        }
        let Some(length) = request.content_length else {
            return;
        };
        let buffered = raw.len().saturating_sub(request.body_offset) as u64;
        let mut remaining = length.saturating_sub(core::cmp::min(buffered, length));
        const MAX_DRAIN: u64 = 64 * 1024;
        if remaining == 0 || remaining > MAX_DRAIN {
            return;
        }
        let mut buffer = [0u8; 4096];
        while remaining > 0 {
            let want = core::cmp::min(remaining, buffer.len() as u64) as usize;
            match self.platform.read(stream, &mut buffer[..want]) {
                Ok(0) | Err(_) => break,
                Ok(count) => remaining -= count as u64,
            }
        }
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

    fn log_port_busy(&mut self, port: u16) {
        let mut line = FixedBuf::<96>::new();
        let _ = line.push(b"syswebd: port ");
        let _ = line.push_u64(port as u64);
        let _ = line.push(b" is busy, trying ");
        let _ = line.push_u64(port as u64 + 1);
        let _ = line.push(b"\n");
        self.platform.log(line.as_slice());
    }

    fn log_startup(&mut self, addr: SocketAddr) {
        log_startup(
            &mut self.platform,
            addr,
            self.config.root(),
            self.config.upload(),
            b"",
        );
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

enum MultipartTarget<'a> {
    Directory(&'a [u8]),
    File(&'a [u8]),
}

struct MultipartSink<'a, P: Platform> {
    platform: &'a mut P,
    target: MultipartTarget<'a>,
    replace: bool,
    file: Option<P::File>,
    path: [u8; MAX_PATH],
    path_len: usize,
    files: usize,
}

impl<'a, P: Platform> MultipartSink<'a, P> {
    fn new(platform: &'a mut P, target: MultipartTarget<'a>, replace: bool) -> Self {
        Self {
            platform,
            target,
            replace,
            file: None,
            path: [0; MAX_PATH],
            path_len: 0,
            files: 0,
        }
    }

    fn abort(&mut self) {
        self.file = None;
        if self.path_len > 0 {
            let _ = self.platform.remove_file(&self.path[..self.path_len]);
        }
    }
}

impl<P: Platform> Sink for MultipartSink<'_, P> {
    fn begin_file(&mut self, filename: &[u8]) -> core::result::Result<(), SinkError> {
        if self.files > 0 {
            return Err(SinkError::TooManyFiles);
        }

        let mut len;
        match self.target {
            MultipartTarget::File(path) => {
                if path.len() + filename.len() + 1 >= MAX_PATH {
                    return Err(SinkError::Io);
                }
                self.path[..path.len()].copy_from_slice(path);
                len = path.len();
            }
            MultipartTarget::Directory(directory) => {
                if directory == b"." {
                    if filename.len() + 1 >= MAX_PATH {
                        return Err(SinkError::Io);
                    }
                    self.path[..filename.len()].copy_from_slice(filename);
                    len = filename.len();
                } else {
                    if directory.len() + filename.len() + 2 >= MAX_PATH {
                        return Err(SinkError::Io);
                    }
                    self.path[..directory.len()].copy_from_slice(directory);
                    len = directory.len();
                    if self.path[len - 1] != b'/' {
                        self.path[len] = b'/';
                        len += 1;
                    }
                    self.path[len..len + filename.len()].copy_from_slice(filename);
                    len += filename.len();
                }
            }
        }

        let file = self
            .platform
            .create_file(&self.path[..len], self.replace)
            .map_err(|error| match error {
                Error::AlreadyExists | Error::IsADirectory => SinkError::Conflict,
                Error::PermissionDenied => SinkError::Forbidden,
                Error::NotFound => SinkError::NotFound,
                _ => SinkError::Io,
            })?;
        self.path_len = len;
        self.file = Some(file);
        Ok(())
    }

    fn write(&mut self, data: &[u8]) -> core::result::Result<(), SinkError> {
        let file = self.file.as_mut().ok_or(SinkError::Io)?;
        self.platform
            .write_file(file, data)
            .map_err(|_| SinkError::Io)
    }

    fn end_file(&mut self) -> core::result::Result<(), SinkError> {
        if self.file.take().is_none() {
            return Err(SinkError::Io);
        }
        self.files += 1;
        Ok(())
    }
}

/// Convert a normalized URL path (`/a/b`) into a path relative to the
/// document-root fd (`a/b`). The root itself is `"."`.
pub(crate) fn root_relative(url_path: &[u8]) -> &[u8] {
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

pub(crate) fn log_startup<P: Platform>(
    platform: &mut P,
    addr: SocketAddr,
    root: &[u8],
    upload: bool,
    async_label: &[u8],
) {
    let mut line = FixedBuf::<LOG_BUF>::new();
    let _ = line.push(b"syswebd ");
    let _ = line.push(crate::VERSION.as_bytes());
    let _ = line.push(async_label);
    if addr.ip == [0, 0, 0, 0] {
        let _ = line.push(b" listening on port ");
        let _ = line.push_u64(addr.port as u64);
        let _ = line.push(b" of all interfaces\n");
        platform.log(line.as_slice());

        let mut interfaces = [Interface::default(); 8];
        let count = platform.interfaces(&mut interfaces);
        for index in 0..count {
            let interface = &interfaces[index];
            if interface.addr == [0, 0, 0, 0] {
                continue;
            }
            let mut duplicate = false;
            for previous in interfaces.iter().take(index) {
                if previous.addr == interface.addr {
                    duplicate = true;
                    break;
                }
            }
            if duplicate {
                continue;
            }
            let mut host = FixedBuf::<128>::new();
            let _ = host.push(b"  ");
            if interface.name_len > 0 {
                let _ = host.push(&interface.name[..interface.name_len]);
            } else {
                let _ = host.push(b"if");
                let _ = host.push_u64(index as u64);
            }
            let _ = host.push(b": http://");
            let _ = host.push_ipv4(interface.addr);
            let _ = host.push_byte(b':');
            let _ = host.push_u64(addr.port as u64);
            let _ = host.push_byte(b'\n');
            platform.log(host.as_slice());
        }
        if count == 0 {
            let mut host = FixedBuf::<128>::new();
            let _ = host.push(b"  127.0.0.1: http://127.0.0.1:");
            let _ = host.push_u64(addr.port as u64);
            let _ = host.push_byte(b'\n');
            platform.log(host.as_slice());
        }

        let mut tail = FixedBuf::<LOG_BUF>::new();
        let _ = tail.push(b"  root: ");
        let _ = tail.push(root);
        let _ = tail.push(b", uploads: ");
        let _ = tail.push(if upload { b"on" } else { b"off" });
        let _ = tail.push_byte(b'\n');
        platform.log(tail.as_slice());
    } else {
        let _ = line.push(b" listening on http://");
        let _ = line.push_ipv4(addr.ip);
        let _ = line.push_byte(b':');
        let _ = line.push_u64(addr.port as u64);
        let _ = line.push(b" (root: ");
        let _ = line.push(root);
        let _ = line.push(b", uploads: ");
        let _ = line.push(if upload { b"on" } else { b"off" });
        let _ = line.push(b")\n");
        platform.log(line.as_slice());
    }
}

pub(crate) fn resolve_range(spec: http::RangeSpec, total: u64) -> Option<(u64, u64)> {
    if total == 0 {
        return None;
    }
    match spec {
        http::RangeSpec::FromTo(start, end) => {
            if start >= total {
                None
            } else {
                Some((start, core::cmp::min(end, total - 1)))
            }
        }
        http::RangeSpec::From(start) => {
            if start >= total {
                None
            } else {
                Some((start, total - 1))
            }
        }
        http::RangeSpec::Suffix(length) => {
            if length == 0 {
                None
            } else {
                Some((total.saturating_sub(length), total - 1))
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn build_header(
    buffer: &mut FixedBuf<HEADER_BUF>,
    status: Status,
    content_length: u64,
    content_type: &[u8],
    allow: &[u8],
    location: Option<&[u8]>,
    content_range: Option<&[u8]>,
    accept_ranges: bool,
    keep_alive: bool,
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

    if accept_ranges && (!buffer.push(b"Accept-Ranges: bytes\r\n")) {
        return false;
    }
    if let Some(content_range) = content_range {
        if !buffer.push(b"Content-Range: ") || !buffer.push(content_range) || !buffer.push(b"\r\n")
        {
            return false;
        }
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

    if keep_alive {
        buffer.push(b"Connection: keep-alive\r\n\r\n")
    } else {
        buffer.push(b"Connection: close\r\n\r\n")
    }
}
