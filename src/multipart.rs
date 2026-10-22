//! Bounded streaming `multipart/form-data` parser.
//!
//! This exists for old browsers and no-JavaScript form uploads. Modern clients
//! can keep using raw `PUT`; this parser adds the classic HTML form path
//! without allocating.
//!
//! The parser holds at most one 32 KiB buffer plus a boundary-sized tail. File
//! bytes are handed to a [`Sink`] as they are confirmed, so a multi-gigabyte
//! part never has to fit in memory.

pub const MAX_BOUNDARY: usize = 70;
pub const MAX_FILENAME: usize = 255;

const BUFFER: usize = 32 * 1024;
const MAX_HEADERS: usize = 4096;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SinkError {
    Conflict,
    Forbidden,
    NotFound,
    TooManyFiles,
    Io,
}

/// Destination for parsed file parts.
pub trait Sink {
    fn begin_file(&mut self, filename: &[u8]) -> Result<(), SinkError>;
    fn write(&mut self, data: &[u8]) -> Result<(), SinkError>;
    fn end_file(&mut self) -> Result<(), SinkError>;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParseError {
    UnsupportedContentType,
    Malformed,
    NoFile,
    Sink(SinkError),
}

enum State {
    Preamble,
    Headers,
    Body,
    Epilogue,
}

pub struct Parser {
    boundary: [u8; MAX_BOUNDARY],
    boundary_len: usize,
    delimiter: [u8; MAX_BOUNDARY + 4],
    delimiter_len: usize,
    state: State,
    buffer: [u8; BUFFER],
    len: usize,
    files: usize,
    writing: bool,
}

impl Parser {
    pub fn new(content_type: &[u8]) -> Result<Self, ParseError> {
        let (boundary, boundary_len) = parse_boundary(content_type)?;
        let mut delimiter = [0u8; MAX_BOUNDARY + 4];
        delimiter[..4].copy_from_slice(b"\r\n--");
        delimiter[4..4 + boundary_len].copy_from_slice(&boundary[..boundary_len]);

        Ok(Self {
            boundary,
            boundary_len,
            delimiter,
            delimiter_len: boundary_len + 4,
            state: State::Preamble,
            buffer: [0; BUFFER],
            len: 0,
            files: 0,
            writing: false,
        })
    }

    pub fn feed<S: Sink>(&mut self, mut data: &[u8], sink: &mut S) -> Result<(), ParseError> {
        while !data.is_empty() {
            self.process(sink)?;
            if matches!(self.state, State::Epilogue) {
                return Ok(());
            }
            let space = BUFFER - self.len;
            let take = core::cmp::min(space, data.len());
            self.buffer[self.len..self.len + take].copy_from_slice(&data[..take]);
            self.len += take;
            data = &data[take..];
            self.process(sink)?;
        }
        Ok(())
    }

    pub fn finish<S: Sink>(&mut self, sink: &mut S) -> Result<(), ParseError> {
        self.process(sink)?;
        if !matches!(self.state, State::Epilogue) {
            return Err(ParseError::Malformed);
        }
        if self.files == 0 {
            return Err(ParseError::NoFile);
        }
        Ok(())
    }

