//! Shared HTTP response/path helpers used by the async server.

use crate::fixedbuf::FixedBuf;
use crate::http::{RangeSpec, Status};
use crate::platform::{Interface, Platform, SocketAddr};

pub(crate) const REQUEST_BUF: usize = 8192;
pub(crate) const HEADER_BUF: usize = 4096;
const LOG_BUF: usize = 1200;

/// Convert a normalized URL path (`/a/b`) into a path relative to the
/// document-root fd (`a/b`). The root itself is `"."`.
pub(crate) fn root_relative(url_path: &[u8]) -> &[u8] {
    if url_path.len() <= 1 {
        b"."
    } else {
        &url_path[1..]
    }
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

pub(crate) fn resolve_range(spec: RangeSpec, total: u64) -> Option<(u64, u64)> {
    if total == 0 {
        return None;
    }
    match spec {
        RangeSpec::FromTo(start, end) => {
            if start >= total {
                None
            } else {
                Some((start, core::cmp::min(end, total - 1)))
            }
        }
        RangeSpec::From(start) => {
            if start >= total {
                None
            } else {
                Some((start, total - 1))
            }
        }
        RangeSpec::Suffix(length) => {
            if length == 0 {
                None
            } else {
                Some((total.saturating_sub(length), total - 1))
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn build_stream_header(
    buffer: &mut FixedBuf<HEADER_BUF>,
    content_type: &[u8],
    chunked: bool,
    keep_alive: bool,
) -> bool {
    buffer.clear();
    if !buffer.push(b"HTTP/1.1 200 OK\r\n")
        || !buffer.push(b"Server: sysweb/")
        || !buffer.push(crate::VERSION.as_bytes())
        || !buffer.push(b"\r\n")
        || !buffer.push(b"Content-Type: ")
        || !buffer.push(content_type)
        || !buffer.push(b"\r\n")
    {
        return false;
    }
    if chunked && !buffer.push(b"Transfer-Encoding: chunked\r\n") {
        return false;
    }
    if keep_alive {
        buffer.push(b"Connection: keep-alive\r\n\r\n")
    } else {
        buffer.push(b"Connection: close\r\n\r\n")
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
    if status == Status::Unauthorized
        && (!buffer.push(b"WWW-Authenticate: Basic realm=\"sysweb\"\r\n"))
    {
        return false;
    }

    if keep_alive {
        buffer.push(b"Connection: keep-alive\r\n\r\n")
    } else {
        buffer.push(b"Connection: close\r\n\r\n")
    }
}
