use serde_json::{Value, json};
use std::{
    env, fs,
    io::{self, Write},
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::{Component, Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};
pub type Result<T> = std::result::Result<T, String>;
pub fn text(v: &Value) -> &str {
    v.as_str().unwrap_or("")
}
pub fn now() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64()
}
pub fn home() -> PathBuf {
    env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/"))
}
fn xdg(key: &str, fallback: &str) -> PathBuf {
    env::var_os(key)
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| home().join(fallback))
}
#[derive(Clone)]
pub struct Paths {
    pub config: PathBuf,
    pub state: PathBuf,
    pub runtime: PathBuf,
    pub legacy: bool,
}
impl Paths {
    pub fn new() -> Self {
        let base = xdg("XDG_CONFIG_HOME", ".config");
        let legacy = base.join("guacamole/config.json").exists()
            && !base.join("omarchy-cloudkit/config.json").exists();
        let app = if legacy {
            "guacamole"
        } else {
            "omarchy-cloudkit"
        };
        let fallback = PathBuf::from(format!("/tmp/{app}-{}", unsafe { libc::getuid() }));
        let runtime = env::var_os("XDG_RUNTIME_DIR")
            .filter(|s| !s.is_empty())
            .map(|p| PathBuf::from(p).join(app))
            .unwrap_or_else(|| fallback.clone());
        let runtime = if runtime.as_os_str().len() + b"/sync-00000000.sock".len() > 100 {
            fallback
        } else {
            runtime
        };
        Self {
            config: base.join(app),
            state: xdg("XDG_STATE_HOME", ".local/state").join(app),
            runtime,
            legacy,
        }
    }
    pub fn socket(&self) -> PathBuf {
        self.runtime.join("daemon.sock")
    }
    pub fn rc(&self) -> PathBuf {
        self.runtime.join("rclone.sock")
    }
    pub fn filter(&self) -> PathBuf {
        self.config.join("filters.txt")
    }
    pub fn sync_socket(&self, id: &str) -> PathBuf {
        self.runtime.join(format!("sync-{id}.sock"))
    }
    pub fn work(&self, id: &str) -> PathBuf {
        self.state.join("bisync").join(id)
    }
    pub fn backup(&self, id: &str) -> PathBuf {
        self.state.join("backups").join(id)
    }
    pub fn unit(&self) -> PathBuf {
        xdg("XDG_CONFIG_HOME", ".config").join("systemd/user/omarchy-cloudkit.service")
    }
}
pub fn expand(raw: &str) -> PathBuf {
    let p = if raw == "~" {
        home()
    } else if let Some(s) = raw.strip_prefix("~/") {
        home().join(s)
    } else {
        PathBuf::from(raw)
    };
    let p = if p.is_absolute() {
        p
    } else {
        env::current_dir().unwrap_or_else(|_| home()).join(p)
    };
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => (),
            _ => out.push(c.as_os_str()),
        }
    }
    out
}
pub fn overlaps(a: &Path, b: &Path) -> bool {
    a.starts_with(b) || b.starts_with(a)
}
pub fn dedicated(path: &Path) -> Result<()> {
    if path == Path::new("/")
        || path == home()
        || [
            "/usr", "/etc", "/bin", "/sbin", "/lib", "/lib64", "/boot", "/proc", "/sys", "/dev",
            "/run", "/var", "/opt", "/srv", "/root",
        ]
        .iter()
        .any(|p| path.starts_with(p))
    {
        Err("Pick a dedicated folder, not your home or a system directory".into())
    } else {
        Ok(())
    }
}
pub fn private(path: &Path) -> Result<()> {
    fs::create_dir_all(path).map_err(|e| e.to_string())?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).map_err(|e| e.to_string())
}
pub fn atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path.parent().ok_or("Missing parent")?;
    private(parent)?;
    let mut temp = parent.join(format!(
        ".cloudkit-{}-{:x}.tmp",
        std::process::id(),
        (now() * 1e9) as u128
    ));
    let mut f = None;
    for _ in 0..100 {
        match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temp)
        {
            Ok(x) => {
                f = Some(x);
                break;
            }
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
                temp.set_extension(format!("{:x}", new_id()));
            }
            Err(e) => return Err(e.to_string()),
        }
    }
    let mut f = f.ok_or("Cannot create private temporary file")?;
    let result = (|| {
        f.write_all(bytes)?;
        f.sync_all()?;
        fs::rename(&temp, path)?;
        fs::File::open(parent)?.sync_all()
    })()
    .map_err(|e: io::Error| e.to_string());
    let _ = fs::remove_file(temp);
    result
}
pub fn write_json(path: &Path, v: &Value) -> Result<()> {
    atomic(path, json_ascii(v, true).as_bytes())
}
pub fn json_ascii(v: &Value, pretty: bool) -> String {
    use std::fmt::Write;
    let raw = if pretty {
        serde_json::to_string_pretty(v).unwrap()
    } else {
        v.to_string()
    };
    let mut out = String::with_capacity(raw.len());
    for c in raw.chars() {
        let n = c as u32;
        if n < 0x7f {
            out.push(c)
        } else if n <= 0xffff {
            let _ = write!(out, "\\u{n:04x}");
        } else {
            let n = n - 0x10000;
            let _ = write!(
                out,
                "\\u{:04x}\\u{:04x}",
                0xd800 + (n >> 10),
                0xdc00 + (n & 0x3ff)
            );
        }
    }
    out
}
pub fn read_json(path: &Path) -> Result<Option<Value>> {
    match fs::read(path) {
        Ok(b) => {
            let v: Value = serde_json::from_slice(&b).map_err(|_| {
                format!(
                    "Invalid JSON in {}; existing file preserved",
                    path.display()
                )
            })?;
            if !v.is_object() {
                return Err("Configuration must be an object".into());
            }
            Ok(Some(v))
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.to_string()),
    }
}
pub fn new_id() -> u32 {
    let mut b = [0; 4];
    use std::io::Read;
    if fs::File::open("/dev/urandom")
        .and_then(|mut f| f.read_exact(&mut b))
        .is_ok()
    {
        u32::from_ne_bytes(b)
    } else {
        (now() * 1e6) as u32
    }
}
pub fn defaults() -> Value {
    json!({"version":1,"mount_root":"~/Cloud/Stream","local_root":"~/Cloud/Sync","cache_max_size_gb":20,"cache_max_age":"720h","cache_min_free_gb":5,"sync_interval_min":5,"notifications":true,"backup_days":30,"remotes":{}})
}
pub const SETTINGS: &[&str] = &[
    "mount_root",
    "local_root",
    "cache_max_size_gb",
    "cache_max_age",
    "cache_min_free_gb",
    "sync_interval_min",
    "notifications",
    "backup_days",
];
pub fn coerce(key: &str, value: &Value) -> Result<Value> {
    if !SETTINGS.contains(&key) {
        return Err("Unknown setting".into());
    }
    if key == "notifications" {
        return match value {
            Value::Bool(b) => Ok(json!(b)),
            _ => match text(value).to_lowercase().as_str() {
                "true" | "yes" | "on" | "1" => Ok(json!(true)),
                "false" | "no" | "off" | "0" => Ok(json!(false)),
                _ => Err("notifications must be true or false".into()),
            },
        };
    }
    if defaults()[key].is_number() {
        let n = value
            .as_u64()
            .or_else(|| text(value).parse().ok())
            .ok_or("Expected a non-negative integer")?;
        if key == "sync_interval_min" && n < 1 {
            return Err("sync_interval_min must be at least 1".into());
        }
        return Ok(json!(n));
    }
    if text(value).trim().is_empty() {
        return Err("Value cannot be empty".into());
    }
    Ok(json!(text(value).trim()))
}
pub fn load(paths: &Paths) -> Result<(Value, Value)> {
    private(&paths.config)?;
    private(&paths.state)?;
    let raw = read_json(&paths.config.join("config.json"))?;
    let fresh = raw.is_none();
    let mut cfg = defaults();
    if let Some(raw) = raw {
        for key in SETTINGS {
            if let Ok(v) = coerce(key, &raw[key]) {
                cfg[key] = v;
            }
        }
        if raw["remotes"].is_object() {
            cfg["remotes"] = raw["remotes"].clone();
        }
    }
    for (_, remote) in cfg["remotes"].as_object_mut().ok_or("Invalid remotes")? {
        if !remote.is_object() {
            *remote = json!({});
        }
        if !remote["folders"].is_array() {
            remote["folders"] = json!([]);
        }
        for f in remote["folders"].as_array_mut().unwrap() {
            if !f.is_object() || !f["local"].is_string() {
                return Err("Invalid saved folder; configuration preserved".into());
            }
            if text(&f["id"]).is_empty() {
                f["id"] = json!(format!("{:08x}", new_id()));
            }
            if !text(&f["id"])
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
            {
                return Err("Unsafe folder id".into());
            }
        }
    }
    if fresh {
        write_json(&paths.config.join("config.json"), &cfg)?;
    }
    if fs::symlink_metadata(paths.filter()).is_err() {
        let mut f = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(paths.filter())
            .map_err(|e| e.to_string())?;
        f.write_all(include_bytes!("../assets/default-filters.txt"))
            .map_err(|e| e.to_string())?;
    }
    let mut state = read_json(&paths.state.join("state.json"))?.unwrap_or(json!({}));
    for key in ["folders", "quota"] {
        if !state[key].is_object() {
            state[key] = json!({})
        }
    }
    Ok((cfg, state))
}
pub fn clean_path(raw: &str) -> Result<String> {
    let raw = raw.replace('\\', "/");
    let parts: Vec<_> = raw
        .split('/')
        .filter(|p| !p.is_empty() && *p != ".")
        .collect();
    if parts.contains(&"..") || raw.contains('\0') {
        return Err("Paths cannot contain '..' or NUL".into());
    }
    Ok(parts.join("/"))
}
pub fn clean_name(raw: &str) -> Result<String> {
    let s: String = raw
        .chars()
        .filter(|c| c.is_alphanumeric() || *c == '-' || *c == '_')
        .collect();
    if s.is_empty() || s.starts_with('-') {
        Err("Give the drive a name (letters, digits, '-' and '_'; no leading '-')".into())
    } else {
        Ok(s)
    }
}
pub fn safe(raw: &str) -> String {
    raw.chars().map(|c| { let n=c as u32; if n<0x20 { char::from_u32(0x2400+n).unwrap().to_string() } else if n==0x7f { "␡".into() } else if n<=0x9f && c.is_control() { format!("\\x{n:02x}") } else if matches!(c,'\u{061c}'|'\u{200e}'|'\u{200f}'|'\u{2028}'..='\u{202e}'|'\u{2066}'..='\u{2069}') { format!("\\u{n:04x}") } else { c.to_string() } }).collect()
}
pub fn classify(raw: &str) -> &'static str {
    let s = raw.to_lowercase();
    if s.starts_with("bad_client:") {
        return "bad_client";
    }
    if s.contains("still uploading") {
        return "uploads_pending";
    }
    if s.contains("resource busy") || s.contains("device busy") {
        return "busy";
    }
    if s.contains("already mounted by another") {
        return "foreign";
    }
    static AUTH: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    static RATE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    static NETWORK: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    if AUTH.get_or_init(||regex::Regex::new(r"(?i)invalid_grant|token expired|unauthorized|\b401\b|invalid_client|couldn't fetch token|re-authenticate|reauthorize|authentication failed|access denied|invalid credentials").unwrap()).is_match(raw){"auth"}
 else if RATE.get_or_init(||regex::Regex::new(r"(?i)ratelimitexceeded|rate limit|too many requests|\b429\b|userratelimit|quota exceeded").unwrap()).is_match(raw){"rate"}
 else if NETWORK.get_or_init(||regex::Regex::new(r"(?i)network|connection|timeout|timed out|no such host|temporary failure|unreachable|dial tcp|context deadline exceeded|tls handshake timeout|server misbehaving|no route to host|eof$").unwrap()).is_match(raw){"network"}else{"other"}
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn untrusted_text_is_visible_but_paths_are_not_mutated() {
        let s = "a\x1b]52;c;fake\x07\u{009d}\u{202e}日本語";
        assert_eq!(safe(s), "a␛]52;c;fake␇\\x9d\\u202e日本語");
        assert!(clean_path("../bad").is_err());
        assert_eq!(clean_path("a/./b").unwrap(), "a/b");
    }
    #[test]
    fn error_categories_match_recovery_policy() {
        for (s, k) in [
            ("invalid_grant", "auth"),
            ("UserRateLimitExceeded", "rate"),
            ("critical error: EOF", "network"),
            ("context deadline exceeded", "network"),
            ("Device or resource busy", "busy"),
            ("Files are still uploading", "uploads_pending"),
        ] {
            assert_eq!(classify(s), k);
        }
    }
    #[test]
    fn invalid_json_is_never_overwritten() {
        let root = std::env::temp_dir().join(format!("ck-config-{:08x}", new_id()));
        private(&root).unwrap();
        let p = root.join("config.json");
        fs::write(&p, b"invalid").unwrap();
        assert!(read_json(&p).is_err());
        assert_eq!(fs::read(&p).unwrap(), b"invalid");
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn atomic_json_replaces_a_symlink_without_touching_its_target() {
        let root = std::env::temp_dir().join(format!("ck-atomic-{:08x}", new_id()));
        private(&root).unwrap();
        let target = root.join("target");
        fs::write(&target, "keep").unwrap();
        let dest = root.join("config.json");
        std::os::unix::fs::symlink(&target, &dest).unwrap();
        write_json(&dest, &json!({"safe":true})).unwrap();
        assert_eq!(fs::read_to_string(&target).unwrap(), "keep");
        assert!(
            !fs::symlink_metadata(&dest)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(
            fs::metadata(&dest).unwrap().permissions().mode() & 0o777,
            0o600
        );
        fs::remove_dir_all(root).unwrap();
    }
}
