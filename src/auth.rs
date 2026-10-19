use crate::{
    daemon::Event,
    providers,
    rc::{self, Rc},
    storage::*,
};
use serde_json::{Value, json};
use std::{process::Stdio, time::Duration};
use tokio::{
    io::AsyncReadExt,
    sync::{mpsc, watch},
};
pub async fn run(
    action: String,
    args: Value,
    remote: Value,
    rcc: Rc,
    tx: mpsc::Sender<Event>,
    mut cancel: watch::Receiver<bool>,
) -> Result<Value> {
    let name = if action == "add_oauth" || action == "add_credentials" {
        clean_name(text(&args["name"]))?
    } else {
        text(&args["remote"]).to_owned()
    };
    let provider = text(&args["provider"]);
    let metadata = providers::data();
    let kind = if action.starts_with("add_") {
        text(&metadata["providers"][provider]["rclone_type"]).to_owned()
    } else {
        text(&remote["type"]).to_owned()
    };
    if kind.is_empty() {
        return Err("Unknown provider".into());
    }
    let mut secrets = vec![];
    let mut params = json!({});
    if action == "add_credentials" {
        let o = &args["options"];
        match provider {
            "webdav" | "nextcloud" => {
                let mut url = text(&o["url"]).trim().to_owned();
                if url.is_empty() {
                    return Err("Server address is required".into());
                }
                if !url.starts_with("https://") && !url.starts_with("http://") {
                    url = format!("https://{url}")
                }
                params["url"] = json!(url);
                params["vendor"] = json!(if provider == "nextcloud" {
                    "nextcloud"
                } else {
                    let v = text(&o["vendor"]);
                    if v.is_empty() { "other" } else { v }
                });
                for k in ["user", "pass"] {
                    if !text(&o[k]).is_empty() {
                        params[k] = o[k].clone();
                    }
                }
                if provider == "nextcloud" {
                    if text(&o["user"]).is_empty() || text(&o["pass"]).is_empty() {
                        return Err("Username and app password are required".into());
                    }
                    if !url.contains("/remote.php/") {
                        params["url"] = json!(format!(
                            "{}/remote.php/dav/files/{}",
                            url.trim_end_matches('/'),
                            text(&o["user"])
                        ));
                    }
                }
            }
            "s3" => {
                params["provider"] = json!(if text(&o["provider"]).is_empty() {
                    "Other"
                } else {
                    text(&o["provider"])
                });
                for k in ["endpoint", "access_key_id", "secret_access_key", "region"] {
                    if !text(&o[k]).is_empty() {
                        params[k] = o[k].clone();
                    }
                }
                if text(&o["access_key_id"]).is_empty() || text(&o["secret_access_key"]).is_empty()
                {
                    return Err("Access key and secret are required".into());
                }
            }
            "protondrive" => {
                for k in ["username", "password", "2fa"] {
                    if !text(&o[k]).is_empty() {
                        params[k] = o[k].clone();
                    }
                }
                if text(&o["username"]).is_empty() || text(&o["password"]).is_empty() {
                    return Err("Proton email and password are required".into());
                }
            }
            _ => return Err("This provider does not use credentials".into()),
        }
        for (k, v) in params.as_object().unwrap() {
            if ["pass", "password", "secret_access_key", "2fa"].contains(&k.as_str()) {
                secrets.push(text(v).to_owned());
            }
        }
    } else {
        let mut id = text(&args["client_id"]).trim().to_owned();
        let mut secret = text(&args["client_secret"]).trim().to_owned();
        if action == "reconnect" {
            let conf = rcc.call("config/get", json!({"name":name})).await?;
            id = text(&conf["client_id"]).to_owned();
            secret = text(&conf["client_secret"]).to_owned();
        } else if !id.is_empty() || !secret.is_empty() || action == "set_client_id" {
            providers::check(&kind, &id, &secret)?;
        }
        let token = authorize(&kind, &id, &secret, &tx, &mut cancel).await?;
        secrets.extend([secret.clone(), token.clone()]);
        params["token"] = json!(token);
        if !id.is_empty() {
            params["client_id"] = json!(id);
            params["client_secret"] = json!(secret);
        }
        if action == "add_oauth" {
            params["config_refresh_token"] = json!("false");
        }
        if kind == "drive" {
            params["scope"] = json!("drive");
        }
        let token_value: Value = serde_json::from_str(&token).map_err(|_| "Invalid OAuth token")?;
        let bearer = text(&token_value["access_token"]);
        for key in ["access_token", "refresh_token", "id_token"] {
            if let Some(value) = token_value[key].as_str()
                && !value.is_empty()
            {
                secrets.push(value.to_owned());
            }
        }
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(15))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|_| "Cannot initialize provider client")?;
        if kind == "onedrive" && action == "add_oauth" {
            let response = client
                .get("https://graph.microsoft.com/v1.0/me/drive")
                .bearer_auth(bearer)
                .send()
                .await
                .map_err(|_| "Could not resolve OneDrive account")?;
            let data: Value = response
                .error_for_status()
                .map_err(|_| "OneDrive account lookup failed")?
                .json()
                .await
                .map_err(|_| "Invalid OneDrive account response")?;
            if text(&data["id"]).is_empty() {
                return Err("OneDrive did not return a drive ID".into());
            }
            params["drive_id"] = data["id"].clone();
            params["drive_type"] = json!(if text(&data["driveType"]).is_empty() {
                "personal"
            } else {
                text(&data["driveType"])
            });
        }
        if kind == "pcloud" {
            let mut found = false;
            for host in ["api.pcloud.com", "eapi.pcloud.com"] {
                if let Ok(response) = client
                    .get(format!("https://{host}/userinfo"))
                    .bearer_auth(bearer)
                    .send()
                    .await
                    && let Ok(data) = response.json::<Value>().await
                    && data["result"] == 0
                {
                    params["hostname"] = json!(host);
                    found = true;
                    break;
                }
            }
            if !found {
                return Err(
                    "Could not identify the pCloud account region; account was not changed".into(),
                );
            }
        }
    }
    let method = if action.starts_with("add_") {
        "config/create"
    } else {
        "config/update"
    };
    let mut request =
        json!({"name":name,"parameters":params,"opt":{"nonInteractive":true,"obscure":true}});
    if method == "config/create" {
        request["type"] = json!(kind);
    }
    if let Err(mut e) = rcc.call(method, request).await {
        for s in &secrets {
            if !s.is_empty() {
                e = e.replace(s, "[redacted]");
            }
        }
        return Err(e);
    }
    if action == "add_credentials"
        && let Err(mut e) = rcc
            .call_timeout(
                "operations/list",
                json!({"fs":format!("{name}:"),"remote":"","opt":{"dirsOnly":true}}),
                30,
            )
            .await
    {
        let _ = rcc.call("config/delete", json!({"name":name})).await;
        for s in &secrets {
            if !s.is_empty() {
                e = e.replace(s, "[redacted]");
            }
        }
        return Err(format!("Connection failed: {e}"));
    }
    let _ = rcc.call("fscache/clear", json!({})).await;
    Ok(
        json!({"remote":name,"message":format!("Connected {name}"),"action":action,"stream":args["stream"].as_bool().unwrap_or(true),"mount_path":args["mount_path"]}),
    )
}
async fn authorize(
    kind: &str,
    id: &str,
    secret: &str,
    tx: &mpsc::Sender<Event>,
    cancel: &mut watch::Receiver<bool>,
) -> Result<String> {
    let mut c = rc::child_command("rclone");
    c.args(["authorize", kind])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if !id.is_empty() {
        c.env(format!("RCLONE_{}_CLIENT_ID", kind.to_uppercase()), id);
    }
    if !secret.is_empty() {
        c.env(
            format!("RCLONE_{}_CLIENT_SECRET", kind.to_uppercase()),
            secret,
        );
    }
    let mut child = c.spawn().map_err(|e| e.to_string())?;
    let stdout = child.stdout.take().ok_or("Missing stdout")?;
    let stderr = child.stderr.take().ok_or("Missing stderr")?;
    let t = tx.clone();
    let out = tokio::spawn(async move {
        let mut bytes = vec![];
        stdout
            .take(1024 * 1024 + 1)
            .read_to_end(&mut bytes)
            .await
            .map_err(|_| "Cannot read authorization output")?;
        if bytes.len() > 1024 * 1024 {
            return Err("Authorization output too large");
        }
        Ok(bytes)
    });
    let err = tokio::spawn(async move {
        let mut reader = stderr;
        let mut buf = [0; 4096];
        let mut tail = String::new();
        let rx = regex::Regex::new(r"https?://127\.0\.0\.1:\d+/auth[^\s]*").unwrap();
        while let Ok(n) = reader.read(&mut buf).await {
            if n == 0 {
                break;
            }
            tail.push_str(&String::from_utf8_lossy(&buf[..n]));
            if let Some(m) = rx.find(&tail) {
                let _ = t.send(Event::AuthUrl(m.as_str().to_owned())).await;
            }
            if tail.len() > 16384 {
                tail = tail
                    .chars()
                    .rev()
                    .take(8192)
                    .collect::<String>()
                    .chars()
                    .rev()
                    .collect();
            }
        }
    });
    let result = tokio::select! {s=child.wait()=>s.map_err(|e|e.to_string()),_=tokio::time::sleep(Duration::from_secs(300))=>Err("Sign-in timed out".into()),_=cancel.changed()=>Err("Sign-in cancelled".into())};
    if result.as_ref().is_err() || !result.as_ref().unwrap().success() {
        let _ = child.kill().await;
        let _ = child.wait().await;
        out.abort();
        err.abort();
        return Err(result
            .err()
            .unwrap_or_else(|| "Browser sign-in failed".into()));
    }
    let bytes = out
        .await
        .map_err(|_| "Authorization task failed")?
        .map_err(str::to_owned)?;
    let _ = err.await;
    let raw = String::from_utf8_lossy(&bytes);
    for (i, c) in raw.char_indices() {
        if c == '{' {
            let mut stream = serde_json::Deserializer::from_str(&raw[i..]).into_iter::<Value>();
            if let Some(Ok(token)) = stream.next()
                && token["access_token"].is_string()
            {
                return Ok(token.to_string());
            }
        }
    }
    Err("Browser sign-in did not return a token".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::{
        io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
        net::UnixListener,
    };
    async fn server(
        fail: bool,
    ) -> (
        Rc,
        tokio::task::JoinHandle<Vec<(String, Value)>>,
        std::path::PathBuf,
    ) {
        let root = std::env::temp_dir().join(format!("ck-auth-{:08x}", new_id()));
        private(&root).unwrap();
        let socket = root.join("rc.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let rc = Rc::new(&socket).unwrap();
        let task = tokio::spawn(async move {
            let mut seen = vec![];
            for i in 0..3 {
                let (stream, _) = listener.accept().await.unwrap();
                let mut r = BufReader::new(stream);
                let mut first = String::new();
                r.read_line(&mut first).await.unwrap();
                let mut length = 0;
                loop {
                    let mut line = String::new();
                    r.read_line(&mut line).await.unwrap();
                    if line == "\r\n" {
                        break;
                    }
                    if let Some((key, value)) = line.split_once(':')
                        && key.eq_ignore_ascii_case("content-length")
                    {
                        length = value.trim().parse::<usize>().unwrap();
                    }
                }
                let mut body = vec![0; length];
                r.read_exact(&mut body).await.unwrap();
                seen.push((
                    first.split_whitespace().nth(1).unwrap().to_owned(),
                    serde_json::from_slice(&body).unwrap(),
                ));
                let error = fail && i == 1;
                let body = if error {
                    r#"{"error":"Connection failed with test-secret"}"#
                } else {
                    "{}"
                };
                let status = if error {
                    "500 Internal Server Error"
                } else {
                    "200 OK"
                };
                r.get_mut().write_all(format!("HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\nContent-Type: application/json\r\n\r\n{body}",body.len()).as_bytes()).await.unwrap();
            }
            seen
        });
        (rc, task, root)
    }
    #[tokio::test]
    async fn nextcloud_credentials_preserve_provider_options() {
        let (rc, server, root) = server(false).await;
        let (tx, _) = mpsc::channel(8);
        let (_, cancel) = watch::channel(false);
        let result=run("add_credentials".into(),json!({"name":"Home","provider":"nextcloud","options":{"url":"https://cloud.example","user":"alice","pass":"test-secret"},"stream":false}),json!({}),rc,tx,cancel).await.unwrap();
        assert_eq!(result["remote"], "Home");
        let seen = server.await.unwrap();
        assert_eq!(seen[0].0, "/config/create");
        assert_eq!(
            seen[0].1["parameters"]["url"],
            "https://cloud.example/remote.php/dav/files/alice"
        );
        assert_eq!(seen[0].1["parameters"]["vendor"], "nextcloud");
        assert_eq!(seen[0].1["parameters"]["pass"], "test-secret");
        assert_eq!(seen[0].1["opt"]["obscure"], true);
        std::fs::remove_dir_all(root).unwrap();
    }
    #[tokio::test]
    async fn failed_credentials_roll_back_and_redact_secrets() {
        let (rc, server, root) = server(true).await;
        let (tx, _) = mpsc::channel(8);
        let (_, cancel) = watch::channel(false);
        let error=run("add_credentials".into(),json!({"name":"Home","provider":"nextcloud","options":{"url":"https://cloud.example","user":"alice","pass":"test-secret"}}),json!({}),rc,tx,cancel).await.unwrap_err();
        assert!(!error.contains("test-secret"));
        assert!(error.contains("[redacted]"));
        let seen = server.await.unwrap();
        assert_eq!(seen[2].0, "/config/delete");
        std::fs::remove_dir_all(root).unwrap();
    }
}
