#![cfg_attr(not(test), windows_subsystem = "windows")]
mod proxy;
mod service;
#[cfg(test)]
mod tests;

use aes_gcm::{Aes256Gcm, KeyInit, Nonce, aead::Aead};
use base64::{Engine as _, engine::general_purpose::STANDARD as B64};
use chrono::{Duration, Utc};
use rand::{RngCore, rngs::OsRng};
use reqwest::blocking::Client;
use serde_json::{Value as V, json};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    thread,
    time::{Duration as Wait, Instant},
};
use tiny_http::{Header, Request, Response, Server, StatusCode};
type Result<T> = std::result::Result<T, String>;

fn s(v: &V, k: &str) -> String {
    v[k].as_str().unwrap_or("").to_owned()
}
fn n(v: &V, k: &str) -> f64 {
    v[k].as_f64()
        .or_else(|| v[k].as_str().and_then(|x| x.parse().ok()))
        .unwrap_or(0.0)
}
fn b(v: &V, k: &str) -> bool {
    v[k].as_bool().unwrap_or(false)
}
fn now() -> i64 {
    Utc::now().timestamp_millis()
}
fn china() -> V {
    let x = Utc::now() + Duration::hours(8);
    json!({"date":x.format("%Y-%m-%d").to_string(),"time":x.format("%H:%M").to_string()})
}
fn random(len: usize) -> Vec<u8> {
    let mut b = vec![0; len];
    OsRng.fill_bytes(&mut b);
    b
}
fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}
fn atomic(p: &Path, data: &[u8]) -> Result<()> {
    if let Some(parent) = p.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let tmp = p.with_extension(format!("{}.tmp", hex(&random(6))));
    fs::write(&tmp, data).map_err(|e| e.to_string())?;
    fs::rename(&tmp, p).map_err(|e| e.to_string())
}
fn safe(x: &str) -> String {
    let mut out = String::new();
    for word in x.split_whitespace() {
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(if word.starts_with("eyJ") || word.starts_with("sk-") {
            "[隐藏]"
        } else {
            word
        });
    }
    out
}
fn respond(req: Request, status: u16, data: V) {
    let body = serde_json::to_vec(&data).unwrap_or_default();
    let _ = req.respond(
        Response::from_data(body)
            .with_status_code(StatusCode(status))
            .with_header(
                Header::from_bytes("Content-Type", "application/json; charset=utf-8").unwrap(),
            ),
    );
}
fn header(req: &Request, name: &'static str) -> String {
    req.headers()
        .iter()
        .find(|h| h.field.equiv(name))
        .map(|h| h.value.as_str().to_owned())
        .unwrap_or_default()
}
fn body(req: &mut Request) -> Result<V> {
    let mut bytes = Vec::new();
    req.as_reader()
        .take(16 * 1024 * 1024 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() > 16 * 1024 * 1024 {
        return Err("请求过大".into());
    }
    if bytes.is_empty() {
        return Ok(json!({}));
    }
    serde_json::from_slice(&bytes).map_err(|_| "JSON 格式错误".into())
}

struct State {
    config: V,
    accounts: Vec<V>,
    inflight: HashMap<String, usize>,
    cursor: HashMap<String, usize>,
    children: HashMap<String, u32>,
}
struct App {
    state: Mutex<State>,
    data: PathBuf,
    key: Vec<u8>,
    http: Client,
    auth: Mutex<HashMap<String, Arc<Mutex<()>>>>,
    checkin: Mutex<()>,
    services: Mutex<()>,
    proxy: Mutex<Option<Arc<Server>>>,
    snapshot: Mutex<(Instant, V)>,
}
fn initialize_data(data: &Path) -> Result<()> {
    fs::create_dir_all(data).map_err(|e| e.to_string())?;
    let config = data.join("config.json");
    let key_path = data.join("vault.key");
    let vault = data.join("accounts.enc");
    if vault.exists() && !key_path.exists() {
        return Err("账号库密钥丢失，请从备份恢复；不会重置已有账号".into());
    }
    if !config.exists() {
        let cfg = json!({"version":1,"port":8787,"adminPort":8790,
            "defaultSite":"cn","localKey":hex(&random(24)),"adminKey":hex(&random(32)),
            "checkinEnabled":true,"checkinTime":"09:00","startupGuard":true,"proxyEnabled":true,
            "workbuddyExe":"","workbuddyIntlExe":"","authDirs":[],
            "services":[{"id":"workbuddy","name":"WorkBuddy 反代","kind":"internal","port":8787,
                "url":"http://127.0.0.1:8787/v1","description":"中国版 / 国际版分区号池","autoStart":true}]});
        atomic(&config, &serde_json::to_vec_pretty(&cfg).unwrap())?;
    }
    if !key_path.exists() {
        atomic(&key_path, &random(32))?;
    }
    if !vault.exists() {
        let key = fs::read(&key_path).map_err(|e| e.to_string())?;
        let iv = random(12);
        let cipher = Aes256Gcm::new_from_slice(&key).map_err(|_| "账号库密钥无效")?
            .encrypt(Nonce::from_slice(&iv), b"[]".as_ref()).map_err(|_| "账号库初始化失败")?;
        let split = cipher.len() - 16;
        let blob = json!({"version":1,"iv":B64.encode(iv),"tag":B64.encode(&cipher[split..]),"data":B64.encode(&cipher[..split])});
        atomic(&vault, &serde_json::to_vec(&blob).unwrap())?;
    }
    Ok(())
}
impl App {
    fn new() -> Result<Arc<Self>> {
        let data = std::env::var_os("BUDDYHUB_DATA")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                PathBuf::from(std::env::var_os("USERPROFILE").unwrap()).join(".buddyhub")
            });
        initialize_data(&data)?;
        let config: V =
            serde_json::from_slice(&fs::read(data.join("config.json")).map_err(|e| e.to_string())?)
                .map_err(|e| e.to_string())?;
        let key = fs::read(data.join("vault.key")).map_err(|e| e.to_string())?;
        let blob: V = serde_json::from_slice(
            &fs::read(data.join("accounts.enc")).map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;
        let iv = B64.decode(s(&blob, "iv")).map_err(|e| e.to_string())?;
        if iv.len() != 12 {
            return Err("账号库格式错误".into());
        }
        let mut cipher = B64.decode(s(&blob, "data")).map_err(|e| e.to_string())?;
        cipher.extend(B64.decode(s(&blob, "tag")).map_err(|e| e.to_string())?);
        let plain = Aes256Gcm::new_from_slice(&key)
            .map_err(|_| "账号库密钥无效")?
            .decrypt(Nonce::from_slice(&iv), cipher.as_ref())
            .map_err(|_| "账号库解密失败")?;
        let accounts = serde_json::from_slice(&plain).map_err(|e| e.to_string())?;
        let http = Client::builder()
            .timeout(Wait::from_secs(720))
            .connect_timeout(Wait::from_secs(15))
            .pool_idle_timeout(Wait::from_secs(30))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|e| e.to_string())?;
        Ok(Arc::new(Self {
            state: Mutex::new(State {
                config,
                accounts,
                inflight: HashMap::new(),
                cursor: HashMap::new(),
                children: HashMap::new(),
            }),
            data,
            key,
            http,
            auth: Mutex::new(HashMap::new()),
            checkin: Mutex::new(()),
            services: Mutex::new(()),
            proxy: Mutex::new(None),
            snapshot: Mutex::new((Instant::now() - Wait::from_secs(60), json!([]))),
        }))
    }
    fn log(&self, event: &str, detail: &str) {
        let p = self.data.join("events.log");
        if fs::metadata(&p)
            .map(|m| m.len() > 2_000_000)
            .unwrap_or(false)
        {
            let _ = fs::rename(&p, self.data.join("events.log.previous"));
        }
        if let Ok(mut f) = fs::OpenOptions::new().create(true).append(true).open(p) {
            let _ = writeln!(f, "{} {} {}", Utc::now().to_rfc3339(), event, safe(detail));
        }
    }
    fn save_locked(&self, state: &State) -> Result<()> {
        let iv = random(12);
        let plain = serde_json::to_vec(&state.accounts).map_err(|e| e.to_string())?;
        let cipher = Aes256Gcm::new_from_slice(&self.key)
            .map_err(|_| "密钥无效")?
            .encrypt(Nonce::from_slice(&iv), plain.as_ref())
            .map_err(|_| "账号库保存失败")?;
        let split = cipher.len() - 16;
        let blob = json!({"version":1,"iv":B64.encode(iv),"tag":B64.encode(&cipher[split..]),"data":B64.encode(&cipher[..split])});
        atomic(
            &self.data.join("accounts.enc"),
            &serde_json::to_vec(&blob).unwrap(),
        )
    }
    fn config(&self) -> V {
        self.state.lock().unwrap().config.clone()
    }
    fn bases(&self, site: &str) -> Vec<String> {
        #[cfg(test)]
        if let Some(hosts) = self.config()["testHosts"][site].as_array() {
            return hosts
                .iter()
                .filter_map(V::as_str)
                .map(str::to_owned)
                .collect();
        }
        hosts(site).iter().map(|s| s.to_string()).collect()
    }
    fn save_config(&self, state: &State) -> Result<()> {
        atomic(
            &self.data.join("config.json"),
            &serde_json::to_vec_pretty(&state.config).unwrap(),
        )
    }
    fn account(&self, id: &str) -> Result<V> {
        self.state
            .lock()
            .unwrap()
            .accounts
            .iter()
            .find(|a| s(a, "id") == id)
            .cloned()
            .ok_or("账号不存在".into())
    }
    fn update(&self, id: &str, patch: V) -> Result<()> {
        let mut state = self.state.lock().unwrap();
        let a = state
            .accounts
            .iter_mut()
            .find(|a| s(a, "id") == id)
            .ok_or("账号不存在")?;
        for (k, v) in patch.as_object().ok_or("字段无效")? {
            a[k] = v.clone();
        }
        self.save_locked(&state)
    }
    fn lock_account(&self, id: &str) -> Arc<Mutex<()>> {
        self.auth
            .lock()
            .unwrap()
            .entry(id.into())
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone()
    }
    fn public_accounts(&self) -> V {
        let state = self.state.lock().unwrap();
        json!(state.accounts.iter().map(|a|json!({"id":a["id"],"site":a["site"],"label":a["label"],"phoneSuffix":a["phoneSuffix"],"enabled":a["enabled"],"authState":a["authState"],"expiresAt":a["expiresAt"],"cooldownUntil":a["cooldownUntil"],"inflight":state.inflight.get(&s(a,"id")).copied().unwrap_or(0),"requests":a["requests"],"successes":a["successes"],"failures":a["failures"],"balance":a["balance"],"balanceAt":a["balanceAt"],"balanceError":a["balanceError"],"checkin":a["checkin"],"lastError":a["lastError"]})).collect::<Vec<_>>())
    }
    fn request(
        &self,
        a: &V,
        base: &str,
        path: &str,
        payload: &V,
    ) -> reqwest::blocking::RequestBuilder {
        let mut r = self
            .http
            .post(format!("{base}{path}"))
            .json(payload)
            .header("Authorization", format!("Bearer {}", s(a, "accessToken")))
            .header("X-User-Id", s(a, "uid"))
            .header("X-Product", "SaaS")
            .header("X-Requested-With", "XMLHttpRequest")
            .header("X-Domain", s(a, "domain"))
            .header("User-Agent", "CLI/2.63.2 CodeBuddy/2.63.2")
            .header("Origin", base)
            .header("Referer", format!("{base}/"));
        if !s(a, "enterpriseId").is_empty() {
            r = r.header("X-Enterprise-Id", s(a, "enterpriseId"));
        }
        r
    }
    fn refresh_locked(&self, id: &str) -> Result<bool> {
        let a = self.account(id)?;
        if s(&a, "refreshToken").is_empty() {
            self.update(
                id,
                json!({"authState":"需重新登录","lastError":"请重新登录后导入"}),
            )?;
            return Ok(false);
        }
        for base in self.bases(&s(&a, "site")) {
            let r = self
                .http
                .post(format!("{base}/v2/plugin/auth/token/refresh"))
                .json(&json!({}))
                .header("X-Refresh-Token", s(&a, "refreshToken"))
                .header("X-User-Id", s(&a, "uid"))
                .header("X-Domain", s(&a, "domain"))
                .header("X-Product", "SaaS")
                .header("X-Auth-Refresh-Source", "workbuddy")
                .header("User-Agent", "CLI/2.63.2 CodeBuddy/2.63.2")
                .timeout(Wait::from_secs(20))
                .send();
            if let Ok(r) = r {
                let status = r.status();
                if let Ok(j) = r.json::<V>() {
                    let d = &j["data"];
                    if n(&j, "code") == 0.0 && d["accessToken"].is_string() {
                        let expiry = if n(d, "expiresAt") > 0.0 {
                            n(d, "expiresAt")
                        } else {
                            now() as f64 + n(d, "expiresIn") * 1000.0
                        };
                        self.update(id,json!({"accessToken":d["accessToken"],"refreshToken":d.get("refreshToken").unwrap_or(&a["refreshToken"]),"expiresAt":expiry,"authState":"有效","lastError":"","cooldownUntil":0}))?;
                        self.log("凭据刷新", &s(&a, "label"));
                        return Ok(true);
                    }
                    if status.as_u16() == 401
                        || status.as_u16() == 403
                        || [401.0, 403.0, 10001.0].contains(&n(&j, "code"))
                    {
                        self.update(id,json!({"authState":"需重新登录","lastError":"登录已失效，请重新登录后导入"}))?;
                    }
                }
            }
        }
        Ok(false)
    }
    fn ensure(&self, id: &str) -> Result<V> {
        let lock = self.lock_account(id);
        let _g = lock.lock().unwrap();
        let a = self.account(id)?;
        if n(&a, "expiresAt") > 0.0
            && n(&a, "expiresAt") < now() as f64 + 60000.0
            && !self.refresh_locked(id)?
        {
            return Err("凭据刷新失败".into());
        }
        self.account(id)
    }
    fn refresh_if_same(&self, id: &str, old: &str) -> Result<bool> {
        let lock = self.lock_account(id);
        let _g = lock.lock().unwrap();
        if s(&self.account(id)?, "accessToken") != old {
            return Ok(true);
        }
        self.refresh_locked(id)
    }
    fn billing(&self, id: &str, endpoint: &str) -> Result<V> {
        let mut a = self.ensure(id)?;
        let prefix = if endpoint == "get-user-resource-summary" {
            ""
        } else {
            "/v2"
        };
        let mut error = "计费接口连接失败".to_owned();
        for base in self.bases(&s(&a, "site")) {
            for retry in 0..2 {
                match self
                    .request(
                        &a,
                        &base,
                        &format!("{prefix}/billing/meter/{endpoint}"),
                        &json!({}),
                    )
                    .header("X-Product", "WorkBuddy")
                    .timeout(Wait::from_secs(20))
                    .send()
                {
                    Ok(r) => {
                        let status = r.status().as_u16();
                        let j = r.json::<V>().map_err(|_| "计费接口返回格式异常")?;
                        if [401, 403].contains(&status)
                            && retry == 0
                            && self.refresh_if_same(id, &s(&a, "accessToken"))?
                        {
                            a = self.account(id)?;
                            continue;
                        }
                        if status >= 400 {
                            error = format!("计费接口 HTTP {status}");
                            break;
                        }
                        if n(&j, "code") == 0.0 {
                            self.update(id, json!({"authState":"有效"}))?;
                        }
                        return Ok(j);
                    }
                    Err(_) => {
                        error = "计费接口网络异常".into();
                        break;
                    }
                }
            }
        }
        Err(error)
    }
    fn balance(&self, id: &str) -> Result<f64> {
        let result = (|| {
            let j = self.billing(id, "get-user-resource-summary")?;
            if n(&j, "code") != 0.0 {
                return Err(format!("余额查询失败，业务码 {}", n(&j, "code")));
            }
            let packs = j["data"]["Packages"]
                .as_array()
                .ok_or("余额接口未返回额度包")?;
            let left = packs
                .iter()
                .map(|p| n(p, "CycleRemainCapacity").max(0.0))
                .sum::<f64>();
            self.update(id,json!({"balance":left,"balanceAt":now(),"balanceSource":"resource-summary","balanceError":""}))?;
            Ok(left)
        })();
        if let Err(e) = &result {
            let _ = self.update(id, json!({"balanceError":e}));
        }
        result
    }
    fn import(&self, history: bool, extra: Option<&str>) -> Result<V> {
        let cfg = self.config();
        let mut files = Vec::new();
        if let Some(p) = extra {
            files.push(PathBuf::from(p));
        } else {
            let home = PathBuf::from(std::env::var_os("USERPROFILE").unwrap());
            let local = PathBuf::from(std::env::var_os("LOCALAPPDATA").unwrap());
            let mut dirs = vec![
                local.join("CodeBuddyExtension/Data/Public/auth"),
                home.join(".codebuddy/auth"),
                home.join(".workbuddy/auth"),
            ];
            if let Some(a) = cfg["authDirs"].as_array() {
                dirs.extend(a.iter().filter_map(|x| x.as_str().map(PathBuf::from)));
            }
            for dir in dirs {
                if let Ok(entries) = fs::read_dir(dir) {
                    for e in entries.flatten() {
                        let name = e.file_name().to_string_lossy().to_string();
                        let historical = name.contains('T') && name.contains('-');
                        if (history || !historical)
                            && (name.ends_with(".info") || name.ends_with(".json"))
                        {
                            files.push(e.path());
                        }
                    }
                }
            }
        }
        let mut state = self.state.lock().unwrap();
        let (mut added, mut updated, mut unsupported) = (0, 0, 0);
        for file in files {
            let Ok(bytes) = fs::read(&file) else { continue };
            let Ok(j) = serde_json::from_slice::<V>(&bytes) else {
                continue;
            };
            let a = j.get("auth").unwrap_or(&j);
            let empty = json!({});
            let u = j.get("account").or_else(|| j.get("user")).unwrap_or(&empty);
            let Some(site) = site_of(&s(a, "domain")) else {
                continue;
            };
            let uid = u.get("uid").or_else(|| a.get("uid")).unwrap_or(&V::Null);
            if uid.is_null() || !a["accessToken"].is_string() {
                continue;
            }
            if s(a, "accessToken").starts_with('{') {
                unsupported += 1;
                continue;
            }
            let ent = u
                .get("enterpriseId")
                .or_else(|| a.get("enterpriseId"))
                .unwrap_or(&V::Null);
            let uidstr = uid
                .as_str()
                .map(str::to_owned)
                .unwrap_or_else(|| uid.to_string());
            let entstr = ent.as_str().unwrap_or("");
            let id = hex(&Sha256::digest(format!("{site}:{uidstr}:{entstr}")))[..20].to_owned();
            let old = state.accounts.iter_mut().find(|x| s(x, "id") == id);
            if old
                .as_ref()
                .is_some_and(|o| n(o, "expiresAt") >= n(a, "expiresAt"))
            {
                continue;
            }
            let phone = s(u, "phoneNumber");
            let tail = phone
                .chars()
                .rev()
                .take(4)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect::<String>();
            let label = if !s(u, "nickname").is_empty() {
                s(u, "nickname")
            } else if !phone.is_empty() {
                format!("手机 ****{tail}")
            } else {
                format!("账号 {}", &id[..6])
            };
            let item = json!({"id":id,"site":site,"uid":uidstr,"enterpriseId":entstr,"domain":a["domain"],"label":label,"phoneSuffix":tail,"accessToken":a["accessToken"],"refreshToken":a.get("refreshToken").unwrap_or(&json!("")),"expiresAt":a["expiresAt"],"refreshExpiresAt":a["refreshExpiresAt"],"source":file.to_string_lossy(),"importedAt":now()});
            if let Some(o) = old {
                for (k, v) in item.as_object().unwrap() {
                    o[k] = v.clone();
                }
                o["authState"] = json!("待验证");
                o["cooldownUntil"] = json!(0);
                updated += 1;
            } else {
                let mut item = item;
                for(k,v)in json!({"enabled":true,"authState":"待验证","cooldownUntil":0,"requests":0,"successes":0,"failures":0,"checkin":{}}).as_object().unwrap(){item[k]=v.clone();}
                state.accounts.push(item);
                added += 1;
            }
        }
        if added + updated > 0 {
            self.save_locked(&state)?;
            self.log("导入账号", &format!("新增 {added}，更新 {updated}"));
        }
        Ok(json!({"added":added,"updated":updated,"unsupported":unsupported}))
    }
    fn checkins(&self, force: bool, only: Option<&str>) -> Result<V> {
        let Ok(_g) = self.checkin.try_lock() else {
            return Ok(json!([]));
        };
        let date = s(&china(), "date");
        let ids = self
            .state
            .lock()
            .unwrap()
            .accounts
            .iter()
            .filter(|a| {
                s(a, "site") == "cn" && b(a, "enabled") && only.is_none_or(|id| s(a, "id") == id)
            })
            .map(|a| s(a, "id"))
            .collect::<Vec<_>>();
        let mut results = Vec::new();
        for id in ids {
            let a = self.account(&id)?;
            let c = &a["checkin"];
            if !force
                && s(c, "date") == date
                && (b(c, "success") || n(c, "nextAttemptAt") > now() as f64)
            {
                continue;
            }
            let result = (|| -> Result<V> {
                let j = self.billing(&id, "checkin-activity-status")?;
                if n(&j, "code") != 0.0 || !j["data"].is_object() {
                    return Err("签到状态查询失败".into());
                }
                let mut d = j["data"].clone();
                if !b(&d, "active") && !b(&d, "today_checked_in") {
                    return Ok(
                        json!({"date":date,"success":false,"status":"活动未开放","credit":0,"at":now(),"nextAttemptAt":now()+14400000}),
                    );
                }
                let mut claimed = false;
                if !b(&d, "today_checked_in") {
                    let j = self.billing(&id, "daily-checkin")?;
                    let code = n(&j, "code");
                    if ![0.0, 1001.0, 10001.0].contains(&code) {
                        return Err(format!("领取失败，业务码 {code}"));
                    }
                    claimed = code == 0.0;
                    let j = self.billing(&id, "checkin-activity-status")?;
                    if n(&j, "code") != 0.0 || !b(&j["data"], "today_checked_in") {
                        return Err("尚未确认签到到账，将稍后重试".into());
                    }
                    d = j["data"].clone();
                }
                self.update(
                    &id,
                    json!({"checkinTotal":d["total_credits"],"authState":"有效","lastError":""}),
                )?;
                let _ = self.balance(&id);
                Ok(
                    json!({"date":date,"success":true,"status":if claimed{"签到成功"}else{"今日已签到"},"credit":if n(&d,"today_credit")>0.0{n(&d,"today_credit")}else{n(&d,"daily_credit")},"at":now()}),
                )
            })();
            let c=result.unwrap_or_else(|e|json!({"date":date,"success":false,"status":if s(&self.account(&id).unwrap_or(V::Null),"authState")=="需重新登录"{"需重新登录"}else{"签到失败"},"credit":0,"at":now(),"nextAttemptAt":now()+900000,"error":e}));
            self.update(&id, json!({"checkin":c}))?;
            self.log(
                "每日签到",
                &format!(
                    "{} {} {} 积分",
                    s(&a, "label"),
                    s(&c, "status"),
                    n(&c, "credit")
                ),
            );
            let mut out = c;
            out["id"] = json!(id);
            out["label"] = a["label"].clone();
            results.push(out);
        }
        Ok(json!(results))
    }
    fn tick(&self) {
        let c = self.config();
        if b(&c, "checkinEnabled") {
            let _ = self.checkins(false, None);
        }
    }
}
fn hosts(site: &str) -> Vec<&'static str> {
    if site == "ai" {
        vec!["https://www.workbuddy.ai", "https://www.codebuddy.ai"]
    } else {
        vec!["https://www.workbuddy.cn", "https://copilot.tencent.com"]
    }
}
fn site_of(domain: &str) -> Option<&'static str> {
    match domain
        .trim_start_matches("https://")
        .trim_start_matches("http://")
        .trim_end_matches('/')
    {
        "www.workbuddy.ai" | "workbuddy.ai" | "www.codebuddy.ai" | "codebuddy.ai" => Some("ai"),
        "www.workbuddy.cn"
        | "workbuddy.cn"
        | "www.codebuddy.cn"
        | "codebuddy.cn"
        | "copilot.tencent.com" => Some("cn"),
        _ => None,
    }
}

