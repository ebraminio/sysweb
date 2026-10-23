//! End-to-end tests: spawn the real freestanding binary and speak HTTP to it.

use std::fs;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

static NEXT_SERVER: AtomicUsize = AtomicUsize::new(0);

struct TestServer {
    child: Child,
    port: u16,
    root: PathBuf,
    outside: PathBuf,
}

impl Drop for TestServer {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = fs::remove_dir_all(&self.root);
        let _ = fs::remove_dir_all(&self.outside);
    }
}

fn free_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind ephemeral port");
    listener.local_addr().expect("local_addr").port()
}

fn write_file(root: &Path, name: &str, contents: &[u8]) {
    let path = root.join(name);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("create parent");
    }
    fs::write(path, contents).expect("write fixture");
}

fn create_fixtures(root: &Path, outside: &Path) {
    write_file(root, "index.html", b"<h1>index</h1>");
    write_file(root, "hello world.txt", b"hello, space");
    write_file(root, "app.js", b"console.log('hi')");
    write_file(root, "sub/index.html", b"sub index");
    write_file(root, "blob.bin", &[0u8, 1, 2, 3, 0xff, 0xfe, 0, 42]);
    let large: Vec<u8> = (0..40_000u32).map(|i| (i % 251) as u8).collect();
    write_file(root, "large.bin", &large);
    let huge: Vec<u8> = (0..8_000_000u32).map(|i| (i % 251) as u8).collect();
    write_file(root, "huge.bin", &huge);

    write_file(root, "assets/hello world.txt", b"hello list");
    write_file(root, "assets/app.js", b"console.log(1)");
    write_file(root, "assets/a&b<c>\".txt", b"escaped");
    write_file(root, "assets/empty.txt", b"");
    write_file(root, "assets/replace.txt", b"old");
    fs::create_dir_all(root.join("assets/nested")).expect("nested dir");
    std::os::unix::fs::symlink("/etc/passwd", root.join("assets/leak")).expect("symlink");
    std::os::unix::fs::symlink(outside, root.join("assets/linkdir")).expect("dir symlink");
}

