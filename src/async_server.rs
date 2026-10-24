//! Experimental non-blocking GET/HEAD server.
//!
//! This is intentionally a separate path from the blocking `Server`: the
//! The readiness reactor drives the upload/response state machine. Metadata and listings remain synchronous;
//! socket progress, header writes, and file transfer are non-blocking.

use crate::config::Config;
use crate::fixedbuf::FixedBuf;
use crate::http::{self, Method, Status};
use crate::listing;
use crate::mime;
use crate::multipart::{self, Sink, SinkError};
use crate::path::{self, MAX_PATH, MAX_URL_PATH};
use crate::platform::{Error, Event, Interest, Platform};
use crate::server::{
    build_header, build_stream_header, resolve_range, root_relative, HEADER_BUF, REQUEST_BUF,
};

#[cfg(windows)]
const MAX_CONNECTIONS: usize = 4;
#[cfg(not(windows))]
const MAX_CONNECTIONS: usize = 8;
const RESPONSE_BUF: usize = HEADER_BUF + 512;
const EVENT_BATCH: usize = 64;
const TICK_MS: i32 = 1000;
const BODY_BUF: usize = 16 * 1024;
const CONTINUE: &[u8] = b"HTTP/1.1 100 Continue\r\n\r\n";
const UPLOADED: &[u8] = b"UPLOADED\n";
const CREATED: &[u8] = b"CREATED\n";
const ALLOW_READ: &[u8] = b"GET, HEAD";
const MAX_REQUESTS_PER_CONNECTION: u32 = 1000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    Idle,
    Reading,
    Writing,
    SendingFile,
    SendingContinue,
    ReadingBody,
    Draining,
    StreamingListing,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ListingPhase {
    Head,
    Entries,
    Tail,
    Final,
}

#[allow(clippy::large_enum_variant)]
enum Upload<P: Platform> {
    Put(PutUpload<P>),
    Post(PostUpload<P>),
    Discard { remaining: u64 },
}

struct PutUpload<P: Platform> {
    file: P::File,
    path: [u8; MAX_PATH],
    path_len: usize,
    remaining: u64,
}

struct PostUpload<P: Platform> {
    parser: multipart::Parser,
    target: [u8; MAX_PATH],
    target_len: usize,
    target_is_dir: bool,
    replace: bool,
    file: Option<P::File>,
    path: [u8; MAX_PATH],
    path_len: usize,
    files: usize,
    remaining: u64,
}

struct PostSink<'a, P: Platform> {
    platform: &'a mut P,
    target: &'a [u8],
    target_is_dir: bool,
    replace: bool,
    file: &'a mut Option<P::File>,
    path: &'a mut [u8; MAX_PATH],
    path_len: &'a mut usize,
    files: &'a mut usize,
}

impl<P: Platform> Sink for PostSink<'_, P> {
    fn begin_file(&mut self, filename: &[u8]) -> core::result::Result<(), SinkError> {
        if *self.files > 0 {
            return Err(SinkError::TooManyFiles);
        }
        let mut len;
        if self.target_is_dir {
            if self.target == b"." {
                if filename.len() + 1 >= MAX_PATH {
                    return Err(SinkError::Io);
                }
                self.path[..filename.len()].copy_from_slice(filename);
                len = filename.len();
            } else {
                if self.target.len() + filename.len() + 2 >= MAX_PATH {
                    return Err(SinkError::Io);
                }
                self.path[..self.target.len()].copy_from_slice(self.target);
                len = self.target.len();
                if self.path[len - 1] != b'/' {
                    self.path[len] = b'/';
                    len += 1;
                }
                self.path[len..len + filename.len()].copy_from_slice(filename);
                len += filename.len();
            }
        } else {
            if self.target.len() + filename.len() + 1 >= MAX_PATH {
                return Err(SinkError::Io);
            }
            self.path[..self.target.len()].copy_from_slice(self.target);
            len = self.target.len();
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
        *self.path_len = len;
        *self.file = Some(file);
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
        *self.files += 1;
        Ok(())
    }
}

struct Slot<P: Platform> {
    stream: Option<P::Stream>,
    file: Option<P::File>,
    state: State,
    request: [u8; REQUEST_BUF],
    request_len: usize,
    response: [u8; RESPONSE_BUF],
    response_len: usize,
    response_pos: usize,
    file_offset: u64,
    file_remaining: u64,
    upload: Option<Upload<P>>,
    body: [u8; BODY_BUF],
    dir: Option<P::Dir>,
    listing_encoded: [u8; MAX_PATH],
    listing_encoded_len: usize,
    listing_entries: usize,
    listing_phase: ListingPhase,
    listing_prefix: [u8; 32],
    listing_prefix_len: usize,
    listing_prefix_pos: usize,
    listing_data_len: usize,
    listing_data_pos: usize,
    listing_suffix_pos: usize,
    listing_chunked: bool,
    listing_final: bool,
    continue_pos: usize,
    drain_remaining: u64,
    head_only: bool,
    status: Status,
    log_method: Method,
    quiet: bool,
    idle_ticks: u32,
    log_path: [u8; MAX_URL_PATH],
    log_path_len: usize,
    log_len: u64,
    keep_alive: bool,
    requests: u32,
    body_end: usize,
    form_redirect: bool,
}

impl<P: Platform> Slot<P> {
    fn new() -> Self {
        Self {
            stream: None,
            file: None,
            state: State::Idle,
            request: [0; REQUEST_BUF],
            request_len: 0,
            response: [0; RESPONSE_BUF],
            response_len: 0,
            response_pos: 0,
            file_offset: 0,
            file_remaining: 0,
            upload: None,
            body: [0; BODY_BUF],
            dir: None,
            listing_encoded: [0; MAX_PATH],
            listing_encoded_len: 0,
            listing_entries: 0,
            listing_phase: ListingPhase::Head,
            listing_prefix: [0; 32],
            listing_prefix_len: 0,
            listing_prefix_pos: 0,
            listing_data_len: 0,
            listing_data_pos: 0,
            listing_suffix_pos: 0,
            listing_chunked: false,
            listing_final: false,
            continue_pos: 0,
            drain_remaining: 0,
            head_only: false,
            status: Status::Ok,
            log_method: Method::Get,
            quiet: true,
            idle_ticks: 0,
            log_path: [0; MAX_URL_PATH],
            log_path_len: 0,
            log_len: 0,
            keep_alive: false,
            requests: 0,
            body_end: 0,
            form_redirect: false,
        }
    }