fn admin(mut req: Request, app: Arc<App>) {
    let cfg = app.config();
    if header(&req, "Authorization") != format!("Bearer {}", s(&cfg, "adminKey")) {
        respond(req, 401, json!({"error":"管理密钥无效"}));
        return;
    }
    if !header(&req, "Origin").is_empty() {
        respond(req, 403, json!({"error":"管理接口仅供桌面软件使用"}));
        return;
    }
    let route = req.url().split('?').next().unwrap_or("").to_owned();
    let get = req.method().as_str() == "GET";
    let data = if get { Ok(json!({})) } else { body(&mut req) };
    let result=data.and_then(|d|->Result<V>{match route.as_str(){
 "/admin/status" if get=>{let cfg=app.config();let mem=service::memory();Ok(json!({"ok":true,"now":china(),"accounts":app.public_accounts(),"services":app.service_snapshot(),"settings":{"defaultSite":cfg["defaultSite"],"checkinEnabled":cfg["checkinEnabled"],"checkinTime":cfg["checkinTime"],"startupGuard":cfg["startupGuard"],"port":cfg["port"]},"proxyRunning":app.proxy.lock().unwrap().is_some(),"checkinRunning":app.checkin.try_lock().is_err(),"dataDir":app.data.to_string_lossy(),"runtime":{"engine":"原生后台","workingBytes":mem.0,"privateBytes":mem.1,"uiAutoStart":false}}))},
 "/admin/logs" if get=>{let txt=fs::read_to_string(app.data.join("events.log")).unwrap_or_default();Ok(json!({"text":txt.lines().rev().take(120).collect::<Vec<_>>().into_iter().rev().collect::<Vec<_>>().join("\n")}))},
 "/admin/client-config" if get=>Ok(json!({"url":format!("http://127.0.0.1:{}/v1",n(&cfg,"port")),"key":cfg["localKey"]})),
 "/admin/import" if !get=>app.import(b(&d,"history"),d["file"].as_str()),
 "/admin/checkin" if !get=>Ok(json!({"results":app.checkins(true,d["id"].as_str())?})),
 "/admin/scheduled-checkin" if !get=>{app.tick();Ok(json!({"ok":true}))},
 "/admin/balances" if !get=>{let ids=app.state.lock().unwrap().accounts.iter().filter(|a|b(a,"enabled")&&(s(&d,"site").is_empty()||s(a,"site")==s(&d,"site"))&&(s(&d,"id").is_empty()||s(a,"id")==s(&d,"id"))).map(|a|s(a,"id")).collect::<Vec<_>>();Ok(json!({"results":ids.iter().map(|id|match app.balance(id){Ok(x)=>json!({"id":id,"balance":x}),Err(e)=>json!({"id":id,"error":e})}).collect::<Vec<_>>()}))},
 "/admin/account" if !get=>{let id=s(&d,"id");let mut patch=json!({});if d["enabled"].is_boolean(){patch["enabled"]=d["enabled"].clone();}if d["label"].is_string(){patch["label"]=json!(safe(&s(&d,"label")).chars().take(60).collect::<String>());}if b(&d,"reset"){patch["authState"]=json!("待验证");patch["cooldownUntil"]=json!(0);patch["lastError"]=json!("");}app.update(&id,patch)?;Ok(json!({"ok":true}))},
 "/admin/workbuddy-path" if !get=>{let field=match s(&d,"site").as_str(){"cn"=>"workbuddyExe","ai"=>"workbuddyIntlExe",_=>return Err("版本无效".into())};let path=s(&d,"path");if !path.to_lowercase().ends_with(".exe")||!Path::new(&path).is_file(){return Err("请选择 WorkBuddy 启动程序".into());}let mut state=app.state.lock().unwrap();state.config[field]=json!(path);app.save_config(&state)?;Ok(json!({"ok":true}))},
 "/admin/settings" if !get=>{if d.get("defaultSite").is_some()&&! ["cn","ai"].contains(&s(&d,"defaultSite").as_str()){return Err("默认号池无效".into());}if d.get("checkinTime").is_some(){let t=s(&d,"checkinTime");if t.len()!=5||t.as_bytes()[2]!=b':'||t[..2].parse::<u8>().map(|x|x>23).unwrap_or(true)||t[3..].parse::<u8>().map(|x|x>59).unwrap_or(true){return Err("时间格式应为 HH:mm".into());}}{let mut state=app.state.lock().unwrap();for k in ["defaultSite","checkinTime","checkinEnabled","startupGuard"]{if let Some(v)=d.get(k){if (k=="checkinEnabled"||k=="startupGuard")&&!v.is_boolean(){return Err("设置格式错误".into());}state.config[k]=v.clone();}}app.save_config(&state)?;}app.guard(false)?;Ok(json!({"ok":true}))},
 "/admin/guard" if !get=>app.guard(true),
 "/admin/service" if !get=>{app.service_action(&s(&d,"id"),&s(&d,"action"))?;Ok(json!({"ok":true}))},
 "/admin/service-autostart" if !get=>{let mut state=app.state.lock().unwrap();let svc=state.config["services"].as_array_mut().unwrap().iter_mut().find(|x|s(x,"id")==s(&d,"id")).ok_or("没有这个服务")?;svc["autoStart"]=json!(b(&d,"autoStart"));app.save_config(&state)?;Ok(json!({"ok":true}))},
 "/admin/service-add" if !get=>{let exe=s(&d,"executable");let port=n(&d,"port");if s(&d,"name").is_empty()||!Path::new(&exe).exists()||!exe.to_lowercase().ends_with(".exe"){return Err("请选择有效的启动程序".into());}if !(1024.0..=65535.0).contains(&port)||port.fract()!=0.0{return Err("端口应在 1024–65535".into());}let args=d["args"].as_array().ok_or("启动参数必须是文本列表")?;if args.iter().any(|v|!v.is_string()){return Err("启动参数必须是文本列表".into());}let mut state=app.state.lock().unwrap();let services=state.config["services"].as_array_mut().unwrap();if services.iter().any(|x|n(x,"port")==port){return Err("端口已登记".into());}let cwd=d["cwd"].as_str().map(str::to_owned).unwrap_or_else(||Path::new(&exe).parent().unwrap().to_string_lossy().to_string());let matchstr=d["match"].as_str().or_else(||args.first().and_then(V::as_str)).unwrap_or(&exe);services.push(json!({"id":format!("custom-{}",now()),"name":safe(&s(&d,"name")),"kind":"process","port":port,"url":format!("http://127.0.0.1:{port}/v1"),"executable":exe,"args":args,"cwd":cwd,"match":matchstr,"autoStart":false,"description":"手动添加"}));app.save_config(&state)?;Ok(json!({"ok":true}))},
 _=>Err("接口不存在".into())}});
    match result {
        Ok(v) => respond(req, 200, v),
        Err(e) => respond(req, 400, json!({"error":safe(&e)})),
    }
}

