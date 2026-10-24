//! HTTP/1.0 and HTTP/1.1 request parsing plus the small status-code table.
//!
//! This module is pure: no I/O, no unsafe, no allocation.

/// The methods this server implements.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Method {
    Get,
    Head,
    Put,
    Post,
}

impl Method {
    pub const fn as_bytes(self) -> &'static [u8] {
        match self {
            Method::Get => b"GET",
            Method::Head => b"HEAD",
            Method::Put => b"PUT",
            Method::Post => b"POST",
        }
    }
}

/// Only the two HTTP versions a static file server needs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Version {
    Http10,
    Http11,
}

/// A single byte range from a `Range: bytes=...` header.
///
/// Multi-range requests are deliberately not represented: the server ignores
/// them and serves the full representation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RangeSpec {
    /// `bytes=start-end`
    FromTo(u64, u64),
    /// `bytes=start-`
    From(u64),
    /// `bytes=-suffix`
    Suffix(u64),
}

/// The connection persistence requested by the client.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Connection {
    Default,
    KeepAlive,
    Close,
}

impl Connection {
    pub const fn keep_alive_for(self, version: Version) -> bool {
        match version {
            Version::Http11 => !matches!(self, Connection::Close),
            Version::Http10 => matches!(self, Connection::KeepAlive),
        }
    }
}

/// A parsed request line plus the headers the server interprets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Request<'a> {
    pub method: Method,
    pub target: &'a [u8],
    pub version: Version,
    pub connection: Connection,
    pub content_length: Option<u64>,
    pub content_type: Option<&'a [u8]>,
    pub range: Option<RangeSpec>,
    pub expect_continue: bool,
    pub authorization: Option<&'a [u8]>,
    /// Index just past `\r\n\r\n`; body bytes may already follow in the buffer.
    pub body_offset: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParseError {
    Malformed,
    UnsupportedMethod,
    UnsupportedVersion,
}

/// Find the end of the header block (`\r\n\r\n`) and return the index just
/// past it.
pub fn header_end(buf: &[u8]) -> Option<usize> {
    buf.windows(4)
        .position(|window| window == b"\r\n\r\n")
        .map(|at| at + 4)
}

/// Parse a complete header block (the caller guarantees `header_end` found
/// one, but this function checks again).
pub fn parse(buf: &[u8]) -> Result<Request<'_>, ParseError> {
    let end = header_end(buf).ok_or(ParseError::Malformed)?;
    let head = &buf[..end - 4];
    // With no header fields the request line is all of `head`; otherwise the
    // first CRLF is still inside `head`.
    let line_end = head
        .windows(2)
        .position(|window| window == b"\r\n")
        .unwrap_or(head.len());
    let line = &head[..line_end];

    let mut parts = line.split(|&b| b == b' ');
    let method = parts.next().ok_or(ParseError::Malformed)?;
    let target = parts.next().ok_or(ParseError::Malformed)?;
    let version = parts.next().ok_or(ParseError::Malformed)?;
    if parts.next().is_some() || method.is_empty() || target.is_empty() {
        return Err(ParseError::Malformed);
    }

    let method = match method {
        b"GET" => Method::Get,
        b"HEAD" => Method::Head,
        b"PUT" => Method::Put,
        b"POST" => Method::Post,
        _ => return Err(ParseError::UnsupportedMethod),
    };
    let version = match version {
        b"HTTP/1.0" => Version::Http10,
        b"HTTP/1.1" => Version::Http11,
        _ => return Err(ParseError::UnsupportedVersion),
    };

    // Request targets are visible ASCII; anything else must be percent-encoded.
    if target.iter().any(|&b| b <= 0x20 || b >= 0x7f) {
        return Err(ParseError::Malformed);
    }

    let mut content_length = None;
    let mut content_type = None;
    let mut range = None;
    let mut expect_continue = false;
    let mut authorization = None;
    let mut connection = Connection::Default;
    if line_end + 2 <= head.len() {
        for header_line in head[line_end + 2..].split(|&b| b == b'\n') {
            let header_line = header_line.strip_suffix(b"\r").unwrap_or(header_line);
            if header_line.is_empty() {
                continue;
            }
            let colon = header_line
                .iter()
                .position(|&b| b == b':')
                .ok_or(ParseError::Malformed)?;
            let name = trim(&header_line[..colon]);
            let value = trim(&header_line[colon + 1..]);

            if name.eq_ignore_ascii_case(b"content-length") {
                let parsed = parse_u64(value).ok_or(ParseError::Malformed)?;
                match content_length {
                    Some(previous) if previous != parsed => return Err(ParseError::Malformed),
                    Some(_) => {}
                    None => content_length = Some(parsed),
                }
            } else if name.eq_ignore_ascii_case(b"transfer-encoding") {
                // Chunked request bodies are not implemented; never guess.
                return Err(ParseError::Malformed);
            } else if name.eq_ignore_ascii_case(b"content-type") {
                content_type = Some(value);
            } else if name.eq_ignore_ascii_case(b"range") && range.is_none() {
                range = parse_range(value);
            } else if name.eq_ignore_ascii_case(b"expect") {
                expect_continue = value.eq_ignore_ascii_case(b"100-continue");
            } else if name.eq_ignore_ascii_case(b"authorization") {
                authorization = Some(value);
            } else if name.eq_ignore_ascii_case(b"connection") {
                for token in value.split(|&byte| byte == b',') {
                    let token = trim(token);
                    if token.eq_ignore_ascii_case(b"close") {
                        connection = Connection::Close;
                    } else if token.eq_ignore_ascii_case(b"keep-alive")
                        && connection != Connection::Close
                    {
                        connection = Connection::KeepAlive;
                    }
                }
            }
        }
    }

    Ok(Request {
        method,
        target,
        version,
        connection,
        content_length,
        content_type,
        range,
        expect_continue,
        authorization,
        body_offset: end,
    })
}

