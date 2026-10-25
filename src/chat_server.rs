//! Built-in local chat mode: fixed ring, polling, no WebSocket, no allocation.
//!
//! This is a separate reactor from the static server. It deliberately serves
//! only the chat UI and its small polling fragment.

use crate::auth;
use crate::fixedbuf::FixedBuf;
use crate::http::{self, Method, Status};
use crate::platform::{Error, Event, Interest, Platform};

const MAX_CONNECTIONS: usize = 32;
const REQUEST_BUF: usize = 4096;
const RESPONSE_BUF: usize = 32768;
const MAX_TEXT: usize = 200;
const MAX_MESSAGES: usize = 16;
const MAX_BODY: usize = 1024;
const EVENT_BATCH: usize = 64;

struct Message {
    seq: u64,
    from: [u8; 16],
    from_len: usize,
    text: [u8; MAX_TEXT],
    len: usize,
}

pub struct Chat {
    messages: [Message; MAX_MESSAGES],
    head: usize,
    count: usize,
    next_seq: u64,
}

impl Chat {
    fn new() -> Self {
        Self {
            messages: core::array::from_fn(|_| Message {
                seq: 0,
                from: [0; 16],
                from_len: 0,
                text: [0; MAX_TEXT],
                len: 0,
            }),
            head: 0,
            count: 0,
            next_seq: 0,
        }
    }

    fn push(&mut self, from: &[u8], text: &[u8]) {
        self.next_seq += 1;
        let from_len = from.len().min(16);
        let len = text.len().min(MAX_TEXT);
        let mut message = Message {
            seq: self.next_seq,
            from: [0; 16],
            from_len,
            text: [0; MAX_TEXT],
            len,
        };
        message.from[..from_len].copy_from_slice(&from[..from_len]);
        message.text[..len].copy_from_slice(&text[..len]);
        self.messages[self.head] = message;
        self.head = (self.head + 1) % MAX_MESSAGES;
        if self.count < MAX_MESSAGES {
            self.count += 1;
        }
    }

    fn render_full(&self, out: &mut FixedBuf<RESPONSE_BUF>, me: &[u8]) -> bool {
        if !out.push(b"<!doctype html><meta charset=\"utf-8\">")
            || !out.push(b"<meta name=\"color-scheme\" content=\"light dark\">")
            || !out.push(b"<title>chat</title><style>")
            || !out.push(CSS)
            || !out.push(b"</style><div id=\"messages\" data-me=\"")
            || !escape(out, me)
            || !out.push(b"\" data-since=\"")
            || !out.push_u64(self.next_seq)
            || !out.push(b"\">")
        {
            return false;
        }
        if self.count == 0 {
            if !out.push(b"<p class=\"empty\">No messages yet.</p>") {
                return false;
            }
        } else {
            let newest = (self.head + MAX_MESSAGES - 1) % MAX_MESSAGES;
            for i in 0..self.count {
                let message = &self.messages[(newest + MAX_MESSAGES - i) % MAX_MESSAGES];
                if !render_message(out, message) {
                    return false;
                }
            }
        }
        out.push(b"</div><noscript><p>Automatic updates need JavaScript. <a href=\"/messages\">Refresh</a>.</p></noscript>")
            && out.push(JS)
    }

    fn render_fragment(&self, since: u64, out: &mut FixedBuf<RESPONSE_BUF>) -> bool {
        if self.count == 0 {
            return true;
        }
        let newest = (self.head + MAX_MESSAGES - 1) % MAX_MESSAGES;
        for i in 0..self.count {
            let message = &self.messages[(newest + MAX_MESSAGES - i) % MAX_MESSAGES];
            if message.seq > since && !render_message(out, message) {
                return false;
            }
        }
        true
    }
}

fn render_message(out: &mut FixedBuf<RESPONSE_BUF>, message: &Message) -> bool {
    out.push(b"<div class=\"line\" data-seq=\"")
        && out.push_u64(message.seq)
        && out.push(b"\" data-from=\"")
        && escape(out, &message.from[..message.from_len])
        && out.push(b"\"><div class=\"from\">")
        && escape(out, &message.from[..message.from_len])
        && out.push(b"</div><div class=\"bubble\">")
        && escape(out, &message.text[..message.len])
        && out.push(b"</div></div>")
}

