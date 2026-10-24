//! Adversarial request fuzzing: no outside reads, no hangs, no crashes.
//!
//! This is a deterministic black-box companion to the libFuzzer targets. It
//! exercises the real server process with traversal, symlink, malformed-path,
//! and randomized requests, asserting a bounded status set and no secret bytes.

#![cfg(unix)]

use std::fs;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

const SECRET: &[u8] = b"TOP_SECRET_SHOULD_NOT_LEAK";

struct Server {
    child: Child,
    root: PathBuf,
    outside: PathBuf,
    port: u16,
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = fs::remove_dir_all(&self.root);
        let _ = fs::remove_dir_all(&self.outside);
    }
}

fn free_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind ephemeral port");
    listener.local_addr().expect("addr").port()
}

fn start() -> Server {
    let root = std::env::temp_dir().join(format!("sysweb-security-{}", std::process::id()));
    let outside = std::env::temp_dir().join(format!("sysweb-secret-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    let _ = fs::remove_dir_all(&outside);
    fs::create_dir_all(root.join("assets")).expect("root");
    fs::create_dir_all(&outside).expect("outside");
    fs::write(root.join("ok.txt"), b"ok").expect("ok");
    fs::write(root.join("assets/inside.txt"), b"inside").expect("inside");
    fs::write(outside.join("secret.txt"), SECRET).expect("secret");
    std::os::unix::fs::symlink(&outside, root.join("assets/linkdir")).expect("dir symlink");
    std::os::unix::fs::symlink(outside.join("secret.txt"), root.join("assets/leak"))
        .expect("file symlink");

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

    let mut server = Server {
        child,
        root,
        outside,
        port,
    };

    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if TcpStream::connect(("127.0.0.1", port)).is_ok() {
            return server;
        }
        if let Some(status) = server.child.try_wait().expect("try_wait") {
            panic!("syswebd exited during startup: {status}");
        }
        if Instant::now() > deadline {
            panic!("syswebd did not start");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn request(port: u16, target: &str) -> Vec<u8> {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect");
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .expect("timeout");
    stream
        .write_all(
            format!("GET {target} HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n").as_bytes(),
        )
        .expect("write");
    let mut response = Vec::new();
    stream
        .read_to_end(&mut response)
        .expect("read (server hung or crashed?)");
    response
}

fn status(response: &[u8]) -> u16 {
    let line_end = response
        .iter()
        .position(|&byte| byte == b'\r')
        .expect("status line");
    let line = std::str::from_utf8(&response[..line_end]).expect("ascii status");
    let mut parts = line.split(' ');
    let version = parts.next().expect("version");
    assert!(version.starts_with("HTTP/1."), "bad version in {line}");
    parts
        .next()
        .expect("status code")
        .parse()
        .expect("numeric status")
}

fn assert_safe(port: u16, target: &str) {
    let response = request(port, target);
    assert!(!response.is_empty(), "empty response for {target:?}");
    let code = status(&response);
    assert!(
        matches!(code, 200 | 301 | 400 | 403 | 404 | 414 | 431 | 505),
        "unexpected status {code} for {target:?}"
    );
    assert!(
        !response
            .windows(SECRET.len())
            .any(|window| window == SECRET),
        "secret leaked for {target:?}"
    );
}

#[test]
fn rejects_traversal_symlinks_and_random_paths() {
    let mut server = start();
    let port = server.port;
    let outside_name = server
        .outside
        .file_name()
        .expect("outside name")
        .to_string_lossy()
        .into_owned();

    assert_safe(port, "/");
    assert_safe(port, "/ok.txt");
    assert_safe(port, "/assets/");
    assert_safe(port, "/assets/inside.txt");

    let mut corpus = vec![
        "/../secret.txt".to_string(),
        format!("/../{outside_name}/secret.txt"),
        "/%2e%2e/secret.txt".to_string(),
        "/%2E%2E/secret.txt".to_string(),
        format!("/%2e%2e/{outside_name}/secret.txt"),
        "/%252e%252e/secret.txt".to_string(),
        "/..%2fsecret.txt".to_string(),
        "/%2e%2e%2fsecret.txt".to_string(),
        "/a/../../secret.txt".to_string(),
        "//../secret.txt".to_string(),
        "/%2f..%2fsecret.txt".to_string(),
        "/assets/linkdir/secret.txt".to_string(),
        "/assets/linkdir/".to_string(),
        "/assets/leak".to_string(),
        "/assets/leak%00".to_string(),
        "/%00".to_string(),
        "/%5c..%5csecret.txt".to_string(),
        "/....//secret.txt".to_string(),
        "http://127.0.0.1/secret.txt".to_string(),
        "*".to_string(),
    ];
    corpus.push(format!("/{}", "a".repeat(2000)));

    for target in &corpus {
        assert_safe(port, target);
    }

    // Deterministic randomized paths from a path-ish alphabet.
    const ALPHABET: &[u8] = b"/.%2e\\?&#;,abcdefghijklmnopqrstuvwxyz0123456789-_~";
    let mut state = 0x9e37_79b9_7f4a_7c15u64;
    for _ in 0..300 {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        let len = 1 + (state as usize % 80);
        let mut target = String::with_capacity(len + 1);
        target.push('/');
        for _ in 0..len {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            target.push(ALPHABET[(state as usize) % ALPHABET.len()] as char);
        }
        assert_safe(port, &target);
    }

    assert!(
        server.child.try_wait().expect("try_wait").is_none(),
        "server exited during fuzzing"
    );
    let response = request(port, "/ok.txt");
    assert_eq!(status(&response), 200);
    assert!(response.ends_with(b"ok"));
}
