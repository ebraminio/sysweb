//! Port selection: the implicit default may fall back; an explicit port may not.

use std::fs;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

static NEXT: AtomicUsize = AtomicUsize::new(0);

struct Server {
    child: Child,
    root: PathBuf,
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn spawn(args: &[&str]) -> Server {
    let id = NEXT.fetch_add(1, Ordering::Relaxed);
    let root = std::env::temp_dir().join(format!("sysweb-port-{}-{id}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).expect("create root");
    fs::write(root.join("ok.txt"), b"ok").expect("fixture");

    let mut command = Command::new(env!("CARGO_BIN_EXE_syswebd"));
    command
        .arg("--root")
        .arg(&root)
        .arg("--quiet")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    for arg in args {
        command.arg(arg);
    }
    let child = command.spawn().expect("spawn syswebd");
    Server { child, root }
}

fn get(port: u16) -> Option<Vec<u8>> {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).ok()?;
    stream.set_read_timeout(Some(Duration::from_secs(2))).ok()?;
    stream
        .write_all(b"GET /ok.txt HTTP/1.1\r\nHost: x\r\n\r\n")
        .ok()?;
    let mut response = Vec::new();
    stream.read_to_end(&mut response).ok()?;
    Some(response)
}

#[test]
fn implicit_default_port_falls_back_when_busy() {
    // Hold 8080 so the server's implicit default must move.
    let _gate = match TcpListener::bind(("0.0.0.0", 8080)) {
        Ok(gate) => gate,
        Err(_) => return, // 8080 is already occupied by the environment
    };

    let mut server = spawn(&[]);
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        for port in 8081..=8100u16 {
            if let Some(response) = get(port) {
                assert!(
                    response.starts_with(b"HTTP/1.1 200 OK"),
                    "unexpected response on {port}: {}",
                    String::from_utf8_lossy(&response)
                );
                assert!(response.ends_with(b"ok"));
                return;
            }
        }
        if let Some(status) = server.child.try_wait().expect("try_wait") {
            panic!("syswebd exited before choosing a fallback port: {status}");
        }
        if Instant::now() > deadline {
            panic!("syswebd did not choose a fallback port");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    // `_gate`, `server`, and the child are cleaned up by Drop.
}

#[test]
fn explicit_port_does_not_fall_back() {
    let gate = TcpListener::bind(("0.0.0.0", 0)).expect("reserve port");
    let port = gate.local_addr().expect("addr").port();
    let mut server = spawn(&["--port", &port.to_string()]);

    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(status) = server.child.try_wait().expect("try_wait") {
            assert!(
                !status.success(),
                "explicit-port bind unexpectedly succeeded"
            );
            return;
        }
        if Instant::now() > deadline {
            panic!("syswebd did not exit when the explicit port was busy");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}
