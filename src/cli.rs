use crate::{daemon, ipc, providers, storage::*};
use serde_json::{Value, json};
use std::{
    env,
    io::{self, BufReader, Read, Write},
    os::unix::{net::UnixStream, process::CommandExt},
    process::{Command, Stdio},
    time::{Duration, Instant},
};
fn runtime() -> Result<tokio::runtime::Runtime> {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(4)
        .max_blocking_threads(8)
        .enable_all()
        .build()
        .map_err(|e| e.to_string())
}
fn connect(start: bool) -> Result<UnixStream> {
    match ipc::connect() {
        Ok(s) => Ok(s),
        Err(e)
            if start
                && matches!(
                    e.kind(),
                    io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused
                ) =>
        {
            ensure()?;
            ipc::connect().map_err(|e| e.to_string())
        }
        Err(e) => Err(e.to_string()),
    }
}
fn state(reader: &mut BufReader<UnixStream>) -> Result<Value> {
    while let Some((_, v)) = ipc::json_line(reader).map_err(|e| e.to_string())? {
        if v["type"] == "state" {
            return Ok(v["data"].clone());
        }
    }
    Err("Daemon disconnected".into())
}
fn request(cmd: &str, args: Value) -> Result<Value> {
    let mut socket = connect(!["shutdown", "power_off"].contains(&cmd))?;
    socket
        .set_read_timeout(Some(Duration::from_secs(330)))
        .map_err(|e| e.to_string())?;
    writeln!(socket, "{}", json!({"id":1,"cmd":cmd,"args":args})).map_err(|e| e.to_string())?;
    let mut reader = BufReader::new(socket);
    while let Some((_, v)) = ipc::json_line(&mut reader).map_err(|e| e.to_string())? {
        if v["type"] == "reply" && v["id"] == 1 {
            return if v["ok"] == true {
                Ok(v["data"].clone())
            } else {
                Err(text(&v["error"]).to_owned())
            };
        }
    }
    Err("Daemon disconnected".into())
}
fn snapshot() -> Result<Value> {
    let s = connect(true)?;
    s.set_read_timeout(Some(Duration::from_secs(15)))
        .map_err(|e| e.to_string())?;
    state(&mut BufReader::new(s))
}
fn ensure() -> Result<()> {
    let paths = Paths::new();
    if !paths.enabled()? {
        return Err("CloudKit is switched off; use omarchy-cloudkit power on".into());
    }
    ipc::private_runtime(&paths.runtime).map_err(|e| e.to_string())?;
    if ipc::connect().is_ok() {
        return Ok(());
    }
    let exe = env::current_exe().map_err(|e| e.to_string())?;
    let systemd = Command::new("systemctl")
        .args(["--user", "is-system-running"])
        .output()
        .ok()
        .is_some_and(|o| {
            ["running", "degraded", "starting", "initializing"]
                .contains(&String::from_utf8_lossy(&o.stdout).trim())
        });
    let mut started = false;
    if systemd {
        if paths.unit().is_file() {
            started = Command::new("systemctl")
                .args(["--user", "start", "omarchy-cloudkit.service"])
                .status()
                .is_ok_and(|s| s.success());
        } else {
            let mut c = Command::new("systemd-run");
            c.args([
                "--user",
                "--quiet",
                "--collect",
                "--unit=omarchy-cloudkit.service",
                "--property=KillMode=mixed",
                "--property=TimeoutStopSec=75",
            ]);
            for k in [
                "PATH",
                "XDG_RUNTIME_DIR",
                "WAYLAND_DISPLAY",
                "DISPLAY",
                "DBUS_SESSION_BUS_ADDRESS",
                "BROWSER",
                "RCLONE_CONFIG",
                "XDG_CONFIG_HOME",
                "XDG_STATE_HOME",
                "XDG_CACHE_HOME",
                "HOME",
                "LANG",
            ] {
                if let Ok(v) = env::var(k) {
                    c.arg(format!("--setenv={k}={v}"));
                }
            }
            started = c
                .arg("--")
                .arg(&exe)
                .arg("daemon")
                .status()
                .is_ok_and(|s| s.success());
        }
    }
    if !started {
        let mut c = Command::new(exe);
        c.arg("daemon")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        unsafe {
            c.pre_exec(|| {
                if libc::setsid() < 0 {
                    return Err(io::Error::last_os_error());
                }
                Ok(())
            });
        }
        c.spawn().map_err(|e| e.to_string())?;
    }
    let until = Instant::now() + Duration::from_secs(20);
    while Instant::now() < until {
        if ipc::connect().is_ok() {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    Err("Daemon did not start; run omarchy-cloudkit daemon to inspect the error".into())
}
fn folder(spec: &str) -> Result<String> {
    let s = snapshot()?;
    let mut found = vec![];
    for d in s["drives"].as_array().unwrap_or(&vec![]) {
        for f in d["folders"].as_array().unwrap_or(&vec![]) {
            if text(&f["id"]) == spec
                || text(&f["label"]) == spec
                || format!("{}:{}", text(&d["name"]), text(&f["path"])) == spec
                || expand(spec) == expand(text(&f["local"]))
            {
                found.push(text(&f["id"]).to_owned());
            }
        }
    }
    if found.len() != 1 {
        return Err("Folder not found or ambiguous; use its ID or REMOTE:PATH".into());
    }
    Ok(found.remove(0))
}
fn split(raw: &str) -> Result<(&str, &str)> {
    raw.split_once(':')
        .ok_or_else(|| "Expected REMOTE:PATH".into())
}
fn prompt(message: &str, secret: bool) -> Result<String> {
    eprint!("{}: ", safe(message));
    io::stderr().flush().map_err(|e| e.to_string())?;
    let mut old = std::mem::MaybeUninit::<libc::termios>::uninit();
    let mut changed = false;
    let saved = unsafe {
        if secret && libc::tcgetattr(0, old.as_mut_ptr()) == 0 {
            let old = old.assume_init();
            let mut new = old;
            new.c_lflag &= !libc::ECHO;
            changed = libc::tcsetattr(0, libc::TCSANOW, &new) == 0;
            Some(old)
        } else {
            None
        }
    };
    let mut line = String::new();
    let read = io::stdin().read_line(&mut line);
    if changed {
        if let Some(old) = saved {
            unsafe {
                libc::tcsetattr(0, libc::TCSANOW, &old);
            }
        }
        eprintln!();
    }
    read.map_err(|e| e.to_string())?;
    Ok(line.trim_end_matches(['\r', '\n']).to_owned())
}
fn guide(g: &Value) {
    eprintln!("{}\n{}", safe(text(&g["title"])), safe(text(&g["why"])));
    for (i, s) in g["steps"].as_array().unwrap_or(&vec![]).iter().enumerate() {
        eprintln!(
            "{}. {}\n{}",
            i + 1,
            safe(text(&s["title"])),
            safe(text(&s["text"]))
        );
        if !s["link"].is_null() {
            eprintln!("{}", safe(text(&s["link"]["url"])));
        }
        for c in s["copy"].as_array().unwrap_or(&vec![]) {
            eprintln!("{}: {}", safe(text(&c["label"])), safe(text(&c["value"])));
        }
    }
}
fn own_client(kind: &str) -> Result<(String, String)> {
    let g = providers::guide(kind)?;
    guide(&g);
    let id = prompt(text(&g["id"]["label"]), false)?;
    let secret = prompt(text(&g["secret"]["label"]), true)?;
    providers::check(kind, &id, &secret)?;
    Ok((id, secret))
}
pub fn main() -> Result<()> {
    let all: Vec<_> = env::args().skip(1).collect();
    let cmd = all.first().map_or("status", String::as_str);
    let rest = all.get(1..).unwrap_or_default();
    let mut pos = vec![];
    let mut opts = std::collections::BTreeMap::<String, Vec<String>>::new();
    let mut i = 0;
    while i < rest.len() {
        let s = &rest[i];
        if s.starts_with('-') {
            let (key, value) = if let Some((k, v)) = s.split_once('=') {
                (k.to_owned(), Some(v.to_owned()))
            } else {
                (s.clone(), None)
            };
            let value = if let Some(v) = value {
                v
            } else if [
                "--client-id",
                "--mount-path",
                "--add",
                "--remove",
                "-n",
                "--lines",
            ]
            .contains(&key.as_str())
            {
                i += 1;
                rest.get(i).ok_or("Missing option value")?.clone()
            } else {
                String::new()
            };
            opts.entry(key).or_default().push(value);
        } else {
            pos.push(s.clone());
        }
        i += 1;
    }
    let arg = |n: usize| pos.get(n).map_or("", String::as_str);
    let flag = |key: &str| opts.contains_key(key);
    let option = |key: &str| {
        opts.get(key)
            .and_then(|v| v.last())
            .map_or("", String::as_str)
    };
    if ["--help", "-h", "help"].contains(&cmd) || flag("--help") {
        println!(
            "omarchy-cloudkit — cloud drives and safe two-way sync\nCommands: power on|off|status, status [--json], daemon [--ensure], stop, watch, bridge, busy,\nstream DRIVE on|off [--force], mount DRIVE, unmount DRIVE [--force],\nstream-all on|off, keep REMOTE:PATH [LOCAL], unkeep FOLDER [--delete-local],\nsync [FOLDER], pause|resume FOLDER..., alias FOLDER LABEL,\nexclude [FOLDER] [--add PATTERN] [--remove PATTERN] [--global],\nresolve FOLDER resync|force|dismiss, browse REMOTE:PATH, size REMOTE:PATH,\nadd-oauth NAME PROVIDER [--own-client] [--client-id ID --client-secret-stdin],\nadd-credentials NAME PROVIDER < credentials.json, set-client-id DRIVE,\nclient-guide PROVIDER, reconnect DRIVE, retry DRIVE, takeover DRIVE,\nremove DRIVE [-y], label DRIVE LABEL, set-path DRIVE PATH,\nconfig [KEY [VALUE]], log [-n N] [--daemon], open [DRIVE], service install|uninstall|status"
        );
        return Ok(());
    }
    if cmd == "--version" {
        println!("omarchy-cloudkit {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    if cmd == "power" {
        let paths = Paths::new();
        match arg(0) {
            "status" => (),
            "on" => {
                paths.set_enabled(true)?;
                ensure()?;
            }
            "off" => {
                match ipc::connect() {
                    Ok(_) => {
                        request("power_off", json!({}))?;
                    }
                    Err(e)
                        if matches!(
                            e.kind(),
                            io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused
                        ) =>
                    {
                        paths.set_enabled(false)?;
                    }
                    Err(e) => return Err(e.to_string()),
                }
                let deadline = Instant::now() + Duration::from_secs(75);
                while ipc::connect().is_ok() {
                    if Instant::now() >= deadline {
                        return Err(
                            "CloudKit is still stopping; pending work is being preserved".into(),
                        );
                    }
                    std::thread::sleep(Duration::from_millis(100));
                }
            }
            _ => return Err("Expected power on|off|status".into()),
        }
        println!("{}", json!({"enabled":paths.enabled()?}));
        return Ok(());
    }
    if cmd == "daemon" {
        if flag("--ensure") {
            ensure()?;
            println!("omarchy-cloudkit daemon is running");
            return Ok(());
        }
        let runtime = runtime()?;
        let result = runtime.block_on(daemon::run());
        runtime.shutdown_timeout(Duration::from_secs(2));
        return result;
    }
    if cmd == "bridge" {
        return ipc::bridge().map_err(|e| e.to_string());
    }
    if cmd == "busy" {
        return ipc::busy().map_err(|e| e.to_string());
    }
    if cmd == "client-guide" {
        guide(&providers::guide(arg(0))?);
        return Ok(());
    }
    if cmd == "watch" {
        let stream = connect(true)?;
        let mut r = BufReader::new(stream);
        while let Some((_, value)) = ipc::json_line(&mut r).map_err(|e| e.to_string())? {
            if value["type"] != "state" {
                continue;
            }
            let bytes = format!("{}\n", json_ascii(&value["data"], false));
            io::stdout()
                .write_all(bytes.as_bytes())
                .map_err(|e| e.to_string())?;
            io::stdout().flush().map_err(|e| e.to_string())?;
        }
        return Ok(());
    }
    if cmd == "service" {
        let paths = Paths::new();
        match arg(0) {
            "status" => {
                let installed = paths.unit().is_file();
                let enabled = installed
                    && Command::new("systemctl")
                        .args([
                            "--user",
                            "is-enabled",
                            "--quiet",
                            "omarchy-cloudkit.service",
                        ])
                        .status()
                        .is_ok_and(|s| s.success());
                println!("{}", json!({"installed":installed,"enabled":enabled}));
            }
            "install" => {
                let exe = env::current_exe().map_err(|e| e.to_string())?;
                let p = exe.to_string_lossy();
                let escaped = p
                    .replace('\\', "\\\\")
                    .replace('"', "\\\"")
                    .replace('%', "%%")
                    .replace('$', "$$")
                    .replace('\n', "\\x0a")
                    .replace('\r', "\\x0d");
                let unit = format!(
                    "[Unit]\nDescription=omarchy-cloudkit cloud drives\nAfter=graphical-session.target\nPartOf=graphical-session.target\n\n[Service]\nExecStart=\"{escaped}\" daemon\nRestart=on-failure\nKillMode=mixed\nTimeoutStopSec=75\n\n[Install]\nWantedBy=default.target\n"
                );
                atomic(&paths.unit(), unit.as_bytes())?;
                for args in [
                    vec!["--user", "daemon-reload"],
                    vec!["--user", "enable", "omarchy-cloudkit.service"],
                ] {
                    if !Command::new("systemctl")
                        .args(args)
                        .status()
                        .map_err(|e| e.to_string())?
                        .success()
                    {
                        return Err("systemctl failed".into());
                    }
                }
            }
            "uninstall" => {
                let _ = Command::new("systemctl")
                    .args(["--user", "disable", "omarchy-cloudkit.service"])
                    .status();
                if paths.unit().exists() {
                    std::fs::remove_file(paths.unit()).map_err(|e| e.to_string())?;
                }
                let _ = Command::new("systemctl")
                    .args(["--user", "daemon-reload"])
                    .status();
            }
            _ => return Err("Expected service install|uninstall|status".into()),
        }
        return Ok(());
    }
    if cmd == "status" {
        let state = snapshot()?;
        if flag("--json") {
            println!("{}", json_ascii(&state, true));
        } else {
            println!("Engine: {}", safe(text(&state["engine"]["state"])));
            for d in state["drives"].as_array().unwrap_or(&vec![]) {
                println!(
                    "{}: {} — {}",
                    safe(text(&d["name"])),
                    safe(text(&d["mountState"])),
                    safe(text(&d["mountPath"]))
                );
                for f in d["folders"].as_array().unwrap_or(&vec![]) {
                    println!(
                        "  {}{} → {}  {} [{}] {}",
                        if text(&f["label"]).is_empty() {
                            String::new()
                        } else {
                            format!("{} ", safe(text(&f["label"])))
                        },
                        safe(text(&f["path"])),
                        safe(text(&f["local"])),
                        if f["state"] == "paused" && f["localChanges"] == true {
                            "paused, changes waiting".to_owned()
                        } else {
                            safe(text(&f["state"]))
                        },
                        safe(text(&f["id"])),
                        safe(text(&f["attention"]))
                    );
                }
            }
        }
        return Ok(());
    }
    if cmd == "open" {
        let s = snapshot()?;
        let path = if arg(0).is_empty() {
            text(&s["cloudRoot"]).to_owned()
        } else {
            let d = s["drives"]
                .as_array()
                .and_then(|a| a.iter().find(|d| d["name"] == arg(0)))
                .ok_or("Unknown drive")?;
            text(&d["mountPath"]).to_owned()
        };
        for program in ["gio", "xdg-open"] {
            let mut c = Command::new(program);
            if program == "gio" {
                c.arg("open");
            }
            if c.arg(&path).spawn().is_ok() {
                return Ok(());
            }
        }
        return Err("No file opener available".into());
    }
    let (method, args) = match cmd {
        "stop" => {
            if let Err(e) = ipc::connect() {
                if matches!(
                    e.kind(),
                    io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused
                ) {
                    println!("Daemon is not running");
                    return Ok(());
                }
                return Err(e.to_string());
            }
            ("shutdown", json!({}))
        }
        "stream" | "mount" | "unmount" => {
            if cmd == "stream" && !arg(1).is_empty() && !["on", "off"].contains(&arg(1)) {
                return Err("Expected on|off".into());
            }
            (
                "stream",
                json!({"remote":arg(0),"on":cmd!="unmount"&&arg(1)!="off","force":flag("--force")}),
            )
        }
        "stream-all" => {
            if !["on", "off"].contains(&arg(0)) {
                return Err("Expected on|off".into());
            }
            ("stream_all", json!({"on":arg(0)=="on"}))
        }
        "retry" | "takeover" | "reconnect" => (cmd, json!({"remote":arg(0)})),
        "keep" => {
            let (n, p) = split(arg(0))?;
            ("add_folder", json!({"remote":n,"path":p,"local":arg(1)}))
        }
        "unkeep" => (
            "remove_folder",
            json!({"id":folder(arg(0))?,"delete_local":flag("--delete-local")}),
        ),
        "sync" => (
            "sync_now",
            if arg(0).is_empty() {
                json!({})
            } else {
                json!({"id":folder(arg(0))?})
            },
        ),
        "pause" | "resume" => {
            if pos.is_empty() {
                return Err("Specify at least one folder".into());
            }
            for p in &pos {
                let v = request(
                    "pause_folder",
                    json!({"id":folder(p)?,"paused":cmd=="pause"}),
                )?;
                println!("{}", safe(text(&v["message"])));
            }
            return Ok(());
        }
        "alias" => ("set_folder", json!({"id":folder(arg(0))?,"label":arg(1)})),
        "resolve" => ("resolve", json!({"id":folder(arg(0))?,"action":arg(1)})),
        "browse" | "size" => {
            let (n, p) = split(arg(0))?;
            (
                if cmd == "browse" {
                    "browse"
                } else {
                    "folder_size"
                },
                json!({"remote":n,"path":p}),
            )
        }
        "label" => ("set_drive", json!({"remote":arg(0),"label":arg(1)})),
        "set-path" => ("set_drive", json!({"remote":arg(0),"mount_path":arg(1)})),
        "remove" => {
            if !flag("-y")
                && !flag("--yes")
                && prompt(
                    &format!("Remove {}? Local files will remain. Type yes", arg(0)),
                    false,
                )? != "yes"
            {
                return Err("Cancelled".into());
            }
            ("remove_remote", json!({"remote":arg(0)}))
        }
        "config" => {
            if pos.len() < 2 {
                let s = snapshot()?;
                println!(
                    "{}",
                    if arg(0).is_empty() {
                        s["settings"].clone()
                    } else {
                        s["settings"][arg(0)].clone()
                    }
                );
                return Ok(());
            }
            ("set_setting", json!({"key":arg(0),"value":arg(1)}))
        }
        "log" => (
            "log",
            json!({"source":if flag("--daemon"){"daemon"}else{"rclone"},"lines":option("-n").parse::<u64>().or_else(|_|option("--lines").parse()).unwrap_or(80)}),
        ),
        "exclude" => {
            let s = snapshot()?;
            let id = if arg(0).is_empty() {
                String::new()
            } else {
                folder(arg(0))?
            };
            let mut patterns = s["filters"]["excludes"]
                .as_array()
                .cloned()
                .unwrap_or_default();
            if !id.is_empty() {
                for d in s["drives"].as_array().unwrap_or(&vec![]) {
                    for f in d["folders"].as_array().unwrap_or(&vec![]) {
                        if f["id"] == id && f["excludes"].as_array().is_some_and(|a| !a.is_empty())
                        {
                            patterns = f["excludes"].as_array().unwrap().clone();
                        }
                    }
                }
            }
            if !flag("--global") && !flag("--add") && !flag("--remove") {
                println!(
                    "{}",
                    if id.is_empty() {
                        "Global never-synced patterns"
                    } else {
                        "Folder rules (own list if configured; otherwise global)"
                    }
                );
                for p in patterns {
                    println!("{}", safe(text(&p)));
                }
                return Ok(());
            }
            for p in opts.get("--add").into_iter().flatten() {
                if !patterns.contains(&json!(p)) {
                    patterns.push(json!(p));
                }
            }
            for p in opts.get("--remove").into_iter().flatten() {
                patterns.retain(|v| v != p);
            }
            if flag("--global") {
                patterns.clear();
            }
            ("set_excludes", json!({"id":id,"patterns":patterns}))
        }
        "add-credentials" => {
            let mut bytes = String::new();
            io::stdin()
                .take(1024 * 1024 + 1)
                .read_to_string(&mut bytes)
                .map_err(|e| e.to_string())?;
            if bytes.len() > 1024 * 1024 {
                return Err("Credentials input too large".into());
            }
            let options: Value =
                serde_json::from_str(&bytes).map_err(|_| "Invalid credentials JSON")?;
            (
                "add_credentials",
                json!({"name":arg(0),"provider":arg(1),"options":options,"stream":!flag("--no-stream"),"mount_path":option("--mount-path")}),
            )
        }
        "add-oauth" | "set-client-id" => {
            let kind = if cmd == "add-oauth" {
                arg(1).to_owned()
            } else {
                let s = snapshot()?;
                s["drives"]
                    .as_array()
                    .and_then(|a| a.iter().find(|d| d["name"] == arg(0)))
                    .map(|d| text(&d["type"]).to_owned())
                    .ok_or("Unknown drive")?
            };
            let (id, secret) = if flag("--own-client")
                || (cmd == "set-client-id" && option("--client-id").is_empty())
            {
                own_client(&kind)?
            } else {
                let id = option("--client-id").to_owned();
                let secret = if !id.is_empty() || flag("--client-secret-stdin") {
                    prompt("Client secret", true)?
                } else {
                    String::new()
                };
                (id, secret)
            };
            (
                if cmd == "add-oauth" {
                    "add_oauth"
                } else {
                    "set_client_id"
                },
                json!({"name":arg(0),"remote":arg(0),"provider":kind,"client_id":id,"client_secret":secret,"stream":!flag("--no-stream"),"mount_path":option("--mount-path")}),
            )
        }
        _ => return Err(format!("Unknown command: {cmd}; use --help")),
    };
    let data = request(method, args)?;
    if cmd == "browse" {
        for d in data["dirs"].as_array().unwrap_or(&vec![]) {
            println!(
                "{}{}",
                safe(text(&d["path"])),
                if d["kept"] == true {
                    "  [kept local]"
                } else {
                    ""
                }
            );
        }
    } else if cmd == "log" {
        for l in data["lines"].as_array().unwrap_or(&vec![]) {
            println!("{}", safe(text(l)));
        }
    } else if !text(&data["message"]).is_empty() {
        println!("{}", safe(text(&data["message"])));
    } else {
        println!("{}", safe(&data.to_string()));
    }
    Ok(())
}
