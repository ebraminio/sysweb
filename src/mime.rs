//! Extension-to-MIME mapping. Static table, no allocation.

const OCTET_STREAM: &[u8] = b"application/octet-stream";

/// MIME types are intentionally conservative: everything textual declares
/// UTF-8, and unknown extensions fall back to `application/octet-stream`.
const TABLE: &[(&[u8], &[u8])] = &[
    (b"html", b"text/html; charset=utf-8"),
    (b"htm", b"text/html; charset=utf-8"),
    (b"css", b"text/css; charset=utf-8"),
    (b"js", b"text/javascript; charset=utf-8"),
    (b"mjs", b"text/javascript; charset=utf-8"),
    (b"json", b"application/json"),
    (b"map", b"application/json"),
    (b"txt", b"text/plain; charset=utf-8"),
    (b"md", b"text/plain; charset=utf-8"),
    (b"xml", b"application/xml"),
    (b"svg", b"image/svg+xml"),
    (b"png", b"image/png"),
    (b"jpg", b"image/jpeg"),
    (b"jpeg", b"image/jpeg"),
    (b"gif", b"image/gif"),
    (b"webp", b"image/webp"),
    (b"avif", b"image/avif"),
    (b"ico", b"image/x-icon"),
    (b"wasm", b"application/wasm"),
    (b"pdf", b"application/pdf"),
    (b"woff", b"font/woff"),
    (b"woff2", b"font/woff2"),
    (b"ttf", b"font/ttf"),
    (b"otf", b"font/otf"),
];

/// Return the content type for `path`, based on its final extension.
pub fn content_type(path: &[u8]) -> &'static [u8] {
    let name = match path.iter().rposition(|&b| b == b'/') {
        Some(slash) => &path[slash + 1..],
        None => path,
    };
    let dot = match name.iter().rposition(|&b| b == b'.') {
        Some(dot) if dot + 1 < name.len() => dot,
        _ => return OCTET_STREAM,
    };
    let ext = &name[dot + 1..];
    for (candidate, mime) in TABLE {
        if ext.eq_ignore_ascii_case(candidate) {
            return mime;
        }
    }
    OCTET_STREAM
}

#[cfg(test)]
mod tests {
    use super::content_type;

    #[test]
    fn common_types() {
        assert_eq!(content_type(b"/index.html"), b"text/html; charset=utf-8");
        assert_eq!(content_type(b"/a/b/style.CSS"), b"text/css; charset=utf-8");
        assert_eq!(content_type(b"/app.js"), b"text/javascript; charset=utf-8");
        assert_eq!(content_type(b"/x.wasm"), b"application/wasm");
    }

    #[test]
    fn unknown_and_weird_names() {
        assert_eq!(content_type(b"/x"), b"application/octet-stream");
        assert_eq!(content_type(b"/x."), b"application/octet-stream");
        assert_eq!(content_type(b"/.plan"), b"application/octet-stream");
        assert_eq!(content_type(b"/dir.html/file"), b"application/octet-stream");
        assert_eq!(content_type(b"/x.tar.gz"), b"application/octet-stream");
    }
}
