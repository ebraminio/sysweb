//! HTTP/1.0 and HTTP/1.1 request parsing plus the small status-code table.
//!
//! This module is pure: no I/O, no unsafe, no allocation.

/// The methods this server implements.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Method {
    Get,
    Head,
}

impl Method {
    pub const fn as_bytes(self) -> &'static [u8] {
        match self {
            Method::Get => b"GET",
            Method::Head => b"HEAD",
        }
    }
}

/// Only the two HTTP versions a static file server needs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Version {
    Http10,
    Http11,
}

/// A parsed request line. Header fields are intentionally not retained yet.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Request<'a> {
    pub method: Method,
    pub target: &'a [u8],
    pub version: Version,
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

    Ok(Request {
        method,
        target,
        version,
    })
}

/// Response status codes used by the server.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Ok,
    BadRequest,
    Forbidden,
    NotFound,
    MethodNotAllowed,
    UriTooLong,
    RequestHeaderFieldsTooLarge,
    InternalServerError,
    VersionNotSupported,
}

impl Status {
    pub const fn code(self) -> u16 {
        match self {
            Status::Ok => 200,
            Status::BadRequest => 400,
            Status::Forbidden => 403,
            Status::NotFound => 404,
            Status::MethodNotAllowed => 405,
            Status::UriTooLong => 414,
            Status::RequestHeaderFieldsTooLarge => 431,
            Status::InternalServerError => 500,
            Status::VersionNotSupported => 505,
        }
    }

    pub const fn reason(self) -> &'static [u8] {
        match self {
            Status::Ok => b"OK",
            Status::BadRequest => b"Bad Request",
            Status::Forbidden => b"Forbidden",
            Status::NotFound => b"Not Found",
            Status::MethodNotAllowed => b"Method Not Allowed",
            Status::UriTooLong => b"URI Too Long",
            Status::RequestHeaderFieldsTooLarge => b"Request Header Fields Too Large",
            Status::InternalServerError => b"Internal Server Error",
            Status::VersionNotSupported => b"HTTP Version Not Supported",
        }
    }

    /// A complete, static error body. Empty for 200.
    pub const fn error_body(self) -> &'static [u8] {
        match self {
            Status::Ok => b"",
            Status::BadRequest => b"400 Bad Request\n",
            Status::Forbidden => b"403 Forbidden\n",
            Status::NotFound => b"404 Not Found\n",
            Status::MethodNotAllowed => b"405 Method Not Allowed\n",
            Status::UriTooLong => b"414 URI Too Long\n",
            Status::RequestHeaderFieldsTooLarge => b"431 Request Header Fields Too Large\n",
            Status::InternalServerError => b"500 Internal Server Error\n",
            Status::VersionNotSupported => b"505 HTTP Version Not Supported\n",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{header_end, parse, Method, ParseError, Status, Version};

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

        let r = parse(b"HEAD / HTTP/1.0\r\n\r\n").unwrap();
        assert_eq!(r.method, Method::Head);
        assert_eq!(r.version, Version::Http10);
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
            parse(b"POST / HTTP/1.1\r\n\r\n"),
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
        assert_eq!(Status::NotFound.code(), 404);
        assert_eq!(Status::NotFound.reason(), b"Not Found");
        assert_eq!(Status::NotFound.error_body(), b"404 Not Found\n");
        assert_eq!(Status::RequestHeaderFieldsTooLarge.code(), 431);
    }
}