    fn process<S: Sink>(&mut self, sink: &mut S) -> Result<(), ParseError> {
        loop {
            match self.state {
                State::Preamble => {
                    let mut needle = [0u8; MAX_BOUNDARY + 2];
                    needle[..2].copy_from_slice(b"--");
                    needle[2..2 + self.boundary_len]
                        .copy_from_slice(&self.boundary[..self.boundary_len]);
                    let needle_len = self.boundary_len + 2;

                    let Some(position) = find(&self.buffer[..self.len], &needle[..needle_len])
                    else {
                        let keep = needle_len - 1;
                        if self.len > keep {
                            self.shift_left(self.len - keep);
                        }
                        return Ok(());
                    };

                    let after = position + needle_len;
                    if after + 2 > self.len {
                        if position > 0 {
                            self.shift_left(position);
                        }
                        return Ok(());
                    }

                    match &self.buffer[after..after + 2] {
                        b"--" => {
                            self.state = State::Epilogue;
                            self.shift_left(after + 2);
                        }
                        b"\r\n" => {
                            self.state = State::Headers;
                            self.shift_left(after + 2);
                        }
                        _ => return Err(ParseError::Malformed),
                    }
                }
                State::Headers => {
                    let Some(position) = find(&self.buffer[..self.len], b"\r\n\r\n") else {
                        if self.len > MAX_HEADERS {
                            return Err(ParseError::Malformed);
                        }
                        return Ok(());
                    };

                    if let Some((filename, filename_len)) =
                        filename_from_headers(&self.buffer[..position])
                    {
                        sink.begin_file(&filename[..filename_len])
                            .map_err(ParseError::Sink)?;
                        self.writing = true;
                        self.files += 1;
                    } else {
                        self.writing = false;
                    }

                    self.shift_left(position + 4);
                    self.state = State::Body;
                }
                State::Body => {
                    let delimiter = &self.delimiter[..self.delimiter_len];
                    let Some(position) = find(&self.buffer[..self.len], delimiter) else {
                        let keep = self.delimiter_len - 1;
                        if self.len > keep {
                            let emit = self.len - keep;
                            if self.writing {
                                sink.write(&self.buffer[..emit]).map_err(ParseError::Sink)?;
                            }
                            self.shift_left(emit);
                        }
                        return Ok(());
                    };

                    if position > 0 && self.writing {
                        sink.write(&self.buffer[..position])
                            .map_err(ParseError::Sink)?;
                    }

                    let after = position + self.delimiter_len;
                    if after + 2 > self.len {
                        if position > 0 {
                            self.shift_left(position);
                        }
                        return Ok(());
                    }

                    if self.writing {
                        sink.end_file().map_err(ParseError::Sink)?;
                        self.writing = false;
                    }

                    match &self.buffer[after..after + 2] {
                        b"--" => {
                            self.state = State::Epilogue;
                            self.shift_left(after + 2);
                        }
                        b"\r\n" => {
                            self.state = State::Headers;
                            self.shift_left(after + 2);
                        }
                        _ => return Err(ParseError::Malformed),
                    }
                }
                State::Epilogue => {
                    self.len = 0;
                    return Ok(());
                }
            }
        }
    }

    fn shift_left(&mut self, count: usize) {
        if count >= self.len {
            self.len = 0;
            return;
        }
        self.buffer.copy_within(count..self.len, 0);
        self.len -= count;
    }
}

fn parse_boundary(content_type: &[u8]) -> Result<([u8; MAX_BOUNDARY], usize), ParseError> {
    let media_end = content_type
        .iter()
        .position(|&byte| byte == b';')
        .unwrap_or(content_type.len());
    let media = trim(&content_type[..media_end]);
    if !media.eq_ignore_ascii_case(b"multipart/form-data") {
        return Err(ParseError::UnsupportedContentType);
    }

    let Some(value) = parameter(&content_type[media_end..], b"boundary") else {
        return Err(ParseError::UnsupportedContentType);
    };
    if value.is_empty()
        || value.len() > MAX_BOUNDARY
        || value
            .iter()
            .any(|&byte| byte < 0x20 || byte == 0x7f || byte == b'"' || byte == b';')
    {
        return Err(ParseError::UnsupportedContentType);
    }
    let mut boundary = [0u8; MAX_BOUNDARY];
    boundary[..value.len()].copy_from_slice(value);
    Ok((boundary, value.len()))
}

fn filename_from_headers(headers: &[u8]) -> Option<([u8; MAX_FILENAME], usize)> {
    for line in headers.split(|&byte| byte == b'\n') {
        let line = line.strip_suffix(b"\r").unwrap_or(line);
        let Some(colon) = line.iter().position(|&byte| byte == b':') else {
            continue;
        };
        let name = trim(&line[..colon]);
        if !name.eq_ignore_ascii_case(b"content-disposition") {
            continue;
        }
        let value = &line[colon + 1..];
        if let Some(raw) = filename_parameter(value) {
            let mut output = [0u8; MAX_FILENAME];
            let name = basename(raw);
            if name.is_empty()
                || name == b"."
                || name == b".."
                || name.len() > MAX_FILENAME
                || name
                    .iter()
                    .any(|&byte| byte < 0x20 || byte == 0x7f || byte == b'/' || byte == b'\\')
            {
                return None;
            }
            output[..name.len()].copy_from_slice(name);
            return Some((output, name.len()));
        }
    }
    None
}

fn filename_parameter(value: &[u8]) -> Option<&[u8]> {
    parameter(value, b"filename")
}

/// Find a `key=value` parameter, splitting on `;` only outside quotes.
fn parameter<'a>(value: &'a [u8], key: &[u8]) -> Option<&'a [u8]> {
    let mut index = 0usize;
    while index < value.len() {
        while index < value.len()
            && (value[index] == b';' || value[index] == b' ' || value[index] == b'\t')
        {
            index += 1;
        }
        let name_start = index;
        while index < value.len() && value[index] != b'=' && value[index] != b';' {
            index += 1;
        }
        if index == value.len() || value[index] == b';' {
            continue;
        }
        let name_end = index;
        index += 1;
        while index < value.len() && (value[index] == b' ' || value[index] == b'\t') {
            index += 1;
        }

        let value_start;
        let mut value_end = 0usize;
        if index < value.len() && value[index] == b'"' {
            index += 1;
            value_start = index;
            let mut closed = false;
            while index < value.len() {
                if value[index] == b'"' {
                    value_end = index;
                    index += 1;
                    closed = true;
                    break;
                }
                if value[index] == b'\\' && index + 1 < value.len() {
                    index += 2;
                } else {
                    index += 1;
                }
            }
            if !closed {
                return None;
            }
        } else {
            value_start = index;
            while index < value.len() && value[index] != b';' {
                index += 1;
            }
            value_end = index;
        }

        if value[name_start..name_end].eq_ignore_ascii_case(key) {
            return Some(trim(&value[value_start..value_end]));
        }
    }
    None
}