    fn reset(&mut self, stream: P::Stream) {
        self.stream = Some(stream);
        self.file = None;
        self.state = State::Reading;
        self.request_len = 0;
        self.response_len = 0;
        self.response_pos = 0;
        self.file_offset = 0;
        self.file_remaining = 0;
        self.upload = None;
        self.dir = None;
        self.listing_encoded_len = 0;
        self.listing_entries = 0;
        self.listing_phase = ListingPhase::Head;
        self.listing_prefix_len = 0;
        self.listing_prefix_pos = 0;
        self.listing_data_len = 0;
        self.listing_data_pos = 0;
        self.listing_suffix_pos = 0;
        self.listing_chunked = false;
        self.listing_final = false;
        self.continue_pos = 0;
        self.drain_remaining = 0;
        self.head_only = false;
        self.status = Status::Ok;
        self.log_method = Method::Get;
        self.quiet = true;
        self.idle_ticks = 0;
        self.log_path_len = 0;
        self.log_len = 0;
        self.keep_alive = false;
        self.requests = 0;
        self.body_end = 0;
        self.form_redirect = false;
    }

    #[allow(clippy::too_many_arguments)]
    fn prepare_response(
        &mut self,
        status: Status,
        content_length: u64,
        content_type: &[u8],
        body: &[u8],
        head_only: bool,
        location: Option<&[u8]>,
        content_range: Option<&[u8]>,
        accept_ranges: bool,
    ) -> bool {
        let mut header = FixedBuf::<HEADER_BUF>::new();
        if !build_header(
            &mut header,
            status,
            content_length,
            content_type,
            b"",
            location,
            content_range,
            accept_ranges,
            self.keep_alive && self.requests + 1 < MAX_REQUESTS_PER_CONNECTION,
        ) {
            return false;
        }
        let header = header.as_slice();
        if header.len() > self.response.len() {
            return false;
        }
        self.response[..header.len()].copy_from_slice(header);
        let mut len = header.len();
        if !head_only && !body.is_empty() {
            if len + body.len() > self.response.len() {
                return false;
            }
            self.response[len..len + body.len()].copy_from_slice(body);
            len += body.len();
        }
        self.response_len = len;
        self.response_pos = 0;
        self.head_only = head_only;
        self.status = status;
        self.state = State::Writing;
        true
    }

    fn prepare_error_allow(&mut self, status: Status, head_only: bool, allow: &[u8]) {
        self.keep_alive = false;
        let body = status.error_body();
        self.log_len = body.len() as u64;
        let mut header = FixedBuf::<HEADER_BUF>::new();
        if !build_header(
            &mut header,
            status,
            body.len() as u64,
            b"text/plain; charset=utf-8",
            allow,
            None,
            None,
            false,
            false,
        ) {
            return;
        }
        let header = header.as_slice();
        self.response[..header.len()].copy_from_slice(header);
        let mut len = header.len();
        if !head_only && !body.is_empty() {
            self.response[len..len + body.len()].copy_from_slice(body);
            len += body.len();
        }
        self.response_len = len;
        self.response_pos = 0;
        self.head_only = head_only;
        self.status = status;
        self.state = State::Writing;
    }

    fn prepare_error(&mut self, status: Status, head_only: bool) {
        self.keep_alive = false;
        let body = status.error_body();
        self.log_len = body.len() as u64;
        let _ = self.prepare_response(
            status,
            body.len() as u64,
            b"text/plain; charset=utf-8",
            body,
            head_only,
            None,
            None,
            false,
        );
    }
}

pub fn run<P: Platform>(mut platform: P, config: Config) -> ! {
    if platform.set_root(config.root()).is_err() {
        platform.log(b"syswebd: root open failed\n");
        platform.exit(1)
    }

    let mut addr = config.addr();
    let mut attempts = 0u16;
    let listener = loop {
        match platform.bind(addr, config.backlog()) {
            Ok(listener) => break listener,
            Err(Error::AddressInUse)
                if !config.port_explicit() && attempts < 100 && addr.port < u16::MAX =>
            {
                addr.port += 1;
                attempts += 1;
            }
            Err(_) => {
                platform.log(b"syswebd: async bind failed\n");
                platform.exit(1)
            }
        }
    };
    if platform.set_listener_nonblocking(&listener).is_err() {
        platform.log(b"syswebd: async listener setup failed\n");
        platform.exit(1)
    }
    let mut poller = match platform.poller() {
        Ok(poller) => poller,
        Err(_) => {
            platform.log(b"syswebd: reactor creation failed\n");
            platform.exit(1)
        }
    };
    if platform
        .register_listener(&mut poller, &listener, 0)
        .is_err()
    {
        platform.log(b"syswebd: listener registration failed\n");
        platform.exit(1)
    }

    if !config.quiet() {
        crate::server::log_startup(
            &mut platform,
            addr,
            config.root(),
            config.upload(),
            b" (async)",
        );
    }

    let mut slots: [Slot<P>; MAX_CONNECTIONS] = core::array::from_fn(|_| Slot::new());
    let mut events = [Event::default(); EVENT_BATCH];

    loop {
        let ready = match platform.wait(&mut poller, &mut events, TICK_MS) {
            Ok(ready) => ready,
            Err(Error::Interrupted) => continue,
            Err(error) => {
                let mut line = FixedBuf::<96>::new();
                let _ = line.push(b"syswebd: reactor wait failed: ");
                let code = match error {
                    Error::Other(code) => code as u64,
                    Error::WouldBlock => 11,
                    Error::Interrupted => 4,
                    Error::InvalidInput => 22,
                    _ => 0,
                };
                let _ = line.push_u64(code);
                let _ = line.push_byte(b'\n');
                platform.log(line.as_slice());
                platform.exit(1)
            }
        };
        if ready == 0 {
            for (index, slot) in slots.iter_mut().enumerate() {
                if slot.state == State::Idle {
                    continue;
                }
                slot.idle_ticks += 1;
                if slot.idle_ticks >= config.idle_seconds() as u32 {
                    close_slot(&mut platform, &mut poller, slot, index + 1);
                }
            }
        }
        for event in &events[..ready] {
            if event.token == 0 {
                accept_ready(
                    &mut platform,
                    &mut poller,
                    &listener,
                    &mut slots,
                    config.quiet(),
                );
            } else if let Some(index) = event.token.checked_sub(1) {
                if index < slots.len() {
                    handle_event(
                        &mut platform,
                        &mut poller,
                        &config,
                        &mut slots,
                        index,
                        event,
                    );
                }
            }
        }
    }
}

fn accept_ready<P: Platform>(
    platform: &mut P,
    poller: &mut P::Poller,
    listener: &P::Listener,
    slots: &mut [Slot<P>; MAX_CONNECTIONS],
    quiet: bool,
) {
    loop {
        match platform.accept_nonblocking(listener) {
            Ok(stream) => {
                let Some(index) = slots.iter().position(|slot| slot.state == State::Idle) else {
                    drop(stream);
                    continue;
                };
                slots[index].reset(stream);
                slots[index].quiet = quiet;
                let token = index + 1;
                let registered = slots[index]
                    .stream
                    .as_ref()
                    .map(|stream| platform.register_stream(poller, stream, token))
                    .unwrap_or(Err(Error::InvalidInput));
                if registered.is_err() {
                    close_slot(platform, poller, &mut slots[index], token);
                }
            }
            Err(Error::WouldBlock) | Err(Error::Interrupted) => break,
            Err(Error::TooManyOpenFiles)
            | Err(Error::ConnectionAborted)
            | Err(Error::ConnectionReset) => break,
            Err(_) => break,
        }
    }
}

