//! Old-browser chat spike: no WebSocket, no JavaScript, no history.
//!
//! The form lives in the page; the message list lives in an iframe that
//! refreshes with <meta http-equiv="refresh">. A POST is answered with 303 so
//! refreshing never resubmits the message.
//!
//! This is a protocol/UI prototype, not production code. It uses std threads
//! and a heap ring; a sysweb mode would keep the ring in `run` and serve the
//! same pages through the fixed-slot reactor.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::time::Duration;

const MAX_MESSAGES: usize = 16;
const MAX_TEXT: usize = 400;

#[derive(Clone, Copy)]
struct Message {
    seq: u64,
    from: [u8; 64],
    from_len: usize,
    text: [u8; MAX_TEXT],
    len: usize,
}

struct Chat {
    messages: [Message; MAX_MESSAGES],
    head: usize,
    count: usize,
    next_seq: u64,
}

impl Chat {
    fn new() -> Self {
        Self {
            messages: [Message {
                seq: 0,
                from: [0; 64],
                from_len: 0,
                text: [0; MAX_TEXT],
                len: 0,
            }; MAX_MESSAGES],
            head: 0,
            count: 0,
            next_seq: 0,
        }
    }

    fn push(&mut self, from: &[u8], text: &[u8]) {
        let from_len = from.len().min(64);
        let len = text.len().min(MAX_TEXT);
        self.next_seq += 1;
        let mut message = Message {
            seq: self.next_seq,
            from: [0; 64],
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

    fn render(&self) -> String {
        let mut out = String::new();
        out.push_str("<!doctype html><meta charset=\"utf-8\">");
        out.push_str("<title>chat</title><style>");
        out.push_str(
            "body{font:14px monospace;margin:0;padding:8px;background:#f5f6f8;color:#16181d}\
             .line{margin:0 0 12px}\
             .from{color:#4a5568;font-weight:bold;margin-bottom:2px}\
             .text{padding-left:1em;white-space:pre-wrap;overflow-wrap:anywhere}\
             .empty{color:#777}a{color:#0645ad}",
        );
        out.push_str("</style><div id=\"messages\" data-since=\"");
        out.push_str(&self.next_seq.to_string());
        out.push_str("\">");
        if self.count == 0 {
            out.push_str("<p class=\"empty\">No messages yet.</p>");
        } else {
            let newest = (self.head + MAX_MESSAGES - 1) % MAX_MESSAGES;
            for i in 0..self.count {
                let message = &self.messages[(newest + MAX_MESSAGES - i) % MAX_MESSAGES];
                render_message(&mut out, message);
            }
        }
        out.push_str("</div>");
        out.push_str("<noscript><p>Automatic updates need JavaScript. <a href=\"/messages\">Refresh</a>.</p></noscript>");
        out.push_str("<script>(function(){");
        out.push_str("var box=document.getElementById('messages');if(!box)return;");
        out.push_str("var since=Number(box.getAttribute('data-since'))||0;var busy=false;");
        out.push_str("function selected(){try{if(window.getSelection&&String(window.getSelection()))return true;}catch(e){}");
        out.push_str("try{if(document.selection&&document.selection.createRange().text)return true;}catch(e){}return false;}");
        out.push_str("function poll(){if(busy||selected())return;busy=true;");
        out.push_str("var x=new XMLHttpRequest();");
        out.push_str("x.onreadystatechange=function(){if(x.readyState!==4)return;busy=false;");
        out.push_str("if(x.status!==200)return;var latest=x.getResponseHeader('X-Since');");
        out.push_str("if(latest)since=Number(latest);");
        out.push_str("if(x.responseText){if(box.insertAdjacentHTML)box.insertAdjacentHTML('afterbegin',x.responseText);");
        out.push_str("else box.innerHTML=x.responseText+box.innerHTML;}};");
        out.push_str("x.open('GET','/messages?since='+since,true);x.send(null);}");
        out.push_str("setInterval(poll,1000);poll();})();</script>");
        out
    }

    fn render_fragment(&self, since: u64) -> String {
        let mut out = String::new();
        if self.count == 0 {
            return out;
        }
        let newest = (self.head + MAX_MESSAGES - 1) % MAX_MESSAGES;
        for i in 0..self.count {
            let message = &self.messages[(newest + MAX_MESSAGES - i) % MAX_MESSAGES];
            if message.seq > since {
                render_message(&mut out, message);
            }
        }
        out
    }
}

fn main() {
    let mut bind = String::from("0.0.0.0");
    let mut port = 19550u16;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--bind" => bind = args.next().unwrap_or(bind),
            "--port" => port = args.next().and_then(|v| v.parse().ok()).unwrap_or(port),
            other => {
                eprintln!("unknown argument: {other}");
                std::process::exit(2);
            }
        }
    }

