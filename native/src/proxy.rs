use super::*;
use std::io::{BufRead, BufReader};
const MODELS: &[&str] = &["deepseek-v4.1-flash"];
const EFFORTS: &[&str] = &["minimal", "low", "medium", "high", "xhigh", "max"];
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
        // Regions select independent pools; reasoning is a request parameter, not a model.
        for site in ["cn", "ai"] {
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
        let mut state = self.state.lock().unwrap();
        let count = state.inflight.entry(id.into()).or_insert(0);
        *count = count.saturating_sub(1);
        if let Some(a) = state.accounts.iter_mut().find(|a| s(a, "id") == id) {
            let key = if ok { "successes" } else { "failures" };
            a[key] = json!(n(a, key) + 1.0);
            a["lastError"] = json!(error);
            a["cooldownUntil"] = json!(if cool > 0 { now() + cool } else { 0 });
            if ok {
                a["authState"] = json!("有效");
            }
        }
        let _ = self.save_locked(&state);
    }
    fn upstream(
        &self,
        id: &str,
        payload: &V,
    ) -> std::result::Result<reqwest::blocking::Response, (u16, bool)> {
        let mut a = self.ensure(id).map_err(|_| (502, true))?;
        let mut last = (502, true);
        for base in self.bases(&s(&a, "site")) {
            for retry in 0..2 {
                let r = self
                    .request(&a, &base, "/v2/chat/completions", payload)
                    .header("Accept", "text/event-stream")
                    .header("X-Request-ID", hex(&random(16)))
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
                                a = self.account(id).map_err(|_| (502, true))?;
                                continue;
                            }
                            last = (code, true);
                            break;
                        }
                        if code < 400 {
                            return Ok(r);
                        }
                        let text = r.text().unwrap_or_default();
                        let rotate = code == 429
                            || code >= 500
                            || [401, 403].contains(&code)
                            || quota(&text);
                        last = (code, rotate);
                        if code == 400 && !rotate {
                            return Err(last);
                        }
                        break;
                    }
                    Err(_) => {
                        last = (502, true);
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
    let body = match body(&mut req) {
        Ok(v) if v.is_object() => v,
        _ => {
            respond(req, 400, json!({"error":{"message":"JSON 格式错误"}}));
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
    let mut payload = prepare(body.clone());
    payload["model"] = json!(upstream);
    if let Some(e) = effort {
        // Legacy aliases supply a default; explicit Hermes settings take precedence.
        if body["reasoning_effort"].is_null() && body["reasoning"].is_null() {
            payload["reasoning_effort"] = json!(e);
        }
    }
    let mut excluded = Vec::new();
    let mut last = 503;
    while let Some(a) = app.pick(&site, &excluded) {
        let id = s(&a, "id");
        excluded.push(id.clone());
        let up = match app.upstream(&id, &payload) {
            Ok(r) => r,
            Err((code, rotate)) => {
                app.finish(
                    &id,
                    false,
                    &format!("上游 HTTP {code}"),
                    if rotate { 60000 } else { 0 },
                );
                last = code;
                if rotate {
                    app.log(
                        "号池切换",
                        &format!("{site} {} HTTP {code}", s(&a, "label")),
                    );
                    continue;
                }
                respond(
                    req,
                    code,
                    json!({"error":{"message":format!("上游拒绝请求，HTTP {code}"),"type":"upstream_error"}}),
                );
                return;
            }
        };
        let mut frames = Frames {
            body: BufReader::new(up),
            done: false,
        };
        let mut first = None;
        let mut nonretry = None;
        for _ in 0..200 {
            match frames.next() {
                Ok(Some(raw)) => {
                    if business(&raw) {
                        if !quota(&raw.to_string())
                            && ![401.0, 403.0, 429.0, 10001.0].contains(&n(&raw, "code"))
                        {
                            nonretry = Some("上游业务错误".to_owned());
                        }
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
            app.finish(&id, false, "额度/连接异常，暂时冷却", 60000);
            last = 502;
            if let Some(e) = nonretry {
                respond(
                    req,
                    502,
                    json!({"error":{"message":e,"type":"upstream_error"}}),
                );
                return;
            }
            continue;
        };
        if b(&body, "stream") {
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
    respond(
        req,
        last,
        json!({"error":{"message":format!("{} 号池暂无可用账号，请检查登录、额度或冷却状态",site.to_uppercase()),"type":"pool_unavailable"}}),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
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
        assert_eq!(sites, ["cn".to_owned(), "ai".to_owned()].into_iter().collect());
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