fn handle_event<P: Platform>(
    platform: &mut P,
    poller: &mut P::Poller,
    config: &Config,
    slots: &mut [Slot<P>; MAX_CONNECTIONS],
    index: usize,
    event: &Event,
) {
    if event.error {
        close_slot(platform, poller, &mut slots[index], index + 1);
        return;
    }
    slots[index].idle_ticks = 0;
    slots[index].quiet = config.quiet();
    match slots[index].state {
        State::Reading => read_slot(platform, poller, config, &mut slots[index], index + 1),
        State::Writing => write_slot(platform, poller, config, &mut slots[index], index + 1),
        State::SendingFile => {
            send_file_slot(platform, poller, config, &mut slots[index], index + 1)
        }
        State::SendingContinue => {
            write_continue_slot(platform, poller, &mut slots[index], index + 1)
        }
        State::ReadingBody => read_body_slot(platform, poller, &mut slots[index], index + 1),
        State::Draining => drain_slot(platform, poller, &mut slots[index], index + 1),
        State::StreamingListing => {
            listing_slot(platform, poller, config, &mut slots[index], index + 1)
        }
        State::Idle => {}
    }
}

fn read_slot<P: Platform>(
    platform: &mut P,
    poller: &mut P::Poller,
    config: &Config,
    slot: &mut Slot<P>,
    token: usize,
) {
    loop {
        if slot.request_len == slot.request.len() {
            slot.prepare_error(Status::RequestHeaderFieldsTooLarge, false);
            transition(platform, poller, slot, token);
            return;
        }
        let stream = match slot.stream.as_mut() {
            Some(stream) => stream,
            None => return,
        };
        let read = platform.read_nonblocking(stream, &mut slot.request[slot.request_len..]);
        match read {
            Ok(0) => {
                close_slot(platform, poller, slot, token);
                return;
            }
            Ok(count) => {
                slot.request_len += count;
                if http::header_end(&slot.request[..slot.request_len]).is_some() {
                    prepare_request(platform, config, slot);
                    transition(platform, poller, slot, token);
                    return;
                }
            }
            Err(Error::Interrupted) => continue,
            Err(Error::WouldBlock) => return,
            Err(_) => {
                close_slot(platform, poller, slot, token);
                return;
            }
        }
    }
}

fn prepare_request<P: Platform>(platform: &mut P, config: &Config, slot: &mut Slot<P>) {
    let request = match http::parse(&slot.request[..slot.request_len]) {
        Ok(request) => request,
        Err(http::ParseError::UnsupportedMethod) => {
            slot.prepare_error_allow(Status::MethodNotAllowed, false, ALLOW_READ);
            return;
        }
        Err(http::ParseError::UnsupportedVersion) => {
            slot.prepare_error(Status::VersionNotSupported, false);
            return;
        }
        Err(http::ParseError::Malformed) => {
            slot.prepare_error(Status::BadRequest, false);
            return;
        }
    };

    if let Some((user, pass)) = config.userpass() {
        if !crate::auth::authorized(request.authorization, user, pass) {
            slot.prepare_error(Status::Unauthorized, request.method == Method::Head);
            return;
        }
    }
    let method = request.method;
    slot.log_method = method;
    let range = request.range;
    let head_only = method == Method::Head;
    let content_length = request.content_length;
    let expect_continue = request.expect_continue;
    let body_offset = request.body_offset;
    let body_len = content_length.unwrap_or(0).min(usize::MAX as u64) as usize;
    slot.body_end = body_offset.saturating_add(body_len).min(slot.request_len);
    slot.form_redirect = method == Method::Post
        && request
            .target
            .windows(b"?sysweb-form=1".len())
            .any(|window| window == b"?sysweb-form=1");
    slot.keep_alive = request.connection.keep_alive_for(request.version)
        && !(matches!(method, Method::Get | Method::Head) && content_length.unwrap_or(0) > 0);
    let mut content_type = [0u8; 256];
    let mut content_type_len = 0usize;
    if let Some(value) = request.content_type {
        if value.len() > content_type.len() {
            slot.prepare_error(Status::UnsupportedMediaType, false);
            return;
        }
        content_type[..value.len()].copy_from_slice(value);
        content_type_len = value.len();
    }
    let mut url_path = [0u8; MAX_URL_PATH];
    let url_len = match path::normalize(request.target, &mut url_path) {
        Ok(len) => len,
        Err(path::PathError::TooLong) => {
            slot.prepare_error(Status::UriTooLong, head_only);
            return;
        }
        Err(_) => {
            slot.prepare_error(Status::BadRequest, head_only);
            return;
        }
    };
    if url_len > slot.log_path.len() {
        slot.prepare_error(Status::UriTooLong, head_only);
        return;
    }
    slot.log_path[..url_len].copy_from_slice(&url_path[..url_len]);
    slot.log_path_len = url_len;

    if method == Method::Get
        && request
            .target
            .windows(b"?sysweb-text=1".len())
            .any(|window| window == b"?sysweb-text=1")
    {
        let mut target = [0u8; REQUEST_BUF];
        let target_len = request.target.len();
        target[..target_len].copy_from_slice(request.target);
        prepare_text_form(
            platform,
            config,
            slot,
            &url_path[..url_len],
            &target[..target_len],
        );
        return;
    }

    if method == Method::Put {
        prepare_put(
            platform,
            config,
            slot,
            content_length,
            expect_continue,
            body_offset,
            slot.request_len,
            &url_path[..url_len],
        );
        return;
    }
    if method == Method::Post {
        prepare_post(
            platform,
            config,
            slot,
            content_length,
            expect_continue,
            body_offset,
            slot.request_len,
            &url_path[..url_len],
            &content_type[..content_type_len],
        );
        return;
    }
    if method != Method::Get && method != Method::Head {
        slot.prepare_error_allow(Status::MethodNotAllowed, false, ALLOW_READ);
        return;
    }

    let rel_path = root_relative(&url_path[..url_len]);
    let file = match platform.open(rel_path) {
        Ok(file) => file,
        Err(Error::NotFound) | Err(Error::IsADirectory) => {
            slot.prepare_error(Status::NotFound, head_only);
            return;
        }
        Err(Error::PermissionDenied) => {
            slot.prepare_error(Status::Forbidden, head_only);
            return;
        }
        Err(_) => {
            slot.prepare_error(Status::InternalServerError, head_only);
            return;
        }
    };
    let info = match platform.file_info(&file) {
        Ok(info) => info,
        Err(_) => {
            slot.prepare_error(Status::InternalServerError, head_only);
            return;
        }
    };

    if !info.is_dir {
        prepare_file(slot, file, &url_path[..url_len], info, range, head_only);
        return;
    }

    if !url_path[..url_len].ends_with(b"/") {
        let mut location = FixedBuf::<{ MAX_URL_PATH * 3 + 2 }>::new();
        if !listing::url_encode_path(&mut location, &url_path[..url_len])
            || !location.push_byte(b'/')
        {
            slot.prepare_error(Status::UriTooLong, head_only);
            return;
        }
        let _ = slot.prepare_response(
            Status::MovedPermanently,
            0,
            b"text/plain; charset=utf-8",
            b"",
            head_only,
            Some(location.as_slice()),
            None,
            false,
        );
        slot.log_len = 0;
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
                slot.prepare_error(Status::UriTooLong, head_only);
                return;
            }
            Err(_) => {
                slot.prepare_error(Status::BadRequest, head_only);
                return;
            }
        }
    };

    match platform.open(&index_path[..index_len]) {
        Ok(index_file) => {
            let index_info = match platform.file_info(&index_file) {
                Ok(info) if !info.is_dir => info,
                _ => {
                    slot.prepare_error(Status::NotFound, head_only);
                    return;
                }
            };
            prepare_file(
                slot,
                index_file,
                &index_path[..index_len],
                index_info,
                range,
                head_only,
            );
        }
        Err(Error::NotFound) | Err(Error::IsADirectory) => {
            if !config.index() {
                slot.prepare_error(Status::Forbidden, head_only);
                return;
            }
            serve_listing(
                platform,
                slot,
                &url_path[..url_len],
                rel_path,
                config.upload(),
                head_only,
                request.version == http::Version::Http11,
            );
        }
        Err(Error::PermissionDenied) => slot.prepare_error(Status::Forbidden, head_only),
        Err(_) => slot.prepare_error(Status::InternalServerError, head_only),
    }
}