/// Decode one query-string field. `+` means space; `%XX` is decoded once.
/// Returns `None` for malformed escapes or a value that does not fit `out`.
pub fn query_value(target: &[u8], key: &[u8], out: &mut [u8]) -> Option<usize> {
    let query = &target[target.iter().position(|&byte| byte == b'?')? + 1..];
    let mut start = 0usize;
    while start <= query.len() {
        let end = query[start..]
            .iter()
            .position(|&byte| byte == b'&')
            .map(|offset| start + offset)
            .unwrap_or(query.len());
        let field = &query[start..end];
        if let Some(equals) = field.iter().position(|&byte| byte == b'=') {
            if &field[..equals] == key {
                return decode_query(&field[equals + 1..], out);
            }
        }
        if end == query.len() {
            break;
        }
        start = end + 1;
    }
    None
}

fn decode_query(value: &[u8], out: &mut [u8]) -> Option<usize> {
    let mut len = 0usize;
    let mut index = 0usize;
    while index < value.len() {
        let byte = if value[index] == b'+' {
            index += 1;
            b' '
        } else if value[index] == b'%' {
            if index + 2 >= value.len() {
                return None;
            }
            let hi = hex_digit(value[index + 1])?;
            let lo = hex_digit(value[index + 2])?;
            index += 3;
            (hi << 4) | lo
        } else {
            let byte = value[index];
            index += 1;
            byte
        };
        if len == out.len() {
            return None;
        }
        out[len] = byte;
        len += 1;
    }
    Some(len)
}