fn run() -> Result<()> {
    let app = App::new()?;
    let cfg = app.config();
    let args = std::env::args().collect::<Vec<_>>();
    if args.iter().any(|a| a == "--init") { return Ok(()); }
    if args
        .iter()
        .any(|a| a == "--guard" || a == "--checkin" || a == "--ensure")
    {
        let url = format!("http://127.0.0.1:{}", n(&cfg, "adminPort"));
        let short = Client::builder()
            .no_proxy()
            .timeout(Wait::from_secs(180))
            .connect_timeout(Wait::from_millis(500))
            .build()
            .map_err(|e| e.to_string())?;
        if short
            .get(format!("{url}/admin/status"))
            .bearer_auth(s(&cfg, "adminKey"))
            .send()
            .and_then(|response| response.error_for_status())
            .is_err()
        {
            service::launch(
                &std::env::current_exe().unwrap(),
                &["--daemon".into()],
                None,
                None,
            )?;
            for _ in 0..40 {
                thread::sleep(Wait::from_millis(150));
                if short
                    .get(format!("{url}/admin/status"))
                    .bearer_auth(s(&cfg, "adminKey"))
                    .send()
                    .and_then(|response| response.error_for_status())
                    .is_ok()
                {
                    break;
                }
            }
        }
        if args.iter().any(|a| a == "--guard") && b(&cfg, "startupGuard") {
            short
                .post(format!("{url}/admin/guard"))
                .bearer_auth(s(&cfg, "adminKey"))
                .json(&json!({}))
                .send()
                .and_then(|response| response.error_for_status())
                .map_err(|_| "无法连接后台")?;
        }
        if args.iter().any(|a| a == "--checkin") {
            short
                .post(format!("{url}/admin/scheduled-checkin"))
                .bearer_auth(s(&cfg, "adminKey"))
                .json(&json!({}))
                .send()
                .and_then(|response| response.error_for_status())
                .map_err(|_| "无法连接后台")?;
        }
        return Ok(());
    }
    let server = Server::http(format!("127.0.0.1:{}", n(&cfg, "adminPort")))
        .map_err(|_| "后台已运行或端口被占用")?;
    atomic(
        &app.data.join("daemon.pid"),
        std::process::id().to_string().as_bytes(),
    )?;
    app.log("管理服务启动", &format!("版本 1.0.0 · 数据目录 {} · 账号 {}", app.data.display(), app.state.lock().unwrap().accounts.len()));
    let _ = app.import(false, None);
    app.guard(false)?;
    if b(&cfg, "proxyEnabled")
        && cfg["services"].as_array().is_none_or(|ss| {
            ss.iter()
                .find(|x| s(x, "id") == "workbuddy")
                .is_none_or(|x| x["autoStart"] != false)
        })
    {
        app.start_proxy()?;
    }
    for x in cfg["services"].as_array().unwrap() {
        if b(x, "autoStart") && s(x, "kind") != "internal" {
            let _ = app.service_action(&s(x, "id"), "start");
        }
    }
    let a = app.clone();
    thread::spawn(move || {
        a.tick();
        loop {
            thread::sleep(Wait::from_secs(60));
            let _ = a.import(false, None);
            a.tick();
        }
    });
    let a = app.clone();
    thread::spawn(move || {
        loop {
            thread::sleep(Wait::from_secs(15));
            let _ = a.guard(false);
        }
    });
    for request in server.incoming_requests() {
        let a = app.clone();
        thread::spawn(move || admin(request, a));
    }
    Ok(())
}
fn main() {
    if let Err(e) = run() {
        if !e.contains("已运行") {
            let data = std::env::var_os("BUDDYHUB_DATA").map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from(std::env::var_os("USERPROFILE").unwrap_or_default()).join(".buddyhub"));
            let p = data.join("native-errors.log");
            if let Ok(mut f) = fs::OpenOptions::new().create(true).append(true).open(p) {
                let _ = writeln!(f, "{} {}", Utc::now(), safe(&e));
            }
            std::process::exit(1);
        }
    }
}