fn prepare_text_form<P: Platform>(
    platform: &mut P,
    config: &Config,
    slot: &mut Slot<P>,
    url_path: &[u8],
    target: &[u8],
) {
    let mut name = [0u8; MAX_PATH];
    let name_len = http::query_value(target, b"name", &mut name).unwrap_or(0);
    let mut text = [0u8; 4096];
    let text_len = match http::query_value(target, b"text", &mut text) {
        Some(len) => len,
        None => {
            slot.prepare_error(Status::UriTooLong, false);
            return;
        }
    };
    let name = if name_len == 0 {
        b"pasted.txt".as_slice()
    } else {
        &name[..name_len]
    };
    let start = name
        .iter()
        .rposition(|&byte| byte == b'/' || byte == b'\\')
        .map(|index| index + 1)
        .unwrap_or(0);
    let name = &name[start..];
    if name.is_empty() || name == b"." || name == b".." {
        slot.prepare_error(Status::BadRequest, false);
        return;
    }
    let rel_path = name;
    let mut file = match platform.create_file(rel_path, config.replace()) {
        Ok(file) => file,
        Err(Error::AlreadyExists) | Err(Error::IsADirectory) => {
            slot.prepare_error(Status::Conflict, false);
            return;
        }
        Err(Error::PermissionDenied) => {
            slot.prepare_error(Status::Forbidden, false);
            return;
        }
        Err(_) => {
            slot.prepare_error(Status::InternalServerError, false);
            return;
        }
    };
    if platform.write_file(&mut file, &text[..text_len]).is_err() {
        drop(file);
        let _ = platform.remove_file(rel_path);
        slot.prepare_error(Status::InternalServerError, false);
        return;
    }
    drop(file);
    let _ = slot.prepare_response(
        Status::SeeOther,
        0,
        b"text/plain; charset=utf-8",
        b"",
        false,
        Some(url_path),
        None,
        false,
    );
    slot.log_len = 0;
}

#[allow(clippy::too_many_arguments)]
fn prepare_put<P: Platform>(
    platform: &mut P,
    config: &Config,
    slot: &mut Slot<P>,
    content_length: Option<u64>,
    expect_continue: bool,
    body_offset: usize,
    request_len: usize,
    url_path: &[u8],
) {
    if !config.upload() {
        slot.prepare_error_allow(Status::MethodNotAllowed, false, ALLOW_READ);
        slot.drain_remaining = content_length
            .unwrap_or(0)
            .saturating_sub((request_len - body_offset) as u64);
        return;
    }
    let url_len = url_path.len();
    if url_len == 1 {
        slot.prepare_error(Status::BadRequest, false);
        return;
    }
    let rel_path = root_relative(url_path);

    if url_path.ends_with(b"/") {
        let total = content_length.unwrap_or(0);
        match platform.create_dir(rel_path) {
            Ok(()) => {
                if total == 0 {
                    prepare_upload_response(slot, CREATED);
                } else {
                    slot.upload = Some(Upload::Discard { remaining: total });
                    slot.state = State::ReadingBody;
                }
            }
            Err(Error::AlreadyExists) => slot.prepare_error(Status::Conflict, false),
            Err(Error::NotFound) => slot.prepare_error(Status::NotFound, false),
            Err(Error::PermissionDenied) => slot.prepare_error(Status::Forbidden, false),
            Err(_) => slot.prepare_error(Status::InternalServerError, false),
        }
        return;
    }

    let Some(total) = content_length else {
        slot.prepare_error(Status::LengthRequired, false);
        return;
    };
    let mut file = match platform.create_file(rel_path, config.replace()) {
        Ok(file) => file,
        Err(Error::AlreadyExists) | Err(Error::IsADirectory) => {
            slot.prepare_error(Status::Conflict, false);
            return;
        }
        Err(Error::NotFound) => {
            slot.prepare_error(Status::NotFound, false);
            return;
        }
        Err(Error::PermissionDenied) => {
            slot.prepare_error(Status::Forbidden, false);
            return;
        }
        Err(_) => {
            slot.prepare_error(Status::InternalServerError, false);
            return;
        }
    };

    let mut remaining = total;
    let buffered = &slot.request[body_offset..request_len];
    let take = core::cmp::min(buffered.len() as u64, remaining) as usize;
    if take > 0 {
        if platform.write_file(&mut file, &buffered[..take]).is_err() {
            drop(file);
            let _ = platform.remove_file(rel_path);
            slot.prepare_error(Status::InternalServerError, false);
            return;
        }
        remaining -= take as u64;
    }

    let expect_continue = expect_continue && take == 0 && remaining > 0;
    if remaining == 0 {
        drop(file);
        prepare_upload_response(slot, UPLOADED);
        return;
    }
    let mut path = [0u8; MAX_PATH];
    if rel_path.len() >= path.len() {
        drop(file);
        let _ = platform.remove_file(rel_path);
        slot.prepare_error(Status::UriTooLong, false);
        return;
    }
    path[..rel_path.len()].copy_from_slice(rel_path);
    slot.upload = Some(Upload::Put(PutUpload {
        file,
        path,
        path_len: rel_path.len(),
        remaining,
    }));
    if expect_continue {
        slot.state = State::SendingContinue;
        slot.continue_pos = 0;
    } else {
        slot.state = State::ReadingBody;
    }
}

