#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use chrono::{Duration, Utc};
#[cfg(test)]
use rand::{RngCore, rngs::OsRng};
use reqwest::blocking::Client;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{collections::HashMap, fs, io::{Read, Write}, path::PathBuf, sync::{Arc, Mutex}, thread, time::Duration as Wait};
use tiny_http::{Header, Request, Response, Server, StatusCode};

const MODELS: [&str; 3] = ["deepseek-v4.1-flash-cn", "deepseek-v4.1-flash-ai", "deepseek-v4.1-flash-auto"];
const BODY_LIMIT: usize = 64 * 1024 * 1024;
fn digest(s: &str) -> String { format!("{:x}", Sha256::digest(s.as_bytes())) }
fn read_json(p: &PathBuf) -> Result<Value, String> {
    serde_json::from_slice(&fs::read(p).map_err(|e| e.to_string())?).map_err(|e| e.to_string())
}
fn error(req: Request, status: u16, code: &str) {
    let mut response=Response::from_string(json!({"error":{"message":code,"type":code}}).to_string())
        .with_status_code(StatusCode(status)).with_header(Header::from_bytes("Content-Type", "application/json").unwrap());
    if status==429 {response.add_header(Header::from_bytes("Retry-After","60").unwrap());}
    let _ = req.respond(response);
}
fn route(method: &str, path: &str) -> bool {
    matches!((method, path), ("GET", "/v1/models") | ("POST", "/v1/chat/completions"))
}
#[derive(Default)]
struct Usage { minute: i64, requests: u64, active: u64 }
struct State { usage: HashMap<String, Usage>, daily: Value }
struct Permit { state: Arc<Mutex<State>>, key: String }
impl Drop for Permit {
    fn drop(&mut self) { if let Ok(mut s) = self.state.lock() { if let Some(u) = s.usage.get_mut(&self.key) { u.active = u.active.saturating_sub(1); } } }
}
fn persist(path: &PathBuf, value: &Value) -> Result<(), String> {
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, serde_json::to_vec_pretty(value).unwrap()).map_err(|e|e.to_string())?;
    fs::rename(&tmp, path).map_err(|e| e.to_string())
}
fn acquire(state: &Arc<Mutex<State>>, hash: &str, key: &Value, path: &PathBuf) -> Result<Permit, u16> {
    let mut state_guard = state.lock().map_err(|_| 503u16)?;
    let now = Utc::now();
    let day = (now + Duration::hours(8)).format("%Y-%m-%d").to_string();
    let month=&day[..7];
    // Migrate known legacy usage without clearing the existing day's allowance.
    if state_guard.daily.get("totalKeys").is_none() { state_guard.daily["totalKeys"]=state_guard.daily["keys"].as_object().map(|v|json!(v)).unwrap_or(json!({})); }
    if state_guard.daily.get("month").is_none() {
        state_guard.daily["monthlyKeys"]=if state_guard.daily["date"].as_str().is_some_and(|d|d.starts_with(month)) {state_guard.daily["keys"].clone()}else{json!({})};
        state_guard.daily["month"]=json!(month);
    }
    if state_guard.daily["month"].as_str()!=Some(month) {state_guard.daily["month"]=json!(month);state_guard.daily["monthlyKeys"]=json!({});}
    if state_guard.daily["date"].as_str() != Some(&day) { state_guard.daily["date"]=json!(day);state_guard.daily["keys"]=json!({}); }
    let count = state_guard.daily["keys"][hash].as_u64().unwrap_or(0);
    let monthly=state_guard.daily["monthlyKeys"][hash].as_u64().unwrap_or(0);
    let total=state_guard.daily["totalKeys"][hash].as_u64().unwrap_or(0);
    for (limit,used) in [(key["dailyRequests"].as_u64().unwrap_or(100),count),(key["monthlyRequests"].as_u64().unwrap_or(0),monthly),(key["totalRequests"].as_u64().unwrap_or(0),total)] {if limit>0 && used>=limit {return Err(429);}}
    let u = state_guard.usage.entry(hash.into()).or_default();
    if u.minute != now.timestamp() / 60 { u.minute = now.timestamp() / 60; u.requests = 0; }
    let rpm=key["requestsPerMinute"].as_u64().unwrap_or(10);
    if u.active >= key["concurrency"].as_u64().unwrap_or(2) || (rpm>0&&u.requests>=rpm) { return Err(429); }
    state_guard.daily["keys"][hash] = json!(count.saturating_add(1));
    state_guard.daily["monthlyKeys"][hash]=json!(monthly.saturating_add(1));state_guard.daily["totalKeys"][hash]=json!(total.saturating_add(1));
    if persist(path, &state_guard.daily).is_err() {state_guard.daily["keys"][hash]=json!(count);state_guard.daily["monthlyKeys"][hash]=json!(monthly);state_guard.daily["totalKeys"][hash]=json!(total);return Err(503);}
    let u = state_guard.usage.get_mut(hash).unwrap();
    u.active += 1; u.requests += 1;
    Ok(Permit { state: state.clone(), key: hash.into() })
}
fn handle(mut req: Request, client: &Client, dir: &PathBuf, state: &Arc<Mutex<State>>) {
    // This dedicated listener never exposes the proxy's health/account metadata or administration.
    if !route(req.method().as_str(), req.url()) { error(req, 404, "not_found"); return; }
    let cfg = match read_json(&dir.join("sharing.json")) { Ok(v) if v["enabled"] == true => v, _ => { error(req, 503, "sharing_disabled"); return; } };
    let auth = req.headers().iter().find(|h| h.field.equiv("Authorization")).and_then(|h| h.value.as_str().strip_prefix("Bearer ")).unwrap_or("");
    let hash = digest(auth);
    let key = cfg["keys"].as_array().and_then(|keys| keys.iter().find(|k| k["enabled"] == true && k["sha256"].as_str() == Some(hash.as_str())));
    let Some(key) = key else { error(req, 401, "invalid_api_key"); return; };
    if key["expiresAt"].as_i64().is_some_and(|v|v>0&&v<=Utc::now().timestamp_millis()) {error(req,403,"api_key_expired");return;}
    let allowed=|id: &str| key["models"].as_array().is_none_or(|a|a.iter().any(|m|m.as_str()==Some(id)));
    if req.method().as_str() == "GET" {
        let models: Vec<Value> = MODELS.iter().filter(|id|allowed(id)).map(|id| json!({"id":id,"object":"model","owned_by":"buddyhub"})).collect();
        let _ = req.respond(Response::from_string(json!({"object":"list","data":models}).to_string()).with_header(Header::from_bytes("Content-Type", "application/json").unwrap()));
        return;
    }
    let counter_id=key["id"].as_str().unwrap_or(&hash);
    let _permit = match acquire(state, counter_id, key, &dir.join("sharing-usage.json")) { Ok(v) => v, Err(code) => { error(req, code, if code == 429 { "sharing_limit_exceeded" } else { "usage_storage_unavailable" }); return; } };
    let limit=cfg["maxBodyMb"].as_u64().filter(|n|(1..=128).contains(n)).map(|n|n as usize*1024*1024).unwrap_or(BODY_LIMIT);
    if req.body_length().is_some_and(|n|n > limit) { error(req, 413, "request_too_large"); return; }
    let mut bytes = Vec::new();
    if req.as_reader().take((limit + 1) as u64).read_to_end(&mut bytes).is_err() { error(req, 400, "invalid_request"); return; }
    if bytes.len() > limit { error(req, 413, "request_too_large"); return; }
    let body: Value = match serde_json::from_slice(&bytes) { Ok(v) => v, Err(_) => { error(req, 400, "invalid_json"); return; } };
    if !body["model"].as_str().is_some_and(|m|MODELS.contains(&m)) { error(req, 400, "model_not_found"); return; }
    if !allowed(body["model"].as_str().unwrap()) {error(req,403,"model_not_allowed");return;}
    let upstream = match read_json(&dir.join("config.json")) { Ok(v) => v, Err(_) => { error(req, 503, "proxy_configuration_unavailable"); return; } };
    let port = upstream["port"].as_u64().unwrap_or(8787);
    let local_key = upstream["localKey"].as_str().unwrap_or("");
    if local_key.is_empty() { error(req, 503, "proxy_configuration_unavailable"); return; }
    // Deliberately replace authentication; never forward client cookies, forwarding headers or arbitrary destinations.
    let response = client.post(format!("http://127.0.0.1:{port}/v1/chat/completions")).bearer_auth(local_key)
        .header("Content-Type", "application/json").body(bytes).send();
    let mut response = match response { Ok(v) => v, Err(_) => { error(req, 502, "local_proxy_unavailable"); return; } };
    let status = response.status().as_u16();
    let content_type = response.headers().get("content-type").and_then(|v|v.to_str().ok()).unwrap_or("application/json").to_owned();
    let retry = response.headers().get("retry-after").and_then(|h|h.to_str().ok()).map(|v|format!("Retry-After: {v}\r\n")).unwrap_or_default();
    // Explicitly flush every network read: tiny_http's normal chunk encoder buffers small SSE events.
    let mut writer = req.into_writer();
    let reason = response.status().canonical_reason().unwrap_or("Response");
    if write!(writer,"HTTP/1.1 {status} {reason}\r\nContent-Type: {content_type}\r\nCache-Control: no-store, no-transform\r\nX-Accel-Buffering: no\r\nTransfer-Encoding: chunked\r\n{retry}\r\n").and_then(|_|writer.flush()).is_err() { return; }
    let mut buf = [0u8; 8192];
    loop {
        match response.read(&mut buf) {
            Ok(0) => { let _ = writer.write_all(b"0\r\n\r\n").and_then(|_|writer.flush()); break; }
            Ok(n) => { if write!(writer,"{n:x}\r\n").and_then(|_|writer.write_all(&buf[..n])).and_then(|_|writer.write_all(b"\r\n")).and_then(|_|writer.flush()).is_err() { break; } }
            Err(_) => break,
        }
    }
}
fn main() {
    if std::env::args().nth(1).as_deref() == Some("--tunnel") {
        let dir = std::env::var_os("BUDDYHUB_DATA").map(PathBuf::from).unwrap_or_else(||PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join(".buddyhub")).join("share");
        let mut command = std::process::Command::new(dir.join("ngrok.exe"));
        command.args(["http", "http://127.0.0.1:8791", "--config"]).arg(dir.join("ngrok.yml"))
            .args(["--inspect=false", "--log"]).arg(dir.join("ngrok.log"))
            .args(["--log-format", "json", "--log-level", "warn"])
            .stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
        #[cfg(windows)]
        { use std::os::windows::process::CommandExt; command.creation_flags(0x08000000); }
        let code = command.status().ok().and_then(|s|s.code()).unwrap_or(1);
        std::process::exit(code);
    }
    let dir = std::env::args().nth(1).map(PathBuf::from).or_else(||std::env::var_os("BUDDYHUB_DATA").map(PathBuf::from)).unwrap_or_else(||PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join(".buddyhub"));
    if !dir.join("sharing.json").exists() { return; }
    let daily = if dir.join("sharing-usage.json").exists() { match read_json(&dir.join("sharing-usage.json")) { Ok(v) => v, Err(_) => return } } else { json!({}) };
    let state = Arc::new(Mutex::new(State { usage: HashMap::new(), daily }));
    let bind=std::env::args().nth(2).unwrap_or_else(||"127.0.0.1:8791".into());
    if bind.parse::<std::net::SocketAddr>().ok().is_none_or(|a|!a.ip().is_loopback()) {return;}
    let Ok(server) = Server::http(&bind) else { return; };
    let client = Client::builder().no_proxy().redirect(reqwest::redirect::Policy::none()).connect_timeout(Wait::from_secs(10)).timeout(Wait::from_secs(900)).build().unwrap();
    let workers = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    for req in server.incoming_requests() {
        use std::sync::atomic::Ordering;
        if workers.fetch_add(1, Ordering::SeqCst) >= 16 { workers.fetch_sub(1, Ordering::SeqCst); error(req, 503, "gateway_busy"); continue; }
        let (client, dir, state, workers) = (client.clone(), dir.clone(), state.clone(), workers.clone());
        thread::spawn(move || { handle(req, &client, &dir, &state); workers.fetch_sub(1, Ordering::SeqCst); });
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn public_routes_are_exact() {
        assert!(route("GET", "/v1/models")); assert!(route("POST", "/v1/chat/completions"));
        for p in ["/health", "/admin/status", "/v1/models?x=1", "/v1/../admin/status", "/"] { assert!(!route("GET", p)); }
        assert!(!route("GET", "/v1/chat/completions"));
    }
    #[test]
    fn limits_persist_and_release() {
        let mut random = [0u8; 12]; OsRng.fill_bytes(&mut random);
        let path = std::env::temp_dir().join(format!("buddyhub-share-{}.json", digest(&format!("{random:?}"))));
        let state = Arc::new(Mutex::new(State{usage:HashMap::new(), daily:json!({})}));
        let key = json!({"dailyRequests":2,"requestsPerMinute":10,"concurrency":1});
        let permit = acquire(&state,"key",&key,&path).unwrap();
        assert!(matches!(acquire(&state,"key",&key,&path),Err(429)));
        drop(permit);
        drop(acquire(&state,"key",&key,&path).unwrap());
        assert!(matches!(acquire(&state,"key",&key,&path),Err(429)));
        assert_eq!(read_json(&path).unwrap()["keys"]["key"],2);
        let _=fs::remove_file(path);
    }
    #[test]
    fn monthly_and_lifetime_limits_survive_day_rollover_and_zero_is_unlimited() {
        let path=std::env::temp_dir().join(format!("buddyhub-share-{}.json",std::process::id()));
        let month=(Utc::now()+Duration::hours(8)).format("%Y-%m").to_string();
        let state=Arc::new(Mutex::new(State{usage:HashMap::new(),daily:json!({"date":"2000-01-01","keys":{"id":3},"month":month,"monthlyKeys":{"id":4},"totalKeys":{"id":8}})}));
        let mut key=json!({"dailyRequests":0,"monthlyRequests":5,"totalRequests":10,"requestsPerMinute":0,"concurrency":2});
        drop(acquire(&state,"id",&key,&path).unwrap());
        assert!(matches!(acquire(&state,"id",&key,&path),Err(429)));
        key["monthlyRequests"]=json!(0);drop(acquire(&state,"id",&key,&path).unwrap());
        assert!(matches!(acquire(&state,"id",&key,&path),Err(429)));
        key["totalRequests"]=json!(0);drop(acquire(&state,"id",&key,&path).unwrap());
        assert_eq!(read_json(&path).unwrap()["totalKeys"]["id"],11);
        let _=fs::remove_file(path);
    }
}