fn basename(mut name: &[u8]) -> &[u8] {
    while let Some(position) = name.iter().rposition(|&byte| byte == b'/' || byte == b'\\') {
        name = &name[position + 1..];
    }
    name
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

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

#[cfg(test)]
mod tests {
    use super::{basename, parse_boundary, Parser, Sink, SinkError, MAX_FILENAME};
    use std::vec::Vec;

    #[derive(Default)]
    struct Collector {
        files: Vec<Vec<u8>>,
        current: Vec<u8>,
        names: Vec<Vec<u8>>,
    }

    impl Sink for Collector {
        fn begin_file(&mut self, filename: &[u8]) -> Result<(), SinkError> {
            self.names.push(filename.to_vec());
            self.current.clear();
            Ok(())
        }

        fn write(&mut self, data: &[u8]) -> Result<(), SinkError> {
            self.current.extend_from_slice(data);
            Ok(())
        }

        fn end_file(&mut self) -> Result<(), SinkError> {
            self.files.push(core::mem::take(&mut self.current));
            Ok(())
        }
    }

    fn body(boundary: &[u8], filename: &[u8], content: &[u8]) -> Vec<u8> {
        let mut body = Vec::new();
        body.extend_from_slice(b"--");
        body.extend_from_slice(boundary);
        body.extend_from_slice(b"\r\nContent-Disposition: form-data; name=\"file\"; filename=\"");
        body.extend_from_slice(filename);
        body.extend_from_slice(b"\"\r\nContent-Type: text/plain\r\n\r\n");
        body.extend_from_slice(content);
        body.extend_from_slice(b"\r\n--");
        body.extend_from_slice(boundary);
        body.extend_from_slice(b"--\r\n");
        body
    }

    #[test]
    fn boundary_parsing() {
        let (boundary, len) = parse_boundary(b"multipart/form-data; boundary=abc123").unwrap();
        assert_eq!(&boundary[..len], b"abc123");
        let (boundary, len) =
            parse_boundary(b"Multipart/Form-Data; charset=utf-8; boundary=\"a b\"").unwrap();
        assert_eq!(&boundary[..len], b"a b");
        assert!(parse_boundary(b"application/json").is_err());
        assert!(parse_boundary(b"multipart/form-data; boundary=").is_err());
    }

    #[test]
    fn basename_handles_old_browser_paths() {
        assert_eq!(basename(b"C:\\fakepath\\a.txt"), b"a.txt");
        assert_eq!(basename(b"/tmp/a.txt"), b"a.txt");
    }

    #[test]
    fn parses_one_file_in_one_chunk() {
        let boundary = b"----sysweb";
        let payload = body(boundary, b"old.txt", b"hello form");
        let mut parser = Parser::new(b"multipart/form-data; boundary=----sysweb").unwrap();
        let mut sink = Collector::default();
        parser.feed(&payload, &mut sink).unwrap();
        parser.finish(&mut sink).unwrap();
        assert_eq!(sink.names, vec![b"old.txt".to_vec()]);
        assert_eq!(sink.files, vec![b"hello form".to_vec()]);
    }

    #[test]
    fn parses_across_arbitrary_chunks() {
        let boundary = b"----sysweb";
        let content: Vec<u8> = (0..100_000).map(|i| (i % 251) as u8).collect();
        let payload = body(boundary, b"C:\\fakepath\\big.bin", &content);
        let mut parser = Parser::new(b"multipart/form-data; boundary=----sysweb").unwrap();
        let mut sink = Collector::default();
        for chunk in payload.chunks(17) {
            parser.feed(chunk, &mut sink).unwrap();
        }
        parser.finish(&mut sink).unwrap();
        assert_eq!(sink.names, vec![b"big.bin".to_vec()]);
        assert_eq!(sink.files, vec![content]);
    }

    #[test]
    fn ignores_non_file_parts_and_keeps_binary_content() {
        let mut payload = Vec::new();
        payload.extend_from_slice(
            b"--b\r\nContent-Disposition: form-data; name=\"note\"\r\n\r\nhello\r\n",
        );
        payload.extend_from_slice(
            b"--b\r\nContent-Disposition: form-data; name=\"f\"; filename=\"x.bin\"\r\n\r\n",
        );
        payload.extend_from_slice(&[0u8, 1, 2, b'\r', b'\n']);
        payload.extend_from_slice(b"\r\n--b--\r\n");

        let mut parser = Parser::new(b"multipart/form-data; boundary=b").unwrap();
        let mut sink = Collector::default();
        parser.feed(&payload, &mut sink).unwrap();
        parser.finish(&mut sink).unwrap();
        assert_eq!(sink.files, vec![vec![0, 1, 2, b'\r', b'\n']]);
    }

    #[test]
    fn rejects_truncated_and_missing_boundary() {
        let mut parser = Parser::new(b"multipart/form-data; boundary=abc").unwrap();
        let mut sink = Collector::default();
        parser
            .feed(
                b"--abc\r\nContent-Disposition: form-data; name=\"f\"; filename=\"x\"\r\n\r\nhello",
                &mut sink,
            )
            .unwrap();
        assert!(parser.finish(&mut sink).is_err());
    }

    #[test]
    fn semicolons_inside_quoted_parameters() {
        let (boundary, len) = parse_boundary(b"multipart/form-data; boundary=\"a b\"").unwrap();
        assert_eq!(&boundary[..len], b"a b");

        let payload = body(b"a b", b"a;b.txt", b"quoted semicolon");
        let mut parser = Parser::new(b"multipart/form-data; boundary=\"a b\"").unwrap();
        let mut sink = Collector::default();
        parser.feed(&payload, &mut sink).unwrap();
        parser.finish(&mut sink).unwrap();
        assert_eq!(sink.names, vec![b"a;b.txt".to_vec()]);
        assert_eq!(sink.files, vec![b"quoted semicolon".to_vec()]);
    }

    #[test]
    fn filename_is_bounded() {
        let mut long = [b'a'; MAX_FILENAME + 1];
        long[0] = b'a';
        let payload = body(b"b", &long, b"x");
        let mut parser = Parser::new(b"multipart/form-data; boundary=b").unwrap();
        let mut sink = Collector::default();
        parser.feed(&payload, &mut sink).unwrap();
        assert!(parser.finish(&mut sink).is_err());
    }
}
