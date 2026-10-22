#![no_main]

use libfuzzer_sys::fuzz_target;
use sysweb::multipart::{Parser, Sink, SinkError};

struct NullSink;

impl Sink for NullSink {
    fn begin_file(&mut self, _filename: &[u8]) -> Result<(), SinkError> {
        Ok(())
    }

    fn write(&mut self, _data: &[u8]) -> Result<(), SinkError> {
        Ok(())
    }

    fn end_file(&mut self) -> Result<(), SinkError> {
        Ok(())
    }
}

fuzz_target!(|data: &[u8]| {
    let Ok(mut parser) =
        Parser::new(b"multipart/form-data; boundary=fuzz", 1 << 20)
    else {
        return;
    };
    let mut sink = NullSink;
    for chunk in data.chunks(13) {
        if parser.feed(chunk, &mut sink).is_err() {
            return;
        }
    }
    let _ = parser.finish(&mut sink);
});
