use crate::{
    filters, providers,
    storage::{self, *},
};
use serde_json::{Value, json};
use std::collections::BTreeMap;
pub struct Model {
    pub paths: Paths,
    pub cfg: Value,
    pub saved: Value,
    pub remotes: Value,
    pub drives: BTreeMap<String, Value>,
    pub folders: BTreeMap<String, Value>,
    pub engine: Value,
    pub auth: Value,
    pub recent: Value,
    pub transfers: Value,
    pub online: bool,
    pub global: String,
    pub backup_files: u64,
}
impl Model {
    pub fn new(paths: Paths) -> Result<Self> {
        let (cfg, saved) = storage::load(&paths)?;
        let global = std::fs::read_to_string(paths.filter()).map_err(|e| e.to_string())?;
        let mut m = Self {
            paths,
            cfg,
            saved,
            remotes: json!({}),
            drives: BTreeMap::new(),
            folders: BTreeMap::new(),
            engine: json!({"state":"starting","error":"","version":""}),
            auth: json!({"busy":false,"waiting":false,"url":"","error":"","success":"","remote":""}),
            recent: json!([]),
            transfers: json!({"count":0,"speed":0,"names":[]}),
            online: true,
            global,
            backup_files: 0,
        };
        m.rebuild()?;
        Ok(m)
    }
    pub fn save(&self) -> Result<()> {
        write_json(&self.paths.config.join("config.json"), &self.cfg)
    }
    pub fn persist(&mut self) -> Result<()> {
        for (id, f) in &self.folders {
            let mut s = json!({});
            for k in [
                "initialized",
                "last_sync",
                "last_changes",
                "conflicts",
                "attention",
                "attention_code",
                "error",
                "filters_md5",
                "resync_reason",
                "recovery_attempted",
            ] {
                s[k] = f[k].clone();
            }
            self.saved["folders"][id] = s;
        }
        write_json(&self.paths.state.join("state.json"), &self.saved)
    }
    pub fn mount_root(&self) -> std::path::PathBuf {
        expand(text(&self.cfg["mount_root"]))
    }
    pub fn local_root(&self) -> std::path::PathBuf {
        expand(text(&self.cfg["local_root"]))
    }
    pub fn point(&self, name: &str) -> std::path::PathBuf {
        let p = text(&self.cfg["remotes"][name]["mount_path"]);
        if p.is_empty() {
            self.mount_root().join(name)
        } else {
            expand(p)
        }
    }
    pub fn rules(&self, f: &Value) -> String {
        if f["excludes"].as_array().is_some_and(|a| !a.is_empty()) {
            filters::override_text(
                &f["excludes"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|s| text(s).to_owned())
                    .collect::<Vec<_>>(),
            )
        } else {
            self.global.clone()
        }
    }
    pub fn rebuild(&mut self) -> Result<()> {
        let mut keep = vec![];
        for (name, r) in self.cfg["remotes"].as_object().ok_or("Invalid remotes")? {
            for spec in r["folders"].as_array().ok_or("Invalid folders")? {
                let id = text(&spec["id"]).to_owned();
                keep.push(id.clone());
                if !self.folders.contains_key(&id) {
                    let mut f = json!({"id":id,"remote":name,"path":text(&spec["path"]),"local":expand(text(&spec["local"])),"paused":false,"label":"","excludes":[],"initialized":false,"last_sync":0,"last_changes":0,"conflicts":[],"attention":"","attention_code":"","error":"","error_kind":"","filters_md5":"","resync_reason":"","recovery_attempted":false,"job_id":0,"next_due":now()+5.0,"retry_at":0,"backoff":0,"dirty_since":0,"last_event":0,"followup_only":false,"dirty_during_run":false,"force_next":false,"rerun":false,"progress":{},"watching":false,"watch_error":"","waiting_network":false,"moving":[]});
                    if let Some(saved) = self.saved["folders"][&id].as_object() {
                        for (k, v) in saved {
                            f[k] = v.clone();
                        }
                    }
                    self.folders.insert(id.clone(), f);
                }
                let f = self.folders.get_mut(&id).unwrap();
                f["paused"] = json!(spec["paused"].as_bool().unwrap_or(false));
                f["label"] = json!(text(&spec["label"]));
                f["excludes"] = spec["excludes"]
                    .as_array()
                    .map(|a| json!(a))
                    .unwrap_or(json!([]));
            }
        }
        self.folders.retain(|id, _| keep.contains(id));
        let rules: Vec<_> = self
            .folders
            .iter()
            .map(|(id, f)| (id.clone(), filters::digest(&self.rules(f))))
            .collect();
        for (id, digest) in rules {
            let f = self.folders.get_mut(&id).unwrap();
            let old = text(&f["filters_md5"]);
            if f["initialized"] == true && !old.is_empty() && old != digest {
                f["resync_reason"] = json!("filters");
            }
        }
        Ok(())
    }
    pub fn load_remotes(&mut self, raw: Value) {
        let mut clean = json!({});
        if let Some(remotes) = raw.as_object() {
            for (name, r) in remotes {
                if !r["type"].is_string() {
                    continue;
                }
                let p = providers::detect(r);
                clean[name] = json!({"type":r["type"],"vendor":r["vendor"],"provider":p,"custom_client":!text(&r["client_id"]).is_empty(),"warnings":providers::warnings(r,&self.cfg["remotes"][name]["hidden_hints"])});
                self.drives.entry(name.clone()).or_insert(json!({"mount_state":"off","error":"","error_kind":"","retry_at":0,"backoff":0,"uploads":0,"cache_bytes":0,"mount_point":"","meta_path":"","data_path":""}));
                if !self.cfg["remotes"][name].is_object() {
                    self.cfg["remotes"][name] = json!({"stream":false,"folders":[],"label":null,"mount_path":null,"hidden_hints":[]});
                }
            }
        }
        self.remotes = clean;
    }
    pub fn folder_key(&self, args: &Value) -> Result<String> {
        let id = text(&args["id"]);
        self.folders
            .contains_key(id)
            .then(|| id.to_owned())
            .ok_or_else(|| "Unknown local folder".into())
    }
    pub fn drive_key(&self, args: &Value) -> Result<String> {
        let n = text(&args["remote"]);
        if self.remotes[n].is_object() {
            Ok(n.to_owned())
        } else {
            Err(format!("Unknown drive: {n}"))
        }
    }
    pub fn folder_spec_mut(&mut self, id: &str) -> Result<&mut Value> {
        for (_, r) in self.cfg["remotes"]
            .as_object_mut()
            .ok_or("Invalid remotes")?
        {
            for f in r["folders"].as_array_mut().ok_or("Invalid folders")? {
                if f["id"] == id {
                    return Ok(f);
                }
            }
        }
        Err("Unknown local folder".into())
    }
    pub fn validate_local(&self, local: &std::path::Path, name: &str, rpath: &str) -> Result<()> {
        dedicated(local)?;
        if overlaps(local, &self.mount_root())
            || overlaps(local, &self.paths.config)
            || overlaps(local, &self.paths.state)
        {
            return Err("Local folder overlaps streaming or application data".into());
        }
        for n in self.drives.keys() {
            if overlaps(local, &self.point(n)) {
                return Err(format!("Local folder overlaps mount of {n}"));
            }
        }
        for f in self.folders.values() {
            if overlaps(local, &expand(text(&f["local"]))) {
                return Err("Local folders must not overlap".into());
            }
            let p = text(&f["path"]);
            if text(&f["remote"]) == name
                && (p.is_empty()
                    || rpath.is_empty()
                    || p == rpath
                    || p.starts_with(&format!("{rpath}/"))
                    || rpath.starts_with(&format!("{p}/")))
            {
                return Err("Remote folders must not overlap".into());
            }
        }
        Ok(())
    }
    pub fn folder_state(&self, f: &Value) -> Value {
        let running = f["job_id"].as_i64().unwrap_or(0) != 0;
        let state = if running {
            "syncing"
        } else if f["paused"] == true {
            "paused"
        } else if !text(&f["attention"]).is_empty() {
            "attention"
        } else if f["waiting_network"] == true {
            "waiting"
        } else if !text(&f["error"]).is_empty() && f["retry_at"].as_f64().unwrap_or(0.0) > now() {
            "error"
        } else if f["initialized"] != true || !text(&f["resync_reason"]).is_empty() {
            "pending"
        } else if f["dirty_since"].as_f64().unwrap_or(0.0) > 0.0 && f["followup_only"] != true {
            "changes"
        } else {
            "synced"
        };
        let default = std::path::Path::new(text(&f["local"]))
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        let label = text(&f["label"]);
        json!({"id":f["id"],"remote":f["remote"],"path":f["path"],"name":if label.is_empty(){&default}else{label},"label":label,"defaultName":default,"local":f["local"],"state":state,"paused":f["paused"],"localChanges":f["dirty_since"].as_f64().unwrap_or(0.0)>0.0&&f["followup_only"]!=true,"initialized":f["initialized"],"lastSync":f["last_sync"].as_f64().unwrap_or(0.0)as u64,"lastChanges":f["last_changes"],"nextSync":if running{0}else{f["next_due"].as_f64().unwrap_or(0.0).max(f["retry_at"].as_f64().unwrap_or(0.0))as u64},"error":f["error"],"errorKind":f["error_kind"],"attention":f["attention"],"attentionCode":f["attention_code"],"resync":if running{f["run_resync"].as_bool().unwrap_or(false)}else{f["initialized"]!=true||!text(&f["resync_reason"]).is_empty()},"progress":f["progress"],"conflicts":f["conflicts"].as_array().unwrap_or(&vec![]).iter().take(5).collect::<Vec<_>>(),"conflictCount":f["conflicts"].as_array().map_or(0,Vec::len),"watching":f["watching"],"watchError":f["watch_error"],"excludes":f["excludes"]})
    }
    pub fn state(&self) -> Value {
        let (mut streaming, mut mounted, mut attention, mut syncing, mut uploads) =
            (0, 0, 0, 0, 0u64);
        let mut drives = vec![];
        if let Some(remotes) = self.remotes.as_object() {
            for (name, r) in remotes {
                let c = &self.cfg["remotes"][name];
                let d = &self.drives[name];
                let p = &r["provider"];
                let q = &self.saved["quota"][name];
                let total = q["total"].as_u64().unwrap_or(0);
                let used = q["used"].as_u64().unwrap_or(0);
                let mut folders: Vec<_> = self
                    .folders
                    .values()
                    .filter(|f| f["remote"] == *name)
                    .map(|f| self.folder_state(f))
                    .collect();
                folders.sort_by_key(|f| text(&f["path"]).to_lowercase());
                streaming += usize::from(c["stream"] == true);
                mounted += usize::from(d["mount_state"] == "mounted");
                attention += usize::from(["error", "foreign"].contains(&text(&d["mount_state"])));
                uploads += d["uploads"].as_u64().unwrap_or(0);
                for f in &folders {
                    syncing += usize::from(f["state"] == "syncing");
                    attention += usize::from(
                        !text(&f["attention"]).is_empty()
                            || f["state"] == "error"
                            || f["conflictCount"].as_u64().unwrap_or(0) > 0,
                    );
                }
                drives.push(json!({"name":name,"label":if text(&c["label"]).is_empty(){name.as_str()}else{text(&c["label"])},"type":r["type"],"provider":p["name"],"providerId":p["id"],"glyph":p["glyph"],"color":p["color"],"stream":c["stream"]==true,"mountState":d["mount_state"],"mounted":d["mount_state"]=="mounted","mountPath":self.point(name),"customPath":!text(&c["mount_path"]).is_empty(),"error":d["error"],"errorKind":d["error_kind"],"retrying":d["retry_at"].as_f64().unwrap_or(0.0)>now(),"quotaKnown":total>0,"quotaTotal":total,"quotaUsed":used,"quotaFree":q["free"].as_u64().unwrap_or(0),"quotaPercent":if total>0{used as f64/total as f64*100.0}else{0.0},"uploads":d["uploads"],"cacheBytes":d["cache_bytes"],"warnings":r["warnings"],"customClient":r["custom_client"],"canCustomClient":providers::guide(text(&r["type"])).is_ok(),"folders":folders}));
            }
        }
        let mut settings = json!({});
        for k in SETTINGS {
            settings[k] = self.cfg[k].clone();
        }
        let mount = self.mount_root();
        let local = self.local_root();
        let cloud = if mount.parent() == local.parent() {
            mount.parent().unwrap_or(&mount).to_owned()
        } else {
            mount.clone()
        };
        json!({"version":env!("CARGO_PKG_VERSION"),"engine":self.engine,"online":self.online,"mountRoot":mount,"localRoot":local,"cloudRoot":cloud,"settings":settings,"summary":{"drives":drives.len(),"streaming":streaming,"mounted":mounted,"folders":self.folders.len(),"syncing":syncing,"attention":attention,"uploads":uploads},"drives":drives,"transfers":self.transfers,"recent":self.recent,"auth":self.auth,"paths":{"log":self.paths.state.join("daemon.log"),"rcloneLog":self.paths.state.join("rclone.log"),"filters":self.paths.filter(),"config":self.paths.config.join("config.json"),"backups":self.paths.state.join("backups")},"backupFiles":self.backup_files,"filters":{"excludes":filters::excludes(&self.global),"otherRules":filters::others(&self.global)}})
    }
}
