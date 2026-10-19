use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

struct Profile(PathBuf);
impl Profile {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("ck-power-{}", std::process::id()));
        fs::create_dir_all(root.join("bin")).unwrap();
        fs::create_dir_all(root.join("run")).unwrap();
        fs::set_permissions(root.join("run"), fs::Permissions::from_mode(0o700)).unwrap();
        // Never involve the desktop user's real systemd service.
        let stub = root.join("bin/systemctl");
        fs::write(&stub, "#!/bin/sh\nexit 1\n").unwrap();
        fs::set_permissions(stub, fs::Permissions::from_mode(0o755)).unwrap();
        fs::write(root.join("rclone.conf"), "").unwrap();
        Self(root)
    }
    fn command(&self) -> Command {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_omarchy-cloudkit"));
        cmd.env("HOME", &self.0)
            .env("XDG_CONFIG_HOME", self.0.join("config"))
            .env("XDG_STATE_HOME", self.0.join("state"))
            .env("XDG_CACHE_HOME", self.0.join("cache"))
            .env("XDG_RUNTIME_DIR", self.0.join("run"))
            .env("RCLONE_CONFIG", self.0.join("rclone.conf"))
            .env(
                "PATH",
                format!(
                    "{}:{}",
                    self.0.join("bin").display(),
                    std::env::var("PATH").unwrap()
                ),
            )
            .stdin(Stdio::null());
        cmd
    }
    fn power(&self, action: &str) -> serde_json::Value {
        let out = self.command().args(["power", action]).output().unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        serde_json::from_slice(&out.stdout).unwrap()
    }
}
impl Drop for Profile {
    fn drop(&mut self) {
        let _ = self.command().args(["power", "off"]).output();
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn power_off_survives_restart_and_power_on_restores_the_engine() {
    let p = Profile::new();
    assert_eq!(p.power("status")["enabled"], true);
    assert_eq!(p.power("off")["enabled"], false);
    let login = p.command().args(["service", "status"]).output().unwrap();
    assert!(login.status.success());
    let login: serde_json::Value = serde_json::from_slice(&login.stdout).unwrap();
    assert_eq!(login["installed"], false);
    assert_eq!(login["enabled"], false);
    assert!(!p.0.join("run/omarchy-cloudkit/daemon.sock").exists());
    assert_eq!(p.power("off")["enabled"], false);
    let disabled_daemon = p.command().arg("daemon").output().unwrap();
    assert!(disabled_daemon.status.success());
    assert!(!p.0.join("run/omarchy-cloudkit/daemon.sock").exists());
    for args in [vec!["daemon", "--ensure"], vec!["status", "--json"]] {
        let out = p.command().args(args).output().unwrap();
        assert!(!out.status.success());
        assert!(String::from_utf8_lossy(&out.stderr).contains("switched off"));
    }
    // Exercise the real rclone engine, but with an empty, isolated account file.
    if Command::new("rclone").arg("version").output().is_err() {
        return;
    }
    assert_eq!(p.power("on")["enabled"], true);
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        let out = p.command().args(["status", "--json"]).output().unwrap();
        assert!(out.status.success());
        let state: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
        if state["engine"]["state"] == "running" {
            break;
        }
        assert!(Instant::now() < deadline, "engine did not start: {state}");
        std::thread::sleep(Duration::from_millis(100));
    }
    let socket = p.0.join("run/omarchy-cloudkit/daemon.sock");
    let rc_socket = p.0.join("run/omarchy-cloudkit/rclone.sock");
    assert!(socket.exists());
    assert!(rc_socket.exists());
    let config = p.0.join("config/omarchy-cloudkit/config.json");
    let saved = fs::read(&config).unwrap();
    assert_eq!(p.power("off")["enabled"], false);
    assert!(!socket.exists());
    assert!(!rc_socket.exists());
    assert_eq!(
        fs::read(&config).unwrap(),
        saved,
        "power must not rewrite folder or mount preferences"
    );
    assert_eq!(p.power("on")["enabled"], true);
    assert_eq!(p.power("off")["enabled"], false);
}