#[allow(clippy::too_many_arguments)]
fn prepare_post<P: Platform>(
    platform: &mut P,
    config: &Config,
    slot: &mut Slot<P>,
    content_length: Option<u64>,
    expect_continue: bool,
    body_offset: usize,
    request_len: usize,
    url_path: &[u8],
    content_type: &[u8],
) {
    if !config.upload() {
        slot.prepare_error_allow(Status::MethodNotAllowed, false, ALLOW_READ);
        slot.drain_remaining = content_length
            .unwrap_or(0)
            .saturating_sub((request_len - body_offset) as u64);
        return;
    }
    let Some(total) = content_length else {
        slot.prepare_error(Status::LengthRequired, false);
        return;
    };
    if content_type.is_empty() {
        slot.prepare_error(Status::UnsupportedMediaType, false);
        return;
    }
    let parser = match multipart::Parser::new(content_type) {
        Ok(parser) => parser,
        Err(multipart::ParseError::UnsupportedContentType) => {
            slot.prepare_error(Status::UnsupportedMediaType, false);
            return;
        }
        Err(_) => {
            slot.prepare_error(Status::BadRequest, false);
            return;
        }
    };
    let rel_path = root_relative(url_path);

    let mut post = PostUpload {
        parser,
        target: [0; MAX_PATH],
        target_len: rel_path.len(),
        target_is_dir: url_path.ends_with(b"/"),
        replace: config.replace(),
        file: None,
        path: [0; MAX_PATH],
        path_len: 0,
        files: 0,
        remaining: total,
    };
    if rel_path.len() > post.target.len() {
        slot.prepare_error(Status::UriTooLong, false);
        return;
    }
    post.target[..rel_path.len()].copy_from_slice(rel_path);

    let buffered = &slot.request[body_offset..request_len];
    let take = core::cmp::min(buffered.len() as u64, post.remaining) as usize;
    if take > 0 {
        if let Err(error) = feed_post(platform, &mut post, &buffered[..take]) {
            abort_post(platform, &mut post);
            slot.prepare_error(multipart_status(error), false);
            return;
        }
        post.remaining -= take as u64;
    }

    let expect_continue = expect_continue && take == 0 && post.remaining > 0;
    if post.remaining == 0 {
        if let Err(error) = finish_post(platform, &mut post) {
            abort_post(platform, &mut post);
            slot.prepare_error(multipart_status(error), false);
            return;
        }
        prepare_upload_response(slot, UPLOADED);
        return;
    }

    slot.upload = Some(Upload::Post(post));
    if expect_continue {
        slot.state = State::SendingContinue;
        slot.continue_pos = 0;
    } else {
        slot.state = State::ReadingBody;
    }
}

fn feed_post<P: Platform>(
    platform: &mut P,
    post: &mut PostUpload<P>,
    data: &[u8],
) -> core::result::Result<(), multipart::ParseError> {
    let mut sink = PostSink {
        platform,
        target: &post.target[..post.target_len],
        target_is_dir: post.target_is_dir,
        replace: post.replace,
        file: &mut post.file,
        path: &mut post.path,
        path_len: &mut post.path_len,
        files: &mut post.files,
    };
    post.parser.feed(data, &mut sink)
}

fn finish_post<P: Platform>(
    platform: &mut P,
    post: &mut PostUpload<P>,
) -> core::result::Result<(), multipart::ParseError> {
    let mut sink = PostSink {
        platform,
        target: &post.target[..post.target_len],
        target_is_dir: post.target_is_dir,
        replace: post.replace,
        file: &mut post.file,
        path: &mut post.path,
        path_len: &mut post.path_len,
        files: &mut post.files,
    };
    post.parser.finish(&mut sink)
}

fn abort_post<P: Platform>(platform: &mut P, post: &mut PostUpload<P>) {
    post.file = None;
    if post.path_len > 0 {
        let _ = platform.remove_file(&post.path[..post.path_len]);
        post.path_len = 0;
    }
}

fn multipart_status(error: multipart::ParseError) -> Status {
    match error {
        multipart::ParseError::UnsupportedContentType => Status::UnsupportedMediaType,
        multipart::ParseError::NoFile | multipart::ParseError::Malformed => Status::BadRequest,
        multipart::ParseError::Sink(SinkError::Conflict) => Status::Conflict,
        multipart::ParseError::Sink(SinkError::Forbidden) => Status::Forbidden,
        multipart::ParseError::Sink(SinkError::NotFound) => Status::NotFound,
        multipart::ParseError::Sink(SinkError::TooManyFiles) => Status::BadRequest,
        multipart::ParseError::Sink(SinkError::Io) => Status::InternalServerError,
    }
}

fn prepare_upload_response<P: Platform>(slot: &mut Slot<P>, body: &[u8]) {
    if slot.form_redirect {
        let mut location = [0u8; MAX_URL_PATH];
        let len = slot.log_path_len.min(location.len());
        location[..len].copy_from_slice(&slot.log_path[..len]);
        let _ = slot.prepare_response(
            Status::SeeOther,
            0,
            b"text/plain; charset=utf-8",
            b"",
            false,
            Some(&location[..len]),
            None,
            false,
        );
        slot.log_len = 0;
        return;
    }
    let _ = slot.prepare_response(
        Status::Created,
        body.len() as u64,
        b"text/plain; charset=utf-8",
        body,
        false,
        None,
        None,
        false,
    );
    slot.log_len = body.len() as u64;
}

fn finish_upload<P: Platform>(platform: &mut P, slot: &mut Slot<P>) {
    let Some(upload) = slot.upload.take() else {
        return;
    };
    let (status, body) = match upload {
        Upload::Put(upload) => {
            drop(upload);
            (Status::Created, UPLOADED)
        }
        Upload::Post(mut post) => {
            if let Err(error) = finish_post(platform, &mut post) {
                abort_post(platform, &mut post);
                let status = multipart_status(error);
                slot.prepare_error(status, false);
                return;
            }
            (Status::Created, UPLOADED)
        }
        Upload::Discard { .. } => (Status::Created, CREATED),
    };
    let _ = status;
    prepare_upload_response(slot, body);
}

