//! HTTP Basic authentication without allocation.
//!
//! The decoded credential is compared with a constant-time byte loop. Length
//! differences are visible, but the credential bytes themselves are not.

const MAX_DECODED: usize = 256;

/// Return true when `header` carries the configured `user:pass` credential.
pub fn authorized(header: Option<&[u8]>, user: &[u8], pass: &[u8]) -> bool {
    let Some(value) = header else {
        return false;
    };
    if value.len() < 6 || !value[..6].eq_ignore_ascii_case(b"basic ") {
        return false;
    }
    let mut decoded = [0u8; MAX_DECODED];
    let Some(len) = decode_base64(&value[6..], &mut decoded) else {
        return false;
    };
    let expected = user.len() + 1 + pass.len();
    if len != expected || expected == 0 || expected > decoded.len() {
        return false;
    }

    let mut difference = 0u8;
    for (index, byte) in decoded[..expected].iter().enumerate() {
        let expected = if index < user.len() {
            user[index]
        } else if index == user.len() {
            b':'
        } else {
            pass[index - user.len() - 1]
        };
        difference |= byte ^ expected;
    }
    difference == 0
}

fn decode_base64(value: &[u8], out: &mut [u8]) -> Option<usize> {
    let mut accumulator = 0u32;
    let mut bits = 0u32;
    let mut len = 0usize;
    let mut padding = false;
    for &byte in value {
        if byte == b'=' {
            padding = true;
            continue;
        }
        if padding {
            return None;
        }
        let digit = match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => return None,
        };
        accumulator = (accumulator << 6) | digit as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            if len == out.len() {
                return None;
            }
            out[len] = (accumulator >> bits) as u8;
            len += 1;
        }
    }
    Some(len)
}

#[cfg(test)]
mod tests {
    use super::authorized;

    #[test]
    fn accepts_valid_credentials() {
        assert!(authorized(Some(b"Basic dXNlcjpwYXNz"), b"user", b"pass"));
        assert!(!authorized(
            Some(b"Basic dXNlcjp3cm9uZw=="),
            b"user",
            b"pass"
        ));
        assert!(authorized(Some(b"basic dXNlcjpwYXNz"), b"user", b"pass"));
        assert!(!authorized(Some(b"Basic !!!"), b"user", b"pass"));
        assert!(!authorized(None, b"user", b"pass"));
    }

    #[test]
    fn supports_password_colons() {
        assert!(authorized(
            Some(b"Basic dXNlcjpwYTpzcw=="),
            b"user",
            b"pa:ss"
        ));
    }
}
