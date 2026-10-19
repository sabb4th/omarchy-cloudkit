use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixListener;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};

static NEXT: AtomicUsize = AtomicUsize::new(0);
struct Sandbox(PathBuf);
impl Sandbox {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "gn-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&root).unwrap();
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
        Self(root)
    }
    fn command(&self, action: &str) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_omarchy-cloudkit"));
        command
            .arg(action)
            .env("XDG_RUNTIME_DIR", &self.0)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        command
    }
    fn listen(&self) -> UnixListener {
        let runtime = self.0.join("omarchy-cloudkit");
        fs::create_dir_all(&runtime).unwrap();
        fs::set_permissions(&runtime, fs::Permissions::from_mode(0o700)).unwrap();
        UnixListener::bind(runtime.join("daemon.sock")).unwrap()
    }
}
impl Drop for Sandbox {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}
#[test]
fn stopped_is_idle_but_invalid_state_is_unknown() {
    let sandbox = Sandbox::new();
    let output = sandbox.command("busy").output().unwrap();
    assert!(output.status.success());
    assert_eq!(output.stdout, b"0\n");
    let listener = sandbox.listen();
    let server = std::thread::spawn(move || {
        let (mut peer, _) = listener.accept().unwrap();
        peer.write_all(b"{\"type\":\"state\",\"data\":{}}\n")
            .unwrap();
    });
    let output = sandbox.command("busy").output().unwrap();
    server.join().unwrap();
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
}

#[test]
fn insecure_runtime_is_unknown_even_without_socket() {
    let sandbox = Sandbox::new();
    let runtime = sandbox.0.join("omarchy-cloudkit");
    fs::create_dir(&runtime).unwrap();
    fs::set_permissions(&runtime, fs::Permissions::from_mode(0o755)).unwrap();
    let output = sandbox.command("busy").output().unwrap();
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert_eq!(
        fs::metadata(runtime).unwrap().permissions().mode() & 0o777,
        0o755
    );
}
#[test]
fn bridge_transports_requests_and_exits_on_stdin_eof() {
    let sandbox = Sandbox::new();
    let listener = sandbox.listen();
    let mut child = sandbox.command("bridge").spawn().unwrap();
    let (mut peer, _) = listener.accept().unwrap();
    peer.set_read_timeout(Some(std::time::Duration::from_secs(5)))
        .unwrap();
    let mut output = BufReader::new(child.stdout.take().unwrap());
    let mut line = String::new();
    output.read_line(&mut line).unwrap();
    assert_eq!(line, "{\"type\":\"connected\"}\n");
    peer.write_all(b"{\"type\":\"state\",\"data\":{}}\n")
        .unwrap();
    line.clear();
    output.read_line(&mut line).unwrap();
    assert!(line.contains("state"));
    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(b"{\"cmd\":\"touch\",\"id\":1}\n")
        .unwrap();
    let mut input = BufReader::new(peer);
    line.clear();
    input.read_line(&mut line).unwrap();
    assert_eq!(line, "{\"cmd\":\"touch\",\"id\":1}\n");
    drop(child.stdin.take());
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            assert!(status.success());
            break;
        }
        if std::time::Instant::now() > deadline {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!("bridge did not exit on stdin EOF");
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}