fn read_body_slot<P: Platform>(
    platform: &mut P,
    poller: &mut P::Poller,
    slot: &mut Slot<P>,
    token: usize,
) {
    loop {
        let remaining = match slot.upload.as_ref() {
            Some(Upload::Put(upload)) => upload.remaining,
            Some(Upload::Post(upload)) => upload.remaining,
            Some(Upload::Discard { remaining }) => *remaining,
            None => {
                close_slot(platform, poller, slot, token);
                return;
            }
        };
        if remaining == 0 {
            finish_upload(platform, slot);
            transition(platform, poller, slot, token);
            return;
        }
        let want = core::cmp::min(remaining, slot.body.len() as u64) as usize;
        let stream = match slot.stream.as_mut() {
            Some(stream) => stream,
            None => return,
        };
        let read = platform.read_nonblocking(stream, &mut slot.body[..want]);
        let count = match read {
            Ok(0) => {
                abort_upload(platform, slot);
                slot.prepare_error(Status::BadRequest, false);
                transition(platform, poller, slot, token);
                return;
            }
            Ok(count) => count,
            Err(Error::Interrupted) => continue,
            Err(Error::WouldBlock) => return,
            Err(_) => {
                abort_upload(platform, slot);
                slot.prepare_error(Status::BadRequest, false);
                transition(platform, poller, slot, token);
                return;
            }
        };
        let error = match slot.upload.as_mut() {
            Some(Upload::Put(upload)) => {
                if platform
                    .write_file(&mut upload.file, &slot.body[..count])
                    .is_err()
                {
                    Some(Status::InternalServerError)
                } else {
                    upload.remaining -= count as u64;
                    None
                }
            }
            Some(Upload::Post(upload)) => match feed_post(platform, upload, &slot.body[..count]) {
                Ok(()) => {
                    upload.remaining -= count as u64;
                    None
                }
                Err(error) => Some(multipart_status(error)),
            },
            Some(Upload::Discard { remaining }) => {
                *remaining -= count as u64;
                None
            }
            None => Some(Status::InternalServerError),
        };
        if let Some(status) = error {
            abort_upload(platform, slot);
            slot.prepare_error(status, false);
            transition(platform, poller, slot, token);
            return;
        }
    }
}

fn abort_upload<P: Platform>(platform: &mut P, slot: &mut Slot<P>) {
    if let Some(upload) = slot.upload.take() {
        match upload {
            Upload::Put(upload) => {
                if upload.path_len > 0 {
                    let _ = platform.remove_file(&upload.path[..upload.path_len]);
                }
            }
            Upload::Post(mut post) => {
                abort_post(platform, &mut post);
            }
            Upload::Discard { .. } => {}
        }
    }
}

fn write_continue_slot<P: Platform>(
    platform: &mut P,
    poller: &mut P::Poller,
    slot: &mut Slot<P>,
    token: usize,
) {
    let stream = match slot.stream.as_mut() {
        Some(stream) => stream,
        None => return,
    };
    while slot.continue_pos < CONTINUE.len() {
        match platform.write_nonblocking(stream, &CONTINUE[slot.continue_pos..]) {
            Ok(0) => {
                close_slot(platform, poller, slot, token);
                return;
            }
            Ok(count) => slot.continue_pos += count,
            Err(Error::Interrupted) => continue,
            Err(Error::WouldBlock) => return,
            Err(_) => {
                close_slot(platform, poller, slot, token);
                return;
            }
        }
    }
    slot.state = State::ReadingBody;
    if platform
        .set_interest(poller, stream, token, Interest::Readable)
        .is_err()
    {
        close_slot(platform, poller, slot, token);
    }
}

#[allow(clippy::too_many_arguments)]
fn serve_listing<P: Platform>(
    platform: &mut P,
    slot: &mut Slot<P>,
    url_path: &[u8],
    rel_path: &[u8],
    upload: bool,
    head_only: bool,
    chunked: bool,
) {
    let dir = match platform.open_dir(rel_path) {
        Ok(dir) => dir,
        Err(Error::NotFound) | Err(Error::IsADirectory) => {
            slot.prepare_error(Status::NotFound, head_only);
            return;
        }
        Err(Error::PermissionDenied) => {
            slot.prepare_error(Status::Forbidden, head_only);
            return;
        }
        Err(_) => {
            slot.prepare_error(Status::InternalServerError, head_only);
            return;
        }
    };

    let mut encoded = FixedBuf::<MAX_PATH>::new();
    if !listing::encode_path(url_path, &mut encoded) {
        slot.prepare_error(Status::InternalServerError, head_only);
        return;
    }
    let mut head = FixedBuf::<BODY_BUF>::new();
    if !listing::render_head(&mut head, url_path, upload, encoded.as_slice()) {
        slot.prepare_error(Status::InternalServerError, head_only);
        return;
    }
    slot.keep_alive = chunked && slot.keep_alive;
    let mut header = FixedBuf::<HEADER_BUF>::new();
    if !build_stream_header(
        &mut header,
        b"text/html; charset=utf-8",
        chunked,
        slot.keep_alive,
    ) {
        slot.prepare_error(Status::InternalServerError, head_only);
        return;
    }
    slot.response[..header.len()].copy_from_slice(header.as_slice());
    slot.response_len = header.len();
    slot.response_pos = 0;
    if head_only {
        slot.state = State::Writing;
        slot.log_len = 0;
        return;
    }
    slot.body[..head.len()].copy_from_slice(head.as_slice());
    slot.listing_data_len = head.len();
    slot.listing_data_pos = 0;
    slot.listing_prefix_len = if chunked {
        chunk_prefix(head.len(), &mut slot.listing_prefix)
    } else {
        0
    };
    slot.listing_prefix_pos = 0;
    slot.listing_suffix_pos = if chunked { 0 } else { 2 };
    slot.listing_encoded[..encoded.len()].copy_from_slice(encoded.as_slice());
    slot.listing_encoded_len = encoded.len();
    slot.listing_entries = 0;
    slot.listing_phase = ListingPhase::Entries;
    slot.listing_chunked = chunked;
    slot.listing_final = false;
    slot.dir = Some(dir);
    slot.state = State::StreamingListing;
    slot.status = Status::Ok;
    slot.log_len = head.len() as u64;
}

fn chunk_prefix(len: usize, out: &mut [u8; 32]) -> usize {
    let mut digits = [0u8; 16];
    let mut value = len;
    let mut count = 0usize;
    if value == 0 {
        digits[0] = b'0';
        count = 1;
    } else {
        while value > 0 {
            let digit = (value & 0xf) as u8;
            digits[count] = if digit < 10 {
                b'0' + digit
            } else {
                b'a' + digit - 10
            };
            value >>= 4;
            count += 1;
        }
    }
    let mut at = 0usize;
    while count > 0 {
        count -= 1;
        out[at] = digits[count];
        at += 1;
    }
    out[at] = b'\r';
    out[at + 1] = b'\n';
    at + 2
}

