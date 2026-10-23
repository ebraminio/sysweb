//! Extension-to-MIME mapping. Static table, no allocation.

const OCTET_STREAM: &[u8] = b"application/octet-stream";

/// MIME types are intentionally conservative: everything textual declares
/// UTF-8, and unknown extensions fall back to `application/octet-stream`.
const TABLE: &[(&[u8], &[u8])] = &[
    // Ported from pad.js's contentTypes table, with UTF-8 declared for
    // textual types and a few modern aliases retained.
    (b"html", b"text/html; charset=utf-8"),
    (b"htm", b"text/html; charset=utf-8"),
    (b"shtml", b"text/html; charset=utf-8"),
    (b"css", b"text/css; charset=utf-8"),
    (b"js", b"text/javascript; charset=utf-8"),
    (b"mjs", b"text/javascript; charset=utf-8"),
    (b"json", b"application/json"),
    (b"map", b"application/json"),
    (b"txt", b"text/plain; charset=utf-8"),
    (b"md", b"text/markdown; charset=utf-8"),
    (b"xml", b"application/xml"),
    (b"atom", b"application/atom+xml"),
    (b"rss", b"application/rss+xml"),
    (b"mml", b"text/mathml; charset=utf-8"),
    (b"jad", b"text/vnd.sun.j2me.app-descriptor"),
    (b"wml", b"text/vnd.wap.wml; charset=utf-8"),
    (b"htc", b"text/x-component"),
    (b"xhtml", b"application/xhtml+xml"),
    (b"xspf", b"application/xspf+xml"),
    (b"svg", b"image/svg+xml"),
    (b"svgz", b"image/svg+xml"),
    (b"png", b"image/png"),
    (b"jpg", b"image/jpeg"),
    (b"jpeg", b"image/jpeg"),
    (b"gif", b"image/gif"),
    (b"webp", b"image/webp"),
    (b"avif", b"image/avif"),
    (b"ico", b"image/x-icon"),
    (b"jng", b"image/x-jng"),
    (b"bmp", b"image/x-ms-bmp"),
    (b"tif", b"image/tiff"),
    (b"tiff", b"image/tiff"),
    (b"wbmp", b"image/vnd.wap.wbmp"),
    (b"wasm", b"application/wasm"),
    (b"pdf", b"application/pdf"),
    (b"ps", b"application/postscript"),
    (b"eps", b"application/postscript"),
    (b"ai", b"application/postscript"),
    (b"rtf", b"application/rtf"),
    (b"doc", b"application/msword"),
    (
        b"docx",
        b"application/vnd.openxmlformats-officedocument.wordprocessingml.document",
    ),
    (b"xls", b"application/vnd.ms-excel"),
    (
        b"xlsx",
        b"application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
    ),
    (b"ppt", b"application/vnd.ms-powerpoint"),
    (
        b"pptx",
        b"application/vnd.openxmlformats-officedocument.presentationml.presentation",
    ),
    (b"hqx", b"application/mac-binhex40"),
    (b"m3u8", b"application/vnd.apple.mpegurl"),
    (b"kml", b"application/vnd.google-earth.kml+xml"),
    (b"kmz", b"application/vnd.google-earth.kmz"),
    (b"jar", b"application/java-archive"),
    (b"war", b"application/java-archive"),
    (b"ear", b"application/java-archive"),
    (b"jnlp", b"application/x-java-jnlp-file"),
    (b"jardiff", b"application/x-java-archive-diff"),
    (b"7z", b"application/x-7z-compressed"),
    (b"rar", b"application/x-rar-compressed"),
    (b"zip", b"application/zip"),
    (b"tar", b"application/x-tar"),
    (b"tgz", b"application/x-tar-gz"),
    (b"gz", b"application/x-gzip"),
    (b"deb", b"application/x-debian-package"),
    (b"rpm", b"application/x-redhat-package-manager"),
    (b"dmg", b"application/octet-stream"),
    (b"iso", b"application/octet-stream"),
    (b"img", b"application/octet-stream"),
    (b"msi", b"application/octet-stream"),
    (b"msp", b"application/octet-stream"),
    (b"msm", b"application/octet-stream"),
    (b"bin", b"application/octet-stream"),
    (b"exe", b"application/octet-stream"),
    (b"dll", b"application/octet-stream"),
    (b"sea", b"application/x-sea"),
    (b"run", b"application/x-makeself"),
    (b"swf", b"application/x-shockwave-flash"),
    (b"sit", b"application/x-stuffit"),
    (b"xpi", b"application/x-xpinstall"),
    (b"der", b"application/x-x509-ca-cert"),
    (b"pem", b"application/x-x509-ca-cert"),
    (b"crt", b"application/x-x509-ca-cert"),
    (b"cco", b"application/x-cocoa"),
    (b"pl", b"application/x-perl"),
    (b"pm", b"application/x-perl"),
    (b"prc", b"application/x-pilot"),
    (b"pdb", b"application/x-pilot"),
    (b"tcl", b"application/x-tcl"),
    (b"tk", b"application/x-tcl"),
    (b"mime", b"www/mime"),
    (b"m4a", b"audio/x-m4a"),
    (b"mid", b"audio/midi"),
    (b"midi", b"audio/midi"),
    (b"kar", b"audio/midi"),
    (b"mp3", b"audio/mpeg"),
    (b"ogg", b"audio/ogg"),
    (b"ra", b"audio/x-realaudio"),
    (b"3gpp", b"video/3gpp"),
    (b"3gp", b"video/3gpp"),
    (b"ts", b"video/mp2t"),
    (b"mp4", b"video/mp4"),
    (b"mpeg", b"video/mpeg"),
    (b"mpg", b"video/mpeg"),
    (b"mov", b"video/quicktime"),
    (b"webm", b"video/webm"),
    (b"flv", b"video/x-flv"),
    (b"m4v", b"video/x-m4v"),
    (b"mng", b"video/x-mng"),
    (b"asx", b"video/x-ms-asf"),
    (b"asf", b"video/x-ms-asf"),
    (b"wmv", b"video/x-ms-wmv"),
    (b"avi", b"video/x-msvideo"),
    (b"mkv", b"video/x-matroska"),
    (b"woff", b"font/woff"),
    (b"woff2", b"font/woff2"),
    (b"ttf", b"font/ttf"),
    (b"otf", b"font/otf"),
    (b"eot", b"application/vnd.ms-fontobject"),
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
    fn pad_js_types_are_available() {
        assert_eq!(content_type(b"/x.shtml"), b"text/html; charset=utf-8");
        assert_eq!(content_type(b"/x.md"), b"text/markdown; charset=utf-8");
        assert_eq!(content_type(b"/x.svgz"), b"image/svg+xml");
        assert_eq!(content_type(b"/x.7z"), b"application/x-7z-compressed");
        assert_eq!(
            content_type(b"/x.docx"),
            b"application/vnd.openxmlformats-officedocument.wordprocessingml.document"
        );
        assert_eq!(content_type(b"/x.mkv"), b"video/x-matroska");
        assert_eq!(content_type(b"/x.gz"), b"application/x-gzip");
    }

    #[test]
    fn unknown_and_weird_names() {
        assert_eq!(content_type(b"/x"), b"application/octet-stream");
        assert_eq!(content_type(b"/x."), b"application/octet-stream");
        assert_eq!(content_type(b"/.plan"), b"application/octet-stream");
        assert_eq!(content_type(b"/dir.html/file"), b"application/octet-stream");
        assert_eq!(content_type(b"/x.tar.gz"), b"application/x-gzip");
    }
}
