use super::*;
use std::io::{BufRead, BufReader};
const MODELS: &[&str] = &["deepseek-v4.1-flash"];
const EFFORTS: &[&str] = &["minimal", "low", "medium", "high", "xhigh", "max"];
struct RequestContext {
    session: String,
    turn: String,
}
impl RequestContext {
    fn new(payload: &V, session_hint: &str) -> Self {
        let messages = payload["messages"].as_array().map(Vec::as_slice).unwrap_or(&[]);
        // Clients may supply an opaque session key. Otherwise retain the initial
        // context through tool continuations, without storing prompts in the cache.
        let initial = messages.iter().find(|m| s(m, "role") == "user");
        let system = messages.iter().find(|m| s(m, "role") == "system");
        let session = if session_hint.is_empty() {
            hex(&Sha256::digest(serde_json::to_vec(&(system, initial)).unwrap()))
        } else {
            hex(&Sha256::digest(session_hint.as_bytes()))
        };
        let end = messages.iter().rposition(|m| s(m, "role") == "user")
            .map(|i| i + 1).unwrap_or(messages.len());
        let turn = hex(&Sha256::digest(serde_json::to_vec(&messages[..end]).unwrap()));
        Self { session, turn }
    }
}
fn uuid7() -> String {
    let mut bytes = random(16);
    bytes[..6].copy_from_slice(&now().to_be_bytes()[2..]);
    bytes[6] = (bytes[6] & 0x0f) | 0x70;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    hex(&bytes)
}
struct UpstreamFailure {
    status: u16,
    rotate: bool,
    code: Option<i64>,
    message: String,
    retry_at: Option<i64>,
}
fn quota_reset_at(message: &str) -> Option<i64> {
    // Only accept the upstream's explicitly identified UTC+8 timestamp, never
    // echo arbitrary error text, account data or prompts.
    let before = message.split_once(" UTC+8")?.0;
    let date = before.get(before.len().checked_sub(19)?..)?;
    let local = chrono::NaiveDateTime::parse_from_str(date, "%Y-%m-%d %H:%M:%S").ok()?;
    let reset = (local - Duration::hours(8)).and_utc().timestamp_millis();
    (reset > now()).then_some(reset)
}
impl UpstreamFailure {
    fn classify(status: u16, value: &V) -> Self {
        let code = value["code"].as_i64().or_else(|| value["error"]["code"].as_i64());
        let msg = value["msg"].as_str().or_else(|| value["error"]["message"].as_str())
            .or_else(|| value["message"].as_str()).or_else(|| value.as_str()).unwrap_or("");
        let retry_at = if code == Some(6004) { quota_reset_at(msg) } else { None };
        let status = if code == Some(6004) { 429 } else { status };
        let rotate = code != Some(11128) && (status == 429 || status >= 500
            || [401, 403].contains(&status) || matches!(code, Some(401 | 403 | 429 | 10001)) || quota(msg));
        // Unknown upstream messages can contain credentials or echoed prompts.
        // Return reviewed summaries and numeric business codes only.
        let message = if code == Some(11128) {
            "上游会话信息缺失（conversation id missing）".to_owned()
        } else if code == Some(6004) {
            if let Some(at) = retry_at {
                let date = (chrono::DateTime::from_timestamp_millis(at).unwrap() + Duration::hours(8)).format("%Y-%m-%d %H:%M:%S");
                format!("上游使用频率额度已达上限，将于 {date}（北京时间）重置")
            } else { "上游使用频率额度已达上限，重置时间请以官方客户端提示为准".into() }
        } else { format!("上游拒绝请求，HTTP {status}") };
        Self { status, rotate, code, message, retry_at }
    }
    fn read(status: u16, response: reqwest::blocking::Response) -> Self {
        let mut bytes = Vec::new();
        let read = response.take(16 * 1024 + 1).read_to_end(&mut bytes);
        let value = if read.is_ok() && bytes.len() <= 16 * 1024 {
            serde_json::from_slice(&bytes).unwrap_or_else(|_| json!(String::from_utf8_lossy(&bytes)))
        } else { V::Null };
        Self::classify(status, &value)
    }
    fn json(&self) -> V {
        let mut result = json!({"error":{"message":self.message,"type":"upstream_error"}});
        if let Some(code) = self.code { result["error"]["code"] = json!(code); }
        result
    }
    fn cooldown_ms(&self) -> i64 {
        if !self.rotate { 0 } else { self.retry_at.map(|at| (at - now()).max(1000)).unwrap_or(60000) }
    }
}
fn parse_model(id: &str, default: &str) -> Result<(String, String, Option<String>)> {
    let mut upstream = id.to_owned();
    let mut effort = None;
    for e in EFFORTS {
        if let Some(m) = upstream.strip_suffix(&format!("-think-{e}")) {
            upstream = m.into();
            effort = Some(e.to_string());
            break;
        }
    }
    if effort.is_none() {
        if let Some(m) = upstream.strip_suffix("-think") {
            upstream = m.into();
            effort = Some("max".into());
        }
    }
    let site = if let Some(m) = upstream.strip_suffix("-cn") {
        let m = m.to_owned();
        upstream = m;
        "cn"
    } else if let Some(m) = upstream.strip_suffix("-ai") {
        let m = m.to_owned();
        upstream = m;
        "ai"
    } else if let Some(m) = upstream.strip_suffix("-auto") {
        let m = m.to_owned();
        upstream = m;
        "auto"
    } else {
        default
    };
    if !MODELS.contains(&upstream.as_str()) {
        return Err("不支持的模型名".into());
    }
    Ok((upstream, site.into(), effort))
}
fn models() -> V {
    let mut ids = Vec::new();
    for m in MODELS {
        // Explicit regions stay isolated; auto opts into both independent pools.
        for site in ["cn", "ai", "auto"] {
            ids.push(format!("{m}-{site}"));
        }
    }
    json!({"object":"list","data":ids.iter().map(|id|json!({"id":id,"object":"model","owned_by":"workbuddy"})).collect::<Vec<_>>()})
}
fn prepare(mut v: V) -> V {
    v["stream"] = json!(true);
    if let Some(m) = v["messages"].as_array_mut() {
        for x in m.iter_mut() {
            if s(x, "role").to_lowercase() == "developer" {
                x["role"] = json!("system");
            }
        }
        if m.first()
            .is_none_or(|x| s(x, "role").to_lowercase() != "system")
        {
            m.insert(
                0,
                json!({"role":"system","content":"You are a helpful AI assistant."}),
            );
        }
    }
    let tc = v["tool_choice"].clone();
    let t = if tc.is_object() {
        s(&tc, "type").to_lowercase()
    } else {
        tc.as_str().unwrap_or("").to_lowercase()
    };
    match t.as_str() {
        "none" => {
            v.as_object_mut().unwrap().remove("tool_choice");
            v.as_object_mut().unwrap().remove("tools");
        }
        "function" => {
            v["tool_choice"] = json!(
                tc["function"]["name"]
                    .as_str()
                    .or_else(|| tc["name"].as_str())
                    .unwrap_or("auto")
            )
        }
        "auto" | "required" if tc.is_object() => v["tool_choice"] = json!(t),
        _ if tc.is_object() => {
            v.as_object_mut().unwrap().remove("tool_choice");
        }
        _ => {}
    }
    if v["reasoning"].is_object() {
        if v["reasoning_effort"].is_null() {
            v["reasoning_effort"] = v["reasoning"]["effort"].clone();
        }
        if v["reasoning"]["enabled"] == false {
            v["reasoning_effort"] = json!("none");
        }
    }
    v.as_object_mut().unwrap().remove("reasoning");
    let e = s(&v, "reasoning_effort").trim().to_lowercase();
    if EFFORTS.contains(&e.as_str()) {
        v["reasoning_effort"] = json!(e);
    } else {
        v.as_object_mut().unwrap().remove("reasoning_effort");
    }
    v
}
fn business(v: &V) -> bool {
    v["choices"].is_null() && (!v["error"].is_null() || n(v, "code") != 0.0)
}
fn quota(v: &str) -> bool {
    let t = v.to_lowercase();
    [
        "quota",
        "credit",
        "balance",
        "insufficient",
        "余额",
        "积分",
        "额度",
        "超限",
        "rate limit",
        "rate_limit",
    ]
    .iter()
    .any(|x| t.contains(x))
}
fn normalize(v: &V, model: &str) -> Option<V> {
    if business(v) {
        return Some(json!({"__error":v}));
    }
    let mut out =
        json!({"object":v["object"].as_str().unwrap_or("chat.completion.chunk"),"model":model});
    for k in ["id", "created", "usage"] {
        if !v[k].is_null() {
            out[k] = v[k].clone();
        }
    }
    let mut meaningful = !v["usage"].is_null();
    if let Some(ch) = v["choices"].as_array().and_then(|x| x.first()) {
        let mut d = json!({});
        for k in ["role", "content", "reasoning_content"] {
            if ch["delta"][k].as_str().is_some_and(|x| !x.is_empty()) {
                d[k] = ch["delta"][k].clone();
            }
        }
        if ch["delta"]["tool_calls"]
            .as_array()
            .is_some_and(|x| !x.is_empty())
        {
            d["tool_calls"] = ch["delta"]["tool_calls"].clone();
        }
        meaningful |= !d.as_object().unwrap().is_empty()
            || ch["finish_reason"].as_str().is_some_and(|x| !x.is_empty());
        out["choices"] = json!([{"index":ch.get("index").unwrap_or(&json!(0)),"delta":d,"finish_reason":ch["finish_reason"]}]);
    }
    if meaningful { Some(out) } else { None }
}
struct Frames {
    body: BufReader<reqwest::blocking::Response>,
    done: bool,
}
impl Frames {
    fn next(&mut self) -> Result<Option<V>> {
        let mut line = String::new();
        loop {
            line.clear();
            if self.done {
                return Ok(None);
            }
            let size = self.body.read_line(&mut line).map_err(|_| "上游流中断")?;
            if size == 0 {
                self.done = true;
                return Ok(None);
            }
            if line.len() > 8 * 1024 * 1024 {
                return Err("上游数据帧过大".into());
            }
            let Some(data) = line.strip_prefix("data:") else {
                continue;
            };
            let data = data.trim();
            if data == "[DONE]" {
                self.done = true;
                return Ok(None);
            }
            if let Ok(v) = serde_json::from_str(data) {
                return Ok(Some(v));
            }
        }
    }
}
fn aggregate(first: V, frames: &mut Frames, model: &str) -> Result<V> {
    let mut out = json!({"id":format!("chatcmpl-{}",hex(&random(8))),"object":"chat.completion","created":Utc::now().timestamp(),"model":model,"choices":[{"index":0,"message":{"role":"assistant","content":""},"finish_reason":"stop"}],"usage":null});
    let mut tools: HashMap<usize, V> = HashMap::new();
    let mut current = Some(first);
    loop {
        let Some(raw) = current.take() else { break };
        if business(&raw) {
            return Err("上游业务错误".into());
        }
        if let Some(v) = normalize(&raw, model) {
            for k in ["id", "usage"] {
                if !v[k].is_null() {
                    out[k] = v[k].clone();
                }
            }
            let ch = &v["choices"][0];
            for k in ["content", "reasoning_content"] {
                if let Some(delta) = ch["delta"][k].as_str() {
                    let text = out["choices"][0]["message"][k]
                        .as_str()
                        .unwrap_or("")
                        .to_owned()
                        + delta;
                    out["choices"][0]["message"][k] = json!(text);
                }
            }
            if let Some(tcs) = ch["delta"]["tool_calls"].as_array() {
                for tc in tcs {
                    let i = n(tc, "index") as usize;
                    let t=tools.entry(i).or_insert_with(||json!({"id":tc["id"],"type":"function","function":{"name":"","arguments":""}}));
                    if tc["id"].is_string() {
                        t["id"] = tc["id"].clone();
                    }
                    for k in ["name", "arguments"] {
                        if let Some(delta) = tc["function"][k].as_str() {
                            t["function"][k] =
                                json!(t["function"][k].as_str().unwrap_or("").to_owned() + delta);
                        }
                    }
                }
            }
            if ch["finish_reason"].is_string() {
                out["choices"][0]["finish_reason"] = ch["finish_reason"].clone();
            }
        }
        current = frames.next()?;
    }
    if !tools.is_empty() {
        let mut t = tools.into_iter().collect::<Vec<_>>();
        t.sort_by_key(|(i, _)| *i);
        out["choices"][0]["message"]["tool_calls"] =
            json!(t.into_iter().map(|(_, t)| t).collect::<Vec<_>>());
        if out["choices"][0]["message"]["content"] == "" {
            out["choices"][0]["message"]["content"] = V::Null;
        }
    }
    Ok(out)
}
struct StreamBody {
    frames: Frames,
    model: String,
    pending: Vec<u8>,
    at: usize,
    ended: bool,
    failed: Arc<Mutex<bool>>,
}
impl Read for StreamBody {
    fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
        while self.at >= self.pending.len() {
            if self.ended {
                return Ok(0);
            }
            match self.frames.next() {
                Ok(Some(raw)) => {
                    if business(&raw) {
                        *self.failed.lock().unwrap() = true;
                        self.pending = format!(
                            "data: {}\n\ndata: [DONE]\n\n",
                            json!({"error":{"message":"上游流中断，请重试"}})
                        )
                        .into_bytes();
                        self.ended = true;
                    } else if let Some(v) = normalize(&raw, &self.model) {
                        self.pending = format!("data: {v}\n\n").into_bytes();
                    } else {
                        continue;
                    }
                }
                Ok(None) => {
                    self.pending = b"data: [DONE]\n\n".to_vec();
                    self.ended = true;
                }
                Err(_) => {
                    *self.failed.lock().unwrap() = true;
                    self.pending="data: {\"error\":{\"message\":\"上游流中断，请重试\"}}\n\ndata: [DONE]\n\n".as_bytes().to_vec();
                    self.ended = true;
                }
            }
            self.at = 0;
        }
        let len = out.len().min(self.pending.len() - self.at);
        out[..len].copy_from_slice(&self.pending[self.at..self.at + len]);
        self.at += len;
        Ok(len)
    }
}
impl App {
    fn record_failure(&self, id: &str, error: &UpstreamFailure) {
        self.finish_with_code(id, false, &error.message, error.cooldown_ms(), error.code);
    }
    fn unavailable_pool(&self, site: &str, fallback: u16) -> (u16, V, Option<i64>) {
        let state = self.state.lock().unwrap();
        let pool = state.accounts.iter().filter(|a| b(a,"enabled") &&
            (s(a,"site") == site || (site == "auto" && ["cn","ai"].contains(&s(a,"site").as_str())))).collect::<Vec<_>>();
        let invalid = pool.iter().filter(|a| s(a,"authState") == "需重新登录").count();
        let valid = pool.len() - invalid;
        let cooling = pool.iter().filter(|a| s(a,"authState") != "需重新登录" && n(a,"cooldownUntil") > now() as f64).collect::<Vec<_>>();
        let limited = cooling.iter().filter(|a| n(a,"upstreamCode") == 6004.0).count();
        let earliest = cooling.iter().map(|a| n(a,"cooldownUntil") as i64).min();
        let status = if valid > 0 && limited == valid { 429 }
            else if !pool.is_empty() && invalid == pool.len() { 401 } else { fallback };
        let mut message = if pool.is_empty() { format!("{} 号池暂无启用账号", site.to_uppercase()) }
            else { format!("{} 号池暂无可用账号：{invalid} 个需重新登录，{limited} 个使用频率受限，{} 个暂时冷却", site.to_uppercase(), cooling.len() - limited) };
        if let Some(at) = earliest {
            if let Some(date) = chrono::DateTime::from_timestamp_millis(at) {
                message.push_str(&format!("；最早可重试：{}（北京时间）", (date + Duration::hours(8)).format("%Y-%m-%d %H:%M:%S")));
            }
        }
        let mut result = json!({"error":{"message":message,"type":"pool_unavailable"}});
        if valid > 0 && limited == valid { result["error"]["code"] = json!(6004); }
        // A known, hours-long AI quota window cannot recover inside one chat turn.
        // Keep HTTP 429 and Retry-After for other clients, while allowing Hermes to
        // show the real reset time immediately instead of silently waiting 600s.
        if site == "ai" && status == 429 && earliest.is_some_and(|at|at > now()+600_000) {
            result["error"]["retryable"] = json!(false);
            result["error"]["reset_at"] = json!(earliest.unwrap()/1000);
        }
        (status, result, earliest.map(|at| ((at - now() + 999) / 1000).max(1)))
    }
    fn cached_request_id(&self, key: String) -> String {
        let mut cache = self.request_ids.lock().unwrap();
        if let Some((id, touched)) = cache.get_mut(&key) {
            *touched = now();
            return id.clone();
        }
        if cache.len() >= 2048 {
            if let Some(oldest) = cache.iter().min_by_key(|(_, (_, t))| *t).map(|(k, _)| k.clone()) {
                cache.remove(&oldest);
            }
        }
        let id = uuid7();
        cache.insert(key, (id.clone(), now()));
        id
    }
    pub fn start_proxy(self: &Arc<Self>) -> Result<()> {
        let mut proxy = self.proxy.lock().unwrap();
        if proxy.is_some() {
            return Ok(());
        }
        let cfg = self.config();
        let server = Arc::new(
            Server::http(format!("127.0.0.1:{}", n(&cfg, "port")))
                .map_err(|_| "反代端口已被占用")?,
        );
        *proxy = Some(server.clone());
        let app = self.clone();
        thread::spawn(move || {
            for req in server.incoming_requests() {
                let app = app.clone();
                thread::spawn(move || handle(req, app));
            }
        });
        let mut st = self.state.lock().unwrap();
        st.config["proxyEnabled"] = json!(true);
        self.save_config(&st)?;
        self.log(
            "反代启动",
            &format!("本机 {}，独立分区号池", n(&cfg, "port")),
        );
        Ok(())
    }
    fn pick(&self, site: &str, excluded: &[String]) -> Option<V> {
        let mut state = self.state.lock().unwrap();
        // Alternate regions first, then use each region's existing account cursor.
        // A region with no eligible account is skipped immediately.
        let site = if site == "auto" {
            let regions = ["cn", "ai"];
            let start = state.cursor.get("auto").copied().unwrap_or(0) % regions.len();
            let selected = (0..regions.len()).map(|offset| (start + offset) % regions.len())
                .find(|index| state.accounts.iter().any(|a| b(a,"enabled") &&
                    s(a,"site") == regions[*index] && !excluded.contains(&s(a,"id")) &&
                    s(a,"authState") != "需重新登录" && n(a,"cooldownUntil") <= now() as f64))?;
            state.cursor.insert("auto".into(), selected + 1);
            regions[selected]
        } else { site };
        let pool = state
            .accounts
            .iter()
            .filter(|a| {
                b(a, "enabled")
                    && s(a, "site") == site
                    && !excluded.contains(&s(a, "id"))
                    && s(a, "authState") != "需重新登录"
                    && n(a, "cooldownUntil") <= now() as f64
            })
            .cloned()
            .collect::<Vec<_>>();
        let min = pool
            .iter()
            .map(|a| state.inflight.get(&s(a, "id")).copied().unwrap_or(0))
            .min()?;
        let free = pool
            .into_iter()
            .filter(|a| state.inflight.get(&s(a, "id")).copied().unwrap_or(0) == min)
            .collect::<Vec<_>>();
        let cursor = state.cursor.entry(site.into()).or_insert(0);
        let a = free[*cursor % free.len()].clone();
        *cursor = cursor.wrapping_add(1);
        let id = s(&a, "id");
        *state.inflight.entry(id.clone()).or_insert(0) += 1;
        if let Some(a) = state.accounts.iter_mut().find(|a| s(a, "id") == id) {
            a["requests"] = json!(n(a, "requests") + 1.0);
        }
        Some(a)
    }
    fn finish(&self, id: &str, ok: bool, error: &str, cool: i64) {
        self.finish_with_code(id, ok, error, cool, None);
    }
    fn finish_with_code(&self, id: &str, ok: bool, error: &str, cool: i64, code: Option<i64>) {
        let mut state = self.state.lock().unwrap();
        let count = state.inflight.entry(id.into()).or_insert(0);
        *count = count.saturating_sub(1);
        if let Some(a) = state.accounts.iter_mut().find(|a| s(a, "id") == id) {
            let key = if ok { "successes" } else { "failures" };
            a[key] = json!(n(a, key) + 1.0);
            // An older in-flight response must not erase a known quota window.
            let keep_quota = n(a,"upstreamCode") == 6004.0 && n(a,"cooldownUntil") > now() as f64;
            if !keep_quota || code == Some(6004) {
                a["lastError"] = json!(error);
                a["cooldownUntil"] = json!(if cool > 0 { now() + cool } else { 0 });
                a["upstreamCode"] = json!(code);
            }
            if ok && s(a,"authState") != "需重新登录" {
                a["authState"] = json!("有效");
            }
        }
        let _ = self.save_locked(&state);
    }
    fn upstream(
        &self,
        id: &str,
        payload: &V,
        context: &RequestContext,
    ) -> std::result::Result<reqwest::blocking::Response, UpstreamFailure> {
        let mut a = self.ensure(id).map_err(|_| UpstreamFailure::classify(502, &V::Null))?;
        let mut last = UpstreamFailure::classify(502, &V::Null);
        // Account-scoped IDs keep independently rotated pools separate. One user
        // turn retains its ID across tools; each model generation gets a new ID.
        let session = self.cached_request_id(format!("session:{id}:{}", context.session));
        let conversation = format!("{}-{}-{}-{}-{}", &session[..8], &session[8..12], &session[12..16], &session[16..20], &session[20..]);
        let turn = self.cached_request_id(format!("turn:{id}:{}:{}", context.session, context.turn));
        let message = uuid7();
        for base in self.bases(&s(&a, "site")) {
            for retry in 0..2 {
                let r = self
                    .request(&a, &base, "/v2/chat/completions", payload)
                    .header("Accept", "text/event-stream")
                    .header("X-Conversation-ID", &conversation)
                    .header("X-Conversation-Request-ID", &turn)
                    .header("X-Conversation-Message-ID", &message)
                    .header("X-Request-ID", &message)
                    .header("X-Request-Trace-Id", hex(&random(16)))
                    .send();
                match r {
                    Ok(r) => {
                        let code = r.status().as_u16();
                        if [401, 403].contains(&code) && retry == 0 {
                            drop(r);
                            if self
                                .refresh_if_same(id, &s(&a, "accessToken"))
                                .unwrap_or(false)
                            {
                                a = self.account(id).map_err(|_| UpstreamFailure::classify(502, &V::Null))?;
                                continue;
                            }
                            last = UpstreamFailure::classify(code, &V::Null);
                            break;
                        }
                        if code < 400 {
                            return Ok(r);
                        }
                        last = UpstreamFailure::read(code, r);
                        if !last.rotate || last.code == Some(6004) {
                            return Err(last);
                        }
                        break;
                    }
                    Err(_) => {
                        last = UpstreamFailure::classify(502, &V::Null);
                        break;
                    }
                }
            }
        }
        Err(last)
    }
}
fn handle(mut req: Request, app: Arc<App>) {
    let path = req
        .url()
        .split('?')
        .next()
        .unwrap_or("")
        .trim_end_matches('/')
        .to_owned();
    let cfg = app.config();
    if req.method().as_str() == "GET" && (path.is_empty() || path == "/health") {
        let accounts = app.public_accounts();
        let aa = accounts.as_array().unwrap();
        respond(
            req,
            200,
            json!({"ok":true,"model":"deepseek-v4.1-flash","default_site":cfg["defaultSite"],"upstream_ready":aa.iter().any(|a|b(a,"enabled")),"pools":{"cn":aa.iter().filter(|a|b(a,"enabled")&&s(a,"site")=="cn").count(),"ai":aa.iter().filter(|a|b(a,"enabled")&&s(a,"site")=="ai").count()}}),
        );
        return;
    }
    if header(&req, "Authorization") != format!("Bearer {}", s(&cfg, "localKey")) {
        respond(
            req,
            401,
            json!({"error":{"message":"invalid api key","code":"invalid_api_key"}}),
        );
        return;
    }
    if req.method().as_str() == "GET" && ["/v1/models", "/models"].contains(&path.as_str()) {
        respond(req, 200, models());
        return;
    }
    if req.method().as_str() != "POST"
        || !["/v1/chat/completions", "/chat/completions"].contains(&path.as_str())
    {
        respond(req, 404, json!({"error":{"message":"not found"}}));
        return;
    }
    let body = match read_json_body(&mut req, CHAT_BODY_LIMIT) {
        Ok(v) if v.is_object() => v,
        Ok(_) => {
            app.log("请求拒绝", "HTTP 400 code=invalid_request 根节点必须是对象");
            respond(req, 400, json!({"error":{"message":"请求 JSON 必须是对象","code":"invalid_request"}}));
            return;
        }
        Err(e) => {
            app.log("请求拒绝", &format!("HTTP {} code={} content_length={:?} limit={CHAT_BODY_LIMIT}", e.status, e.code, req.body_length()));
            respond(req, e.status, json!({"error":{"message":e.message,"code":e.code,"type":"invalid_request_error"}}));
            return;
        }
    };
    let requested = match body["model"].as_str() {
        Some(m) if m != "default" => m,
        _ => "deepseek-v4.1-flash",
    }
    .to_owned();
    let (upstream, site, effort) = match parse_model(&requested, &s(&cfg, "defaultSite")) {
        Ok(x) => x,
        Err(e) => {
            respond(
                req,
                400,
                json!({"error":{"message":e,"code":"model_not_found"}}),
            );
            return;
        }
    };
    let stream = b(&body, "stream");
    let explicit_reasoning = !body["reasoning_effort"].is_null() || !body["reasoning"].is_null();
    // Move the message tree so image-heavy histories are not duplicated in RAM.
    let mut payload = prepare(body);
    payload["model"] = json!(upstream);
    if let Some(e) = effort {
        // Legacy aliases supply a default; explicit Hermes settings take precedence.
        if !explicit_reasoning {
            payload["reasoning_effort"] = json!(e);
        }
    }
    let mut excluded = Vec::new();
    let mut last = 503;
    let context = RequestContext::new(&payload, &header(&req, "X-BuddyHub-Session-ID"));
    while let Some(a) = app.pick(&site, &excluded) {
        let id = s(&a, "id");
        excluded.push(id.clone());
        let up = match app.upstream(&id, &payload, &context) {
            Ok(r) => r,
            Err(error) => {
                let code = error.status;
                let rotate = error.rotate;
                app.record_failure(&id, &error);
                last = code;
                if rotate {
                    app.log(
                        "号池切换",
                        &format!("{site} {} HTTP {code}", s(&a, "label")),
                    );
                    continue;
                }
                app.log("上游拒绝", &format!("{site} HTTP {code} code={:?}", error.code));
                respond(req, code, error.json());
                return;
            }
        };
        let mut frames = Frames {
            body: BufReader::new(up),
            done: false,
        };
        let mut first = None;
        let mut failure = None;
        for _ in 0..200 {
            match frames.next() {
                Ok(Some(raw)) => {
                    if business(&raw) {
                        let error = UpstreamFailure::classify(400, &raw);
                        failure = Some(error);
                        break;
                    }
                    if normalize(&raw, &requested).is_some() {
                        first = Some(raw);
                        break;
                    }
                }
                _ => break,
            }
        }
        let Some(first) = first else {
            if let Some(e) = failure {
                app.record_failure(&id, &e);
                if !e.rotate {
                    app.log("上游拒绝", &format!("{site} SSE code={:?}", e.code));
                    respond(req, e.status, e.json());
                    return;
                }
                last = e.status;
                continue;
            }
            app.finish(&id, false, "额度/连接异常，暂时冷却", 60000);
            last = 502;
            continue;
        };
        if stream {
            let failed = Arc::new(Mutex::new(false));
            let reader = StreamBody {
                frames,
                model: requested.clone(),
                pending: format!("data: {}\n\n", normalize(&first, &requested).unwrap())
                    .into_bytes(),
                at: 0,
                ended: false,
                failed: failed.clone(),
            };
            let headers = vec![
                Header::from_bytes("Content-Type", "text/event-stream; charset=utf-8").unwrap(),
                Header::from_bytes("Cache-Control", "no-cache, no-transform").unwrap(),
                Header::from_bytes("X-Accel-Buffering", "no").unwrap(),
            ];
            let result = req.respond(Response::new(StatusCode(200), headers, reader, None, None));
            let ok = result.is_ok() && !*failed.lock().unwrap();
            app.finish(&id, ok, if ok { "" } else { "上游流中断" }, 0);
            app.log(
                "模型调用",
                &format!(
                    "{site} {} {requested} {}",
                    s(&a, "label"),
                    if ok { "成功" } else { "流中断" }
                ),
            );
            return;
        }
        match aggregate(first, &mut frames, &requested) {
            Ok(full) => {
                respond(req, 200, full);
                app.finish(&id, true, "", 0);
                app.log(
                    "模型调用",
                    &format!("{site} {} {requested} 成功", s(&a, "label")),
                );
                return;
            }
            Err(_) => {
                app.finish(&id, false, "上游请求异常", 30000);
                last = 502;
            }
        }
    }
    let (status, error, retry) = app.unavailable_pool(&site, last);
    let mut response = Response::from_data(serde_json::to_vec(&error).unwrap())
        .with_status_code(StatusCode(status))
        .with_header(Header::from_bytes("Content-Type", "application/json; charset=utf-8").unwrap());
    if let Some(seconds) = retry {
        response.add_header(Header::from_bytes("Retry-After", seconds.to_string()).unwrap());
    }
    let _ = req.respond(response);
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn quota_reset_rejects_unknown_or_past_timestamps_and_private_text() {
        for msg in ["private-secret", "2000-01-01 12:00:00 UTC+8 private-secret",
            "2099-01-01 12:00:00 UTC+9 private-secret", "garbled UTC+8 private-secret"] {
            let failure = UpstreamFailure::classify(400, &json!({"code":6004,"msg":msg}));
            assert_eq!(failure.status,429);
            assert_eq!(failure.retry_at,None);
            assert_eq!(failure.cooldown_ms(),60000);
            assert!(!failure.message.contains("private-secret"));
        }
    }
    #[test]
    fn routing() {
        assert_eq!(
            parse_model("deepseek-v4.1-flash-ai-think-high", "cn").unwrap(),
            ("deepseek-v4.1-flash".into(), "ai".into(), Some("high".into()))
        );
        assert_eq!(parse_model("deepseek-v4.1-flash", "cn").unwrap().1, "cn");
        assert!(parse_model("bad-cn", "cn").is_err());
        assert!(parse_model("deepseek-v4-pro-cn", "cn").is_err());
        let catalog = models();
        let entries = catalog["data"].as_array().unwrap();
        let sites = entries.iter().map(|row| {
            let (model, site, effort) = parse_model(&s(row, "id"), "cn").unwrap();
            assert_eq!(model, MODELS[0]);
            assert!(effort.is_none());
            site
        }).collect::<std::collections::HashSet<_>>();
        assert_eq!(entries.len(), sites.len());
        assert_eq!(sites, ["cn".to_owned(), "ai".to_owned(), "auto".to_owned()].into_iter().collect());
    }
    #[test]
    fn compatibility() {
        let p = prepare(
            json!({"messages":[{"role":"developer","content":"test"}],"reasoning":{"enabled":true,"effort":"max"},"tool_choice":{"type":"function","function":{"name":"f"}}}),
        );
        assert_eq!(p["messages"][0]["role"], "system");
        assert_eq!(p["reasoning_effort"], "max");
        assert_eq!(p["tool_choice"], "f");
        assert!(p.get("reasoning").is_none());
        let p = prepare(json!({"reasoning":{"enabled":false,"effort":"max"}}));
        assert!(p.get("reasoning_effort").is_none());
    }
    #[test]
    fn sparse_chunks() {
        let n=normalize(&json!({"choices":[{"delta":{"content":"","reasoning_content":"","tool_calls":[{"index":0,"id":"call"}]}}]}),"auto-cn").unwrap();
        assert!(n["choices"][0]["delta"].get("reasoning_content").is_none());
        assert_eq!(n["choices"][0]["delta"]["tool_calls"][0]["id"], "call");
        assert!(quota("积分不足"));
    }
}
