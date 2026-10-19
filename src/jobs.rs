use crate::{daemon::Event, filters, rc, storage::*};
use serde_json::{Value, json};
use std::{
    collections::{HashMap, VecDeque},
    fs,
    io::Read,
    path::{Path, PathBuf},
    time::Duration,
};
use tokio::sync::{mpsc, watch};
pub fn notify(cfg: &Value, title: &str, body: &str) {
    if cfg["notifications"] == false {
        return;
    }
    let mut c = tokio::process::Command::new("notify-send");
    c.args([
        "--app-name=omarchy-cloudkit",
        "--",
        &safe(title),
        &safe(body),
    ])
    .stdin(std::process::Stdio::null())
    .stdout(std::process::Stdio::null())
    .stderr(std::process::Stdio::null());
    tokio::spawn(async move {
        if let Ok(mut p) = c.spawn() {
            let _ = tokio::time::timeout(Duration::from_secs(5), p.wait()).await;
        }
    });
}
pub fn mount_present(point: &Path) -> Option<String> {
    let canonical = point
        .parent()
        .and_then(|p| p.canonicalize().ok())
        .map(|p| p.join(point.file_name().unwrap_or_default()))
        .unwrap_or_else(|| point.to_owned());
    let target = canonical.to_string_lossy();
    let raw = fs::read_to_string("/proc/self/mountinfo").ok()?;
    for line in raw.lines() {
        let fields: Vec<_> = line.split_whitespace().collect();
        if fields.len() < 7 {
            continue;
        }
        let decoded = fields[4]
            .replace("\\040", " ")
            .replace("\\011", "\t")
            .replace("\\012", "\n")
            .replace("\\134", "\\");
        if decoded == target {
            return Some(line.to_owned());
        }
    }
    None
}
pub async fn unmount(rcc: &rc::Rc, point: &Path, force: bool, name: &str) -> Result<()> {
    if !force {
        let st = rcc
            .call_timeout("vfs/stats", json!({"fs":format!("{name}:")}), 5)
            .await?;
        let dc = &st["diskCache"];
        if dc["uploadsInProgress"].as_u64().unwrap_or(0) + dc["uploadsQueued"].as_u64().unwrap_or(0)
            > 0
        {
            return Err("Files are still uploading; wait or use Unmount anyway".into());
        }
    }
    let result = rcc
        .call_timeout("mount/unmount", json!({"mountPoint":point}), 30)
        .await;
    match result {
        Ok(_) => (),
        Err(e) if !force => return Err(e),
        Err(_) => {
            let status = tokio::process::Command::new("fusermount3")
                .args(["-uz", "--"])
                .arg(point)
                .status()
                .await
                .map_err(|e| e.to_string())?;
            if !status.success() {
                return Err("Could not unmount".into());
            }
        }
    }
    let p = point.to_owned();
    let _ = tokio::task::spawn_blocking(move || fs::remove_dir(p)).await;
    Ok(())
}
pub async fn mount(
    rcc: &rc::Rc,
    name: &str,
    point: &Path,
    cfg: &Value,
    takeover: bool,
) -> Result<()> {
    let probe_point = point.to_owned();
    let existing = tokio::time::timeout(
        Duration::from_secs(5),
        tokio::task::spawn_blocking(move || mount_present(&probe_point)),
    )
    .await
    .map_err(|_| "Mount path did not respond")?
    .map_err(|e| e.to_string())?;
    if existing.is_some() {
        let p = point.to_owned();
        let alive = matches!(
            tokio::time::timeout(
                Duration::from_secs(3),
                tokio::task::spawn_blocking(move || fs::read_dir(p)
                    .and_then(|mut it| match it.next() {
                        Some(Err(e)) => Err(e),
                        _ => Ok(()),
                    })
                    .is_ok())
            )
            .await,
            Ok(Ok(true))
        );
        if !takeover && alive {
            return Err("Already mounted by another program".into());
        }
        let status = tokio::process::Command::new("fusermount3")
            .args([if alive { "-u" } else { "-uz" }, "--"])
            .arg(point)
            .status()
            .await
            .map_err(|e| e.to_string())?;
        if !status.success() {
            let _ = tokio::process::Command::new("fusermount3")
                .args(["-uz", "--"])
                .arg(point)
                .status()
                .await;
        }
    }
    let p = point.to_owned();
    tokio::task::spawn_blocking(move || -> Result<()> {
        fs::create_dir_all(&p).map_err(|e| e.to_string())?;
        if fs::read_dir(&p)
            .map_err(|e| e.to_string())?
            .next()
            .is_some()
        {
            return Err("Mount folder must be empty".into());
        }
        Ok(())
    })
    .await
    .map_err(|e| e.to_string())??;
    let info = rcc
        .call("operations/fsinfo", json!({"fs":format!("{name}:")}))
        .await?;
    let change = info["Features"]["ChangeNotify"] == true;
    rcc.call_timeout("mount/mount",json!({"fs":format!("{name}:"),"mountPoint":point,"vfsOpt":{"CacheMode":"full","CacheMaxSize":format!("{}G",cfg["cache_max_size_gb"].as_u64().unwrap_or(20)),"CacheMaxAge":cfg["cache_max_age"],"CacheMinFreeSpace":format!("{}G",cfg["cache_min_free_gb"].as_u64().unwrap_or(5)),"DirCacheTime":if change{"168h"}else{"10m"},"PollInterval":"30s","FastFingerprint":true,"ReadAhead":"16M","WriteBack":"5s"},"mountOpt":{"AttrTimeout":"5s","VolumeName":name,"AsyncRead":true}}),90).await?;
    let r = rcc.clone();
    let fs = format!("{name}:");
    tokio::spawn(async move {
        let _ = r
            .call_timeout("vfs/refresh", json!({"fs":fs,"recursive":false}), 30)
            .await;
        if let Ok(list)=r.call_timeout("operations/list",json!({"fs":fs,"remote":"","opt":{"dirsOnly":true,"noModTime":true,"noMimeType":true}}),30).await {
            let mut params=json!({"fs":fs});
            for (i,dir) in list["list"].as_array().unwrap_or(&vec![]).iter().filter(|d|d["IsDir"]==true).take(60).enumerate(){params[format!("dir{i}")]=dir["Path"].clone();}
            if params.as_object().unwrap().len()>1 {let _=r.call_timeout("vfs/refresh",params,120).await;}
        }
    });
    Ok(())
}
pub async fn sync(
    paths: Paths,
    folder: Value,
    kind: String,
    rules: String,
    tx: mpsc::Sender<Event>,
    mut cancel: watch::Receiver<bool>,
) {
    let id = text(&folder["id"]).to_owned();
    let result = sync_inner(&paths, &folder, &kind, &rules, &tx, &mut cancel).await;
    let _ = tx.send(Event::SyncDone(id, result)).await;
}
async fn sync_inner(
    paths: &Paths,
    f: &Value,
    kind: &str,
    rules: &str,
    tx: &mpsc::Sender<Event>,
    cancel: &mut watch::Receiver<bool>,
) -> Result<Value> {
    let id = text(&f["id"]);
    let local = expand(text(&f["local"]));
    if !local.is_dir() {
        if f["initialized"] == true {
            return Err(
                "missing_local: Local folder is missing; restore it or choose Resync".into(),
            );
        }
        fs::create_dir_all(&local).map_err(|e| e.to_string())?;
    }
    private(&paths.work(id))?;
    private(&paths.state.join("l"))?;
    let link = paths.state.join("l").join(id);
    if fs::symlink_metadata(&link).is_ok() {
        if !fs::symlink_metadata(&link)
            .map_err(|e| e.to_string())?
            .file_type()
            .is_symlink()
        {
            return Err("Unsafe bisync link".into());
        }
        let old = fs::read_link(&link).map_err(|e| e.to_string())?;
        if old != local {
            fs::remove_file(&link).map_err(|e| e.to_string())?;
            std::os::unix::fs::symlink(&local, &link).map_err(|e| e.to_string())?;
        }
    } else {
        std::os::unix::fs::symlink(&local, &link).map_err(|e| e.to_string())?;
    }
    let filter_path = paths.state.join(format!("filters-{id}.txt"));
    atomic(&filter_path, rules.as_bytes())?;
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
        / 86400;
    let backup = paths.backup(id).join(format!("day-{stamp}"));
    private(&backup)?;
    let mut params = json!({"path1":rc::spec(text(&f["remote"]),kind,text(&f["path"])),"path2":link,"workdir":paths.work(id),"filtersFile":filter_path,"createEmptySrcDirs":true,"compare":"size,modtime","conflictResolve":"newer","conflictLoser":"num","conflictSuffix":"conflict","resilient":true,"recover":true,"maxLock":"5m","maxDelete":50,"backupDir2":backup,"_async":true,"_group":format!("cloudkit/{id}"),"_config":{"SuffixKeepExtension":true}});
    if f["run_resync"] == true {
        params["resync"] = json!(true);
        params["resyncMode"] = json!("newer");
    }
    if f["force_next"] == true {
        params["force"] = json!(true);
    }
    let (mut child, rcc) =
        rc::launch(&paths.sync_socket(id), &rc::log_path(&paths.state), false).await?;
    let run = async {
        let job = rcc.call_timeout("sync/bisync", params, 30).await?["jobid"]
            .as_u64()
            .ok_or("Missing bisync job id")?;
        loop {
            let js = rcc
                .call_timeout("job/status", json!({"jobid":job}), 5)
                .await?;
            if js["finished"] == true {
                let stats = rcc
                    .call_timeout("core/stats", json!({"group":format!("cloudkit/{id}")}), 5)
                    .await
                    .unwrap_or(json!({}));
                return Ok(json!({"job":js,"stats":stats}));
            }
            let stats = rcc
                .call_timeout("core/stats", json!({"group":format!("cloudkit/{id}")}), 5)
                .await
                .unwrap_or(json!({}));
            let _ = tx.send(Event::SyncProgress(id.to_owned(), stats)).await;
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    };
    let result = tokio::select! {result=run=>result,_=cancel.changed()=>Err("Sync cancelled; existing state will be recovered on the next run".into())};
    rc::stop(&mut child).await;
    let _ = fs::remove_file(paths.sync_socket(id));
    result
}
pub struct Watcher {
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
}
impl Drop for Watcher {
    fn drop(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::Relaxed);
    }
}
pub fn watch_folder(id: String, root: PathBuf, rules: String, tx: mpsc::Sender<Event>) -> Watcher {
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let flag = stop.clone();
    std::thread::spawn(move || {
        let result = watch_loop(&id, &root, &rules, &tx, &flag);
        if let Err(e) = result {
            let _ = tx.blocking_send(Event::WatchError(id, e));
        }
    });
    Watcher { stop }
}
fn watch_loop(
    id: &str,
    root: &Path,
    rules: &str,
    tx: &mpsc::Sender<Event>,
    stop: &std::sync::atomic::AtomicBool,
) -> Result<()> {
    use std::os::fd::{AsRawFd, FromRawFd};
    use std::os::unix::ffi::OsStrExt;
    let fd = unsafe { libc::inotify_init1(libc::IN_NONBLOCK | libc::IN_CLOEXEC) };
    if fd < 0 {
        return Err(std::io::Error::last_os_error().to_string());
    }
    let mut file = unsafe { fs::File::from_raw_fd(fd) };
    let matcher = filters::Matcher::new(rules);
    let mut map = HashMap::new();
    let mut watched = std::collections::HashSet::new();
    let mut queue = VecDeque::from([root.to_owned()]);
    let mut buffer = vec![0u8; 65536];
    while !stop.load(std::sync::atomic::Ordering::Relaxed) {
        while let Some(dir) = queue.pop_front() {
            if !watched.insert(dir.clone()) {
                continue;
            }
            if map.len() >= 100000 {
                return Err("Too many directories to watch".into());
            }
            let relative = dir
                .strip_prefix(root)
                .unwrap_or(&dir)
                .to_string_lossy()
                .replace('\\', "/");
            if !relative.is_empty() && matcher.ignored(&relative, true) {
                continue;
            }
            let Ok(c) = std::ffi::CString::new(dir.as_os_str().as_bytes()) else {
                continue;
            };
            let wd = unsafe {
                libc::inotify_add_watch(
                    file.as_raw_fd(),
                    c.as_ptr(),
                    libc::IN_CLOSE_WRITE
                        | libc::IN_CREATE
                        | libc::IN_DELETE
                        | libc::IN_MOVED_FROM
                        | libc::IN_MOVED_TO
                        | libc::IN_DELETE_SELF
                        | libc::IN_MOVE_SELF
                        | libc::IN_ONLYDIR
                        | libc::IN_DONT_FOLLOW,
                )
            };
            if wd < 0 {
                if dir == root && !dir.exists() {
                    watched.remove(&dir);
                    continue;
                }
                return Err(std::io::Error::last_os_error().to_string());
            }
            map.insert(wd, dir.clone());
            if let Ok(entries) = fs::read_dir(&dir) {
                for e in entries.flatten() {
                    if e.file_type().is_ok_and(|t| t.is_dir()) {
                        queue.push_back(e.path());
                    }
                }
            }
        }
        let mut dirty = false;
        match file.read(&mut buffer) {
            Ok(n) => {
                let mut at = 0;
                while at + 16 <= n {
                    let wd = i32::from_ne_bytes(buffer[at..at + 4].try_into().unwrap());
                    let mask = u32::from_ne_bytes(buffer[at + 4..at + 8].try_into().unwrap());
                    let len =
                        u32::from_ne_bytes(buffer[at + 12..at + 16].try_into().unwrap()) as usize;
                    if at + 16 + len > n {
                        break;
                    }
                    let bytes = &buffer[at + 16..at + 16 + len];
                    let name = String::from_utf8_lossy(
                        bytes.split(|b| *b == 0).next().unwrap_or_default(),
                    );
                    if mask & libc::IN_Q_OVERFLOW != 0 {
                        let _ = tx.blocking_send(Event::Changed(id.to_owned()));
                        return Err("File watcher overflow; periodic sync remains available while watches are rebuilt".into());
                    }
                    if let Some(dir) = map.get(&wd) {
                        let path = dir.join(name.as_ref());
                        let rel = path
                            .strip_prefix(root)
                            .unwrap_or(&path)
                            .to_string_lossy()
                            .replace('\\', "/");
                        let is_dir = mask & libc::IN_ISDIR != 0;
                        if is_dir && mask & (libc::IN_CREATE | libc::IN_MOVED_TO) != 0 {
                            queue.push_back(path.clone());
                        }
                        if !matcher.ignored(&rel, is_dir) {
                            dirty = true;
                        }
                        if is_dir && mask & (libc::IN_MOVED_FROM | libc::IN_DELETE) != 0 {
                            let removed = map
                                .iter()
                                .filter(|(_, p)| p.starts_with(&path))
                                .map(|(wd, _)| *wd)
                                .collect::<Vec<_>>();
                            for wd in removed {
                                if let Some(p) = map.remove(&wd) {
                                    watched.remove(&p);
                                }
                                unsafe {
                                    libc::inotify_rm_watch(file.as_raw_fd(), wd);
                                }
                            }
                        }
                    }
                    if mask & libc::IN_IGNORED != 0
                        && let Some(p) = map.remove(&wd)
                    {
                        watched.remove(&p);
                    }
                    at += 16 + len;
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => (),
            Err(e) => return Err(e.to_string()),
        }
        if dirty {
            let _ = tx.blocking_send(Event::Changed(id.to_owned()));
        }
        if map.is_empty() && root.is_dir() {
            queue.push_back(root.to_owned());
        }
        let mut pollfd = libc::pollfd {
            fd: file.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        unsafe {
            libc::poll(&mut pollfd, 1, 1000);
        }
    }
    Ok(())
}
pub fn recent_and_backups(
    folders: Vec<Value>,
    drives: Vec<(String, Value)>,
    backup_root: PathBuf,
    days: u64,
) -> (Value, u64) {
    let mut files = vec![];
    for f in folders {
        let root = PathBuf::from(text(&f["local"]));
        for (stamp, path, size) in newest(&root, true) {
            let rel = path.strip_prefix(&root).unwrap_or(&path);
            let folder = rel.parent().unwrap_or(Path::new(""));
            let shown = Path::new(text(&f["path"])).join(folder);
            files.push(json!({"name":path.file_name().unwrap_or_default().to_string_lossy(),"path":path,"folder":if shown.as_os_str().is_empty(){"/".to_owned()}else{shown.to_string_lossy().to_string()},"remote":f["remote"],"where":"local","modifiedTs":stamp,"sizeBytes":size}));
        }
    }
    for (name, d) in drives {
        let meta = PathBuf::from(text(&d["meta_path"]));
        if meta.as_os_str().is_empty() {
            continue;
        }
        let data = PathBuf::from(text(&d["data_path"]));
        for (stamp, path, _) in newest(&meta, false) {
            let rel = path.strip_prefix(&meta).unwrap_or(&path);
            if rel
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .starts_with('.')
            {
                continue;
            }
            let folder = rel.parent().unwrap_or(Path::new(""));
            let size = fs::metadata(data.join(rel)).map_or(0, |m| m.len());
            files.push(json!({"name":rel.file_name().unwrap_or_default().to_string_lossy(),"path":Path::new(text(&d["mount_point"])).join(rel),"folder":if folder.as_os_str().is_empty(){"/".to_owned()}else{folder.to_string_lossy().to_string()},"remote":name,"where":"stream","modifiedTs":stamp,"sizeBytes":size}));
        }
    }
    files.sort_by(|a, b| b["modifiedTs"].as_u64().cmp(&a["modifiedTs"].as_u64()));
    let mut seen = std::collections::HashSet::new();
    files.retain(|f| seen.insert(text(&f["path"]).to_owned()));
    files.truncate(12);
    let mut count = 0;
    let mut queue = VecDeque::from([(backup_root, 0)]);
    while let Some((dir, depth)) = queue.pop_front() {
        if let Ok(entries) = fs::read_dir(dir) {
            for e in entries.flatten() {
                if e.file_type().is_ok_and(|t| t.is_dir()) {
                    let n = e.file_name();
                    let n = n.to_string_lossy();
                    let dated = n.starts_with("day-")
                        || (n.len() == 10 && n.as_bytes()[4] == b'-' && n.as_bytes()[7] == b'-');
                    if depth == 1
                        && days > 0
                        && dated
                        && e.metadata()
                            .and_then(|m| m.modified())
                            .ok()
                            .and_then(|t| t.elapsed().ok())
                            .is_some_and(|d| d.as_secs() > days.saturating_mul(86400))
                    {
                        let _ = fs::remove_dir_all(e.path());
                    } else {
                        queue.push_back((e.path(), depth + 1));
                    }
                } else {
                    count += 1;
                }
            }
        }
    }
    (json!(files), count)
}
fn newest(root: &Path, skip_hidden: bool) -> Vec<(u64, PathBuf, u64)> {
    let mut heap = std::collections::BinaryHeap::<std::cmp::Reverse<(u64, PathBuf, u64)>>::new();
    let mut queue = VecDeque::from([root.to_owned()]);
    let mut budget = 20000;
    while let Some(dir) = queue.pop_front() {
        if budget == 0 {
            break;
        }
        if let Ok(entries) = fs::read_dir(dir) {
            for e in entries.flatten() {
                if budget == 0 {
                    break;
                }
                budget -= 1;
                if skip_hidden && e.file_name().to_string_lossy().starts_with('.') {
                    continue;
                }
                let Ok(t) = e.file_type() else { continue };
                if t.is_symlink() {
                    continue;
                }
                if t.is_dir() {
                    queue.push_back(e.path());
                    continue;
                }
                if !t.is_file() {
                    continue;
                }
                let Ok(m) = e.metadata() else { continue };
                let stamp = m
                    .modified()
                    .ok()
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map_or(0, |d| d.as_secs());
                heap.push(std::cmp::Reverse((stamp, e.path(), m.len())));
                if heap.len() > 12 {
                    heap.pop();
                }
            }
        }
    }
    heap.into_iter().map(|v| v.0).collect()
}
pub fn count_backups(root: &Path) -> u64 {
    let mut count = 0;
    let mut queue = VecDeque::from([root.to_owned()]);
    while let Some(dir) = queue.pop_front() {
        if let Ok(entries) = fs::read_dir(dir) {
            for e in entries.flatten() {
                if e.file_type().is_ok_and(|t| t.is_dir()) {
                    queue.push_back(e.path());
                } else {
                    count += 1;
                }
            }
        }
    }
    count
}
pub fn has_route() -> bool {
    let v4 = fs::read_to_string("/proc/net/route").ok();
    let v6 = fs::read_to_string("/proc/net/ipv6_route").ok();
    if v4.as_deref().is_some_and(|s| {
        s.lines().skip(1).any(|l| {
            let f: Vec<_> = l.split_whitespace().collect();
            f.len() > 3
                && f[0] != "lo"
                && f[1] == "00000000"
                && u32::from_str_radix(f[3], 16).is_ok_and(|flags| flags & 1 != 0)
        })
    }) {
        return true;
    }
    if v6.as_deref().is_some_and(|s| {
        s.lines().any(|l| {
            let f: Vec<_> = l.split_whitespace().collect();
            f.len() > 9
                && f[9] != "lo"
                && f[0] == "00000000000000000000000000000000"
                && f[1] == "00"
        })
    }) {
        return true;
    }
    // Treat unavailable route information as unknown rather than declaring offline.
    v4.is_none() && v6.is_none()
}
pub async fn browse(
    rcc: rc::Rc,
    name: String,
    path: String,
    point: Option<PathBuf>,
    local: PathBuf,
    kept: Vec<String>,
) -> Result<Value> {
    let mut entries = None;
    if let Some(point) = point {
        let p = point.join(&path);
        if let Ok(Ok(Ok(dirs))) = tokio::time::timeout(
            Duration::from_secs(20),
            tokio::task::spawn_blocking(move || -> std::io::Result<Vec<Value>> {
                let mut out = vec![];
                for e in fs::read_dir(p)? {
                    let e = e?;
                    let name = e.file_name().to_string_lossy().to_string();
                    if e.file_type()?.is_dir() && !name.starts_with('.') {
                        out.push(json!({"Name":name,"IsDir":true}));
                        if out.len() >= 2000 {
                            break;
                        }
                    }
                }
                Ok(out)
            }),
        )
        .await
        {
            entries = Some(dirs);
        }
    }
    let mut dirs=match entries{Some(dirs)=>dirs,None=>rcc.call("operations/list",json!({"fs":format!("{name}:"),"remote":path,"opt":{"dirsOnly":true,"noModTime":true,"noMimeType":true}})).await?["list"].as_array().cloned().unwrap_or_default()};
    dirs.retain(|e| e["IsDir"] == true && !text(&e["Name"]).starts_with('.'));
    dirs.sort_by_key(|e| text(&e["Name"]).to_lowercase());
    let truncated = dirs.len() >= 2000;
    dirs.truncate(2000);
    let dirs=dirs.iter().map(|e|{let n=text(&e["Name"]);let p=if path.is_empty(){n.to_owned()}else{format!("{path}/{n}")};json!({"name":safe(n),"path":p,"kept":kept.iter().any(|k|k.is_empty()||*k==p||p.starts_with(&format!("{k}/"))),"containsKept":kept.iter().any(|k|k.starts_with(&format!("{p}/")))})}).collect::<Vec<_>>();
    Ok(
        json!({"remote":name,"path":path,"dirs":dirs,"keptHere":kept.iter().any(|k|k.is_empty()||*k==path||path.starts_with(&format!("{k}/"))),"containsKept":kept.iter().any(|k|if path.is_empty(){!k.is_empty()}else{k.starts_with(&format!("{path}/"))}),"suggestedLocal":local,"truncated":truncated}),
    )
}