fn hex_digit(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn parse_range(value: &[u8]) -> Option<RangeSpec> {
    let value = trim(value);
    if value.len() < 6 || !value[..6].eq_ignore_ascii_case(b"bytes=") {
        return None;
    }
    let spec = &value[6..];
    // Multipart ranges are legal HTTP but not implemented; ignore the header
    // and serve the full file rather than inventing a multipart response.
    if spec.contains(&b',') {
        return None;
    }
    let dash = spec.iter().position(|&byte| byte == b'-')?;
    let start = trim(&spec[..dash]);
    let end = trim(&spec[dash + 1..]);

    if start.is_empty() {
        let suffix = parse_u64(end)?;
        return Some(RangeSpec::Suffix(suffix));
    }
    let start = parse_u64(start)?;
    if end.is_empty() {
        return Some(RangeSpec::From(start));
    }
    let end = parse_u64(end)?;
    if end < start {
        return None;
    }
    Some(RangeSpec::FromTo(start, end))
}

fn trim(mut value: &[u8]) -> &[u8] {
    while let [first, rest @ ..] = value {
        if first == &b' ' || first == &b'\t' {
            value = rest;
        } else {
            break;
        }
    }
    while let [rest @ .., last] = value {
        if last == &b' ' || last == &b'\t' {
            value = rest;
        } else {
            break;
        }
    }
    value
}

fn parse_u64(value: &[u8]) -> Option<u64> {
    if value.is_empty() {
        return None;
    }
    let mut number = 0u64;
    for &byte in value {
        if !byte.is_ascii_digit() {
            return None;
        }
        number = number.checked_mul(10)?;
        number = number.checked_add((byte - b'0') as u64)?;
    }
    Some(number)
}

/// Response status codes used by the server.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Ok,
    Created,
    MovedPermanently,
    SeeOther,
    BadRequest,
    Forbidden,
    NotFound,
    MethodNotAllowed,
    Unauthorized,
    PartialContent,
    UnsupportedMediaType,
    RangeNotSatisfiable,
    Conflict,
    LengthRequired,
    UriTooLong,
    RequestHeaderFieldsTooLarge,
    InternalServerError,
    VersionNotSupported,
}

impl Status {
    pub const fn code(self) -> u16 {
        match self {
            Status::Ok => 200,
            Status::Created => 201,
            Status::MovedPermanently => 301,
            Status::SeeOther => 303,
            Status::BadRequest => 400,
            Status::Forbidden => 403,
            Status::NotFound => 404,
            Status::MethodNotAllowed => 405,
            Status::Unauthorized => 401,
            Status::PartialContent => 206,
            Status::UnsupportedMediaType => 415,
            Status::RangeNotSatisfiable => 416,
            Status::Conflict => 409,
            Status::LengthRequired => 411,
            Status::UriTooLong => 414,
            Status::RequestHeaderFieldsTooLarge => 431,
            Status::InternalServerError => 500,
            Status::VersionNotSupported => 505,
        }
    }