fn escape(out: &mut FixedBuf<RESPONSE_BUF>, bytes: &[u8]) -> bool {
    for &byte in bytes {
        let ok = match byte {
            b'&' => out.push(b"&amp;"),
            b'<' => out.push(b"&lt;"),
            b'>' => out.push(b"&gt;"),
            b'"' => out.push(b"&quot;"),
            b'\'' => out.push(b"&#39;"),
            b'\r' => true,
            b'\n' => out.push(b"<br>"),
            byte => out.push(&[byte]),
        };
        if !ok {
            return false;
        }
    }
    true
}

fn form_page() -> FixedBuf<RESPONSE_BUF> {
    let mut out = FixedBuf::new();
    let _ = out.push(b"<!doctype html><meta charset=\"utf-8\">");
    let _ = out.push(b"<meta name=\"color-scheme\" content=\"light dark\">");
    let _ = out.push(b"<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">");
    let _ = out.push(b"<title>chat</title><style>");
    let _ = out.push(FORM_CSS);
    let _ = out.push(b"</style><form id=\"chat-form\" method=\"post\" action=\"/chat\">");
    let _ = out.push(b"<textarea id=\"chat-text\" name=\"text\" rows=\"4\" maxlength=\"200\" autofocus autocomplete=\"off\"></textarea>");
    let _ = out.push(b"<button type=\"submit\">Send</button></form>");
    let _ = out.push(b"<iframe src=\"/messages\" title=\"messages\"></iframe>");
    let _ = out.push(ENTER_JS);
    out
}

const CSS: &[u8] = b"body{font:15px -apple-system,BlinkMacSystemFont,'Segoe UI',sans-serif;margin:0;padding:12px 20px 24px;background:#f2f2f7;color:#1c1c1e;box-sizing:border-box}\
.line{display:flex;flex-direction:column;align-items:flex-start;margin:0 0 14px}\
.from{color:#8e8e93;font-size:11px;line-height:1.2;margin:0 0 4px 14px}\
.bubble{background:#e5e5ea;border-radius:18px;border-bottom-left-radius:5px;padding:8px 13px;max-width:78%;line-height:1.35;white-space:pre-wrap;overflow-wrap:anywhere;word-wrap:break-word}\
.line.mine{align-items:flex-end}.line.mine .from{margin:0 14px 4px 0}\
.line.mine .bubble{background:#0a84ff;color:#fff;border-bottom-left-radius:18px;border-bottom-right-radius:5px}\
.empty{color:#8e8e93;text-align:center;margin:24px 0}a{color:#0a84ff}\
@media (prefers-color-scheme: dark){body{background:#101216;color:#e7eaf0}\
.from,.empty{color:#9aa3b2}.bubble{background:#171a20;color:#e7eaf0}\
.line.mine .bubble{background:#0a84ff;color:#fff}a{color:#7bb8ff}}";

const FORM_CSS: &[u8] = b"body{font:15px -apple-system,BlinkMacSystemFont,'Segoe UI',sans-serif;margin:0;padding:16px 20px;background:#f2f2f7;color:#1c1c1e;box-sizing:border-box}\
form{display:flex;gap:8px;align-items:flex-end;margin:0 0 12px}\
textarea{flex:1;box-sizing:border-box;padding:9px 12px;font:inherit;border:1px solid #c7c7cc;border-radius:18px;background:#fff;resize:vertical;min-height:44px;max-height:160px}\
button{padding:10px 16px;font:inherit;font-weight:600;border:0;border-radius:18px;background:#0a84ff;color:#fff}\
iframe{width:100%;height:65vh;border:1px solid #d1d1d6;border-radius:16px;background:#f2f2f7}\
@media (prefers-color-scheme: dark){body{background:#101216;color:#e7eaf0}\
textarea{background:#101216;border-color:#272c35;color:#e7eaf0}\
iframe{background:#101216;border-color:#272c35}}";

const ENTER_JS: &[u8] = b"<script>(function(){var f=document.getElementById('chat-form'),t=document.getElementById('chat-text');if(!f||!t||!t.addEventListener)return;t.addEventListener('keydown',function(e){if(e.isComposing)return;if(e.key==='Enter'&&!e.ctrlKey&&!e.shiftKey&&!e.altKey){e.preventDefault();f.submit();}});})();</script>";

const JS: &[u8] = b"<script>(function(){var box=document.getElementById('messages');if(!box)return;var since=Number(box.getAttribute('data-since'))||0;var busy=false;var me=box.getAttribute('data-me')||'';\
function paint(){if(!box.getElementsByClassName)return;var lines=box.getElementsByClassName('line');for(var i=0;i<lines.length;i++){var line=lines[i];line.className=(line.getAttribute('data-from')===me)?'line mine':'line';}}\
paint();function selected(){try{if(window.getSelection&&String(window.getSelection()))return true;}catch(e){}try{if(document.selection&&document.selection.createRange().text)return true;}catch(e){}return false;}\
function poll(){if(busy||selected())return;busy=true;var x=new XMLHttpRequest();x.onreadystatechange=function(){if(x.readyState!==4)return;busy=false;if(x.status!==200)return;var latest=x.getResponseHeader('X-Since');if(latest)since=Number(latest);if(x.responseText){if(box.insertAdjacentHTML)box.insertAdjacentHTML('afterbegin',x.responseText);else box.innerHTML=x.responseText+box.innerHTML;paint();}};x.open('GET','/messages?since='+since,true);x.send(null);}\
setInterval(poll,1000);poll();})();</script>";

struct Slot<P: Platform> {
    stream: Option<P::Stream>,
    state: State,
    request: [u8; REQUEST_BUF],
    request_len: usize,
    response: [u8; RESPONSE_BUF],
    response_len: usize,
    response_pos: usize,
    from: [u8; 16],
    from_len: usize,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum State {
    Reading,
    Writing,
}

impl<P: Platform> Slot<P> {
    fn new(stream: P::Stream, from: [u8; 16], from_len: usize) -> Self {
        Self {
            stream: Some(stream),
            state: State::Reading,
            request: [0; REQUEST_BUF],
            request_len: 0,
            response: [0; RESPONSE_BUF],
            response_len: 0,
            response_pos: 0,
            from,
            from_len,
        }
    }
}

pub fn run<P: Platform>(mut platform: P, config: crate::Config) -> ! {
    let listener = match platform.bind(config.addr(), config.backlog()) {
        Ok(listener) => listener,
        Err(_) => {
            platform.log(b"chat: bind failed\n");
            platform.exit(1);
        }
    };
    let _ = platform.set_listener_nonblocking(&listener);
    let mut poller = match platform.poller() {
        Ok(poller) => poller,
        Err(_) => platform.exit(1),
    };
    if platform
        .register_listener(&mut poller, &listener, 0)
        .is_err()
    {
        platform.exit(1);
    }

    let mut slots: [Option<Slot<P>>; MAX_CONNECTIONS] = core::array::from_fn(|_| None);
    let mut events = [Event::default(); EVENT_BATCH];
    let mut chat = Chat::new();
    if !config.quiet() {
        platform.log(b"chat: polling mode\n");
    }

    loop {
        let count = match platform.wait(&mut poller, &mut events, -1) {
            Ok(count) => count,
            Err(Error::Interrupted) => continue,
            Err(_) => platform.exit(1),
        };
        for event in &events[..count] {
            if event.token == 0 {
                accept(&mut platform, &mut poller, &listener, &mut slots);
                continue;
            }
            let index = event.token - 1;
            if index >= slots.len() {
                continue;
            }
            if event.error {
                close(&mut platform, &mut poller, &mut slots, index);
                continue;
            }
            let keep = match slots[index].as_mut() {
                Some(slot) => match slot.state {
                    State::Reading => {
                        read_slot(&mut platform, &mut poller, &config, &mut chat, slot, index)
                    }
                    State::Writing => write_slot(&mut platform, &mut poller, slot, index),
                },
                None => false,
            };
            if !keep {
                close(&mut platform, &mut poller, &mut slots, index);
            }
        }
    }
}

fn accept<P: Platform>(
    platform: &mut P,
    poller: &mut P::Poller,
    listener: &P::Listener,
    slots: &mut [Option<Slot<P>>; MAX_CONNECTIONS],
) {
    loop {
        match platform.accept_nonblocking(listener) {
            Ok(stream) => {
                let Some(index) = slots.iter().position(|slot| slot.is_none()) else {
                    continue;
                };
                let (from, from_len) = peer(platform, &stream);
                let token = index + 1;
                if platform.register_stream(poller, &stream, token).is_err() {
                    continue;
                }
                slots[index] = Some(Slot::new(stream, from, from_len));
            }
            Err(Error::WouldBlock) | Err(Error::Interrupted) => break,
            Err(_) => break,
        }
    }
}

fn peer<P: Platform>(platform: &mut P, stream: &P::Stream) -> ([u8; 16], usize) {
    let mut out = [0u8; 16];
    let Ok(addr) = platform.peer_addr(stream) else {
        out[..7].copy_from_slice(b"unknown");
        return (out, 7);
    };
    let mut len = 0;
    for (i, octet) in addr.ip.iter().enumerate() {
        if i > 0 {
            out[len] = b'.';
            len += 1;
        }
        let mut digits = [0u8; 3];
        let mut n = *octet;
        let mut count = 0;
        loop {
            digits[count] = b'0' + (n % 10);
            n /= 10;
            count += 1;
            if n == 0 {
                break;
            }
        }
        while count > 0 {
            count -= 1;
            out[len] = digits[count];
            len += 1;
        }
    }
    (out, len)
}

fn read_slot<P: Platform>(
    platform: &mut P,
    poller: &mut P::Poller,
    config: &crate::Config,
    chat: &mut Chat,
    slot: &mut Slot<P>,
    index: usize,
) -> bool {
    loop {
        if slot.request_len == REQUEST_BUF {
            return false;
        }
        let stream = match slot.stream.as_mut() {
            Some(stream) => stream,
            None => return false,
        };
        match platform.read_nonblocking(stream, &mut slot.request[slot.request_len..]) {
            Ok(0) => return false,
            Ok(n) => slot.request_len += n,
            Err(Error::Interrupted) => continue,
            Err(Error::WouldBlock) => return true,
            Err(_) => return false,
        }
        let Some(header_end) = http::header_end(&slot.request[..slot.request_len]) else {
            continue;
        };
        let length = match http::parse(&slot.request[..slot.request_len]) {
            Ok(request) => request.content_length.unwrap_or(0) as usize,
            Err(_) => {
                respond(
                    slot,
                    Status::BadRequest,
                    b"text/plain; charset=utf-8",
                    b"bad request\n",
                    None,
                    None,
                    false,
                );
                return set_write(platform, poller, slot, index);
            }
        };
        if length > MAX_BODY {
            respond(
                slot,
                Status::BadRequest,
                b"text/plain; charset=utf-8",
                b"body too large\n",
                None,
                None,
                false,
            );
            return set_write(platform, poller, slot, index);
        }
        if slot.request_len < header_end + length {
            continue;
        }
        prepare(config, chat, slot, header_end, length);
        return set_write(platform, poller, slot, index);
    }
}

fn prepare<P: Platform>(
    config: &crate::Config,
    chat: &mut Chat,
    slot: &mut Slot<P>,
    _header_end: usize,
    length: usize,
) {
    let request = match http::parse(&slot.request[..slot.request_len]) {
        Ok(request) => request,
        Err(_) => {
            respond(
                slot,
                Status::BadRequest,
                b"text/plain; charset=utf-8",
                b"bad request\n",
                None,
                None,
                false,
            );
            return;
        }
    };
    if let Some((user, pass)) = config.userpass() {
        if !auth::authorized(request.authorization, user, pass) {
            respond(
                slot,
                Status::Unauthorized,
                b"text/plain; charset=utf-8",
                b"401 Unauthorized\n",
                None,
                None,
                true,
            );
            return;
        }
    }
    let mut target = [0u8; 512];
    if request.target.len() > target.len() {
        respond(
            slot,
            Status::UriTooLong,
            b"text/plain; charset=utf-8",
            b"uri too long\n",
            None,
            None,
            false,
        );
        return;
    }
    target[..request.target.len()].copy_from_slice(request.target);
    let target_len = request.target.len();
    let method = request.method;
    let body_offset = request.body_offset;
    let from = slot.from;
    let from_len = slot.from_len;

    let (path, query) = split_query(&target[..target_len]);
    match (method, path) {
        (Method::Get, b"/") => {
            let page = form_page();
            respond(
                slot,
                Status::Ok,
                b"text/html; charset=utf-8",
                page.as_slice(),
                None,
                None,
                false,
            );
        }
        (Method::Get, b"/messages") => {
            let mut page = FixedBuf::<RESPONSE_BUF>::new();
            if let Some(since) = since_value(query) {
                if !chat.render_fragment(since, &mut page) {
                    respond(
                        slot,
                        Status::InternalServerError,
                        b"text/plain; charset=utf-8",
                        b"render failed\n",
                        None,
                        None,
                        false,
                    );
                } else {
                    respond(
                        slot,
                        Status::Ok,
                        b"text/html; charset=utf-8",
                        page.as_slice(),
                        None,
                        Some(chat.next_seq),
                        false,
                    );
                }
            } else {
                if !chat.render_full(&mut page, &from[..from_len]) {
                    respond(
                        slot,
                        Status::InternalServerError,
                        b"text/plain; charset=utf-8",
                        b"render failed\n",
                        None,
                        None,
                        false,
                    );
                } else {
                    respond(
                        slot,
                        Status::Ok,
                        b"text/html; charset=utf-8",
                        page.as_slice(),
                        None,
                        None,
                        false,
                    );
                }
            }
        }
        (Method::Post, b"/chat") => {
            let body = &slot.request[body_offset..body_offset + length];
            let mut text = [0u8; MAX_TEXT];
            let Some(len) = form_text(body, &mut text) else {
                respond(
                    slot,
                    Status::BadRequest,
                    b"text/plain; charset=utf-8",
                    b"bad form\n",
                    None,
                    None,
                    false,
                );
                return;
            };
            if len == 0 {
                respond(
                    slot,
                    Status::SeeOther,
                    b"text/plain; charset=utf-8",
                    b"",
                    Some(b"/"),
                    None,
                    false,
                );
                return;
            }
            chat.push(&from[..from_len], &text[..len]);
            respond(
                slot,
                Status::SeeOther,
                b"text/plain; charset=utf-8",
                b"",
                Some(b"/"),
                None,
                false,
            );
        }
        _ => {
            respond(
                slot,
                Status::NotFound,
                b"text/plain; charset=utf-8",
                b"not found\n",
                None,
                None,
                false,
            );
        }
    }
}

fn respond<P: Platform>(
    slot: &mut Slot<P>,
    status: Status,
    content_type: &[u8],
    body: &[u8],
    location: Option<&[u8]>,
    since: Option<u64>,
    unauthorized: bool,
) {
    let mut header = FixedBuf::<RESPONSE_BUF>::new();
    if !header.push(b"HTTP/1.1 ")
        || !header.push_u64(status.code() as u64)
        || !header.push(b" ")
        || !header.push(status.reason())
        || !header.push(b"\r\nContent-Type: ")
        || !header.push(content_type)
        || !header.push(b"\r\nContent-Length: ")
        || !header.push_u64(body.len() as u64)
        || !header.push(b"\r\nCache-Control: no-store\r\nConnection: close\r\n")
    {
        return;
    }
    if let Some(location) = location {
        if !header.push(b"Location: ") || !header.push(location) || !header.push(b"\r\n") {
            return;
        }
    }
    if let Some(since) = since {
        if !header.push(b"X-Since: ") || !header.push_u64(since) || !header.push(b"\r\n") {
            return;
        }
    }
    if unauthorized && !header.push(b"WWW-Authenticate: Basic realm=\"sysweb\"\r\n") {
        return;
    }
    if !header.push(b"\r\n") || header.len() + body.len() > RESPONSE_BUF {
        return;
    }
    slot.response[..header.len()].copy_from_slice(header.as_slice());
    slot.response[header.len()..header.len() + body.len()].copy_from_slice(body);
    slot.response_len = header.len() + body.len();
    slot.response_pos = 0;
    slot.state = State::Writing;
}

fn set_write<P: Platform>(
    platform: &mut P,
    poller: &mut P::Poller,
    slot: &mut Slot<P>,
    index: usize,
) -> bool {
    let Some(stream) = slot.stream.as_ref() else {
        return false;
    };
    platform
        .set_interest(poller, stream, index + 1, Interest::Writable)
        .is_ok()
}

fn write_slot<P: Platform>(
    platform: &mut P,
    poller: &mut P::Poller,
    slot: &mut Slot<P>,
    _index: usize,
) -> bool {
    let stream = match slot.stream.as_mut() {
        Some(stream) => stream,
        None => return false,
    };
    while slot.response_pos < slot.response_len {
        match platform
            .write_nonblocking(stream, &slot.response[slot.response_pos..slot.response_len])
        {
            Ok(0) => return false,
            Ok(n) => slot.response_pos += n,
            Err(Error::Interrupted) => continue,
            Err(Error::WouldBlock) => return true,
            Err(_) => return false,
        }
    }
    let _ = poller;
    false
}

fn close<P: Platform>(
    platform: &mut P,
    poller: &mut P::Poller,
    slots: &mut [Option<Slot<P>>; MAX_CONNECTIONS],
    index: usize,
) {
    if let Some(slot) = slots[index].as_ref() {
        if let Some(stream) = slot.stream.as_ref() {
            let _ = platform.deregister_stream(poller, stream, index + 1);
        }
    }
    slots[index] = None;
}

fn split_query(target: &[u8]) -> (&[u8], &[u8]) {
    match target.iter().position(|byte| *byte == b'?') {
        Some(at) => (&target[..at], &target[at + 1..]),
        None => (target, b""),
    }
}

fn since_value(query: &[u8]) -> Option<u64> {
    let mut value = [0u8; 20];
    let mut len = 0;
    for field in query.split(|byte| *byte == b'&') {
        let mut parts = field.splitn(2, |byte| *byte == b'=');
        if parts.next()? != b"since" {
            continue;
        }
        let raw = parts.next().unwrap_or(b"");
        for &byte in raw {
            if byte == b'%' {
                continue;
            }
            if len < value.len() {
                value[len] = byte;
                len += 1;
            }
        }
        let mut number = 0u64;
        if len == 0 {
            return None;
        }
        for &byte in &value[..len] {
            if !byte.is_ascii_digit() {
                return None;
            }
            number = number.checked_mul(10)?.checked_add((byte - b'0') as u64)?;
        }
        return Some(number);
    }
    None
}

fn form_text(body: &[u8], out: &mut [u8; MAX_TEXT]) -> Option<usize> {
    for field in body.split(|byte| *byte == b'&') {
        let mut parts = field.splitn(2, |byte| *byte == b'=');
        if parts.next()? != b"text" {
            continue;
        }
        let raw = parts.next().unwrap_or(b"");
        let mut len = 0;
        let mut i = 0;
        while i < raw.len() {
            let byte = match raw[i] {
                b'+' => {
                    i += 1;
                    b' '
                }
                b'%' if i + 2 < raw.len() => {
                    let hi = hex(raw[i + 1])?;
                    let lo = hex(raw[i + 2])?;
                    i += 3;
                    (hi << 4) | lo
                }
                byte => {
                    i += 1;
                    byte
                }
            };
            if len == out.len() {
                return None;
            }
            out[len] = byte;
            len += 1;
        }
        return Some(len);
    }
    None
}

fn hex(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}
