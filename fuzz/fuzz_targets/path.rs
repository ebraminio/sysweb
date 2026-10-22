#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let mut buffer = [0u8; sysweb::path::MAX_URL_PATH];
    if let Ok(len) = sysweb::path::normalize(data, &mut buffer) {
        let path = &buffer[..len];
        assert_eq!(path.first(), Some(&b'/'), "normalized path lost its root");
        assert!(
            !path.split(|&byte| byte == b'/').any(|segment| segment == b".."),
            "normalized path retained a parent segment"
        );
        if path.ends_with(b"/") {
            let mut index = [0u8; sysweb::path::MAX_PATH];
            let _ = sysweb::path::append_index(path, &mut index);
        }
    }
});