    let listener = match TcpListener::bind((bind.as_str(), port)) {
        Ok(listener) => listener,
        Err(error) => {
            eprintln!("bind {}:{}: {error}", bind, port);
            return;
        }
    };
    let chat = Arc::new(Mutex::new(Chat::new()));
    println!("chat: http://{bind}:{port}/");

    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                let chat = Arc::clone(&chat);
                std::thread::spawn(move || {
                    let _ = handle(stream, &chat);
                });
            }
            Err(error) => eprintln!("accept: {error}"),
        }
    }
}

fn handle(mut stream: TcpStream, chat: &Mutex<Chat>) -> std::io::Result<()> {
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    stream.set_write_timeout(Some(Duration::from_secs(5)))?;

    let mut request = [0u8; 8192];
    let mut used = 0;
    let header_end = loop {
        if used == request.len() {
            return respond(&mut stream, 431, "text/plain", b"request too large\n");
        }
        let n = stream.read(&mut request[used..])?;
        if n == 0 {
            return Ok(());
        }
        used += n;
        if let Some(at) = find(&request[..used], b"\r\n\r\n") {
            break at + 4;
        }
    };

    let length = content_length(&request[..header_end]).unwrap_or(0);
    if length > 1024 {
        return respond(&mut stream, 413, "text/plain", b"message too large\n");
    }
    if used < header_end + length {
        let needed = header_end + length;
        if needed > request.len() {
            return respond(&mut stream, 413, "text/plain", b"message too large\n");
        }
        while used < needed {
            let n = stream.read(&mut request[used..needed])?;
            if n == 0 {
                return Ok(());
            }
            used += n;
        }
    }
    let head = &request[..header_end];
    let (method, target) = match parse_request_line(head) {
        Some(value) => value,
        None => return respond(&mut stream, 400, "text/plain", b"bad request\n"),
    };
    let body = &request[header_end..header_end + length];
    let from = match stream.peer_addr() {
        Ok(addr) => addr.ip().to_string(),
        Err(_) => String::from("unknown"),
    };
    let (path, query) = split_query(target);

    match (method, path) {
        (b"GET", b"/") => {
            let page = form_page();
            respond(
                &mut stream,
                200,
                "text/html; charset=utf-8",
                page.as_bytes(),
            )
        }
        (b"GET", b"/messages") => {
            if let Some(since) = query_value(query, b"since").and_then(parse_u64) {
                let (page, latest) = {
                    let chat = chat.lock().unwrap();
                    (chat.render_fragment(since), chat.next_seq)
                };
                respond_since(
                    &mut stream,
                    200,
                    "text/html; charset=utf-8",
                    page.as_bytes(),
                    latest,
                )
            } else {
                let page = chat.lock().unwrap().render();
                respond(
                    &mut stream,
                    200,
                    "text/html; charset=utf-8",
                    page.as_bytes(),
                )
            }
        }
        (b"POST", b"/chat") => {
            let text = match form_text(body) {
                Some(text) if !text.is_empty() => text,
                _ => return redirect(&mut stream, "/"),
            };
            chat.lock().unwrap().push(from.as_bytes(), &text);
            redirect(&mut stream, "/")
        }
        _ => respond(&mut stream, 404, "text/plain", b"not found\n"),
    }
}

fn form_page() -> String {
    let mut out = String::new();
    out.push_str("<!doctype html><meta charset=\"utf-8\">");
    out.push_str("<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">");
    out.push_str("<title>chat</title><style>");
    out.push_str(
        "body{font:15px monospace;margin:0;padding:12px;background:#f5f6f8;color:#16181d}\
         form{margin:0 0 12px}textarea{width:100%;box-sizing:border-box;padding:8px;font:inherit}\
         button{margin-top:8px;padding:8px 12px;font:inherit}iframe{width:100%;height:65vh;border:1px solid #ccc;background:#fff}",
    );
    out.push_str("</style>");
    out.push_str("<form id=\"chat-form\" method=\"post\" action=\"/chat\">");
    out.push_str("<textarea id=\"chat-text\" name=\"text\" rows=\"4\" maxlength=\"400\" autofocus autocomplete=\"off\"></textarea>");
    out.push_str("<button type=\"submit\">Send</button></form>");
    out.push_str("<iframe src=\"/messages\" title=\"messages\"></iframe>");
    out.push_str("<script>(function(){var f=document.getElementById('chat-form'),t=document.getElementById('chat-text');");
    out.push_str("if(!f||!t||!t.addEventListener)return;");
    out.push_str("t.addEventListener('keydown',function(e){if(e.isComposing)return;");
    out.push_str("if(e.key==='Enter'&&!e.ctrlKey&&!e.shiftKey&&!e.altKey){e.preventDefault();f.submit();}});})();</script>");
    out
}

fn render_message(out: &mut String, message: &Message) {
    out.push_str("<div class=\"line\" data-seq=\"");
    out.push_str(&message.seq.to_string());
    out.push_str("\"><div class=\"from\">");
    escape_html(out, &message.from[..message.from_len]);
    out.push_str("</div><div class=\"text\">");
    escape_html(out, &message.text[..message.len]);
    out.push_str("</div></div>");
}

fn split_query(target: &[u8]) -> (&[u8], &[u8]) {
    match target.iter().position(|byte| *byte == b'?') {
        Some(at) => (&target[..at], &target[at + 1..]),
        None => (target, b""),
    }
}

fn query_value<'a>(query: &'a [u8], key: &[u8]) -> Option<&'a [u8]> {
    for field in query.split(|byte| *byte == b'&') {
        let mut parts = field.splitn(2, |byte| *byte == b'=');
        if parts.next()? == key {
            return parts.next();
        }
    }
    None
}

