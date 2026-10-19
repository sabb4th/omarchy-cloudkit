use crate::storage::{Result, text};
use serde_json::{Value, json};
use std::{
    path::{Path, PathBuf},
    time::Duration,
};
use tokio::process::{Child, Command};
#[derive(Clone)]
pub struct Rc {
    client: reqwest::Client,
}
impl Rc {
    pub fn new(path: &Path) -> Result<Self> {
        Ok(Self {
            client: reqwest::Client::builder()
                .unix_socket(path)
                .no_proxy()
                .timeout(Duration::from_secs(60))
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .map_err(|_| "Cannot initialize rclone client")?,
        })
    }
    pub async fn call(&self, method: &str, params: Value) -> Result<Value> {
        self.call_timeout(method, params, 60).await
    }
    pub async fn call_timeout(&self, method: &str, params: Value, seconds: u64) -> Result<Value> {
        let response = self
            .client
            .post(format!("http://localhost/{method}"))
            .timeout(Duration::from_secs(seconds.clamp(1, 300)))
            .json(&params)
            .send()
            .await
            .map_err(|e| {
                if e.is_timeout() {
                    "rclone timed out".to_owned()
                } else {
                    "rclone is not reachable".to_owned()
                }
            })?;
        let status = response.status();
        let mut bytes = vec![];
        let mut response = response;
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| "Cannot read rclone response")?
        {
            if bytes.len() + chunk.len() > 32 * 1024 * 1024 {
                return Err("rclone returned too much data".into());
            }
            bytes.extend_from_slice(&chunk)
        }
        let v: Value = serde_json::from_slice(&bytes).map_err(|_| "Invalid rclone response")?;
        if !status.is_success() {
            return Err(if text(&v["error"]).is_empty() {
                format!("rclone HTTP {status}")
            } else {
                text(&v["error"]).to_owned()
            });
        }
        Ok(v)
    }
}
pub fn child_command(program: &str) -> Command {
    let mut c = Command::new(program);
    c.kill_on_drop(true);
    let parent = unsafe { libc::getpid() };
    unsafe {
        c.pre_exec(move || {
            if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            if libc::getppid() != parent {
                return Err(std::io::Error::other("Parent exited before rclone startup"));
            }
            Ok(())
        });
    }
    c
}
pub async fn launch(socket: &Path, log: &Path, engine: bool) -> Result<(Child, Rc)> {
    let _ = std::fs::remove_file(socket);
    let rc = Rc::new(socket)?;
    use std::os::unix::fs::OpenOptionsExt;
    let err = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .open(log)
        .map_err(|e| e.to_string())?;
    let mut command = child_command("rclone");
    command.args([
        "rcd",
        &format!("--rc-addr=unix://{}", socket.display()),
        "--rc-no-auth",
        &format!("--log-file={}", log.display()),
        "--log-level=INFO",
        "--ask-password=false",
        "--stats=0",
        "--fast-list",
        "--transfers=4",
        "--checkers=8",
        "--timeout=60s",
        "--contimeout=15s",
        "--low-level-retries=5",
        "--use-mmap",
    ]);
    if engine {
        command.args(["--log-file-max-size=5M", "--log-file-max-backups=1"]);
    }
    let mut child = command
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(err)
        .spawn()
        .map_err(|e| e.to_string())?;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    loop {
        if child.try_wait().map_err(|e| e.to_string())?.is_some() {
            return Err("rclone exited during startup; inspect its log".into());
        }
        if rc.call_timeout("rc/noop", json!({}), 1).await.is_ok() {
            return Ok((child, rc));
        }
        if tokio::time::Instant::now() > deadline {
            return Err("rclone startup timed out".into());
        }
        tokio::time::sleep(Duration::from_millis(50)).await
    }
}
pub async fn stop(child: &mut Child) {
    if let Some(pid) = child.id() {
        unsafe {
            libc::kill(pid as i32, libc::SIGTERM);
        }
    }
    if tokio::time::timeout(Duration::from_secs(10), child.wait())
        .await
        .is_err()
    {
        let _ = child.kill().await;
        let _ = child.wait().await;
    }
}
pub fn spec(name: &str, kind: &str, path: &str) -> String {
    if kind == "drive" {
        format!("{name},skip_gdocs=true:{path}")
    } else {
        format!("{name}:{path}")
    }
}
pub fn log_path(state: &Path) -> PathBuf {
    state.join("rclone.log")
}
