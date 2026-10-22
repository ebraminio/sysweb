//! End-to-end test: spawn the real freestanding binary and speak HTTP to it.

use std::fs;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

struct TestServer {
    child: Child,
    port: u16,
    root: PathBuf,
}

impl Drop for TestServer {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = fs::remove_dir_all(&self.root);
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

fn start_server() -> TestServer {
    let root = std::env::temp_dir().join(format!("sysweb-e2e-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).expect("create root");

    write_file(&root, "index.html", b"<h1>index</h1>");
    write_file(&root, "hello world.txt", b"hello, space");
    write_file(&root, "app.js", b"console.log('hi')");
    write_file(&root, "sub/index.html", b"sub index");
    write_file(&root, "blob.bin", &[0u8, 1, 2, 3, 0xff, 0xfe, 0, 42]);
    let large: Vec<u8> = (0..40_000u32).map(|i| (i % 251) as u8).collect();
    write_file(&root, "large.bin", &large);

    let port = free_port();
    let child = Command::new(env!("CARGO_BIN_EXE_syswebd"))
        .arg("--root")
        .arg(&root)
        .arg("--port")
        .arg(port.to_string())
        .arg("--quiet")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn syswebd");

    let mut server = TestServer { child, port, root };

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
    let mut response = Vec::new();
    stream.read_to_end(&mut response).expect("read response");
    response
}

fn get(port: u16, target: &str) -> Vec<u8> {
    let raw = format!("GET {target} HTTP/1.1\r\nHost: localhost\r\n\r\n");
    request(port, raw.as_bytes())
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

fn assert_status(head: &str, expected: &str) {
    assert!(
        head.starts_with(expected),
        "expected status {expected:?}, response was:\n{head}"
    );
}

#[test]
fn serves_static_files_and_rejects_bad_requests() {
    let server = start_server();
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
    assert_eq!(body.len(), 40_000);
    assert_eq!(body[0], 0);
    assert_eq!(body[39_999], (39_999u32 % 251) as u8);

    let response = get(port, "/missing");
    let (head, body) = split(&response);
    assert_status(&head, "HTTP/1.1 404 Not Found");
    assert_eq!(body, b"404 Not Found\n");

    // A directory without a trailing slash does not get an index.
    let response = get(port, "/sub");
    let (head, _) = split(&response);
    assert_status(&head, "HTTP/1.1 404 Not Found");

    // Longer than MAX_URL_PATH: rejected before touching the filesystem.
    let long_target = format!("/{}", "a".repeat(1500));
    let response = get(port, &long_target);
    let (head, _) = split(&response);
    assert_status(&head, "HTTP/1.1 414 URI Too Long");

    // HTTP/1.0 is accepted; the response is still Connection: close.
    let response = request(port, b"GET / HTTP/1.0\r\n\r\n");
    let (head, body) = split(&response);
    assert_status(&head, "HTTP/1.1 200 OK");
    assert_eq!(body, b"<h1>index</h1>");

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
}
