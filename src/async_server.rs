//! Experimental non-blocking GET/HEAD server.
//!
//! This is intentionally a separate path from the blocking `Server`: the
//! `--async` flag exercises the readiness reactor while the upload state
//! machine is still being built. Metadata and listings remain synchronous;
//! socket progress, header writes, and file transfer are non-blocking.

use crate::config::Config;
use crate::fixedbuf::FixedBuf;
use crate::http::{self, Method, Status};
use crate::listing::{self, LISTING_BUF};
use crate::mime;
use crate::path::{self, MAX_PATH, MAX_URL_PATH};
use crate::platform::{AsyncPlatform, Error, Event, Interest};
use crate::server::{build_header, resolve_range, root_relative, HEADER_BUF, REQUEST_BUF};

const MAX_CONNECTIONS: usize = 8;
const RESPONSE_BUF: usize = LISTING_BUF + HEADER_BUF;
const EVENT_BATCH: usize = 64;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    Idle,
    Reading,
    Writing,
    SendingFile,
}

struct Slot<P: AsyncPlatform> {
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
    head_only: bool,
    status: Status,
    log_path: [u8; MAX_URL_PATH],
    log_path_len: usize,
    log_len: u64,
}

impl<P: AsyncPlatform> Slot<P> {
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
            head_only: false,
            status: Status::Ok,
            log_path: [0; MAX_URL_PATH],
            log_path_len: 0,
            log_len: 0,
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
        self.head_only = false;
        self.status = Status::Ok;
        self.log_path_len = 0;
        self.log_len = 0;
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

    fn prepare_error(&mut self, status: Status, head_only: bool) {
        let body = status.error_body();
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

pub fn run<P: AsyncPlatform>(mut platform: P, config: Config) -> ! {
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
        let mut line = FixedBuf::<512>::new();
        let _ = line.push(b"syswebd ");
        let _ = line.push(crate::VERSION.as_bytes());
        let _ = line.push(b" listening on http://");
        let _ = line.push_ipv4(addr.ip);
        let _ = line.push_byte(b':');
        let _ = line.push_u64(addr.port as u64);
        let _ = line.push(b" (async GET/HEAD, root: ");
        let _ = line.push(config.root());
        let _ = line.push(b")\n");
        platform.log(line.as_slice());
    }

    let mut slots: [Slot<P>; MAX_CONNECTIONS] = core::array::from_fn(|_| Slot::new());
    let mut events = [Event::default(); EVENT_BATCH];

    loop {
        let ready = match platform.wait(&mut poller, &mut events, -1) {
            Ok(ready) => ready,
            Err(Error::Interrupted) => continue,
            Err(_) => {
                platform.log(b"syswebd: reactor wait failed\n");
                platform.exit(1)
            }
        };
        for event in &events[..ready] {
            if event.token == 0 {
                accept_ready(&mut platform, &mut poller, &listener, &mut slots);
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

fn accept_ready<P: AsyncPlatform>(
    platform: &mut P,
    poller: &mut P::Poller,
    listener: &P::Listener,
    slots: &mut [Slot<P>; MAX_CONNECTIONS],
) {
    loop {
        match platform.accept_nonblocking(listener) {
            Ok(stream) => {
                let Some(index) = slots.iter().position(|slot| slot.state == State::Idle) else {
                    drop(stream);
                    continue;
                };
                slots[index].reset(stream);
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

fn handle_event<P: AsyncPlatform>(
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
    match slots[index].state {
        State::Reading => read_slot(platform, poller, config, &mut slots[index], index + 1),
        State::Writing => write_slot(platform, poller, &mut slots[index], index + 1),
        State::SendingFile => send_file_slot(platform, poller, &mut slots[index], index + 1),
        State::Idle => {}
    }
}

fn read_slot<P: AsyncPlatform>(
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

fn prepare_request<P: AsyncPlatform>(platform: &mut P, config: &Config, slot: &mut Slot<P>) {
    let request = match http::parse(&slot.request[..slot.request_len]) {
        Ok(request) => request,
        Err(http::ParseError::UnsupportedMethod) => {
            slot.prepare_error(Status::MethodNotAllowed, false);
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

    let method = request.method;
    let range = request.range;
    let head_only = method == Method::Head;
    if method != Method::Get && method != Method::Head {
        // Uploads are still handled by the blocking path; this experimental
        // server refuses them rather than silently dropping a body.
        slot.prepare_error(Status::MethodNotAllowed, false);
        return;
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
            serve_listing(platform, slot, &url_path[..url_len], rel_path, head_only);
        }
        Err(Error::PermissionDenied) => slot.prepare_error(Status::Forbidden, head_only),
        Err(_) => slot.prepare_error(Status::InternalServerError, head_only),
    }
}

fn serve_listing<P: AsyncPlatform>(
    platform: &mut P,
    slot: &mut Slot<P>,
    url_path: &[u8],
    rel_path: &[u8],
    head_only: bool,
) {
    let mut dir = match platform.open_dir(rel_path) {
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
    let mut html = FixedBuf::<LISTING_BUF>::new();
    if !listing::render(platform, &mut dir, url_path, false, &mut html) {
        slot.prepare_error(Status::InternalServerError, head_only);
        return;
    }
    let _ = slot.prepare_response(
        Status::Ok,
        html.len() as u64,
        b"text/html; charset=utf-8",
        html.as_slice(),
        head_only,
        None,
        None,
        false,
    );
    slot.log_len = html.len() as u64;
}

fn prepare_file<P: AsyncPlatform>(
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

fn write_slot<P: AsyncPlatform>(
    platform: &mut P,
    poller: &mut P::Poller,
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
    } else {
        close_slot(platform, poller, slot, token);
    }
}

fn send_file_slot<P: AsyncPlatform>(
    platform: &mut P,
    poller: &mut P::Poller,
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
                    close_slot(platform, poller, slot, token);
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

fn transition<P: AsyncPlatform>(
    platform: &mut P,
    poller: &mut P::Poller,
    slot: &mut Slot<P>,
    token: usize,
) {
    if slot.state != State::Writing {
        return;
    }
    let interest = if slot.file_remaining > 0 || slot.response_pos < slot.response_len {
        Interest::Writable
    } else {
        close_slot(platform, poller, slot, token);
        return;
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

fn close_slot<P: AsyncPlatform>(
    platform: &mut P,
    poller: &mut P::Poller,
    slot: &mut Slot<P>,
    token: usize,
) {
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
}
