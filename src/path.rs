//! Request-target decoding and filesystem path construction.
//!
//! Everything here is safe Rust and operates on fixed-size buffers. The
//! security-critical rule is that the normalized URL path can never contain a
//! `..` segment, so joining it to the document root cannot escape that root
//! (symlinks are a separate, documented limitation).

/// Maximum normalized URL path, excluding any root prefix.
pub const MAX_URL_PATH: usize = 1024;
/// Maximum filesystem path, including the document root and a NUL byte.
pub const MAX_PATH: usize = 1536;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PathError {
    /// The decoded/normalized path does not fit the destination buffer.
    TooLong,
    /// A `%` escape was malformed.
    BadEscape,
    /// The target was not origin-form, escaped the root, or contained a byte
    /// that is never valid in a served path.
    Forbidden,
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

/// Decode percent escapes and normalize an HTTP origin-form target.
///
/// Rules:
/// * the target must start with `/`;
/// * the query string and fragment are discarded;
/// * `%XX` is decoded once;
/// * NUL, backslash, and ASCII control bytes are rejected;
/// * empty and `.` segments are dropped;
/// * `..` pops the previous segment and is rejected if it would escape `/`;
/// * a trailing slash is preserved (the caller maps it to `index.html`).
///
/// Returns the length written to `out` (no NUL terminator).
pub fn normalize(target: &[u8], out: &mut [u8]) -> Result<usize, PathError> {
    let end = target
        .iter()
        .position(|&b| b == b'?' || b == b'#')
        .unwrap_or(target.len());
    let target = &target[..end];

    if target.first() != Some(&b'/') {
        return Err(PathError::Forbidden);
    }

    // Pass 1: decode into a scratch buffer while validating bytes.
    let mut decoded = [0u8; MAX_URL_PATH];
    let mut decoded_len = 0usize;
    let mut i = 0usize;
    while i < target.len() {
        let raw = target[i];
        let byte = if raw == b'%' {
            if i + 2 >= target.len() {
                return Err(PathError::BadEscape);
            }
            let hi = hex_value(target[i + 1]).ok_or(PathError::BadEscape)?;
            let lo = hex_value(target[i + 2]).ok_or(PathError::BadEscape)?;
            i += 3;
            (hi << 4) | lo
        } else {
            i += 1;
            raw
        };

        if byte == 0 || byte == b'\\' || byte < 0x20 || byte == 0x7f {
            return Err(PathError::Forbidden);
        }
        if decoded_len == decoded.len() {
            return Err(PathError::TooLong);
        }
        decoded[decoded_len] = byte;
        decoded_len += 1;
    }

    let trailing_slash = decoded_len > 1 && decoded[decoded_len - 1] == b'/';

    // Pass 2: segment-wise normalization.
    if out.is_empty() {
        return Err(PathError::TooLong);
    }
    out[0] = b'/';
    let mut out_len = 1usize;

    let mut segment_start = 1usize;
    let mut i = 1usize;
    while i <= decoded_len {
        let at_end = i == decoded_len;
        if at_end || decoded[i] == b'/' {
            let segment = &decoded[segment_start..i];
            if !segment.is_empty() && segment != b"." {
                if segment == b".." {
                    if out_len == 1 {
                        return Err(PathError::Forbidden);
                    }
                    // Remove the previous segment and its separator.
                    while out_len > 1 && out[out_len - 1] != b'/' {
                        out_len -= 1;
                    }
                    if out_len > 1 {
                        out_len -= 1;
                    }
                } else {
                    let needed = segment.len() + usize::from(out_len > 1);
                    if out_len + needed > out.len() {
                        return Err(PathError::TooLong);
                    }
                    if out_len > 1 {
                        out[out_len] = b'/';
                        out_len += 1;
                    }
                    out[out_len..out_len + segment.len()].copy_from_slice(segment);
                    out_len += segment.len();
                }
            }
            segment_start = i + 1;
        }
        i += 1;
    }

    if trailing_slash && out_len > 1 {
        if out_len == out.len() {
            return Err(PathError::TooLong);
        }
        out[out_len] = b'/';
        out_len += 1;
    }

    Ok(out_len)
}

fn append(out: &mut [u8], len: &mut usize, bytes: &[u8]) -> Result<(), PathError> {
    if *len + bytes.len() >= out.len() {
        return Err(PathError::TooLong);
    }
    out[*len..*len + bytes.len()].copy_from_slice(bytes);
    *len += bytes.len();
    Ok(())
}

/// Join the document root and a normalized URL path into `out`, without
/// appending a directory index. The returned length excludes the trailing
/// NUL, which is written at `out[length]`.
pub fn join_dir(root: &[u8], path: &[u8], out: &mut [u8]) -> Result<usize, PathError> {
    if root.is_empty() || path.first() != Some(&b'/') {
        return Err(PathError::Forbidden);
    }
    if root.contains(&0) || path.contains(&0) {
        return Err(PathError::Forbidden);
    }
    // Defense in depth: `normalize` already removes `..`, but a future caller
    // must not be able to join an escaping path by accident.
    if path.split(|&byte| byte == b'/').any(|seg| seg == b"..") {
        return Err(PathError::Forbidden);
    }

    let mut len = 0usize;
    if root == b"/" {
        append(out, &mut len, path)?;
    } else {
        append(out, &mut len, root)?;
        append(out, &mut len, path)?;
    }

    out[len] = 0;
    Ok(len)
}

/// Append `index.html` to a directory path. The returned length excludes the
/// trailing NUL.
pub fn append_index(path: &[u8], out: &mut [u8]) -> Result<usize, PathError> {
    if path.last() != Some(&b'/') || path.contains(&0) {
        return Err(PathError::Forbidden);
    }
    if path.len() + 10 >= out.len() {
        return Err(PathError::TooLong);
    }
    out[..path.len()].copy_from_slice(path);
    out[path.len()..path.len() + 10].copy_from_slice(b"index.html");
    out[path.len() + 10] = 0;
    Ok(path.len() + 10)
}

fn append_index_in_place(out: &mut [u8], len: usize) -> Result<usize, PathError> {
    if len + 10 >= out.len() {
        return Err(PathError::TooLong);
    }
    out[len..len + 10].copy_from_slice(b"index.html");
    out[len + 10] = 0;
    Ok(len + 10)
}

/// Join the document root and a normalized URL path into `out`.
///
/// A path ending in `/` gets `index.html` appended. The returned length
/// excludes the trailing NUL, which is written at `out[length]`.
pub fn join(root: &[u8], path: &[u8], out: &mut [u8]) -> Result<usize, PathError> {
    let len = join_dir(root, path, out)?;
    if path.ends_with(b"/") {
        append_index_in_place(out, len)
    } else {
        Ok(len)
    }
}

#[cfg(test)]
mod tests {
    use super::{append_index, join, join_dir, normalize, PathError, MAX_PATH, MAX_URL_PATH};

    fn norm(target: &[u8]) -> Result<&[u8], PathError> {
        // Test helper leaks through a static scratch buffer; tests run
        // single-threaded per test, but use a local anyway.
        let mut buf = [0u8; MAX_URL_PATH];
        let n = normalize(target, &mut buf)?;
        // Copy into a stable Vec for the assertion.
        let v = buf[..n].to_vec();
        Ok(Box::leak(v.into_boxed_slice()))
    }

    #[test]
    fn simple_paths() {
        assert_eq!(norm(b"/").unwrap(), b"/");
        assert_eq!(norm(b"/index.html").unwrap(), b"/index.html");
        assert_eq!(norm(b"/a/b/c").unwrap(), b"/a/b/c");
        assert_eq!(norm(b"//a///b//").unwrap(), b"/a/b/");
    }

    #[test]
    fn dot_segments() {
        assert_eq!(norm(b"/a/./b").unwrap(), b"/a/b");
        assert_eq!(norm(b"/a/../b").unwrap(), b"/b");
        assert_eq!(norm(b"/a/b/..").unwrap(), b"/a");
        assert_eq!(norm(b"/a/b/../").unwrap(), b"/a/");
        assert_eq!(norm(b"/.").unwrap(), b"/");
        assert_eq!(norm(b"/..").unwrap_err(), PathError::Forbidden);
        assert_eq!(norm(b"/a/../../b").unwrap_err(), PathError::Forbidden);
    }

    #[test]
    fn percent_decoding() {
        assert_eq!(norm(b"/hello%20world.txt").unwrap(), b"/hello world.txt");
        assert_eq!(norm(b"/%68%65%6C%6C%6F").unwrap(), b"/hello");
        assert_eq!(norm(b"/a%2Fb").unwrap(), b"/a/b");
        assert_eq!(norm(b"/%2e%2e/etc").unwrap_err(), PathError::Forbidden);
        assert_eq!(norm(b"/%00").unwrap_err(), PathError::Forbidden);
        assert_eq!(norm(b"/%5c").unwrap_err(), PathError::Forbidden);
        assert_eq!(norm(b"/%zz").unwrap_err(), PathError::BadEscape);
        assert_eq!(norm(b"/%2").unwrap_err(), PathError::BadEscape);
    }

    #[test]
    fn query_and_fragment_are_stripped() {
        assert_eq!(norm(b"/x?y=1").unwrap(), b"/x");
        assert_eq!(norm(b"/x#frag").unwrap(), b"/x");
        // A literal % in the query is not decoded.
        assert_eq!(norm(b"/x?y=%zz").unwrap(), b"/x");
    }

    #[test]
    fn rejects_bad_targets() {
        assert_eq!(norm(b"x").unwrap_err(), PathError::Forbidden);
        assert_eq!(norm(b"http://x/").unwrap_err(), PathError::Forbidden);
        assert_eq!(norm(b"").unwrap_err(), PathError::Forbidden);
        assert_eq!(norm(b"/a\x01").unwrap_err(), PathError::Forbidden);
    }

    #[test]
    fn long_paths() {
        let mut buf = [0u8; MAX_URL_PATH];
        let mut long = [b'a'; MAX_URL_PATH + 8];
        long[0] = b'/';
        assert_eq!(normalize(&long, &mut buf), Err(PathError::TooLong));

        let exact = MAX_URL_PATH - 1;
        let mut ok = [b'a'; MAX_URL_PATH - 1];
        ok[0] = b'/';
        let n = normalize(&ok, &mut buf).unwrap();
        assert_eq!(n, exact);
    }

    #[test]
    fn join_paths() {
        let mut out = [0u8; MAX_PATH];

        let n = join(b"public", b"/index.html", &mut out).unwrap();
        assert_eq!(&out[..n], b"public/index.html");
        assert_eq!(out[n], 0);

        let n = join(b"public", b"/sub/", &mut out).unwrap();
        assert_eq!(&out[..n], b"public/sub/index.html");

        let n = join(b"public", b"/", &mut out).unwrap();
        assert_eq!(&out[..n], b"public/index.html");

        let n = join(b"/", b"/", &mut out).unwrap();
        assert_eq!(&out[..n], b"/index.html");

        let n = join(b".", b"/a", &mut out).unwrap();
        assert_eq!(&out[..n], b"./a");
    }

    #[test]
    fn joins_directories_without_an_index() {
        let mut out = [0u8; MAX_PATH];

        let n = join_dir(b"public", b"/assets/", &mut out).unwrap();
        assert_eq!(&out[..n], b"public/assets/");
        assert_eq!(out[n], 0);

        let mut index = [0u8; MAX_PATH];
        let n = append_index(&out[..n], &mut index).unwrap();
        assert_eq!(&index[..n], b"public/assets/index.html");
        assert_eq!(index[n], 0);

        let mut out2 = [0u8; MAX_PATH];
        let n = join_dir(b"public", b"/assets", &mut out2).unwrap();
        assert_eq!(&out2[..n], b"public/assets");
        let n = join(b"public", b"/assets", &mut out2).unwrap();
        assert_eq!(&out2[..n], b"public/assets");

        assert_eq!(
            append_index(b"public/assets", &mut out2),
            Err(PathError::Forbidden)
        );
    }

    #[test]
    fn join_rejects_escape_and_overflow() {
        let mut out = [0u8; 16];
        assert_eq!(
            join(b"public", b"/../etc/passwd", &mut out),
            Err(PathError::Forbidden)
        );
        assert_eq!(join(b"", b"/x", &mut out), Err(PathError::Forbidden));
        assert_eq!(
            join(b"public", b"/0123456789abcdef", &mut out),
            Err(PathError::TooLong)
        );
    }
}
