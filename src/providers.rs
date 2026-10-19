use crate::storage::{Result, text};
use serde_json::{Value, json};
pub fn data() -> &'static Value {
    static CATALOG: std::sync::OnceLock<Value> = std::sync::OnceLock::new();
    CATALOG.get_or_init(|| {
        serde_json::from_str(include_str!("../assets/providers.json")).expect("embedded metadata")
    })
}
pub fn detect(remote: &Value) -> Value {
    let d = data();
    let kind = text(&remote["type"]);
    let key = if kind == "webdav" && ["nextcloud", "owncloud"].contains(&text(&remote["vendor"])) {
        "nextcloud"
    } else {
        kind
    };
    let mut p = if d["providers"][key].is_object() {
        d["providers"][key].clone()
    } else {
        d["generic"].clone()
    };
    p["id"] = json!(if d["providers"][key].is_object() {
        key
    } else {
        "generic"
    });
    if text(&p["id"]) == "generic" {
        p["name"] = json!(kind);
        p["rclone_type"] = json!(kind);
    }
    p
}
pub fn guide(kind: &str) -> Result<Value> {
    let d = data();
    let mut g = d["guides"][kind].clone();
    if !g.is_object() {
        return Err("No own-client guide for this provider".into());
    }
    g["provider"] = json!(kind);
    g["name"] = d["providers"][kind]["name"].clone();
    Ok(g)
}
pub fn check(kind: &str, id: &str, secret: &str) -> Result<()> {
    if id.is_empty() || secret.is_empty() {
        return Err("Both client ID and client secret are needed".into());
    }
    let g = guide(kind)?;
    for (v, s) in [(id, &g["id"]), (secret, &g["secret"])] {
        if !text(&s["reject"]).is_empty()
            && regex::Regex::new(text(&s["reject"]))
                .map_err(|e| e.to_string())?
                .is_match(v)
        {
            return Err(text(&s["reject_error"]).into());
        }
        if !regex::Regex::new(text(&s["pattern"]))
            .map_err(|e| e.to_string())?
            .is_match(v)
        {
            return Err(text(&s["error"]).into());
        }
    }
    if id == secret {
        return Err("Client ID and secret must be different".into());
    }
    Ok(())
}
pub fn warnings(remote: &Value, hidden: &Value) -> Value {
    let kind = text(&remote["type"]);
    if !text(&remote["client_id"]).is_empty() || guide(kind).is_err() {
        return json!([]);
    }
    let required = kind == "drive";
    let code = if required {
        "shared_client_id"
    } else {
        "shared_app"
    };
    if !required && hidden.as_array().is_some_and(|a| a.contains(&json!(code))) {
        return json!([]);
    }
    json!([{"code":code,"level":"warning","dismissible":!required,"text":if required{"Uses rclone's shared Google client. Set up your own client."}else{"Uses rclone's shared OAuth app. Your own client can reduce throttling."}}])
}
