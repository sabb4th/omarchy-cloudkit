use crate::{
    auth, filters, ipc, jobs,
    model::Model,
    providers,
    rc::{self, Rc},
    storage::*,
};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    fs,
    os::{fd::AsRawFd, unix::fs::OpenOptionsExt},
    path::PathBuf,
    time::Duration,
};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    net::{UnixListener, UnixStream},
    sync::{mpsc, oneshot, watch},
};
pub type Reply = oneshot::Sender<Result<Value>>;
pub enum Event {
    Request(String, Value, Reply),
    EngineReady(Rc, String),
    EngineLost(String),
    Remotes(Result<Value>),
    Changed(String),
    WatchError(String, String),
    SyncProgress(String, Value),
    SyncDone(String, Result<Value>),
    Mount(String, PathBuf, bool, Result<()>, Option<Reply>),
    Metrics(String, Value),
    Quota(String, Value),
    Recent(Value, u64),
    AuthUrl(String),
    AuthDone(Result<Value>, Reply),
    EngineStats(Value),
    BackupCount(u64),
}
struct Core {
    m: Model,
    tx: mpsc::Sender<Event>,
    push: watch::Sender<Value>,
    rc: Option<Rc>,
    syncs: BTreeMap<String, watch::Sender<bool>>,
    watchers: BTreeMap<String, (String, jobs::Watcher)>,
    mounting: Vec<String>,
    auth_cancel: Option<watch::Sender<bool>>,
    stopping: bool,
    refresh_at: f64,
    recent_at: f64,
    metrics_busy: bool,
    metrics_at: f64,
    backups_at: f64,
    last_offset: f64,
    last_config: Option<std::time::SystemTime>,
}
fn answer(reply: Reply, result: Result<Value>) {
    let _ = reply.send(result);
}
fn log(paths: &Paths, message: &str) {
    let file = paths.state.join("daemon.log");
    if fs::metadata(&file).is_ok_and(|m| m.len() > 1024 * 1024) {
        let _ = fs::rename(&file, paths.state.join("daemon.log.1"));
    }
    if let Ok(mut f) = fs::OpenOptions::new()
        .append(true)
        .create(true)
        .mode(0o600)
        .open(file)
    {
        use std::io::Write;
        let _ = writeln!(f, "{:.0} omarchy-cloudkit: {}", now(), safe(message));
    }
}
fn offset() -> f64 {
    let mut boot = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    let mut mono = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    unsafe {
        libc::clock_gettime(libc::CLOCK_BOOTTIME, &mut boot);
        libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut mono);
    }
    boot.tv_sec as f64 + boot.tv_nsec as f64 / 1e9 - mono.tv_sec as f64 - mono.tv_nsec as f64 / 1e9
}
impl Core {
    fn publish(&self) {
        let state = self.m.state();
        self.push.send_if_modified(|old| {
            if *old == state {
                false
            } else {
                *old = state;
                true
            }
        });
    }
    fn reload_remotes(&self) {
        if let Some(r) = self.rc.clone() {
            let tx = self.tx.clone();
            tokio::spawn(async move {
                let result = r.call_timeout("config/dump", json!({}), 15).await;
                let _ = tx.send(Event::Remotes(result)).await;
            });
        }
    }
    fn start_mount(
        &mut self,
        name: String,
        on: bool,
        force: bool,
        takeover: bool,
        reply: Option<Reply>,
    ) {
        if self.mounting.contains(&name) {
            if let Some(r) = reply {
                answer(
                    r,
                    Err("A mount operation is already running; retry shortly".into()),
                )
            }
            return;
        }
        let Some(rcc) = self.rc.clone() else {
            if let Some(r) = reply {
                answer(r, Err("rclone is not ready".into()))
            }
            return;
        };
        let point = if !on && !text(&self.m.drives[&name]["mount_point"]).is_empty() {
            PathBuf::from(text(&self.m.drives[&name]["mount_point"]))
        } else {
            self.m.point(&name)
        };
        let cfg = self.m.cfg.clone();
        self.mounting.push(name.clone());
        self.m.drives.get_mut(&name).unwrap()["mount_state"] =
            json!(if on { "mounting" } else { "unmounting" });
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let result = if on {
                jobs::mount(&rcc, &name, &point, &cfg, takeover).await
            } else {
                jobs::unmount(&rcc, &point, force, &name).await
            };
            let _ = tx.send(Event::Mount(name, point, on, result, reply)).await;
        });
    }
    fn watch(&mut self) {
        let ids: Vec<_> = self.m.folders.keys().cloned().collect();
        self.watchers.retain(|id, _| ids.contains(id));
        for id in ids {
            let f = self.m.folders[&id].clone();
            if f["watch_retry_at"].as_f64().unwrap_or(0.0) > now() {
                continue;
            }
            let rules = self.m.rules(&f);
            let digest = filters::digest(&rules);
            if self.watchers.get(&id).is_some_and(|(d, _)| d == &digest) {
                continue;
            }
            let watcher = jobs::watch_folder(
                id.clone(),
                expand(text(&f["local"])),
                rules,
                self.tx.clone(),
            );
            self.watchers.insert(id.clone(), (digest, watcher));
            self.m.folders.get_mut(&id).unwrap()["watching"] = json!(true);
        }
    }
    fn schedule(&mut self) {
        if self.rc.is_none() || self.stopping {
            return;
        }
        let mut started = false;
        let mut ids: Vec<_> = self.m.folders.keys().cloned().collect();
        ids.sort_by(|a, b| {
            self.m.folders[b]["dirty_since"]
                .as_f64()
                .unwrap_or(0.0)
                .partial_cmp(&self.m.folders[a]["dirty_since"].as_f64().unwrap_or(0.0))
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        for id in ids {
            if self.syncs.len() >= 3 {
                break;
            }
            let f = &self.m.folders[&id];
            let at = now();
            if self.syncs.contains_key(&id)
                || f["paused"] == true
                || !text(&f["attention"]).is_empty()
                || f["retry_at"].as_f64().unwrap_or(0.0) > at
                || !self.m.remotes[text(&f["remote"])].is_object()
            {
                continue;
            }
            let dirty = f["dirty_since"].as_f64().unwrap_or(0.0);
            let due = f["initialized"] != true
                || !text(&f["resync_reason"]).is_empty()
                || f["force_next"] == true
                || at >= f["next_due"].as_f64().unwrap_or(0.0)
                || (dirty > 0.0
                    && (at - f["last_event"].as_f64().unwrap_or(0.0) >= 4.0 || at - dirty >= 30.0));
            if !due {
                continue;
            }
            let rules = self.m.rules(f);
            let kind = text(&self.m.remotes[text(&f["remote"])]["type"]).to_owned();
            let f = self.m.folders.get_mut(&id).unwrap();
            let resync = f["initialized"] != true || !text(&f["resync_reason"]).is_empty();
            f["run_resync"] = json!(resync);
            f["run_filters_md5"] = json!(filters::digest(&rules));
            if resync && f["resync_reason"] == "recover" {
                f["recovery_attempted"] = json!(true);
            }
            f["job_id"] = json!(-1);
            f["dirty_since"] = json!(0);
            f["dirty_during_run"] = json!(false);
            let folder = f.clone();
            f["force_next"] = json!(false);
            let paths = self.m.paths.clone();
            let tx = self.tx.clone();
            let (cancel, rx) = watch::channel(false);
            self.syncs.insert(id, cancel);
            started = true;
            log(
                &self.m.paths,
                &format!(
                    "sync {} ({}:{}) started{}",
                    text(&folder["id"]),
                    text(&folder["remote"]),
                    text(&folder["path"]),
                    if resync { " [resync]" } else { "" }
                ),
            );
            tokio::spawn(jobs::sync(paths, folder, kind, rules, tx, rx));
        }
        if started && let Err(e) = self.m.persist() {
            log(&self.m.paths, &e);
        }
    }
    fn finish_sync(&mut self, id: &str, result: Result<Value>) {
        self.syncs.remove(id);
        let Some(f) = self.m.folders.get_mut(id) else {
            return;
        };
        let (js, stats, error) = match result {
            Ok(v) => (v["job"].clone(), v["stats"].clone(), String::new()),
            Err(e) => (json!({}), json!({}), e),
        };
        let intentional_cancel = self.stopping && error.starts_with("Sync cancelled;");
        let output = text(&js["output"]["output"]);
        let blob = format!("{}\n{}\n{}", text(&js["error"]), error, output);
        let lower = blob.to_lowercase();
        let mut conflicts = f["conflicts"].as_array().cloned().unwrap_or_default();
        for line in output.lines() {
            if line.contains("Renaming Path1 copy") || line.contains("Renaming Path2 copy") {
                let target = line.rsplit(" - ").next().unwrap_or("");
                let name = safe(
                    &PathBuf::from(target)
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy(),
                );
                if !name.is_empty() && !conflicts.contains(&json!(name)) {
                    conflicts.insert(0, json!(name));
                    jobs::notify(&self.m.cfg, "Sync conflict; both versions kept", &name);
                }
            }
        }
        conflicts.truncate(20);
        f["conflicts"] = json!(conflicts);
        f["job_id"] = json!(0);
        f["progress"] = json!({});
        f["moving"] = json!([]);
        if js["success"] == true {
            log(&self.m.paths, &format!("sync {id} done"));
            f["initialized"] = json!(true);
            if f["run_resync"] != true {
                f["recovery_attempted"] = json!(false);
            }
            f["filters_md5"] = f["run_filters_md5"].clone();
            f["resync_reason"] = json!("");
            f["last_sync"] = json!(now());
            f["last_changes"] = json!(
                ["transfers", "deletes", "renames"]
                    .iter()
                    .map(|k| stats[k].as_u64().unwrap_or(0))
                    .sum::<u64>()
            );
            for k in ["error", "error_kind", "attention", "attention_code"] {
                f[k] = json!("");
            }
            f["backoff"] = json!(0);
            f["retry_at"] = json!(0);
            f["next_due"] = json!(if f["rerun"] == true {
                now()
            } else {
                now() + self.m.cfg["sync_interval_min"].as_f64().unwrap_or(5.0) * 60.0
            });
            if f["dirty_during_run"] == true {
                f["dirty_since"] = json!(now());
                f["last_event"] = json!(now() + 6.0);
                f["followup_only"] = json!(true);
            }
            f["rerun"] = json!(false);
        } else {
            let transient = lower
                .lines()
                .filter(|l| l.contains("critical error"))
                .map(classify)
                .find(|k| ["network", "rate"].contains(k));
            let kind = transient.unwrap_or_else(|| classify(&lower));
            let detail = safe(
                blob.lines()
                    .find(|l| l.to_lowercase().contains("critical error"))
                    .unwrap_or(blob.trim()),
            );
            let detail = detail.chars().take(600).collect::<String>();
            let mut attention = None;
            if intentional_cancel {
                // Power-off is not a failed initial sync or repair. Preserve its
                // initialization/recovery flags so the next start can finish it.
                f["error"] = json!("");
                f["error_kind"] = json!("");
                f["retry_at"] = json!(0);
                f["backoff"] = json!(0);
            } else if lower.contains("missing_local:") {
                attention = Some(("missing_local", detail));
            } else if lower.contains("run with --force")
                || lower.contains("too many deletes")
                || lower.contains("safety abort")
            {
                attention=Some(("safety","Safety stop: check both copies, then choose Resync (never deletes) or Sync anyway".into()));
            } else if lower.contains("cannot sync to an empty directory") {
                attention=Some(("empty","One side is empty. Resync restores files; Sync anyway can delete the other side".into()));
            } else if lower.contains("file name too long") {
                attention = Some((
                    "too_long",
                    "Cloud path is too long for bisync state; choose a higher folder".into(),
                ));
            } else if lower.contains("prior lock file found") {
                f["retry_at"] = json!(now() + 90.0);
                f["error"] = json!("Waiting for previous sync lock");
                f["error_kind"] = json!("lock");
            } else if lower.contains("directory not found")
                && !text(&f["path"]).is_empty()
                && lower.contains(&text(&f["remote"]).to_lowercase())
            {
                attention = Some((
                    "remote_missing",
                    "The cloud folder was moved or deleted. Check both copies before resuming."
                        .into(),
                ));
            } else if kind == "auth" {
                attention = Some((
                    "auth",
                    "Sign-in expired or was revoked; reconnect the account".into(),
                ));
                if lower.contains("must run --resync") {
                    f["resync_reason"] = json!("recover");
                }
            } else if !["network", "rate"].contains(&kind)
                && (f["run_resync"] == true
                    || lower.contains("must run --resync")
                    || lower.contains("cannot find prior")
                    || lower.contains("md5 hash not found"))
            {
                if f["run_resync"] == true || f["recovery_attempted"] == true {
                    attention = Some((
                        "resync_failed",
                        format!(
                            "Could not repair sync state: {detail}. Choose Resync after checking both copies; nothing will be deleted."
                        ),
                    ));
                } else {
                    f["resync_reason"] = json!("recover");
                    f["retry_at"] = json!(now() + 2.0);
                }
            } else {
                if lower.contains("must run --resync") {
                    f["resync_reason"] = json!("recover");
                }
                let old = f["backoff"].as_f64().unwrap_or(0.0);
                let backoff = match kind {
                    "rate" => (old * 2.0).clamp(120.0, 1800.0),
                    "network" => (old * 2.0).clamp(30.0, 600.0),
                    _ => (old * 2.0).clamp(60.0, 1800.0),
                };
                f["backoff"] = json!(backoff);
                f["retry_at"] = json!(now() + backoff);
                f["error"] = json!(detail);
                f["error_kind"] = json!(kind);
            }
            if let Some((code, message)) = attention {
                f["attention_code"] = json!(code);
                f["attention"] = json!(message);
                jobs::notify(&self.m.cfg, "Cloud folder needs attention", &message);
            }
            log(
                &self.m.paths,
                &format!(
                    "Sync {id} {}: {}",
                    if intentional_cancel {
                        "suspended"
                    } else {
                        "failed"
                    },
                    safe(&blob).chars().take(1000).collect::<String>()
                ),
            );
        }
        if let Err(e) = self.m.rebuild().and_then(|_| self.m.persist()) {
            log(&self.m.paths, &e);
        }
        self.recent_at = 0.0;
    }
    async fn handle(&mut self, event: Event) {
        match event {
            Event::Request(cmd, args, reply) => self.dispatch(&cmd, args, reply).await,
            Event::EngineReady(r, version) => {
                self.rc = Some(r);
                self.m.engine = json!({"state":"starting","error":"","version":version});
                self.reload_remotes();
            }
            Event::EngineLost(error) => {
                self.rc = None;
                self.m.engine = json!({"state":"restarting","error":safe(&error),"version":""});
                self.mounting.clear();
                for d in self.m.drives.values_mut() {
                    if d["mount_state"] == "mounted" {
                        d["mount_state"] = json!("off");
                    }
                }
            }
            Event::Remotes(result) => match result {
                Ok(v) => {
                    self.m.load_remotes(v);
                    self.m.engine["state"] = json!("running");
                    let _ = self.m.save();
                    self.refresh_at = now();
                }
                Err(e) => log(&self.m.paths, &e),
            },
            Event::Changed(id) => {
                if let Some(f) = self.m.folders.get_mut(&id) {
                    if f["job_id"].as_i64().unwrap_or(0) != 0 {
                        f["dirty_during_run"] = json!(true)
                    } else {
                        if f["dirty_since"].as_f64().unwrap_or(0.0) == 0.0 {
                            f["dirty_since"] = json!(now());
                        }
                        f["last_event"] = json!(now());
                        f["followup_only"] = json!(false);
                    }
                }
            }
            Event::WatchError(id, e) => {
                self.watchers.remove(&id);
                if let Some(f) = self.m.folders.get_mut(&id) {
                    f["watching"] = json!(false);
                    f["watch_error"] = json!(safe(&e));
                    f["watch_retry_at"] = json!(now() + 30.0);
                }
            }
            Event::SyncProgress(id, stats) => {
                if let Some(f) = self.m.folders.get_mut(&id) {
                    let current = &stats["transferring"][0];
                    f["moving"] = json!(
                        stats["transferring"]
                            .as_array()
                            .unwrap_or(&vec![])
                            .iter()
                            .map(|t| t["name"].clone())
                            .collect::<Vec<_>>()
                    );
                    f["progress"] = json!({"bytes":stats["bytes"].as_u64().unwrap_or(0),"totalBytes":stats["totalBytes"].as_u64().unwrap_or(0),"transfers":stats["transfers"].as_u64().unwrap_or(0),"totalTransfers":stats["totalTransfers"].as_u64().unwrap_or(0),"checks":stats["checks"].as_u64().unwrap_or(0),"current":safe(&PathBuf::from(text(&current["name"])).file_name().unwrap_or_default().to_string_lossy()),"speed":stats["speed"].as_f64().unwrap_or(0.0)as u64});
                }
            }
            Event::SyncDone(id, result) => self.finish_sync(&id, result),
            Event::Mount(name, point, on, result, reply) => {
                self.metrics_at = 0.0;
                self.mounting.retain(|n| n != &name);
                if let Some(d) = self.m.drives.get_mut(&name) {
                    match &result {
                        Ok(()) => {
                            d["mount_state"] = json!(if on { "mounted" } else { "off" });
                            d["mount_point"] = json!(if on {
                                point.to_string_lossy().to_string()
                            } else {
                                String::new()
                            });
                            d["error"] = json!("");
                            d["error_kind"] = json!("");
                            d["retry_at"] = json!(0);
                            d["backoff"] = json!(0);
                        }
                        Err(e) => {
                            let foreign = e.contains("Already mounted");
                            if !on {
                                d["mount_state"] = json!("mounted");
                                self.m.cfg["remotes"][&name]["stream"] = json!(true);
                            } else {
                                d["mount_state"] = json!(if foreign { "foreign" } else { "error" });
                            }
                            d["error"] = json!(safe(e));
                            d["error_kind"] = json!(if foreign { "foreign" } else { classify(e) });
                            let backoff =
                                (d["backoff"].as_f64().unwrap_or(0.0) * 2.0).clamp(15.0, 600.0);
                            d["backoff"] = json!(backoff);
                            d["retry_at"] = json!(now() + backoff);
                        }
                    }
                }
                let _ = self.m.save();
                self.publish();
                if let Some(reply) = reply {
                    answer(reply,result.map(|_|json!({"message":if on{format!("Streaming {name}")}else{format!("Stopped streaming {name}")}})));
                }
            }
            Event::Metrics(name, v) => {
                if let Some(d) = self.m.drives.get_mut(&name) {
                    if v["disappeared"] == true {
                        d["mount_state"] = json!("off");
                        d["retry_at"] = json!(0);
                    }
                    let dc = &v["diskCache"];
                    if dc.is_object() {
                        d["uploads"] = json!(
                            dc["uploadsInProgress"].as_u64().unwrap_or(0)
                                + dc["uploadsQueued"].as_u64().unwrap_or(0)
                        );
                        d["cache_bytes"] = dc["bytesUsed"].clone();
                        d["meta_path"] = dc["pathMeta"].clone();
                        d["data_path"] = dc["path"].clone();
                    }
                }
            }
            Event::Quota(name, v) => {
                self.m.saved["quota"][name] = v;
                let _ = self.m.persist();
            }
            Event::Recent(recent, count) => {
                self.m.recent = recent;
                self.m.backup_files = count;
            }
            Event::BackupCount(count) => self.m.backup_files = count,
            Event::AuthUrl(url) => self.m.auth["url"] = json!(url),
            Event::AuthDone(result, reply) => {
                self.auth_cancel = None;
                match &result {
                    Ok(v) => {
                        let name = text(&v["remote"]);
                        if text(&v["action"]).starts_with("add_") {
                            self.m.cfg["remotes"][name] = json!({"folders":[],"stream":v["stream"],"mount_path":v["mount_path"],"label":null,"hidden_hints":[]});
                            let _ = self.m.save();
                        }
                        for f in self.m.folders.values_mut() {
                            if f["remote"] == name && f["attention_code"] == "auth" {
                                f["attention"] = json!("");
                                f["attention_code"] = json!("");
                                f["retry_at"] = json!(0);
                            }
                        }
                        if let Some(d) = self.m.drives.get_mut(name) {
                            d["retry_at"] = json!(0);
                            if d["mount_state"] == "error" {
                                d["mount_state"] = json!("off");
                            }
                        }
                        self.m.auth = json!({"busy":false,"waiting":false,"url":"","error":"","success":v["message"],"remote":name});
                        self.reload_remotes();
                        if v["action"] == "set_client_id"
                            && self
                                .m
                                .drives
                                .get(name)
                                .is_some_and(|d| d["mount_state"] == "mounted")
                        {
                            self.start_mount(name.to_owned(), false, false, false, None);
                        }
                    }
                    Err(e) => {
                        self.m.auth = json!({"busy":false,"waiting":false,"url":"","error":safe(e),"success":"","remote":self.m.auth["remote"]})
                    }
                }
                self.publish();
                answer(reply, result);
            }
            Event::EngineStats(v) => {
                self.metrics_busy = false;
                let mut names = vec![];
                let mut speed = v["speed"].as_f64().unwrap_or(0.0);
                for t in v["transferring"].as_array().unwrap_or(&vec![]) {
                    names.push(safe(text(&t["name"])));
                }
                for f in self.m.folders.values() {
                    speed += f["progress"]["speed"].as_f64().unwrap_or(0.0);
                    for n in f["moving"].as_array().unwrap_or(&vec![]) {
                        names.push(safe(text(n)));
                    }
                }
                self.m.transfers = json!({"count":names.len(),"speed":speed as u64,"names":names.into_iter().take(3).collect::<Vec<_>>()});
            }
        }
        self.publish();
    }
    async fn dispatch(&mut self, cmd: &str, args: Value, reply: Reply) {
        let mut reply = Some(reply);
        let result = self.execute(cmd, args, &mut reply).await;
        self.publish();
        if let Some(reply) = reply {
            answer(reply, result);
        }
    }
    async fn execute(
        &mut self,
        cmd: &str,
        args: Value,
        reply: &mut Option<Reply>,
    ) -> Result<Value> {
        if self.stopping && !["ping", "state", "log", "shutdown"].contains(&cmd) {
            return Err("Daemon is stopping; wait before making changes".into());
        }
        match cmd {
            "ping" => Ok(json!({"pong":true,"version":env!("CARGO_PKG_VERSION")})),
            "state" => Ok(json!({"state":self.m.state()})),
            "touch" => {
                self.recent_at = 0.0;
                Ok(json!({}))
            }
            "power_off" => {
                if !self.mounting.is_empty() {
                    return Err(
                        "A drive is mounting or unmounting; wait before switching off".into(),
                    );
                }
                // Check the VFS directly: the displayed metrics can be stale. Never
                // stop an engine with dirty streaming cache without explicit force.
                if let Some(rcc) = &self.rc {
                    for (name, drive) in &self.m.drives {
                        if drive["mount_state"] == "mounted" {
                            let stats = rcc
                                .call_timeout("vfs/stats", json!({"fs":format!("{name}:")}), 5)
                                .await?;
                            let cache = &stats["diskCache"];
                            if cache["uploadsInProgress"].as_u64().unwrap_or(0)
                                + cache["uploadsQueued"].as_u64().unwrap_or(0)
                                > 0
                            {
                                return Err(
                                    "Files are still uploading; wait before switching off".into()
                                );
                            }
                        }
                    }
                }
                self.m.paths.set_enabled(false)?;
                self.stopping = true;
                Ok(json!({"message":"Stopping CloudKit and rclone; settings are preserved"}))
            }
            "shutdown" => {
                self.stopping = true;
                Ok(json!({"message":"Stopping"}))
            }
            "restart_engine" => {
                if let Some(r) = &self.rc {
                    let r = r.clone();
                    tokio::spawn(async move {
                        let _ = r.call("core/quit", json!({})).await;
                    });
                }
                Ok(json!({"message":"Restarting rclone"}))
            }
            "log" => {
                let source = text(&args["source"]);
                if !["", "rclone", "daemon"].contains(&source) {
                    return Err("Log source must be rclone or daemon".into());
                }
                let path = self.m.paths.state.join(if source == "daemon" {
                    "daemon.log"
                } else {
                    "rclone.log"
                });
                let n = args["lines"].as_u64().unwrap_or(80).clamp(10, 500) as usize;
                let p = path.clone();
                let lines = tokio::task::spawn_blocking(move || {
                    use std::io::{Read, Seek, SeekFrom};
                    let mut bytes = vec![];
                    if let Ok(mut f) = fs::File::open(p) {
                        let _ = f.seek(SeekFrom::End(
                            -((f.metadata().map_or(0, |m| m.len()).min(1024 * 1024)) as i64),
                        ));
                        let _ = f.read_to_end(&mut bytes);
                    }
                    let lines: Vec<_> = String::from_utf8_lossy(&bytes).lines().map(safe).collect();
                    lines
                        .into_iter()
                        .rev()
                        .take(n)
                        .collect::<Vec<_>>()
                        .into_iter()
                        .rev()
                        .collect::<Vec<_>>()
                })
                .await
                .map_err(|e| e.to_string())?;
                Ok(json!({"lines":lines,"path":path}))
            }
            "set_setting" => {
                let key = text(&args["key"]);
                let value = coerce(key, &args["value"])?;
                let old_points: Vec<_> = self
                    .m
                    .drives
                    .iter()
                    .filter(|(_, d)| d["mount_state"] != "mounted")
                    .map(|(n, _)| self.m.point(n))
                    .collect();
                if ["mount_root", "local_root"].contains(&key) {
                    let path = expand(text(&value));
                    dedicated(&path)?;
                    let other = if key == "mount_root" {
                        self.m.local_root()
                    } else {
                        self.m.mount_root()
                    };
                    if overlaps(&path, &other) {
                        return Err("Streaming and local sync roots must be separate".into());
                    }
                    if key == "mount_root" {
                        for f in self.m.folders.values() {
                            if overlaps(&path, &expand(text(&f["local"]))) {
                                return Err("Streaming root overlaps a local folder".into());
                            }
                        }
                    }
                }
                self.m.cfg[key] = value;
                self.m.save()?;
                if key == "mount_root" {
                    for p in old_points {
                        let _ =
                            tokio::time::timeout(Duration::from_secs(5), tokio::fs::remove_dir(p))
                                .await;
                    }
                }
                if key == "sync_interval_min" {
                    for f in self.m.folders.values_mut() {
                        f["next_due"] = json!(f["next_due"].as_f64().unwrap_or(0.0).min(
                            f["last_sync"].as_f64().unwrap_or(0.0)
                                + self.m.cfg[key].as_f64().unwrap_or(5.0) * 60.0
                        ));
                    }
                }
                if key == "local_root" {
                    tokio::fs::create_dir_all(self.m.local_root())
                        .await
                        .map_err(|e| e.to_string())?;
                }
                Ok(json!({"message":"Saved"}))
            }
            "stream" | "retry" | "takeover" => {
                let name = self.m.drive_key(&args)?;
                let on = if cmd == "stream" {
                    args["on"].as_bool().unwrap_or(true)
                } else {
                    true
                };
                if cmd == "retry" && self.m.cfg["remotes"][&name]["stream"] != true {
                    return Ok(json!({"message":"Streaming is off"}));
                }
                self.m.cfg["remotes"][&name]["stream"] = json!(on);
                self.m.save()?;
                if (on
                    && self.m.drives[&name]["mount_state"] == "mounted"
                    && self.m.point(&name) == *text(&self.m.drives[&name]["mount_point"]))
                    || (!on && self.m.drives[&name]["mount_state"] == "off")
                {
                    return Ok(json!({"message":"Already in the requested state"}));
                }
                self.start_mount(
                    name,
                    on,
                    args["force"] == true,
                    cmd == "takeover",
                    reply.take(),
                );
                Ok(json!({}))
            }
            "stream_all" => {
                let tx = self.tx.clone();
                let names = self
                    .m
                    .remotes
                    .as_object()
                    .ok_or("Invalid remotes")?
                    .keys()
                    .cloned()
                    .collect::<Vec<_>>();
                let on = args["on"].as_bool().unwrap_or(true);
                let force = args["force"] == true;
                let reply = reply.take().unwrap();
                tokio::spawn(async move {
                    let mut results = vec![];
                    for n in names {
                        let (s, r) = oneshot::channel();
                        if tx
                            .send(Event::Request(
                                "stream".into(),
                                json!({"remote":n,"on":on,"force":force}),
                                s,
                            ))
                            .await
                            .is_ok()
                        {
                            results.push(r);
                        }
                    }
                    let mut errors = vec![];
                    for r in results {
                        match r.await {
                            Ok(Ok(_)) => (),
                            Ok(Err(e)) => errors.push(e),
                            Err(_) => errors.push("Service stopped".into()),
                        }
                    }
                    answer(
                        reply,
                        if errors.is_empty() {
                            Ok(
                                json!({"message":if on{"Streaming all drives"}else{"Stopped streaming all drives"}}),
                            )
                        } else {
                            Err(errors.join("; "))
                        },
                    );
                });
                Ok(json!({}))
            }
            "set_drive" => {
                let name = self.m.drive_key(&args)?;
                if args.get("mount_path").is_some() {
                    let raw = text(&args["mount_path"]);
                    if !raw.is_empty() {
                        let path = expand(raw);
                        dedicated(&path)?;
                        for f in self.m.folders.values() {
                            if overlaps(&path, &expand(text(&f["local"]))) {
                                return Err("Mount path overlaps a local folder".into());
                            }
                        }
                        for other in self.m.drives.keys() {
                            if other != &name && overlaps(&path, &self.m.point(other)) {
                                return Err("Mount paths must not overlap".into());
                            }
                        }
                    }
                    self.m.cfg["remotes"][&name]["mount_path"] = json!(raw);
                }
                if args.get("label").is_some() {
                    self.m.cfg["remotes"][&name]["label"] = json!(text(&args["label"]).trim());
                }
                self.m.save()?;
                Ok(json!({"message":"Saved"}))
            }
            "browse" | "folder_size" => {
                let name = self.m.drive_key(&args)?;
                let path = clean_path(text(&args["path"]))?;
                let r = self.rc.clone().ok_or("rclone is not ready")?;
                let txreply = reply.take().unwrap();
                let kind = text(&self.m.remotes[&name]["type"]).to_owned();
                let local = self.m.local_root().join(&name).join(&path);
                let kept = self
                    .m
                    .folders
                    .values()
                    .filter(|f| f["remote"] == name)
                    .map(|f| text(&f["path"]).to_owned())
                    .collect::<Vec<_>>();
                let point = if self.m.drives[&name]["mount_state"] == "mounted" {
                    Some(PathBuf::from(text(&self.m.drives[&name]["mount_point"])))
                } else {
                    None
                };
                let cmd = cmd.to_owned();
                tokio::spawn(async move {
                    let result = if cmd == "folder_size" {
                        r.call("operations/size", json!({"fs":rc::spec(&name,&kind,&path)}))
                            .await
                    } else {
                        jobs::browse(r, name, path, point, local, kept).await
                    };
                    answer(txreply, result);
                });
                Ok(json!({}))
            }
            "add_folder" => {
                let name = self.m.drive_key(&args)?;
                let path = clean_path(text(&args["path"]))?;
                let local = if text(&args["local"]).is_empty() {
                    self.m.local_root().join(&name).join(&path)
                } else {
                    expand(text(&args["local"]))
                };
                self.m.validate_local(&local, &name, &path)?;
                let size = args["size_bytes"].as_u64().unwrap_or(0);
                if size > 0 {
                    let mut probe = local.clone();
                    while !probe.exists() && probe.pop() {}
                    use std::os::unix::ffi::OsStrExt;
                    let c = std::ffi::CString::new(probe.as_os_str().as_bytes())
                        .map_err(|_| "Invalid local path")?;
                    let mut info = std::mem::MaybeUninit::<libc::statvfs>::uninit();
                    if unsafe { libc::statvfs(c.as_ptr(), info.as_mut_ptr()) } != 0 {
                        return Err("Cannot check free disk space".into());
                    }
                    let info = unsafe { info.assume_init() };
                    let free = info.f_bavail.saturating_mul(info.f_frsize);
                    if size > free.saturating_sub(1 << 30) {
                        return Err("Not enough free disk space (including 1 GiB reserve)".into());
                    }
                }
                let spec = rc::spec(&name, text(&self.m.remotes[&name]["type"]), &path);
                if spec.len() + self.m.paths.state.join("l/00000000").as_os_str().len() > 240 {
                    return Err(
                        "Path is too long for bisync state files; choose a higher folder".into(),
                    );
                }
                let id = format!("{:08x}", new_id());
                self.m.cfg["remotes"][&name]["folders"]
                    .as_array_mut()
                    .ok_or("Invalid folder list")?
                    .push(json!({"id":id,"path":path,"local":local,"paused":false}));
                self.m.save()?;
                self.m.rebuild()?;
                Ok(
                    json!({"id":id,"local":local,"message":format!("Keeping {name}:{path} on this device")}),
                )
            }
            "remove_folder" => {
                let id = self.m.folder_key(&args)?;
                if let Some(cancel) = self.syncs.get(&id) {
                    let _ = cancel.send(true);
                    let mut state = self.push.subscribe();
                    let tx = self.tx.clone();
                    let reply = reply.take().unwrap();
                    tokio::spawn(async move {
                        loop {
                            let busy = state.borrow()["drives"].as_array().is_some_and(|drives| {
                                drives.iter().any(|d| {
                                    d["folders"].as_array().is_some_and(|folders| {
                                        folders
                                            .iter()
                                            .any(|f| f["id"] == id && f["state"] == "syncing")
                                    })
                                })
                            });
                            if !busy {
                                let _ = tx
                                    .send(Event::Request("remove_folder".into(), args, reply))
                                    .await;
                                return;
                            }
                            if state.changed().await.is_err() {
                                answer(reply, Err("Service stopped".into()));
                                return;
                            }
                        }
                    });
                    return Ok(json!({}));
                }
                let f = self.m.folders[&id].clone();
                let name = text(&f["remote"]);
                self.m.cfg["remotes"][name]["folders"]
                    .as_array_mut()
                    .unwrap()
                    .retain(|f| f["id"] != id);
                self.m.save()?;
                self.m.rebuild()?;
                self.m.saved["folders"].as_object_mut().unwrap().remove(&id);
                self.m.persist()?;
                self.watchers.remove(&id);
                let work = self.m.paths.work(&id);
                let link = self.m.paths.state.join("l").join(&id);
                let local = PathBuf::from(text(&f["local"]));
                let delete = args["delete_local"] == true;
                let reply = reply.take().unwrap();
                self.publish();
                tokio::spawn(async move {
                    let _ = tokio::fs::remove_dir_all(work).await;
                    let _ = tokio::fs::remove_file(link).await;
                    let result = if delete {
                        match tokio::process::Command::new("gio").arg("trash").arg(&local).output().await{Ok(v) if v.status.success()=>Ok(json!({"message":"Stopped syncing; local files moved to trash"})),_=>Err("Stopped syncing, but could not move local files to trash; files were preserved".into())}
                    } else {
                        Ok(json!({"message":"Stopped syncing; local files were preserved"}))
                    };
                    answer(reply, result);
                });
                Ok(json!({}))
            }
            "pause_folder" | "set_folder" => {
                let id = self.m.folder_key(&args)?;
                if cmd == "pause_folder" {
                    let paused = args["paused"].as_bool().unwrap_or(true);
                    self.m.folder_spec_mut(&id)?["paused"] = json!(paused);
                    let f = self.m.folders.get_mut(&id).unwrap();
                    f["paused"] = json!(paused);
                    f["rerun"] = json!(false);
                    if !paused {
                        for k in ["retry_at", "backoff", "next_due"] {
                            f[k] = json!(0);
                        }
                        f["error"] = json!("");
                        f["error_kind"] = json!("");
                    }
                } else {
                    let label = text(&args["label"]).trim();
                    let local = PathBuf::from(text(&self.m.folders[&id]["local"]));
                    let default = local.file_name().unwrap_or_default().to_string_lossy();
                    let label = if label == default { "" } else { label };
                    self.m.folder_spec_mut(&id)?["label"] = if label.is_empty() {
                        Value::Null
                    } else {
                        json!(label)
                    };
                    self.m.folders.get_mut(&id).unwrap()["label"] = json!(label);
                }
                self.m.save()?;
                Ok(
                    json!({"message":if cmd=="set_folder"{"Saved"}else if args["paused"]==false{"Resumed; catching up now"}else if self.syncs.contains_key(&id){"Paused once the sync under way ends; nothing else syncs until you resume"}else{"Paused; nothing syncs until you resume"}}),
                )
            }
            "set_excludes" => {
                let patterns = filters::clean(&args["patterns"])?;
                let id = text(&args["id"]);
                if id.is_empty() {
                    let raw = filters::replace(&self.m.global, &patterns);
                    let dest = fs::canonicalize(self.m.paths.filter())
                        .unwrap_or_else(|_| self.m.paths.filter());
                    atomic(&dest, raw.as_bytes())?;
                    self.m.global = raw;
                } else {
                    let id = self.m.folder_key(&args)?;
                    self.m.folder_spec_mut(&id)?["excludes"] = if patterns.is_empty() {
                        Value::Null
                    } else {
                        json!(patterns)
                    };
                    self.m.save()?;
                }
                self.m.rebuild()?;
                self.m.persist()?;
                Ok(
                    json!({"message":if id.is_empty() || patterns.is_empty(){"Saved global never-synced patterns; affected folders resync safely"}else{"Saved own list; folder resyncs safely"},"excludes":patterns}),
                )
            }
            "sync_now" => {
                let id = text(&args["id"]);
                let ids = if id.is_empty() {
                    self.m.folders.keys().cloned().collect::<Vec<_>>()
                } else {
                    vec![self.m.folder_key(&args)?]
                };
                for id in ids {
                    let f = self.m.folders.get_mut(&id).unwrap();
                    if f["paused"] == true {
                        if !text(&args["id"]).is_empty() {
                            return Err("Folder is paused; resume it to sync".into());
                        }
                        continue;
                    }
                    f["retry_at"] = json!(0);
                    f["backoff"] = json!(0);
                    f["next_due"] = json!(0);
                    f["error"] = json!("");
                    f["rerun"] = json!(self.syncs.contains_key(&id));
                }
                Ok(json!({"message":"Syncing now (paused folders stay paused)"}))
            }
            "resolve" => {
                let id = self.m.folder_key(&args)?;
                let f = self.m.folders.get_mut(&id).unwrap();
                let action = text(&args["action"]);
                match action {
                    "force" => {
                        if (f["initialized"] != true || !text(&f["resync_reason"]).is_empty())
                            && f["attention_code"] != "missing_local"
                        {
                            return Err(
                                "Sync state needs Resync first; nothing will be deleted".into()
                            );
                        }
                        f["force_next"] = json!(true);
                    }
                    "resync" => {
                        f["resync_reason"] = json!("manual");
                        f["force_next"] = json!(false);
                        f["recovery_attempted"] = json!(false);
                    }
                    "dismiss" => (),
                    _ => return Err("Unknown action".into()),
                }
                if f["attention_code"] == "missing_local" && action != "dismiss" {
                    tokio::fs::create_dir_all(text(&f["local"]))
                        .await
                        .map_err(|e| e.to_string())?;
                    f["resync_reason"] = json!("manual");
                    f["force_next"] = json!(false);
                }
                for k in ["attention", "attention_code", "error", "error_kind"] {
                    f[k] = json!("");
                }
                for k in ["retry_at", "backoff", "next_due"] {
                    f[k] = json!(0);
                }
                self.m.persist()?;
                Ok(
                    json!({"message":if action=="resync"{"Resyncing; nothing will be deleted"}else if action=="force"{"Syncing anyway"}else{"Dismissed"}}),
                )
            }
            "clear_conflicts" => {
                let id = self.m.folder_key(&args)?;
                self.m.folders.get_mut(&id).unwrap()["conflicts"] = json!([]);
                self.m.persist()?;
                Ok(json!({}))
            }
            "client_guide" => {
                let kind = if !text(&args["remote"]).is_empty() {
                    let name = self.m.drive_key(&args)?;
                    text(&self.m.remotes[&name]["type"]).to_owned()
                } else {
                    text(&args["provider"]).to_owned()
                };
                Ok(json!({"guide":providers::guide(&kind)?}))
            }
            "dismiss_hint" => {
                let name = self.m.drive_key(&args)?;
                let code = text(&args["code"]);
                if code != "shared_app"
                    || !self.m.remotes[&name]["warnings"]
                        .as_array()
                        .is_some_and(|a| {
                            a.iter()
                                .any(|w| w["code"] == code && w["dismissible"] == true)
                        })
                {
                    return Err("That warning cannot be hidden".into());
                }
                let mut hidden = self.m.cfg["remotes"][&name]["hidden_hints"]
                    .as_array()
                    .cloned()
                    .unwrap_or_default();
                if !hidden.contains(&json!(code)) {
                    hidden.push(json!(code));
                }
                self.m.cfg["remotes"][&name]["hidden_hints"] = json!(hidden);
                self.m.save()?;
                if let Some(r) = self.m.remotes.get_mut(&name) {
                    r["warnings"] = json!([]);
                }
                Ok(json!({}))
            }
            "cancel_auth" => {
                if let Some(cancel) = &self.auth_cancel {
                    let _ = cancel.send(true);
                }
                Ok(json!({"message":"Cancelled"}))
            }
            "dismiss_auth" => {
                if self.m.auth["busy"] != true {
                    self.m.auth = json!({"busy":false,"waiting":false,"url":"","error":"","success":"","remote":""});
                }
                Ok(json!({}))
            }
            "add_oauth" | "add_credentials" | "set_client_id" | "reconnect" => {
                if self.auth_cancel.is_some() {
                    return Err("Another sign-in is already in progress".into());
                }
                let mut args = args;
                let name = if cmd.starts_with("add_") {
                    let raw = if text(&args["name"]).is_empty() {
                        text(&args["provider"])
                    } else {
                        text(&args["name"])
                    };
                    let n = clean_name(raw)?;
                    if self.m.remotes[&n].is_object() {
                        return Err("A drive with this name already exists".into());
                    }
                    args["name"] = json!(n);
                    n
                } else {
                    self.m.drive_key(&args)?
                };
                let kind = if cmd.starts_with("add_") {
                    text(&args["provider"]).to_owned()
                } else {
                    text(&self.m.remotes[&name]["type"]).to_owned()
                };
                if cmd != "add_credentials" {
                    providers::guide(&kind)?;
                    if cmd == "set_client_id"
                        || !text(&args["client_id"]).is_empty()
                        || !text(&args["client_secret"]).is_empty()
                    {
                        providers::check(
                            &kind,
                            text(&args["client_id"]),
                            text(&args["client_secret"]),
                        )
                        .map_err(|e| format!("bad_client: {e}"))?;
                    }
                }
                if !text(&args["mount_path"]).is_empty() {
                    let point = expand(text(&args["mount_path"]));
                    dedicated(&point)?;
                    for f in self.m.folders.values() {
                        if overlaps(&point, &expand(text(&f["local"]))) {
                            return Err("Mount path overlaps local sync folder".into());
                        }
                    }
                }
                let rcc = self.rc.clone().ok_or("rclone is not ready")?;
                let tx = self.tx.clone();
                let remote = self.m.remotes[&name].clone();
                let action = cmd.to_owned();
                let (cancel, rx) = watch::channel(false);
                self.auth_cancel = Some(cancel);
                self.m.auth = json!({"busy":true,"waiting":cmd!="add_credentials","url":"","error":"","success":"","remote":name});
                let reply = reply.take().unwrap();
                tokio::spawn(async move {
                    let result = auth::run(action, args, remote, rcc, tx.clone(), rx).await;
                    let _ = tx.send(Event::AuthDone(result, reply)).await;
                });
                Ok(json!({}))
            }
            "remove_remote" => {
                let name = self.m.drive_key(&args)?;
                if self.mounting.contains(&name) {
                    return Err(
                        "A mount operation is still running; retry after it finishes".into(),
                    );
                }
                let ids = self
                    .m
                    .folders
                    .iter()
                    .filter(|(_, f)| f["remote"] == name)
                    .map(|(id, _)| id.clone())
                    .collect::<Vec<_>>();
                let mounted = self.m.drives[&name]["mount_state"] == "mounted";
                if !ids.is_empty() || mounted {
                    let tx = self.tx.clone();
                    let reply = reply.take().unwrap();
                    let force = args["force"] == true;
                    tokio::spawn(async move {
                        let result = async {
                            for id in ids {
                                let (s, r) = oneshot::channel();
                                tx.send(Event::Request(
                                    "remove_folder".into(),
                                    json!({"id":id}),
                                    s,
                                ))
                                .await
                                .map_err(|_| "Service stopped")?;
                                r.await.map_err(|_| "Service stopped")??;
                            }
                            if mounted {
                                let (s, r) = oneshot::channel();
                                tx.send(Event::Request(
                                    "stream".into(),
                                    json!({"remote":name,"on":false,"force":force}),
                                    s,
                                ))
                                .await
                                .map_err(|_| "Service stopped")?;
                                r.await.map_err(|_| "Service stopped")??;
                            }
                            let (s, r) = oneshot::channel();
                            tx.send(Event::Request(
                                "remove_remote".into(),
                                json!({"remote":name}),
                                s,
                            ))
                            .await
                            .map_err(|_| "Service stopped")?;
                            r.await.map_err(|_| "Service stopped")?
                        }
                        .await;
                        answer(reply, result);
                    });
                    return Ok(json!({}));
                }
                let rcc = self.rc.clone().ok_or("rclone is not ready")?;
                rcc.call("config/delete", json!({"name":name})).await?;
                let _ = rcc.call("fscache/clear", json!({})).await;
                self.m.cfg["remotes"].as_object_mut().unwrap().remove(&name);
                self.m.saved["quota"].as_object_mut().unwrap().remove(&name);
                self.m.save()?;
                self.m.rebuild()?;
                self.m.persist()?;
                self.m.drives.remove(&name);
                self.m.remotes.as_object_mut().unwrap().remove(&name);
                self.reload_remotes();
                Ok(json!({"message":"Account removed; local files were preserved"}))
            }
            _ => Err(format!("Unknown command: {cmd}")),
        }
    }
    async fn tick(&mut self) {
        let online = jobs::has_route();
        if online != self.m.online {
            self.m.online = online;
            if online {
                for f in self.m.folders.values_mut() {
                    f["next_due"] = json!(0);
                    f["retry_at"] = json!(0);
                    f["waiting_network"] = json!(false);
                }
                self.refresh_at = 0.0;
                self.recent_at = 0.0;
            }
        }
        let off = offset();
        if off - self.last_offset > 20.0 {
            for f in self.m.folders.values_mut() {
                if f["paused"] != true {
                    f["next_due"] = json!(0);
                    f["retry_at"] = json!(0);
                }
            }
            self.refresh_at = 0.0;
        }
        self.last_offset = off;
        if let Ok(raw) = fs::read_to_string(self.m.paths.filter())
            && raw != self.m.global
        {
            self.m.global = raw;
            let _ = self.m.rebuild();
        }
        self.watch();
        self.schedule();
        if self.rc.is_none() {
            self.publish();
            return;
        }
        let rcc = self.rc.clone().unwrap();
        let config = std::env::var_os("RCLONE_CONFIG")
            .map(PathBuf::from)
            .unwrap_or_else(|| home().join(".config/rclone/rclone.conf"));
        let sig = fs::metadata(config).and_then(|m| m.modified()).ok();
        if sig != self.last_config || now() - self.refresh_at > 300.0 {
            self.last_config = sig;
            self.reload_remotes();
            self.refresh_at = now();
        }
        let names: Vec<_> = self.m.drives.keys().cloned().collect();
        for name in names {
            if self.mounting.contains(&name) {
                continue;
            }
            let d = &self.m.drives[&name];
            let want = self.m.cfg["remotes"][&name]["stream"] == true;
            let state = text(&d["mount_state"]);
            if state == "mounted" && (!want || *text(&d["mount_point"]) != self.m.point(&name)) {
                self.start_mount(name, false, false, false, None);
            } else if want
                && ["off", "error", "waiting"].contains(&state)
                && d["retry_at"].as_f64().unwrap_or(0.0) <= now()
            {
                self.start_mount(name, true, false, false, None);
            }
        }
        let active = !self.syncs.is_empty()
            || self.m.transfers["count"].as_u64().unwrap_or(0) > 0
            || self
                .m
                .drives
                .values()
                .any(|d| d["uploads"].as_u64().unwrap_or(0) > 0);
        let interval = if active {
            1.0
        } else if self.push.receiver_count() > 1 {
            3.0
        } else {
            10.0
        };
        if !self.metrics_busy && now() - self.metrics_at >= interval {
            self.metrics_at = now();
            self.metrics_busy = true;
            let mounted = self
                .m
                .drives
                .iter()
                .filter(|(_, d)| d["mount_state"] == "mounted")
                .map(|(n, d)| (n.clone(), text(&d["mount_point"]).to_owned()))
                .collect::<Vec<_>>();
            let tx = self.tx.clone();
            let r = rcc.clone();
            tokio::spawn(async move {
                if !mounted.is_empty() {
                    let mounts = r
                        .call_timeout("mount/listmounts", json!({}), 5)
                        .await
                        .unwrap_or(json!({}));
                    for (n, p) in mounted {
                        let mut stats = r
                            .call_timeout("vfs/stats", json!({"fs":format!("{n}:")}), 5)
                            .await
                            .unwrap_or(json!({}));
                        if mounts["mountPoints"].is_array()
                            && !mounts["mountPoints"]
                                .as_array()
                                .unwrap()
                                .iter()
                                .any(|m| m["MountPoint"] == p)
                        {
                            stats["disappeared"] = json!(true);
                        }
                        let _ = tx.send(Event::Metrics(n, stats)).await;
                    }
                }
                let stats = r
                    .call_timeout("core/stats", json!({}), 5)
                    .await
                    .unwrap_or(json!({}));
                let _ = tx.send(Event::EngineStats(stats)).await;
            });
        }
        for name in self.m.remotes.as_object().unwrap().keys() {
            let q = &self.m.saved["quota"][name];
            if q["unsupported"] == true || now() - q["at"].as_f64().unwrap_or(0.0) < 1800.0 {
                continue;
            }
            self.m.saved["quota"][name]["at"] = json!(now());
            let n = name.clone();
            let tx = self.tx.clone();
            let r = rcc.clone();
            tokio::spawn(async move {
                let result = r
                    .call_timeout("operations/about", json!({"fs":format!("{n}:")}), 15)
                    .await;
                let mut v = match result {
                    Ok(v) => v,
                    Err(e) => {
                        json!({"unsupported":(["not supported","unsupported","doesn't support","not implemented"].iter().any(|p|e.to_lowercase().contains(p))),"at":now()-1770.0})
                    }
                };
                if v["at"].is_null() {
                    v["at"] = json!(now());
                }
                let _ = tx.send(Event::Quota(n, v)).await;
            });
        }
        if now() - self.backups_at > 10.0 {
            self.backups_at = now();
            let root = self.m.paths.state.join("backups");
            let tx = self.tx.clone();
            tokio::task::spawn_blocking(move || {
                let _ = tx.blocking_send(Event::BackupCount(jobs::count_backups(&root)));
            });
        }
        if now() - self.recent_at > 60.0 {
            self.recent_at = now();
            let folders = self.m.folders.values().cloned().collect();
            let drives = self
                .m
                .drives
                .iter()
                .map(|(n, d)| (n.clone(), d.clone()))
                .collect();
            let backups = self.m.paths.state.join("backups");
            let days = self.m.cfg["backup_days"].as_u64().unwrap_or(30);
            let tx = self.tx.clone();
            tokio::task::spawn_blocking(move || {
                let (recent, count) = jobs::recent_and_backups(folders, drives, backups, days);
                let _ = tx.blocking_send(Event::Recent(recent, count));
            });
        }
        self.publish();
    }
}
async fn connection(
    stream: UnixStream,
    mut state: watch::Receiver<Value>,
    tx: mpsc::Sender<Event>,
) -> Result<()> {
    if stream.peer_cred().map_err(|e| e.to_string())?.uid() != unsafe { libc::getuid() } {
        return Err("Foreign client refused".into());
    }
    let (reader, mut writer) = stream.into_split();
    let (reply_tx, mut replies) = mpsc::channel::<Value>(32);
    let writer_task = tokio::spawn(async move {
        let initial = json!({"type":"state","data":*state.borrow()});
        if writer
            .write_all(format!("{}\n", json_ascii(&initial, false)).as_bytes())
            .await
            .is_err()
        {
            return;
        }
        loop {
            let value = tokio::select! {r=replies.recv()=>match r{Some(v)=>v,None=>break},s=state.changed()=>{if s.is_err(){break}json!({"type":"state","data":*state.borrow_and_update()})}};
            if tokio::time::timeout(
                Duration::from_secs(10),
                writer.write_all(format!("{}\n", json_ascii(&value, false)).as_bytes()),
            )
            .await
            .is_err()
            {
                break;
            }
        }
    });
    let mut reader = BufReader::new(reader);
    let limit = std::sync::Arc::new(tokio::sync::Semaphore::new(16));
    loop {
        use tokio::io::AsyncReadExt;
        let mut bytes = vec![];
        let n = (&mut reader)
            .take(1024 * 1024 + 1)
            .read_until(b'\n', &mut bytes)
            .await
            .map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        if n > 1024 * 1024 || bytes.last() != Some(&b'\n') {
            break;
        }
        let Ok(request) = serde_json::from_slice::<Value>(&bytes) else {
            continue;
        };
        if !request.is_object() {
            continue;
        }
        let permit = match limit.clone().try_acquire_owned() {
            Ok(p) => p,
            Err(_) => break,
        };
        let (s, r) = oneshot::channel();
        let id = request["id"].clone();
        if tx
            .send(Event::Request(
                text(&request["cmd"]).replace('-', "_"),
                request["args"]
                    .as_object()
                    .map(|o| json!(o))
                    .unwrap_or(json!({})),
                s,
            ))
            .await
            .is_err()
        {
            break;
        }
        let replies = reply_tx.clone();
        tokio::spawn(async move {
            let _permit = permit;
            let result = r.await.unwrap_or(Err("Service stopped".into()));
            let response = match result {
                Ok(data) => {
                    json!({"type":"reply","id":id,"ok":true,"message":data["message"],"data":data})
                }
                Err(error) => {
                    json!({"type":"reply","id":id,"ok":false,"error":safe(&error),"code":classify(&error)})
                }
            };
            let _ = replies.send(response).await;
        });
    }
    writer_task.abort();
    Ok(())
}
pub async fn run() -> Result<()> {
    let paths = Paths::new();
    // A disabled login service must exit successfully instead of restarting.
    if !paths.enabled()? {
        return Ok(());
    }
    ipc::private_runtime(&paths.runtime).map_err(|e| e.to_string())?;
    let lock = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(paths.runtime.join("daemon.lock"))
        .map_err(|e| e.to_string())?;
    if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        return Err("A cloud daemon is already running for this profile".into());
    }
    // Recheck after taking the daemon lock, in case power changed during startup.
    if !paths.enabled()? {
        return Ok(());
    }
    let m = Model::new(paths.clone())?;
    tokio::fs::create_dir_all(m.local_root())
        .await
        .map_err(|e| e.to_string())?;
    let _ = fs::remove_file(paths.socket());
    let listener = UnixListener::bind(paths.socket()).map_err(|e| e.to_string())?;
    let (push, state) = watch::channel(m.state());
    let (tx, mut rx) = mpsc::channel(256);
    let mut core = Core {
        m,
        tx: tx.clone(),
        push,
        rc: None,
        syncs: BTreeMap::new(),
        watchers: BTreeMap::new(),
        mounting: vec![],
        auth_cancel: None,
        stopping: false,
        refresh_at: 0.0,
        recent_at: 0.0,
        metrics_busy: false,
        metrics_at: 0.0,
        backups_at: 0.0,
        last_offset: offset(),
        last_config: None,
    };
    let server = tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            let s = state.clone();
            let t = tx.clone();
            tokio::spawn(async move {
                let _ = connection(stream, s, t).await;
            });
        }
    });
    let (engine_stop, mut stop_rx) = watch::channel(false);
    let tx = core.tx.clone();
    let engine_paths = paths.clone();
    let supervisor = tokio::spawn(async move {
        let mut delay = 1;
        loop {
            if *stop_rx.borrow() {
                break;
            }
            match rc::launch(&engine_paths.rc(), &rc::log_path(&engine_paths.state), true).await {
                Ok((mut child, rcc)) => {
                    let version = rcc
                        .call_timeout("core/version", json!({}), 5)
                        .await
                        .unwrap_or(json!({}));
                    let _ = tx
                        .send(Event::EngineReady(
                            rcc,
                            text(&version["version"]).to_owned(),
                        ))
                        .await;
                    tokio::select! {_=child.wait()=>(),_=stop_rx.changed()=>{rc::stop(&mut child).await;break}}
                    let _ = tx
                        .send(Event::EngineLost("rclone exited; restarting".into()))
                        .await;
                    delay = 1;
                }
                Err(error) => {
                    let _ = tx.send(Event::EngineLost(error)).await;
                    delay = (delay * 2).min(60);
                }
            }
            tokio::select! {_=tokio::time::sleep(Duration::from_secs(delay))=>(),_=stop_rx.changed()=>break}
        }
    });
    log(
        &paths,
        if paths.legacy {
            "Starting native daemon with existing Guacamole data"
        } else {
            "Starting native daemon"
        },
    );
    let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        .map_err(|e| e.to_string())?;
    let mut tick = tokio::time::interval(Duration::from_millis(500));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tokio::select! {event=rx.recv()=>if let Some(event)=event{core.handle(event).await},_=tick.tick()=>core.tick().await,_=term.recv()=>core.stopping=true,_=tokio::signal::ctrl_c()=>core.stopping=true}
        if core.stopping {
            break;
        }
    }
    // Finish active bisyncs before stopping the engine; only cancel after the grace period.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(35);
    while !core.syncs.is_empty() && tokio::time::Instant::now() < deadline {
        if let Ok(Some(event)) = tokio::time::timeout(Duration::from_millis(500), rx.recv()).await {
            core.handle(event).await;
        }
    }
    for cancel in core.syncs.values() {
        let _ = cancel.send(true);
    }
    if let Some(cancel) = &core.auth_cancel {
        let _ = cancel.send(true);
    }
    let cancel_deadline = tokio::time::Instant::now() + Duration::from_secs(12);
    while !core.syncs.is_empty() && tokio::time::Instant::now() < cancel_deadline {
        if let Ok(Some(event)) = tokio::time::timeout(Duration::from_millis(200), rx.recv()).await {
            core.handle(event).await;
        }
    }
    if let Some(rcc) = &core.rc {
        for (name, d) in &core.m.drives {
            if d["mount_state"] == "mounted" {
                let point = PathBuf::from(text(&d["mount_point"]));
                if let Err(e) = jobs::unmount(rcc, &point, false, name).await {
                    log(&paths, &format!("Preserving VFS cache for {name}: {e}"));
                }
            }
        }
    }
    core.m.persist()?;
    let _ = engine_stop.send(true);
    let _ = tokio::time::timeout(Duration::from_secs(12), supervisor).await;
    core.watchers.clear();
    server.abort();
    let _ = fs::remove_file(paths.socket());
    let _ = fs::remove_file(paths.rc());
    log(&paths, "Daemon stopped");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Fixture {
        core: Core,
        root: PathBuf,
    }
    impl Fixture {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!("ck-recovery-{:08x}", new_id()));
            let paths = Paths {
                config: root.join("cfg"),
                state: root.join("state"),
                runtime: root.join("run"),
                legacy: false,
            };
            let mut m = Model::new(paths).unwrap();
            m.cfg["notifications"] = json!(false);
            m.cfg["remotes"]["Cloud"] = json!({"folders":[{"id":"f1","path":"docs","local":root.join("local"),"paused":false}]});
            m.rebuild().unwrap();
            let f = m.folders.get_mut("f1").unwrap();
            f["initialized"] = json!(true);
            f["run_resync"] = json!(false);
            f["run_filters_md5"] = json!(filters::digest(&m.global));
            let (tx, _) = mpsc::channel(32);
            let (push, _) = watch::channel(m.state());
            Self {
                core: Core {
                    m,
                    tx,
                    push,
                    rc: None,
                    syncs: BTreeMap::new(),
                    watchers: BTreeMap::new(),
                    mounting: vec![],
                    auth_cancel: None,
                    stopping: false,
                    refresh_at: 0.0,
                    recent_at: 0.0,
                    metrics_busy: false,
                    metrics_at: 0.0,
                    backups_at: 0.0,
                    last_offset: 0.0,
                    last_config: None,
                },
                root,
            }
        }
        fn failure(&mut self, message: &str) {
            self.core.finish_sync(
                "f1",
                Ok(json!({"job":{"success":false,"error":message},"stats":{}})),
            );
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }
    #[tokio::test]
    async fn global_power_preserves_preferences_and_refuses_mount_transitions() {
        let mut f = Fixture::new();
        let config = f.core.m.cfg.clone();
        let mut reply = None;
        f.core.mounting.push("Cloud".into());
        assert!(
            f.core
                .execute("power_off", json!({}), &mut reply)
                .await
                .is_err()
        );
        assert!(f.core.m.paths.enabled().unwrap());
        assert!(!f.core.stopping);
        f.core.mounting.clear();
        f.core
            .execute("power_off", json!({}), &mut reply)
            .await
            .unwrap();
        assert!(f.core.stopping);
        assert!(!f.core.m.paths.enabled().unwrap());
        assert_eq!(f.core.m.cfg, config);
    }

    #[tokio::test]
    async fn power_off_during_initial_sync_does_not_become_a_failed_repair() {
        let mut f = Fixture::new();
        f.core.stopping = true;
        f.core.m.folders.get_mut("f1").unwrap()["initialized"] = json!(false);
        f.core.m.folders.get_mut("f1").unwrap()["run_resync"] = json!(true);
        f.core.finish_sync(
            "f1",
            Err("Sync cancelled; existing state will be recovered on the next run".into()),
        );
        let folder = &f.core.m.folders["f1"];
        assert_eq!(folder["initialized"], false);
        assert_eq!(folder["attention"], "");
        assert_eq!(folder["error"], "");
        assert_eq!(folder["retry_at"], 0);
        assert_eq!(f.core.m.saved["folders"]["f1"]["initialized"], false);
    }

    #[tokio::test]
    async fn transient_errors_back_off_without_entering_attention() {
        for (message, kind) in [
            (
                "critical error: connection reset\nMust run --resync to recover",
                "network",
            ),
            (
                "critical error: rate limit exceeded\nMust run --resync to recover",
                "rate",
            ),
        ] {
            let mut f = Fixture::new();
            f.failure(message);
            let folder = &f.core.m.folders["f1"];
            assert_eq!(folder["attention"], "");
            assert_eq!(folder["error_kind"], kind);
            assert!(folder["retry_at"].as_f64().unwrap() > now());
        }
    }
    #[tokio::test]
    async fn repair_is_limited_and_persists_across_restart() {
        let mut f = Fixture::new();
        f.failure("Cannot find prior listings. Must run --resync");
        assert_eq!(f.core.m.folders["f1"]["resync_reason"], "recover");
        f.core.m.folders.get_mut("f1").unwrap()["recovery_attempted"] = json!(true);
        f.core.m.folders.get_mut("f1").unwrap()["run_resync"] = json!(true);
        f.core
            .finish_sync("f1", Ok(json!({"job":{"success":true},"stats":{}})));
        let restarted = Model::new(f.core.m.paths.clone()).unwrap();
        // Save config as the actual daemon does before rebuilding the saved folder.
        f.core.m.save().unwrap();
        let restarted = Model::new(restarted.paths).unwrap();
        assert_eq!(restarted.folders["f1"]["recovery_attempted"], true);
        f.core.m.folders.get_mut("f1").unwrap()["run_resync"] = json!(false);
        f.failure("Cannot find prior listings. Must run --resync");
        assert_eq!(f.core.m.folders["f1"]["attention_code"], "resync_failed");
    }
    #[tokio::test]
    async fn failed_jobs_still_keep_conflict_notices() {
        let mut f = Fixture::new();
        f.core.finish_sync("f1",Ok(json!({"job":{"success":false,"error":"network timeout","output":{"output":"Renaming Path2 copy - note.conflict1.md"}},"stats":{}})));
        assert_eq!(
            f.core.m.folders["f1"]["conflicts"],
            json!(["note.conflict1.md"])
        );
    }
    #[tokio::test]
    async fn safety_stop_and_missing_local_require_attention() {
        for (message, code) in [
            ("too many deletes; run with --force", "safety"),
            ("missing_local: folder disappeared", "missing_local"),
        ] {
            let mut f = Fixture::new();
            f.failure(message);
            assert_eq!(f.core.m.folders["f1"]["attention_code"], code);
        }
    }
}