fn parse_u64(bytes: &[u8]) -> Option<u64> {
    std::str::from_utf8(bytes).ok()?.parse().ok()
}

fn respond_since(
    stream: &mut TcpStream,
    status: u16,
    content_type: &str,
    body: &[u8],
    latest: u64,
) -> std::io::Result<()> {
    let header = format!(
        "HTTP/1.1 {status} OK\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nX-Since: {latest}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(header.as_bytes())?;
    stream.write_all(body)?;
    stream.flush()
}

fn respond(
    stream: &mut TcpStream,
    status: u16,
    content_type: &str,
    body: &[u8],
) -> std::io::Result<()> {
    let reason = match status {
        200 => "OK",
        303 => "See Other",
        400 => "Bad Request",
        404 => "Not Found",
        413 => "Payload Too Large",
        431 => "Request Header Fields Too Large",
        _ => "Error",
    };
    let header = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(header.as_bytes())?;
    stream.write_all(body)?;
    stream.flush()
}

fn redirect(stream: &mut TcpStream, location: &str) -> std::io::Result<()> {
    let header = format!(
        "HTTP/1.1 303 See Other\r\nLocation: {location}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
    );
    stream.write_all(header.as_bytes())?;
    stream.flush()
}

fn parse_request_line(head: &[u8]) -> Option<(&[u8], &[u8])> {
    let end = find(head, b"\r\n")?;
    let line = &head[..end];
    let mut parts = line.split(|byte| *byte == b' ');
    let method = parts.next()?;
    let target = parts.next()?;
    Some((method, target))
}

fn content_length(head: &[u8]) -> Option<usize> {
    for line in head.split(|byte| *byte == b'\n') {
        let line = line.strip_suffix(b"\r").unwrap_or(line);
        let Some(colon) = line.iter().position(|byte| *byte == b':') else {
            continue;
        };
        if !line[..colon].eq_ignore_ascii_case(b"content-length") {
            continue;
        }
        let value = trim(&line[colon + 1..]);
        return std::str::from_utf8(value).ok()?.parse().ok();
    }
    None
}

fn form_text(body: &[u8]) -> Option<Vec<u8>> {
    for field in body.split(|byte| *byte == b'&') {
        let mut parts = field.splitn(2, |byte| *byte == b'=');
        let name = parts.next()?;
        let value = parts.next().unwrap_or(b"");
        if name == b"text" {
            let mut out = Vec::new();
            let mut i = 0;
            while i < value.len() && out.len() < MAX_TEXT {
                match value[i] {
                    b'+' => {
                        out.push(b' ');
                        i += 1;
                    }
                    b'%' if i + 2 < value.len() => {
                        let hi = hex(value[i + 1])?;
                        let lo = hex(value[i + 2])?;
                        out.push((hi << 4) | lo);
                        i += 3;
                    }
                    byte => {
                        out.push(byte);
                        i += 1;
                    }
                }
            }
            return Some(out);
        }
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

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

fn trim(bytes: &[u8]) -> &[u8] {
    let mut start = 0;
    let mut end = bytes.len();
    while start < end && (bytes[start] == b' ' || bytes[start] == b'\t') {
        start += 1;
    }
    while end > start && (bytes[end - 1] == b' ' || bytes[end - 1] == b'\t') {
        end -= 1;
    }
    &bytes[start..end]
}

fn escape_html(out: &mut String, bytes: &[u8]) {
    let text = String::from_utf8_lossy(bytes);
    for ch in text.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            '\n' => out.push_str("<br>"),
            ch => out.push(ch),
        }
    }
}