    pub const fn reason(self) -> &'static [u8] {
        match self {
            Status::Ok => b"OK",
            Status::Created => b"Created",
            Status::MovedPermanently => b"Moved Permanently",
            Status::SeeOther => b"See Other",
            Status::BadRequest => b"Bad Request",
            Status::Forbidden => b"Forbidden",
            Status::NotFound => b"Not Found",
            Status::MethodNotAllowed => b"Method Not Allowed",
            Status::Unauthorized => b"Unauthorized",
            Status::PartialContent => b"Partial Content",
            Status::UnsupportedMediaType => b"Unsupported Media Type",
            Status::RangeNotSatisfiable => b"Range Not Satisfiable",
            Status::Conflict => b"Conflict",
            Status::LengthRequired => b"Length Required",
            Status::UriTooLong => b"URI Too Long",
            Status::RequestHeaderFieldsTooLarge => b"Request Header Fields Too Large",
            Status::InternalServerError => b"Internal Server Error",
            Status::VersionNotSupported => b"HTTP Version Not Supported",
        }
    }

    /// A complete, static error body. Empty for successful/redirect statuses.
    pub const fn error_body(self) -> &'static [u8] {
        match self {
            Status::Ok | Status::Created | Status::MovedPermanently | Status::SeeOther => b"",
            Status::BadRequest => b"400 Bad Request\n",
            Status::Forbidden => b"403 Forbidden\n",
            Status::NotFound => b"404 Not Found\n",
            Status::MethodNotAllowed => b"405 Method Not Allowed\n",
            Status::Unauthorized => b"401 Unauthorized\n",
            Status::PartialContent => b"",
            Status::UnsupportedMediaType => b"415 Unsupported Media Type\n",
            Status::RangeNotSatisfiable => b"416 Range Not Satisfiable\n",
            Status::Conflict => b"409 Conflict\n",
            Status::LengthRequired => b"411 Length Required\n",
            Status::UriTooLong => b"414 URI Too Long\n",
            Status::RequestHeaderFieldsTooLarge => b"431 Request Header Fields Too Large\n",
            Status::InternalServerError => b"500 Internal Server Error\n",
            Status::VersionNotSupported => b"505 HTTP Version Not Supported\n",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        header_end, parse, query_value, Connection, Method, ParseError, RangeSpec, Status, Version,
    };

    #[test]
    fn finds_header_end() {
        assert_eq!(header_end(b"GET / HTTP/1.1\r\n\r\n"), Some(18));
        assert_eq!(header_end(b"GET / HTTP/1.1\r\n"), None);
        assert_eq!(header_end(b""), None);
    }

    #[test]
    fn parses_get_and_head() {
        let r = parse(b"GET /index.html HTTP/1.1\r\nHost: x\r\n\r\n").unwrap();
        assert_eq!(r.method, Method::Get);
        assert_eq!(r.target, b"/index.html");
        assert_eq!(r.version, Version::Http11);
        assert_eq!(r.content_length, None);
        assert_eq!(r.content_type, None);
        assert_eq!(r.range, None);
        assert_eq!(r.body_offset, 37);

        let r = parse(b"HEAD / HTTP/1.0\r\n\r\n").unwrap();
        assert_eq!(r.method, Method::Head);
        assert_eq!(r.version, Version::Http10);
    }

    #[test]
    fn parses_query_fields() {
        let mut out = [0u8; 32];
        let target = b"/dir/?sysweb-text=1&name=note%20one.txt&text=hello+world";
        assert_eq!(query_value(target, b"name", &mut out), Some(12));
        assert_eq!(&out[..12], b"note one.txt");
        assert_eq!(query_value(target, b"text", &mut out), Some(11));
        assert_eq!(&out[..11], b"hello world");
        assert_eq!(query_value(target, b"missing", &mut out), None);
        assert_eq!(query_value(b"/", b"name", &mut out), None);
    }

    #[test]
    fn parses_connection_persistence() {
        let r = parse(b"GET / HTTP/1.1\r\n\r\n").unwrap();
        assert_eq!(r.connection, Connection::Default);
        assert!(r.connection.keep_alive_for(r.version));

        let r = parse(b"GET / HTTP/1.1\r\nConnection: close\r\n\r\n").unwrap();
        assert!(!r.connection.keep_alive_for(r.version));

        let r = parse(b"GET / HTTP/1.0\r\nConnection: keep-alive\r\n\r\n").unwrap();
        assert!(r.connection.keep_alive_for(r.version));

        let r = parse(b"GET / HTTP/1.0\r\n\r\n").unwrap();
        assert!(!r.connection.keep_alive_for(r.version));
    }

    #[test]
    fn parses_single_byte_ranges() {
        let raw = b"GET /x HTTP/1.1\r\nRange: bytes=10-19\r\n\r\n";
        assert_eq!(parse(raw).unwrap().range, Some(RangeSpec::FromTo(10, 19)));
        let raw = b"GET /x HTTP/1.1\r\nRange: bytes=10-\r\n\r\n";
        assert_eq!(parse(raw).unwrap().range, Some(RangeSpec::From(10)));
        let raw = b"GET /x HTTP/1.1\r\nRange: bytes=-10\r\n\r\n";
        assert_eq!(parse(raw).unwrap().range, Some(RangeSpec::Suffix(10)));
        let raw = b"GET /x HTTP/1.1\r\nRange: bytes=0-0,3-4\r\n\r\n";
        assert_eq!(parse(raw).unwrap().range, None);
        let raw = b"GET /x HTTP/1.1\r\nRange: bytes=9-3\r\n\r\n";
        assert_eq!(parse(raw).unwrap().range, None);
        let raw = b"GET /x HTTP/1.1\r\nRange: pages=1-2\r\n\r\n";
        assert_eq!(parse(raw).unwrap().range, None);
    }

    #[test]
    fn parses_authorization_header() {
        let r = parse(b"GET / HTTP/1.1\r\nAuthorization: Basic dXNlcjpwYXNz\r\n\r\n").unwrap();
        assert_eq!(r.authorization, Some(&b"Basic dXNlcjpwYXNz"[..]));
        assert_eq!(
            parse(b"GET / HTTP/1.1\r\n\r\n").unwrap().authorization,
            None
        );
    }

    #[test]
    fn parses_put_headers_and_body_offset() {
        let raw = b"PUT /file.txt HTTP/1.1\r\nContent-Length: 5\r\nContent-Type: text/plain\r\nExpect: 100-continue\r\n\r\nhello";
        let r = parse(raw).unwrap();
        assert_eq!(r.method, Method::Put);
        assert_eq!(r.content_length, Some(5));
        assert_eq!(r.content_type, Some(&b"text/plain"[..]));
        assert_eq!(r.range, None);
        assert!(r.expect_continue);
        assert_eq!(&raw[r.body_offset..], b"hello");
    }

    #[test]
    fn accepts_repeated_equal_content_length() {
        let r =
            parse(b"PUT /x HTTP/1.1\r\nContent-Length: 0\r\nContent-Length: 0\r\n\r\n").unwrap();
        assert_eq!(r.content_length, Some(0));
    }

    #[test]
    fn rejects_bad_headers() {
        assert_eq!(
            parse(b"PUT /x HTTP/1.1\r\nContent-Length: 1\r\nContent-Length: 2\r\n\r\n"),
            Err(ParseError::Malformed)
        );
        assert_eq!(
            parse(b"PUT /x HTTP/1.1\r\nContent-Length: nope\r\n\r\n"),
            Err(ParseError::Malformed)
        );
        assert_eq!(
            parse(b"PUT /x HTTP/1.1\r\nContent-Length: 18446744073709551616\r\n\r\n"),
            Err(ParseError::Malformed)
        );
        assert_eq!(
            parse(b"PUT /x HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n"),
            Err(ParseError::Malformed)
        );
        assert_eq!(
            parse(b"PUT /x HTTP/1.1\r\nNoColonHere\r\n\r\n"),
            Err(ParseError::Malformed)
        );
    }

    #[test]
    fn rejects_bad_request_lines() {
        assert_eq!(parse(b"GET /\r\n\r\n"), Err(ParseError::Malformed));
        assert_eq!(
            parse(b"GET / HTTP/1.1 extra\r\n\r\n"),
            Err(ParseError::Malformed)
        );
        assert_eq!(
            parse(b"GET  / HTTP/1.1\r\n\r\n"),
            Err(ParseError::Malformed)
        );
        assert_eq!(
            parse(b"get / HTTP/1.1\r\n\r\n"),
            Err(ParseError::UnsupportedMethod)
        );
        assert_eq!(
            parse(b"DELETE / HTTP/1.1\r\n\r\n"),
            Err(ParseError::UnsupportedMethod)
        );
        assert_eq!(
            parse(b"GET / HTTP/2.0\r\n\r\n"),
            Err(ParseError::UnsupportedVersion)
        );
        assert_eq!(
            parse(b"GET /a\tb HTTP/1.1\r\n\r\n"),
            Err(ParseError::Malformed)
        );
    }

    #[test]
    fn status_table_is_consistent() {
        assert_eq!(Status::Ok.code(), 200);
        assert_eq!(Status::Ok.error_body(), b"");
        assert_eq!(Status::Created.code(), 201);
        assert_eq!(Status::MovedPermanently.code(), 301);
        assert_eq!(Status::Unauthorized.code(), 401);
        assert_eq!(Status::SeeOther.code(), 303);
        assert_eq!(Status::NotFound.code(), 404);
        assert_eq!(Status::NotFound.reason(), b"Not Found");
        assert_eq!(Status::NotFound.error_body(), b"404 Not Found\n");
        assert_eq!(Status::Conflict.code(), 409);
        assert_eq!(Status::UnsupportedMediaType.code(), 415);
        assert_eq!(Status::LengthRequired.code(), 411);
        assert_eq!(Status::PartialContent.code(), 206);
        assert_eq!(Status::RangeNotSatisfiable.code(), 416);
        assert_eq!(Status::RequestHeaderFieldsTooLarge.code(), 431);
    }
}
