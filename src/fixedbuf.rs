//! A tiny fixed-capacity byte buffer. No heap, no `alloc`.

/// A fixed-capacity byte buffer that silently reports overflow.
///
/// The buffer never grows and never panics on overflow: `push*` returns
/// `false` and leaves the contents unchanged.
pub struct FixedBuf<const N: usize> {
    buf: [u8; N],
    len: usize,
}

impl<const N: usize> FixedBuf<N> {
    pub const fn new() -> Self {
        Self {
            buf: [0; N],
            len: 0,
        }
    }

    pub fn clear(&mut self) {
        self.len = 0;
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn capacity(&self) -> usize {
        N
    }

    pub fn remaining(&self) -> usize {
        N - self.len
    }

    pub fn as_slice(&self) -> &[u8] {
        &self.buf[..self.len]
    }

    /// Append `bytes`. Returns `false` (and does nothing) if they do not fit.
    pub fn push(&mut self, bytes: &[u8]) -> bool {
        let end = match self.len.checked_add(bytes.len()) {
            Some(end) if end <= N => end,
            _ => return false,
        };
        self.buf[self.len..end].copy_from_slice(bytes);
        self.len = end;
        true
    }

    pub fn push_byte(&mut self, byte: u8) -> bool {
        if self.len == N {
            return false;
        }
        self.buf[self.len] = byte;
        self.len += 1;
        true
    }

    /// Append the decimal representation of `value`. Returns `false` on
    /// overflow.
    pub fn push_u64(&mut self, mut value: u64) -> bool {
        let mut digits = [0u8; 20];
        let mut at = digits.len();
        loop {
            at -= 1;
            digits[at] = b'0' + (value % 10) as u8;
            value /= 10;
            if value == 0 {
                break;
            }
        }
        self.push(&digits[at..])
    }

    /// Append a dotted-quad IPv4 address.
    pub fn push_ipv4(&mut self, ip: [u8; 4]) -> bool {
        for (i, octet) in ip.iter().enumerate() {
            if i != 0 && !self.push_byte(b'.') {
                return false;
            }
            if !self.push_u64(*octet as u64) {
                return false;
            }
        }
        true
    }
}

impl<const N: usize> Default for FixedBuf<N> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::FixedBuf;

    #[test]
    fn push_and_overflow() {
        let mut b = FixedBuf::<4>::new();
        assert!(b.push(b"abc"));
        assert_eq!(b.as_slice(), b"abc");
        assert!(!b.push(b"de"));
        assert_eq!(b.as_slice(), b"abc");
        assert!(b.push_byte(b'd'));
        assert!(!b.push_byte(b'e'));
        assert_eq!(b.as_slice(), b"abcd");
    }

    #[test]
    fn numbers() {
        let mut b = FixedBuf::<32>::new();
        assert!(b.push_u64(0));
        assert!(b.push_byte(b' '));
        assert!(b.push_u64(9_007_199_254_740_993));
        assert_eq!(b.as_slice(), b"0 9007199254740993");

        let mut ip = FixedBuf::<16>::new();
        assert!(ip.push_ipv4([127, 0, 0, 1]));
        assert_eq!(ip.as_slice(), b"127.0.0.1");
    }

    #[test]
    fn clear_resets_len() {
        let mut b = FixedBuf::<8>::new();
        assert!(b.push(b"hello"));
        b.clear();
        assert!(b.is_empty());
        assert_eq!(b.remaining(), 8);
    }
}