fn start_server(extra: &[&str]) -> TestServer {
    let id = NEXT_SERVER.fetch_add(1, Ordering::Relaxed);
    let root = std::env::temp_dir().join(format!("sysweb-e2e-{}-{id}", std::process::id()));
    let outside = std::env::temp_dir().join(format!("sysweb-outside-{}-{id}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    let _ = fs::remove_dir_all(&outside);
    fs::create_dir_all(&root).expect("create root");
    fs::create_dir_all(&outside).expect("create outside");
    write_file(&outside, "secret.txt", b"outside secret");
    create_fixtures(&root, &outside);

    let port = free_port();
    let mut command = Command::new(env!("CARGO_BIN_EXE_syswebd"));
    command
        .arg("--root")
        .arg(&root)
        .arg("--port")
        .arg(port.to_string())
        .arg("--quiet");
    for arg in extra {
        command.arg(arg);
    }
    let child = command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn syswebd");

    let mut server = TestServer {
        child,
        port,
        root,
        outside,
    };

    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Ok(stream) = TcpStream::connect(("127.0.0.1", port)) {
            drop(stream);
            return server;
        }
        if let Some(status) = server.child.try_wait().expect("try_wait") {
            panic!("syswebd exited during startup: {status}");
        }
        if Instant::now() > deadline {
            panic!("syswebd did not start listening on port {port}");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn request(port: u16, raw: &[u8]) -> Vec<u8> {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect");
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .expect("read timeout");
    stream.write_all(raw).expect("write request");
    let _ = stream.shutdown(std::net::Shutdown::Write);
    let mut response = Vec::new();
    stream.read_to_end(&mut response).expect("read response");
    response
}

fn get(port: u16, target: &str) -> Vec<u8> {
    let raw = format!("GET {target} HTTP/1.1\r\nHost: localhost\r\n\r\n");
    request(port, raw.as_bytes())
}

fn request_split(port: u16, head: &str, body: &[u8]) -> Vec<u8> {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect");
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .expect("read timeout");
    stream.write_all(head.as_bytes()).expect("headers");
    std::thread::sleep(Duration::from_millis(50));
    stream.write_all(body).expect("body");
    let _ = stream.shutdown(std::net::Shutdown::Write);
    let mut response = Vec::new();
    stream.read_to_end(&mut response).expect("read response");
    response
}

fn get_range(port: u16, target: &str, range: &str) -> Vec<u8> {
    let raw = format!("GET {target} HTTP/1.1\r\nHost: localhost\r\nRange: {range}\r\n\r\n");
    request(port, raw.as_bytes())
}

fn head_range(port: u16, target: &str, range: &str) -> Vec<u8> {
    let raw = format!("HEAD {target} HTTP/1.1\r\nHost: localhost\r\nRange: {range}\r\n\r\n");
    request(port, raw.as_bytes())
}

fn put(port: u16, target: &str, body: &[u8]) -> Vec<u8> {
    let mut raw = format!(
        "PUT {target} HTTP/1.1\r\nHost: localhost\r\nContent-Length: {}\r\n\r\n",
        body.len()
    )
    .into_bytes();
    raw.extend_from_slice(body);
    request(port, &raw)
}

fn multipart_body(boundary: &str, filename: &str, content: &[u8]) -> Vec<u8> {
    let mut body = Vec::new();
    body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
    body.extend_from_slice(
        format!("Content-Disposition: form-data; name=\"file\"; filename=\"{filename}\"\r\n")
            .as_bytes(),
    );
    body.extend_from_slice(b"Content-Type: application/octet-stream\r\n\r\n");
    body.extend_from_slice(content);
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    body
}

fn post_multipart(port: u16, target: &str, boundary: &str, body: &[u8]) -> Vec<u8> {
    let mut raw = format!(
        "POST {target} HTTP/1.1\r\nHost: localhost\r\n         Content-Type: multipart/form-data; boundary={boundary}\r\n         Content-Length: {}\r\n\r\n",
        body.len()
    )
    .into_bytes();
    raw.extend_from_slice(body);
    request(port, &raw)
}

fn split(response: &[u8]) -> (String, &[u8]) {
    let at = response
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .expect("response has header terminator");
    (
        String::from_utf8_lossy(&response[..at]).into_owned(),
        &response[at + 4..],
    )
}

fn header<'a>(head: &'a str, name: &str) -> Option<&'a str> {
    head.split("\r\n").find_map(|line| {
        let (key, value) = line.split_once(':')?;
        key.eq_ignore_ascii_case(name).then(|| value.trim())
    })
}

fn read_one_response(stream: &mut TcpStream) -> Vec<u8> {
    let mut response = Vec::new();
    let mut byte = [0u8; 1];
    while !response.ends_with(b"\r\n\r\n") {
        stream.read_exact(&mut byte).expect("response header");
        response.push(byte[0]);
    }
    let head = String::from_utf8_lossy(&response).into_owned();
    let length = header(&head, "Content-Length")
        .expect("content length")
        .parse::<usize>()
        .expect("content length number");
    let start = response.len();
    response.resize(start + length, 0);
    stream
        .read_exact(&mut response[start..])
        .expect("response body");
    response
}

fn assert_status(head: &str, expected: &str) {
    assert!(
        head.starts_with(expected),
        "expected status {expected:?}, response was:\n{head}"
    );
}

fn text(response: &[u8]) -> String {
    String::from_utf8_lossy(response).into_owned()
}

#[test]
fn serves_static_files_and_directory_indexes() {
    let server = start_server(&[]);
    let port = server.port;

    let response = get(port, "/");
    let (head, body) = split(&response);
    assert_status(&head, "HTTP/1.1 200 OK");
    assert_eq!(
        header(&head, "Content-Type"),
        Some("text/html; charset=utf-8")
    );
    assert_eq!(body, b"<h1>index</h1>");

    let response = request(port, b"HEAD / HTTP/1.1\r\nHost: localhost\r\n\r\n");
    let (head, body) = split(&response);
    assert_status(&head, "HTTP/1.1 200 OK");
    assert_eq!(header(&head, "Content-Length"), Some("14"));
    assert!(body.is_empty());

    let response = get(port, "/hello%20world.txt");
    let (head, body) = split(&response);
    assert_status(&head, "HTTP/1.1 200 OK");
    assert_eq!(
        header(&head, "Content-Type"),
        Some("text/plain; charset=utf-8")
    );
    assert_eq!(body, b"hello, space");

    let response = get(port, "/app.js");
    let (head, body) = split(&response);
    assert_status(&head, "HTTP/1.1 200 OK");
    assert_eq!(
        header(&head, "Content-Type"),
        Some("text/javascript; charset=utf-8")
    );
    assert_eq!(body, b"console.log('hi')");

    let response = get(port, "/sub/");
    let (head, body) = split(&response);
    assert_status(&head, "HTTP/1.1 200 OK");
    assert_eq!(body, b"sub index");

    let response = get(port, "/sub");
    let (head, _) = split(&response);
    assert_status(&head, "HTTP/1.1 301 Moved Permanently");
    assert_eq!(header(&head, "Location"), Some("/sub/"));

    let response = get(port, "/blob.bin");
    let (head, body) = split(&response);
    assert_status(&head, "HTTP/1.1 200 OK");
    assert_eq!(
        header(&head, "Content-Type"),
        Some("application/octet-stream")
    );
    assert_eq!(body, &[0u8, 1, 2, 3, 0xff, 0xfe, 0, 42]);

    let response = get(port, "/large.bin");
    let (head, body) = split(&response);
    assert_status(&head, "HTTP/1.1 200 OK");
    assert_eq!(header(&head, "Content-Length"), Some("40000"));
    assert_eq!(header(&head, "Accept-Ranges"), Some("bytes"));
    assert_eq!(body.len(), 40_000);
    assert_eq!(body[0], 0);
    assert_eq!(body[39_999], (39_999u32 % 251) as u8);

    let response = get_range(port, "/large.bin", "bytes=10-19");
    let (head, body) = split(&response);
    assert_status(&head, "HTTP/1.1 206 Partial Content");
    assert_eq!(header(&head, "Content-Range"), Some("bytes 10-19/40000"));
    assert_eq!(header(&head, "Content-Length"), Some("10"));
    assert_eq!(header(&head, "Accept-Ranges"), Some("bytes"));
    assert_eq!(body.len(), 10);
    for (offset, byte) in body.iter().enumerate() {
        assert_eq!(*byte, ((10 + offset) as u32 % 251) as u8);
    }

    let response = get_range(port, "/large.bin", "bytes=39990-");
    let (head, body) = split(&response);
    assert_status(&head, "HTTP/1.1 206 Partial Content");
    assert_eq!(
        header(&head, "Content-Range"),
        Some("bytes 39990-39999/40000")
    );
    assert_eq!(body.len(), 10);

    let response = get_range(port, "/large.bin", "bytes=-10");
    let (head, body) = split(&response);
    assert_status(&head, "HTTP/1.1 206 Partial Content");
    assert_eq!(
        header(&head, "Content-Range"),
        Some("bytes 39990-39999/40000")
    );
    assert_eq!(body.len(), 10);

    let response = get_range(port, "/large.bin", "bytes=0-0");
    let (head, body) = split(&response);
    assert_status(&head, "HTTP/1.1 206 Partial Content");
    assert_eq!(header(&head, "Content-Range"), Some("bytes 0-0/40000"));
    assert_eq!(body.len(), 1);

    let response = get_range(port, "/large.bin", "bytes=40000-");
    let (head, body) = split(&response);
    assert_status(&head, "HTTP/1.1 416 Range Not Satisfiable");
    assert_eq!(header(&head, "Content-Range"), Some("bytes */40000"));
    assert_eq!(body, b"416 Range Not Satisfiable\n");

    // A suffix longer than the file covers the whole file.
    let response = get_range(port, "/large.bin", "bytes=-50000");
    let (head, body) = split(&response);
    assert_status(&head, "HTTP/1.1 206 Partial Content");
    assert_eq!(header(&head, "Content-Range"), Some("bytes 0-39999/40000"));
    assert_eq!(body.len(), 40_000);

    // Empty files cannot satisfy any byte range.
    let response = get_range(port, "/assets/empty.txt", "bytes=0-");
    let (head, body) = split(&response);
    assert_status(&head, "HTTP/1.1 416 Range Not Satisfiable");
    assert_eq!(header(&head, "Content-Range"), Some("bytes */0"));
    assert_eq!(body, b"416 Range Not Satisfiable\n");

    let response = head_range(port, "/large.bin", "bytes=10-19");
    let (head, body) = split(&response);
    assert_status(&head, "HTTP/1.1 206 Partial Content");
    assert_eq!(header(&head, "Content-Range"), Some("bytes 10-19/40000"));
    assert_eq!(header(&head, "Content-Length"), Some("10"));
    assert_eq!(body, b"");

    // Unsupported multi-ranges are ignored rather than half-implemented.
    let response = get_range(port, "/large.bin", "bytes=0-1,3-4");
    let (head, body) = split(&response);
    assert_status(&head, "HTTP/1.1 200 OK");
    assert_eq!(body.len(), 40_000);

    let response = get(port, "/huge.bin");
    let (head, body) = split(&response);
    assert_status(&head, "HTTP/1.1 200 OK");
    assert_eq!(header(&head, "Content-Length"), Some("8000000"));
    assert_eq!(body.len(), 8_000_000);
    assert_eq!(body[0], 0);
    assert_eq!(body[7_999_999], (7_999_999u32 % 251) as u8);

    let response = get(port, "/missing");
    let (head, body) = split(&response);
    assert_status(&head, "HTTP/1.1 404 Not Found");
    assert_eq!(body, b"404 Not Found\n");

    let response = get(port, "/../Cargo.toml");
    let (head, _) = split(&response);
    assert_status(&head, "HTTP/1.1 400 Bad Request");

    let response = get(port, "/%2e%2e/Cargo.toml");
    let (head, _) = split(&response);
    assert_status(&head, "HTTP/1.1 400 Bad Request");

    let response = request(
        port,
        b"POST / HTTP/1.1\r\nHost: localhost\r\nContent-Length: 0\r\n\r\n",
    );
    let (head, _) = split(&response);
    assert_status(&head, "HTTP/1.1 405 Method Not Allowed");
    assert_eq!(header(&head, "Allow"), Some("GET, HEAD"));

    let response = request(port, b"GET / HTTP/2.0\r\n\r\n");
    let (head, _) = split(&response);
    assert_status(&head, "HTTP/1.1 505 HTTP Version Not Supported");

    let response = request(port, b"GET\r\n\r\n");
    let (head, _) = split(&response);
    assert_status(&head, "HTTP/1.1 400 Bad Request");

    // A directory without an index.html gets the built-in listing.
    let response = get(port, "/assets/");
    let (head, body) = split(&response);
    assert_status(&head, "HTTP/1.1 200 OK");
    assert_eq!(
        header(&head, "Content-Type"),
        Some("text/html; charset=utf-8")
    );
    let page = text(body);
    assert!(page.contains("data-path=\"/assets/\""));
    assert!(page.contains("data-upload=\"0\""));
    assert!(page.contains("hello world.txt"));
    assert!(page.contains("text/javascript"));
    assert!(page.contains("a&amp;b&lt;c&gt;&quot;.txt"));
    assert!(!page.contains("a&b<c>\""));
    assert!(page.contains("href=\"/assets/a%26b%3Cc%3E%22.txt\""));

    // A symlink is listed but not followed.
    let response = get(port, "/assets/leak");
    let (head, _) = split(&response);
    assert_status(&head, "HTTP/1.1 403 Forbidden");
    assert!(text(&get(port, "/assets/")).contains("symlinks are not served"));

    // A symlinked directory must not be traversed, even for a real file
    // behind it.
    let response = get(port, "/assets/linkdir/secret.txt");
    let (head, _) = split(&response);
    assert_status(&head, "HTTP/1.1 403 Forbidden");

    let response = get(port, "/assets/nested/");
    let (head, body) = split(&response);
    assert_status(&head, "HTTP/1.1 200 OK");
    assert!(text(body).contains("This directory is empty."));

    let response = request(
        port,
        b"HEAD /assets/empty.txt HTTP/1.1\r\nHost: localhost\r\n\r\n",
    );
    let (head, body) = split(&response);
    assert_status(&head, "HTTP/1.1 200 OK");
    assert_eq!(header(&head, "Content-Length"), Some("0"));
    assert!(body.is_empty());
}

#[test]
fn no_index_is_403_but_static_index_is_still_served() {
    let server = start_server(&["--no-index"]);
    let port = server.port;

    let response = get(port, "/");
    assert_status(&split(&response).0, "HTTP/1.1 200 OK");

    let response = get(port, "/assets/");
    let (head, _) = split(&response);
    assert_status(&head, "HTTP/1.1 403 Forbidden");

    let response = get(port, "/assets");
    assert_status(&split(&response).0, "HTTP/1.1 301 Moved Permanently");

    // Uploads are off unless explicitly requested.
    let response = put(port, "/assets/new.txt", b"nope");
    let (head, _) = split(&response);
    assert_status(&head, "HTTP/1.1 405 Method Not Allowed");
    assert_eq!(header(&head, "Allow"), Some("GET, HEAD"));

    let body = multipart_body("noindex", "x.txt", b"x");
    let response = post_multipart(port, "/assets/", "noindex", &body);
    let (head, _) = split(&response);
    assert_status(&head, "HTTP/1.1 405 Method Not Allowed");
    assert_eq!(header(&head, "Allow"), Some("GET, HEAD"));

    // Regression: send the body after the headers so the server must drain it
    // before closing. Closing with unread input can reset the connection on
    // macOS and discard the 405 response.
    let head = "PUT /assets/new.txt HTTP/1.1\r\nHost: localhost\r\nContent-Length: 4\r\n\r\n";
    let response = request_split(port, head, b"nope");
    assert_status(&split(&response).0, "HTTP/1.1 405 Method Not Allowed");

    let body = multipart_body("splitnoindex", "x.txt", b"x");
    let head = format!(
        "POST /assets/ HTTP/1.1\r\nHost: localhost\r\nContent-Type: multipart/form-data; boundary=splitnoindex\r\nContent-Length: {}\r\n\r\n",
        body.len()
    );
    let response = request_split(port, &head, &body);
    assert_status(&split(&response).0, "HTTP/1.1 405 Method Not Allowed");
}

#[test]
fn uploads_create_files_and_directories() {
    let server = start_server(&["--upload"]);
    let port = server.port;

    let response = put(port, "/assets/up.txt", b"hello upload");
    let (head, body) = split(&response);
    assert_status(&head, "HTTP/1.1 201 Created");
    assert_eq!(body, b"UPLOADED\n");

    let response = get(port, "/assets/up.txt");
    let (head, body) = split(&response);
    assert_status(&head, "HTTP/1.1 200 OK");
    assert_eq!(body, b"hello upload");

    // Existing files are not replaced without --replace.
    let response = put(port, "/assets/up.txt", b"second");
    assert_status(&split(&response).0, "HTTP/1.1 409 Conflict");

    let response = put(port, "/assets/brand-new/", b"");
    assert_status(&split(&response).0, "HTTP/1.1 201 Created");
    let response = get(port, "/assets/brand-new/");
    let (head, body) = split(&response);
    assert_status(&head, "HTTP/1.1 200 OK");
    assert!(text(body).contains("This directory is empty."));

    // Nested paths used by the folder uploader.
    let response = put(port, "/assets/tree/", b"");
    assert_status(&split(&response).0, "HTTP/1.1 201 Created");
    let response = put(port, "/assets/tree/sub/", b"");
    assert_status(&split(&response).0, "HTTP/1.1 201 Created");
    let response = put(port, "/assets/tree/sub/file.txt", b"nested");
    assert_status(&split(&response).0, "HTTP/1.1 201 Created");
    let response = get(port, "/assets/tree/sub/file.txt");
    let (head, body) = split(&response);
    assert_status(&head, "HTTP/1.1 200 OK");
    assert_eq!(body, b"nested");
    assert!(text(&get(port, "/assets/")).contains("data-upload=\"1\""));

    let response = put(port, "/assets/nested/", b"");
    assert_status(&split(&response).0, "HTTP/1.1 409 Conflict");

    let response = put(port, "/assets/no-parent/file.txt", b"x");
    assert_status(&split(&response).0, "HTTP/1.1 404 Not Found");

    // Writes through a symlinked directory are refused as well.
    let response = put(port, "/assets/linkdir/upload.txt", b"x");
    assert_status(&split(&response).0, "HTTP/1.1 403 Forbidden");

    let response = put(port, "/", b"x");
    assert_status(&split(&response).0, "HTTP/1.1 400 Bad Request");

    let response = put(port, "/../escape.txt", b"x");
    assert_status(&split(&response).0, "HTTP/1.1 400 Bad Request");

    // Missing Content-Length is 411, not a guessed body.
    let response = request(
        port,
        b"PUT /assets/no-length.txt HTTP/1.1\r\nHost: x\r\n\r\n",
    );
    assert_status(&split(&response).0, "HTTP/1.1 411 Length Required");

    // Binary bodies round-trip exactly.
    let binary: Vec<u8> = (0..=255u8).collect();
    let response = put(port, "/assets/binary.dat", &binary);
    assert_status(&split(&response).0, "HTTP/1.1 201 Created");
    let response = get(port, "/assets/binary.dat");
    let (_, body) = split(&response);
    assert_eq!(body, binary);

    // Expect: 100-continue: the client waits for the interim response.
    let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect");
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .expect("timeout");
    stream
        .write_all(
            b"PUT /assets/expect.txt HTTP/1.1\r\nHost: x\r\nContent-Length: 5\r\nExpect: 100-continue\r\nConnection: close\r\n\r\n",
        )
        .expect("headers");
    let mut interim = [0u8; 64];
    let count = stream.read(&mut interim).expect("interim");
    assert!(
        interim[..count].starts_with(b"HTTP/1.1 100 Continue"),
        "unexpected interim response: {:?}",
        &interim[..count]
    );
    stream.write_all(b"hello").expect("body");
    let mut final_response = Vec::new();
    stream
        .read_to_end(&mut final_response)
        .expect("final response");
    assert_status(&split(&final_response).0, "HTTP/1.1 201 Created");

    let response = get(port, "/assets/expect.txt");
    let (head, body) = split(&response);
    assert_status(&head, "HTTP/1.1 200 OK");
    assert_eq!(body, b"hello");

    // Early EOF cleans up the partial file (and exercises unlinkat).
    let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect");
    stream
        .write_all(b"PUT /assets/partial.txt HTTP/1.1\r\nHost: x\r\nContent-Length: 10\r\n\r\nabc")
        .expect("partial body");
    drop(stream);
    std::thread::sleep(Duration::from_millis(100));
    let response = get(port, "/assets/partial.txt");
    assert_status(&split(&response).0, "HTTP/1.1 404 Not Found");
}

#[test]
fn multipart_forms_work_for_old_browsers() {
    let server = start_server(&["--upload"]);
    let port = server.port;

    // The page contains a classic multipart form and the inline JS stays in
    // the ES3/feature-detected subset so old browsers do not fail parsing.
    let page = text(&get(port, "/assets/"));
    assert!(page.contains("enctype=\"multipart/form-data\""));
    assert!(page.contains("id=\"classic-upload\""));
    assert!(!page.contains("Promise"));
    assert!(!page.contains("querySelectorAll"));
    assert!(!page.contains("dataset"));
    assert!(!page.contains("classList"));
    assert!(!page.contains("fetch("));
    assert!(!page.contains("=>"));
    // Legacy engines get a hard-coded light theme; modern rules use one
    // dark media query at the end of the stylesheet.
    assert!(page.contains("background: #f5f6f8"));
    assert!(page.contains("background: #101216"));
    assert!(page.contains("prefers-color-scheme: dark"));
    assert!(!page.contains("var(--"));
    assert!(page.contains("webkitdirectory"));
    assert!(page.contains("webkitRelativePath"));
    assert!(page.contains("clipboardData"));
    assert!(page.contains("id=\"paste-text\""));
    assert!(page.contains("id=\"text-dialog\""));
    assert!(page.contains("id=\"text-body\""));
    assert!(page.contains("action=\"?sysweb-form=1\""));
    assert!(page.contains("id=\"classic-text\""));
    assert!(page.contains("action=\"?sysweb-text=1\""));
    assert!(!page.contains("display: flex"));
    assert!(!page.contains("inline-flex"));
    assert!(!page.contains("rem;"));
    assert!(page.contains("webkitGetAsEntry"));
    assert!(page.contains("name.split('.')[0] === 'image'"));
    assert!(page.contains("Drop files or folders here, or paste files/images/text"));
    assert!(page.contains("System.Net.WebClient"));
    assert!(page.contains("<span class=\"time\" data-ts="));
    assert!(!page.contains("<time "));
    assert!(page.contains("id=\"transfer-wrap\""));
    assert!(page.contains("id=\"transfer\""));
    assert!(page.contains("#transfer-wrap{display:none}"));
    assert!(!page.contains("' in '"));
    assert!(!page.contains("'webkitdirectory' in"));

    // Classic form post to a directory: filename comes from the part.
    let body = multipart_body("----syswebForm", "old.txt", b"hello from a form");
    let response = post_multipart(port, "/assets/", "----syswebForm", &body);
    let (head, body) = split(&response);
    assert_status(&head, "HTTP/1.1 201 Created");
    assert_eq!(body, b"UPLOADED\n");
    let response = get(port, "/assets/old.txt");
    let (head, body) = split(&response);
    assert_status(&head, "HTTP/1.1 200 OK");
    assert_eq!(body, b"hello from a form");

    // Old IE sends a full Windows path in filename=; only the basename is used.
    let body = multipart_body(
        "----syswebIE",
        "C:\\fakepath\\ie.txt",
        b"from an old browser",
    );
    let response = post_multipart(port, "/assets/", "----syswebIE", &body);
    assert_status(&split(&response).0, "HTTP/1.1 201 Created");
    let response = get(port, "/assets/ie.txt");
    let (head, body) = split(&response);
    assert_status(&head, "HTTP/1.1 200 OK");
    assert_eq!(body, b"from an old browser");

    // A large part crosses many 16 KiB reads and parser buffer refills.
    let large: Vec<u8> = (0..100_000u32).map(|i| (i % 251) as u8).collect();
    let body = multipart_body("----syswebLarge", "large-form.bin", &large);
    let response = post_multipart(port, "/assets/", "----syswebLarge", &body);
    assert_status(&split(&response).0, "HTTP/1.1 201 Created");
    let response = get(port, "/assets/large-form.bin");
    let (head, body) = split(&response);
    assert_status(&head, "HTTP/1.1 200 OK");
    assert_eq!(body, large);

    // A form upload can also use Expect: 100-continue.
    let body = multipart_body("----expectform", "expect-form.txt", b"waited");
    let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect");
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .expect("timeout");
    let headers = format!(
        "POST /assets/ HTTP/1.1\r\nHost: x\r\nContent-Type: multipart/form-data; boundary=----expectform\r\nContent-Length: {}\r\nExpect: 100-continue\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(headers.as_bytes()).expect("headers");
    let mut interim = [0u8; 64];
    let count = stream.read(&mut interim).expect("interim");
    assert!(interim[..count].starts_with(b"HTTP/1.1 100 Continue"));
    stream.write_all(&body).expect("body");
    let mut final_response = Vec::new();
    stream
        .read_to_end(&mut final_response)
        .expect("final response");
    assert_status(&split(&final_response).0, "HTTP/1.1 201 Created");
    assert_eq!(&split(&get(port, "/assets/expect-form.txt")).1, b"waited");

    // When the target is a file path, the URL wins over the part filename.
    let body = multipart_body("----fixed", "ignored.txt", b"fixed target");
    let response = post_multipart(port, "/assets/fixed.txt", "----fixed", &body);
    assert_status(&split(&response).0, "HTTP/1.1 201 Created");
    assert_eq!(&split(&get(port, "/assets/fixed.txt")).1, b"fixed target");

    // A truncated multipart body cleans up the partial file.
    let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect");
    let prefix = b"--partial\r\nContent-Disposition: form-data; name=\"file\"; filename=\"partial-form.txt\"\r\n\r\nabc";
    let headers = "POST /assets/ HTTP/1.1\r\nHost: x\r\nContent-Type: multipart/form-data; boundary=partial\r\nContent-Length: 1000\r\n\r\n";
    stream.write_all(headers.as_bytes()).expect("headers");
    stream.write_all(prefix).expect("partial body");
    drop(stream);
    std::thread::sleep(Duration::from_millis(100));
    let response = get(port, "/assets/partial-form.txt");
    assert_status(&split(&response).0, "HTTP/1.1 404 Not Found");

    // Wrong media type and missing file part are rejected.
    let response = request(
        port,
        b"POST /assets/x HTTP/1.1\r\nHost: x\r\nContent-Length: 0\r\nContent-Type: text/plain\r\n\r\n",
    );
    assert_status(&split(&response).0, "HTTP/1.1 415 Unsupported Media Type");

    let body =
        b"--nofile\r\nContent-Disposition: form-data; name=\"note\"\r\n\r\nhello\r\n--nofile--\r\n"
            .to_vec();
    let response = post_multipart(port, "/assets/", "nofile", &body);
    assert_status(&split(&response).0, "HTTP/1.1 400 Bad Request");
}

#[test]
fn replace_mode_overwrites() {
    let server = start_server(&["--upload", "--replace"]);
    let port = server.port;

    let response = put(port, "/assets/replace.txt", b"new contents");
    assert_status(&split(&response).0, "HTTP/1.1 201 Created");
    let response = get(port, "/assets/replace.txt");
    let (_, body) = split(&response);
    assert_eq!(body, b"new contents");
}

#[test]
fn async_get_and_liveness() {
    let server = start_server(&["--async", "--upload"]);

    let response = get(server.port, "/");
    let (head, body) = split(&response);
    assert_status(&head, "HTTP/1.1 200 OK");
    assert_eq!(body, b"<h1>index</h1>");

    let response = get(server.port, "/hello%20world.txt");
    let (head, body) = split(&response);
    assert_status(&head, "HTTP/1.1 200 OK");
    assert_eq!(body, b"hello, space");

    let response = get_range(server.port, "/hello%20world.txt", "bytes=1-4");
    let (head, body) = split(&response);
    assert_status(&head, "HTTP/1.1 206 Partial Content");
    assert_eq!(body, b"ello");

    let response = head_range(server.port, "/hello%20world.txt", "bytes=0-3");
    let (head, body) = split(&response);
    assert_status(&head, "HTTP/1.1 206 Partial Content");
    assert!(body.is_empty());

    let response = get(server.port, "/missing");
    assert_status(&split(&response).0, "HTTP/1.1 404 Not Found");

    let payload: Vec<u8> = (0..40_000u32).map(|i| (i % 251) as u8).collect();
    let response = put(server.port, "/async-put.bin", &payload);
    assert_status(&split(&response).0, "HTTP/1.1 201 Created");
    assert_eq!(
        fs::read(server.root.join("async-put.bin")).unwrap(),
        payload
    );

    let body = multipart_body("----asyncForm", "async-post.txt", b"multipart async");
    let response = post_multipart(server.port, "/assets/", "----asyncForm", &body);
    assert_status(&split(&response).0, "HTTP/1.1 201 Created");
    assert_eq!(
        fs::read(server.root.join("assets/async-post.txt")).unwrap(),
        b"multipart async"
    );

    let response = request(
        server.port,
        b"PUT /async-dir/ HTTP/1.1\r\nHost: localhost\r\n\r\n",
    );
    assert_status(&split(&response).0, "HTTP/1.1 201 Created");
    assert!(server.root.join("async-dir").is_dir());

    let response = request_split(
        server.port,
        "PUT /async-expect.txt HTTP/1.1\r\nHost: localhost\r\nContent-Length: 11\r\nExpect: 100-continue\r\n\r\n",
        b"expect body",
    );
    let response_text = String::from_utf8_lossy(&response);
    assert!(response_text.starts_with("HTTP/1.1 100 Continue\r\n\r\n"));
    assert!(response_text.contains("HTTP/1.1 201 Created"));
    assert!(response_text.ends_with("UPLOADED\n"));
    assert_eq!(
        fs::read(server.root.join("async-expect.txt")).unwrap(),
        b"expect body"
    );

    let response = get(server.port, "/sub");
    let (head, _) = split(&response);
    assert_status(&head, "HTTP/1.1 301 Moved Permanently");

    let response = get(server.port, "/assets/");
    let (head, body) = split(&response);
    assert_status(&head, "HTTP/1.1 200 OK");
    assert!(text(body).contains("hello world.txt"));

    // A client that stops reading a large download must not prevent another
    // request from completing. The first connection occupies one slot only.
    let mut slow = TcpStream::connect(("127.0.0.1", server.port)).expect("slow connect");
    slow.write_all(b"GET /huge.bin HTTP/1.1\r\nHost: localhost\r\n\r\n")
        .expect("slow request");
    std::thread::sleep(Duration::from_millis(150));
    let response = get(server.port, "/");
    assert_status(&split(&response).0, "HTTP/1.1 200 OK");
    drop(slow);

    // A stalled upload also occupies only its slot, and disconnecting removes
    // the partial file.
    let mut slow_upload = TcpStream::connect(("127.0.0.1", server.port)).expect("upload connect");
    slow_upload
        .write_all(b"PUT /partial-upload.bin HTTP/1.1\r\nHost: localhost\r\nContent-Length: 1000000\r\n\r\npartial")
        .expect("upload headers");
    std::thread::sleep(Duration::from_millis(100));
    let response = get(server.port, "/");
    assert_status(&split(&response).0, "HTTP/1.1 200 OK");
    drop(slow_upload);
    let deadline = Instant::now() + Duration::from_secs(2);
    while server.root.join("partial-upload.bin").exists() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(!server.root.join("partial-upload.bin").exists());
}

#[test]
fn blocking_fallback_still_works() {
    let server = start_server(&["--blocking"]);
    let response = get(server.port, "/hello%20world.txt");
    let (head, body) = split(&response);
    assert_status(&head, "HTTP/1.1 200 OK");
    assert_eq!(body, b"hello, space");
}

#[test]
fn async_idle_timeout_closes_stale_connection() {
    let server = start_server(&["--idle-timeout", "1"]);
    let mut stale = TcpStream::connect(("127.0.0.1", server.port)).expect("connect");
    stale
        .set_read_timeout(Some(Duration::from_secs(2)))
        .expect("read timeout");
    stale
        .write_all(b"GET / HTTP/1.1\r\nHost: localhost\r\n")
        .expect("partial request");
    std::thread::sleep(Duration::from_millis(1500));
    let mut byte = [0u8; 1];
    let closed = matches!(stale.read(&mut byte), Ok(0));
    assert!(closed, "idle connection was not closed");

    let response = get(server.port, "/");
    assert_status(&split(&response).0, "HTTP/1.1 200 OK");
}

#[test]
fn async_concurrent_gets_fill_all_slots() {
    let server = start_server(&["--async"]);
    let mut clients = Vec::new();
    for _ in 0..7 {
        let port = server.port;
        clients.push(std::thread::spawn(move || {
            let response = get(port, "/large.bin");
            let (head, body) = split(&response);
            assert_status(&head, "HTTP/1.1 200 OK");
            assert_eq!(body.len(), 40_000);
        }));
    }
    for client in clients {
        client.join().expect("client thread");
    }
}

#[test]
fn http11_keep_alive_reuses_one_connection() {
    let server = start_server(&[]);
    let mut stream = TcpStream::connect(("127.0.0.1", server.port)).expect("connect");
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .expect("read timeout");
    stream
        .write_all(b"GET /hello%20world.txt HTTP/1.1\r\nHost: x\r\n\r\n")
        .expect("first request");
    let first = read_one_response(&mut stream);
    let (head, body) = split(&first);
    assert_status(&head, "HTTP/1.1 200 OK");
    assert_eq!(header(&head, "Connection"), Some("keep-alive"));
    assert_eq!(body, b"hello, space");

    stream
        .write_all(b"GET /app.js HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n")
        .expect("second request");
    let second = read_one_response(&mut stream);
    let (head, body) = split(&second);
    assert_status(&head, "HTTP/1.1 200 OK");
    assert_eq!(header(&head, "Connection"), Some("close"));
    assert_eq!(body, b"console.log('hi')");
    let mut extra = [0u8; 1];
    assert_eq!(stream.read(&mut extra).expect("close read"), 0);
}

#[test]
fn pipelined_requests_keep_order_and_leftover_bytes() {
    let server = start_server(&[]);
    let mut stream = TcpStream::connect(("127.0.0.1", server.port)).expect("connect");
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .expect("read timeout");
    stream
        .write_all(
            b"GET /hello%20world.txt HTTP/1.1\r\nHost: x\r\n\r\nGET /app.js HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n",
        )
        .expect("pipeline");
    let first = read_one_response(&mut stream);
    let (head, body) = split(&first);
    assert_status(&head, "HTTP/1.1 200 OK");
    assert_eq!(body, b"hello, space");
    let second = read_one_response(&mut stream);
    let (head, body) = split(&second);
    assert_status(&head, "HTTP/1.1 200 OK");
    assert_eq!(header(&head, "Connection"), Some("close"));
    assert_eq!(body, b"console.log('hi')");
}

#[test]
fn http10_defaults_to_close_and_honors_keep_alive() {
    let server = start_server(&[]);
    let mut stream = TcpStream::connect(("127.0.0.1", server.port)).expect("connect");
    stream
        .write_all(b"GET /hello%20world.txt HTTP/1.0\r\nHost: x\r\n\r\n")
        .expect("http10");
    let response = read_one_response(&mut stream);
    let (head, _) = split(&response);
    assert_eq!(header(&head, "Connection"), Some("close"));

    let mut stream = TcpStream::connect(("127.0.0.1", server.port)).expect("connect");
    stream
        .write_all(b"GET /hello%20world.txt HTTP/1.0\r\nHost: x\r\nConnection: keep-alive\r\n\r\n")
        .expect("http10 keepalive");
    let response = read_one_response(&mut stream);
    let (head, _) = split(&response);
    assert_eq!(header(&head, "Connection"), Some("keep-alive"));
}

#[test]
fn upload_then_get_on_the_same_connection() {
    let server = start_server(&["--upload"]);
    let mut stream = TcpStream::connect(("127.0.0.1", server.port)).expect("connect");
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .expect("read timeout");
    stream
        .write_all(
            b"PUT /same-connection.txt HTTP/1.1\r\nHost: x\r\nContent-Length: 5\r\n\r\nhello",
        )
        .expect("put");
    let response = read_one_response(&mut stream);
    let (head, body) = split(&response);
    assert_status(&head, "HTTP/1.1 201 Created");
    assert_eq!(body, b"UPLOADED\n");

    stream
        .write_all(b"GET /same-connection.txt HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n")
        .expect("get");
    let response = read_one_response(&mut stream);
    let (head, body) = split(&response);
    assert_status(&head, "HTTP/1.1 200 OK");
    assert_eq!(body, b"hello");
}

#[test]
fn keep_alive_idle_timeout_closes_between_requests() {
    let server = start_server(&["--idle-timeout", "1"]);
    let mut stream = TcpStream::connect(("127.0.0.1", server.port)).expect("connect");
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .expect("read timeout");
    stream
        .write_all(b"GET /hello%20world.txt HTTP/1.1\r\nHost: x\r\n\r\n")
        .expect("request");
    let response = read_one_response(&mut stream);
    assert_status(&split(&response).0, "HTTP/1.1 200 OK");

    std::thread::sleep(Duration::from_millis(1500));
    let mut byte = [0u8; 1];
    assert_eq!(stream.read(&mut byte).expect("idle read"), 0);

    let response = get(server.port, "/");
    assert_status(&split(&response).0, "HTTP/1.1 200 OK");
}

#[test]
fn classic_form_post_redirects_back_to_the_listing() {
    let server = start_server(&["--upload"]);
    let body = multipart_body("----redirect", "redirected.txt", b"back to listing");
    let raw = format!(
        "POST /?sysweb-form=1 HTTP/1.1\r\nHost: localhost\r\nContent-Type: multipart/form-data; boundary=----redirect\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    )
    .into_bytes();
    let mut request_bytes = raw;
    request_bytes.extend_from_slice(&body);
    let response = request(server.port, &request_bytes);
    let (head, body) = split(&response);
    assert_status(&head, "HTTP/1.1 303 See Other");
    assert_eq!(header(&head, "Location"), Some("/"));
    assert!(body.is_empty());

    let response = get(server.port, "/redirected.txt");
    let (head, body) = split(&response);
    assert_status(&head, "HTTP/1.1 200 OK");
    assert_eq!(body, b"back to listing");
}

#[test]
fn classic_form_redirect_also_works_in_blocking_mode() {
    let server = start_server(&["--upload", "--blocking"]);
    let body = multipart_body("----redirectb", "redirected-blocking.txt", b"blocking");
    let raw = format!(
        "POST /?sysweb-form=1 HTTP/1.1\r\nHost: localhost\r\nContent-Type: multipart/form-data; boundary=----redirectb\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    )
    .into_bytes();
    let mut request_bytes = raw;
    request_bytes.extend_from_slice(&body);
    let response = request(server.port, &request_bytes);
    let (head, body) = split(&response);
    assert_status(&head, "HTTP/1.1 303 See Other");
    assert_eq!(header(&head, "Location"), Some("/"));
    assert!(body.is_empty());

    let response = get(server.port, "/redirected-blocking.txt");
    let (head, body) = split(&response);
    assert_status(&head, "HTTP/1.1 200 OK");
    assert_eq!(body, b"blocking");
}

#[test]
fn classic_text_form_uploads_via_get() {
    for extra in [&["--upload"][..], &["--upload", "--blocking"][..]] {
        let server = start_server(extra);
        let response = request(
            server.port,
            b"GET /?sysweb-text=1&name=note%20one.txt&text=hello+world HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
        );
        let (head, body) = split(&response);
        assert_status(&head, "HTTP/1.1 303 See Other");
        assert_eq!(header(&head, "Location"), Some("/"));
        assert!(body.is_empty());

        let response = get(server.port, "/note%20one.txt");
        let (head, body) = split(&response);
        assert_status(&head, "HTTP/1.1 200 OK");
        assert_eq!(body, b"hello world");
    }
}