fn prepare_file<P: Platform>(
    slot: &mut Slot<P>,
    file: P::File,
    mime_path: &[u8],
    info: crate::platform::FileInfo,
    range: Option<http::RangeSpec>,
    head_only: bool,
) {
    let selected = match range {
        Some(spec) => match resolve_range(spec, info.len) {
            Some(range) => Some(range),
            None => {
                let mut content_range = FixedBuf::<64>::new();
                let _ = content_range.push(b"bytes */");
                let _ = content_range.push_u64(info.len);
                let _ = slot.prepare_response(
                    Status::RangeNotSatisfiable,
                    Status::RangeNotSatisfiable.error_body().len() as u64,
                    b"text/plain; charset=utf-8",
                    Status::RangeNotSatisfiable.error_body(),
                    head_only,
                    None,
                    Some(content_range.as_slice()),
                    true,
                );
                slot.log_len = Status::RangeNotSatisfiable.error_body().len() as u64;
                return;
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
    if !slot.prepare_response(
        status,
        length,
        mime::content_type(mime_path),
        b"",
        head_only,
        None,
        if selected.is_some() {
            Some(content_range.as_slice())
        } else {
            None
        },
        true,
    ) {
        slot.prepare_error(Status::InternalServerError, head_only);
        return;
    }
    slot.file = Some(file);
    slot.file_offset = start;
    slot.file_remaining = if head_only { 0 } else { length };
    slot.status = status;
    slot.log_len = length;
}

fn listing_slot<P: Platform>(
    platform: &mut P,
    poller: &mut P::Poller,
    config: &Config,
    slot: &mut Slot<P>,
    token: usize,
) {
    loop {
        let stream = match slot.stream.as_mut() {
            Some(stream) => stream,
            None => return,
        };
        if slot.response_pos < slot.response_len {
            match platform
                .write_nonblocking(stream, &slot.response[slot.response_pos..slot.response_len])
            {
                Ok(0) => {
                    close_slot(platform, poller, slot, token);
                    return;
                }
                Ok(count) => slot.response_pos += count,
                Err(Error::Interrupted) => continue,
                Err(Error::WouldBlock) => return,
                Err(_) => {
                    close_slot(platform, poller, slot, token);
                    return;
                }
            }
        }
        if slot.listing_prefix_pos < slot.listing_prefix_len {
            match platform.write_nonblocking(
                stream,
                &slot.listing_prefix[slot.listing_prefix_pos..slot.listing_prefix_len],
            ) {
                Ok(0) => {
                    close_slot(platform, poller, slot, token);
                    return;
                }
                Ok(count) => slot.listing_prefix_pos += count,
                Err(Error::Interrupted) => continue,
                Err(Error::WouldBlock) => return,
                Err(_) => {
                    close_slot(platform, poller, slot, token);
                    return;
                }
            }
        }
        if slot.listing_data_pos < slot.listing_data_len {
            match platform.write_nonblocking(
                stream,
                &slot.body[slot.listing_data_pos..slot.listing_data_len],
            ) {
                Ok(0) => {
                    close_slot(platform, poller, slot, token);
                    return;
                }
                Ok(count) => slot.listing_data_pos += count,
                Err(Error::Interrupted) => continue,
                Err(Error::WouldBlock) => return,
                Err(_) => {
                    close_slot(platform, poller, slot, token);
                    return;
                }
            }
        }
        if slot.listing_suffix_pos < 2 {
            match platform.write_nonblocking(stream, &b"\r\n"[slot.listing_suffix_pos..]) {
                Ok(0) => {
                    close_slot(platform, poller, slot, token);
                    return;
                }
                Ok(count) => slot.listing_suffix_pos += count,
                Err(Error::Interrupted) => continue,
                Err(Error::WouldBlock) => return,
                Err(_) => {
                    close_slot(platform, poller, slot, token);
                    return;
                }
            }
        }
        if slot.listing_final {
            finish_response(platform, poller, config, slot, token);
            return;
        }
        if slot.listing_phase == ListingPhase::Final {
            if slot.listing_chunked {
                slot.listing_prefix[..5].copy_from_slice(b"0\r\n\r\n");
                slot.listing_prefix_len = 5;
                slot.listing_prefix_pos = 0;
                slot.listing_data_len = 0;
                slot.listing_data_pos = 0;
                slot.listing_suffix_pos = 2;
                slot.listing_final = true;
                continue;
            }
            finish_response(platform, poller, config, slot, token);
            return;
        }

        let mut chunk = FixedBuf::<BODY_BUF>::new();
        let rendered = if slot.listing_phase == ListingPhase::Entries {
            let mut name = [0u8; 256];
            let next = {
                let Some(dir) = slot.dir.as_mut() else {
                    close_slot(platform, poller, slot, token);
                    return;
                };
                platform.next_dir(dir, &mut name)
            };
            match next {
                Ok(Some(entry)) => {
                    slot.listing_entries += 1;
                    listing::render_entry(
                        &mut chunk,
                        &slot.listing_encoded[..slot.listing_encoded_len],
                        &name[..entry.name_len],
                        entry,
                    )
                }
                Ok(None) => {
                    slot.listing_phase = ListingPhase::Tail;
                    continue;
                }
                Err(_) => {
                    close_slot(platform, poller, slot, token);
                    return;
                }
            }
        } else {
            let entries = slot.listing_entries;
            slot.listing_phase = ListingPhase::Final;
            listing::render_tail(&mut chunk, entries, false)
        };
        if !rendered || chunk.is_empty() {
            if slot.listing_phase == ListingPhase::Final {
                continue;
            }
            close_slot(platform, poller, slot, token);
            return;
        }
        if chunk.len() > slot.body.len() {
            close_slot(platform, poller, slot, token);
            return;
        }
        slot.body[..chunk.len()].copy_from_slice(chunk.as_slice());
        slot.listing_data_len = chunk.len();
        slot.listing_data_pos = 0;
        slot.listing_suffix_pos = if slot.listing_chunked { 0 } else { 2 };
        slot.listing_prefix_len = if slot.listing_chunked {
            chunk_prefix(chunk.len(), &mut slot.listing_prefix)
        } else {
            0
        };
        slot.listing_prefix_pos = 0;
        slot.log_len += chunk.len() as u64;
    }
}

fn write_slot<P: Platform>(
    platform: &mut P,
    poller: &mut P::Poller,
    config: &Config,
    slot: &mut Slot<P>,
    token: usize,
) {
    let stream = match slot.stream.as_mut() {
        Some(stream) => stream,
        None => return,
    };
    while slot.response_pos < slot.response_len {
        match platform
            .write_nonblocking(stream, &slot.response[slot.response_pos..slot.response_len])
        {
            Ok(0) => {
                close_slot(platform, poller, slot, token);
                return;
            }
            Ok(count) => slot.response_pos += count,
            Err(Error::Interrupted) => continue,
            Err(Error::WouldBlock) => return,
            Err(_) => {
                close_slot(platform, poller, slot, token);
                return;
            }
        }
    }
    if slot.file_remaining > 0 {
        slot.state = State::SendingFile;
        if platform
            .set_interest(poller, stream, token, Interest::Writable)
            .is_err()
        {
            close_slot(platform, poller, slot, token);
        }
    } else if slot.drain_remaining > 0 {
        slot.state = State::Draining;
        if platform
            .set_interest(poller, stream, token, Interest::Readable)
            .is_err()
        {
            close_slot(platform, poller, slot, token);
        }
    } else {
        finish_response(platform, poller, config, slot, token);
    }
}

fn send_file_slot<P: Platform>(
    platform: &mut P,
    poller: &mut P::Poller,
    config: &Config,
    slot: &mut Slot<P>,
    token: usize,
) {
    loop {
        let stream = match slot.stream.as_mut() {
            Some(stream) => stream,
            None => return,
        };
        let file = match slot.file.as_mut() {
            Some(file) => file,
            None => {
                close_slot(platform, poller, slot, token);
                return;
            }
        };
        match platform.send_file_nonblocking(stream, file, slot.file_offset, slot.file_remaining) {
            Ok(0) => {
                close_slot(platform, poller, slot, token);
                return;
            }
            Ok(count) => {
                slot.file_offset += count;
                slot.file_remaining -= count;
                if slot.file_remaining == 0 {
                    finish_response(platform, poller, config, slot, token);
                    return;
                }
            }
            Err(Error::Interrupted) => continue,
            Err(Error::WouldBlock) => return,
            Err(_) => {
                close_slot(platform, poller, slot, token);
                return;
            }
        }
    }
}

fn drain_slot<P: Platform>(
    platform: &mut P,
    poller: &mut P::Poller,
    slot: &mut Slot<P>,
    token: usize,
) {
    loop {
        if slot.drain_remaining == 0 {
            close_slot(platform, poller, slot, token);
            return;
        }
        let want = core::cmp::min(slot.drain_remaining, slot.body.len() as u64) as usize;
        let stream = match slot.stream.as_mut() {
            Some(stream) => stream,
            None => return,
        };
        match platform.read_nonblocking(stream, &mut slot.body[..want]) {
            Ok(0) => {
                close_slot(platform, poller, slot, token);
                return;
            }
            Ok(count) => slot.drain_remaining -= count as u64,
            Err(Error::Interrupted) => continue,
            Err(Error::WouldBlock) => return,
            Err(_) => {
                close_slot(platform, poller, slot, token);
                return;
            }
        }
    }
}

fn finish_response<P: Platform>(
    platform: &mut P,
    poller: &mut P::Poller,
    config: &Config,
    slot: &mut Slot<P>,
    token: usize,
) {
    log_slot(platform, slot);
    slot.log_path_len = 0;
    if !slot.keep_alive || slot.requests + 1 >= MAX_REQUESTS_PER_CONNECTION {
        close_slot(platform, poller, slot, token);
        return;
    }
    slot.requests += 1;
    let consumed = slot.body_end.min(slot.request_len);
    if consumed > 0 && consumed < slot.request_len {
        slot.request.copy_within(consumed..slot.request_len, 0);
    }
    slot.request_len -= consumed;
    slot.file = None;
    slot.upload = None;
    slot.dir = None;
    slot.listing_prefix_len = 0;
    slot.listing_data_len = 0;
    slot.response_len = 0;
    slot.response_pos = 0;
    slot.file_offset = 0;
    slot.file_remaining = 0;
    slot.drain_remaining = 0;
    slot.continue_pos = 0;
    slot.head_only = false;
    slot.status = Status::Ok;
    slot.keep_alive = false;
    slot.body_end = 0;
    slot.form_redirect = false;
    slot.idle_ticks = 0;
    slot.state = State::Reading;

    if slot.request_len > 0 && http::header_end(&slot.request[..slot.request_len]).is_some() {
        prepare_request(platform, config, slot);
        transition(platform, poller, slot, token);
        return;
    }
    let Some(stream) = slot.stream.as_ref() else {
        return;
    };
    if platform
        .set_interest(poller, stream, token, Interest::Readable)
        .is_err()
    {
        close_slot(platform, poller, slot, token);
    }
}

fn log_slot<P: Platform>(platform: &mut P, slot: &Slot<P>) {
    if slot.quiet || slot.log_path_len == 0 {
        return;
    }
    let mut line = FixedBuf::<1200>::new();
    let _ = line.push(slot.log_method.as_bytes());
    let _ = line.push_byte(b' ');
    let _ = line.push(&slot.log_path[..slot.log_path_len]);
    let _ = line.push_byte(b' ');
    let _ = line.push_u64(slot.status.code() as u64);
    let _ = line.push_byte(b' ');
    let _ = line.push_u64(slot.log_len);
    let _ = line.push_byte(b'\n');
    platform.log(line.as_slice());
}

fn transition<P: Platform>(
    platform: &mut P,
    poller: &mut P::Poller,
    slot: &mut Slot<P>,
    token: usize,
) {
    let interest = match slot.state {
        State::SendingContinue => Interest::Writable,
        State::ReadingBody => Interest::Readable,
        State::Writing => {
            if slot.file_remaining == 0
                && slot.response_pos >= slot.response_len
                && slot.drain_remaining == 0
            {
                close_slot(platform, poller, slot, token);
                return;
            }
            Interest::Writable
        }
        State::StreamingListing => Interest::Writable,
        _ => return,
    };
    let Some(stream) = slot.stream.as_ref() else {
        return;
    };
    if platform
        .set_interest(poller, stream, token, interest)
        .is_err()
    {
        close_slot(platform, poller, slot, token);
    }
}

fn close_slot<P: Platform>(
    platform: &mut P,
    poller: &mut P::Poller,
    slot: &mut Slot<P>,
    token: usize,
) {
    abort_upload(platform, slot);
    log_slot(platform, slot);
    if let Some(stream) = slot.stream.as_ref() {
        let _ = platform.deregister_stream(poller, stream, token);
    }
    slot.stream = None;
    slot.file = None;
    slot.state = State::Idle;
    slot.request_len = 0;
    slot.response_len = 0;
    slot.response_pos = 0;
    slot.file_offset = 0;
    slot.file_remaining = 0;
    slot.dir = None;
    slot.listing_prefix_len = 0;
    slot.listing_data_len = 0;
    slot.keep_alive = false;
    slot.requests = 0;
    slot.body_end = 0;
    slot.form_redirect = false;
}
