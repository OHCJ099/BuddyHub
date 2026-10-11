use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};

#[test]
fn first_run_initializes_empty_encrypted_vault_without_rotating_keys() {
    let data=std::env::temp_dir().join(format!("buddyhub-init-test-{}",hex(&random(8))));
    initialize_data(&data).unwrap();
    let config=fs::read(data.join("config.json")).unwrap();
    let key=fs::read(data.join("vault.key")).unwrap();
    let cfg:V=serde_json::from_slice(&config).unwrap();
    assert_ne!(s(&cfg,"localKey"),s(&cfg,"adminKey"));
    let blob:V=serde_json::from_slice(&fs::read(data.join("accounts.enc")).unwrap()).unwrap();
    let mut ciphertext=B64.decode(s(&blob,"data")).unwrap();
    ciphertext.extend(B64.decode(s(&blob,"tag")).unwrap());
    let iv=B64.decode(s(&blob,"iv")).unwrap();
    let plain=Aes256Gcm::new_from_slice(&key).unwrap().decrypt(Nonce::from_slice(&iv),ciphertext.as_ref()).unwrap();
    assert_eq!(plain,b"[]");
    initialize_data(&data).unwrap();
    assert_eq!(config,fs::read(data.join("config.json")).unwrap());
    assert_eq!(key,fs::read(data.join("vault.key")).unwrap());
    fs::remove_file(data.join("vault.key")).unwrap();
    assert!(initialize_data(&data).is_err());
    assert!(data.join("accounts.enc").exists());
    fs::remove_dir_all(data).unwrap();
}

struct Fixture {
    app: Arc<App>,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        if let Some(server) = self.app.proxy.lock().unwrap().take() {
            server.unblock();
        }
        thread::sleep(Wait::from_millis(20));
        let _ = fs::remove_dir_all(&self.app.data);
    }
}
fn fixture() -> Fixture {
    let data = std::env::temp_dir().join(format!("buddyhub-native-test-{}", hex(&random(8))));
    fs::create_dir_all(&data).unwrap();
    let accounts=[("cn1","cn"),("cn2","cn"),("ai1","ai")].iter().map(|(id,site)|json!({"id":id,"uid":id,"site":site,"domain":format!("www.workbuddy.{site}"),"label":id,"enabled":true,"accessToken":format!("fake-token-{id}"),"refreshToken":"fake-refresh","expiresAt":now()+3600000,"authState":"有效","requests":0,"successes":0,"failures":0,"checkin":{}})).collect();
    let app = Arc::new(App {
        state: Mutex::new(State {
            config: json!({"port":0,"localKey":"test-key","defaultSite":"cn","services":[],"startupGuard":false,"checkinEnabled":true,"checkinTime":"00:00"}),
            accounts,
            inflight: HashMap::new(),
            cursor: HashMap::new(),
            children: HashMap::new(),
        }),
        data,
        key: random(32),
        http: Client::builder()
            .no_proxy()
            .timeout(Wait::from_secs(5))
            .build()
            .unwrap(),
        auth: Mutex::new(HashMap::new()),
        checkin: Mutex::new(()),
        services: Mutex::new(()),
        proxy: Mutex::new(None),
        snapshot: Mutex::new((Instant::now() - Wait::from_secs(60), json!([]))),
    });
    Fixture { app }
}
struct Mock {
    server: Arc<Server>,
    url: String,
    join: Option<thread::JoinHandle<()>>,
}
impl Drop for Mock {
    fn drop(&mut self) {
        self.server.unblock();
        if let Some(t) = self.join.take() {
            t.join().unwrap();
        }
    }
}
fn mock(handler: impl Fn(Request) + Send + 'static) -> Mock {
    let server = Arc::new(Server::http("127.0.0.1:0").unwrap());
    let url = format!("http://{}", server.server_addr().to_ip().unwrap());
    let svr = server.clone();
    let join = thread::spawn(move || {
        for req in svr.incoming_requests() {
            handler(req)
        }
    });
    Mock {
        server,
        url,
        join: Some(join),
    }
}
fn bind(app: &Arc<App>, mock: &Mock) {
    app.state.lock().unwrap().config["testHosts"] = json!({"cn":[mock.url],"ai":[mock.url]});
}
fn proxy_url(app: &Arc<App>) -> String {
    app.start_proxy().unwrap();
    format!(
        "http://{}",
        app.proxy
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .server_addr()
            .to_ip()
            .unwrap()
    )
}
fn chat(app: &Arc<App>, base: &str, model: &str, stream: bool) -> reqwest::blocking::Response {
    app.http
        .post(format!("{base}/v1/chat/completions"))
        .bearer_auth("test-key")
        .json(&json!({"model":model,"stream":stream,"messages":[{"role":"user","content":"test"}]}))
        .send()
        .unwrap()
}
fn ok_stream(req: Request) {
    let _=req.respond(Response::from_string("data: {\"id\":\"chat-test\",\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\",\"content\":\"OK\",\"reasoning_content\":\"\"},\"finish_reason\":null}]}\n\ndata: {\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}],\"usage\":{\"total_tokens\":2}}\n\ndata: [DONE]\n\n").with_header(Header::from_bytes("Content-Type","text/event-stream").unwrap()));
}

#[test]
fn native_hermes_effort_is_independent_of_model_and_legacy_alias() {
    let f = fixture();
    let observed = Arc::new(Mutex::new(Vec::<V>::new()));
    let saved = observed.clone();
    let m = mock(move |mut req| {
        let payload = body(&mut req).unwrap();
        saved.lock().unwrap().push(payload);
        ok_stream(req);
    });
    bind(&f.app, &m);
    let url = proxy_url(&f.app);
    for (model, effort) in [
        ("deepseek-v4.1-flash-cn", "low"),
        ("deepseek-v4.1-flash-ai", "max"),
        ("deepseek-v4.1-flash-ai-think-max", "medium"),
        ("deepseek-v4.1-flash-cn-think-max", "none"),
    ] {
        let response = f.app.http.post(format!("{url}/v1/chat/completions"))
            .bearer_auth("test-key")
            .json(&json!({"model":model,"reasoning_effort":effort,"stream":false,"messages":[{"role":"user","content":"test"}]}))
            .send().unwrap();
        assert!(response.status().is_success());
        response.text().unwrap();
    }
    let calls = observed.lock().unwrap();
    assert!(calls.iter().all(|v| v["model"] == "deepseek-v4.1-flash"));
    assert_eq!(calls[0]["reasoning_effort"], "low");
    assert_eq!(calls[1]["reasoning_effort"], "max");
    assert_eq!(calls[2]["reasoning_effort"], "medium");
    assert!(calls[3].get("reasoning_effort").is_none());
}

#[test]
fn native_automatic_checkin_does_not_wait_for_fixed_time() {
    let f = fixture();
    f.app.update("cn2", json!({"enabled":false})).unwrap();
    f.app.state.lock().unwrap().config["checkinTime"] = json!("23:59");
    let claims = Arc::new(AtomicUsize::new(0));
    let c = claims.clone();
    let m = mock(move |req| {
        let d = if req.url().ends_with("daily-checkin") {
            c.fetch_add(1, Ordering::SeqCst);
            json!({"code":0,"data":{"credit":100}})
        } else if req.url() == "/billing/meter/get-user-resource-summary" {
            json!({"code":0,"data":{"Packages":[{"CycleRemainCapacity":100}]}})
        } else {
            json!({"code":0,"data":{"active":true,"today_checked_in":c.load(Ordering::SeqCst)>0,"today_credit":100}})
        };
        respond(req, 200, d);
    });
    bind(&f.app, &m);
    f.app.state.lock().unwrap().config["checkinEnabled"] = json!(false);
    f.app.tick();
    assert_eq!(claims.load(Ordering::SeqCst), 0);
    f.app.state.lock().unwrap().config["checkinEnabled"] = json!(true);
    f.app.tick();
    assert!(b(&f.app.account("cn1").unwrap()["checkin"], "success"));
    f.app.tick();
    assert_eq!(claims.load(Ordering::SeqCst), 1);
    f.app.update("cn1", json!({"checkin":{"date":"2000-01-01","success":true}})).unwrap();
    f.app.tick();
    assert_eq!(s(&f.app.account("cn1").unwrap()["checkin"], "date"), s(&china(), "date"));
}

#[test]
fn native_checkin_confirms_once_and_uses_spendable_balance() {
    let f = fixture();
    f.app.update("cn2", json!({"enabled":false})).unwrap();
    let claims = Arc::new(AtomicUsize::new(0));
    let c = claims.clone();
    let m = mock(move |req| {
        let d = if req.url().ends_with("daily-checkin") {
            c.fetch_add(1, Ordering::SeqCst);
            json!({"code":0,"data":{"credit":100}})
        } else if req.url() == "/billing/meter/get-user-resource-summary" {
            json!({"code":0,"data":{"Packages":[{"CycleRemainCapacity":"32"}]}})
        } else {
            json!({"code":0,"data":{"active":true,"today_checked_in":c.load(Ordering::SeqCst)>0,"today_credit":100,"total_credits":432}})
        };
        respond(req, 200, d);
    });
    bind(&f.app, &m);
    let r = f.app.checkins(false, None).unwrap();
    assert!(b(&r[0], "success"));
    assert_eq!(f.app.account("cn1").unwrap()["balance"], 32.0);
    f.app.checkins(false, None).unwrap();
    assert_eq!(claims.load(Ordering::SeqCst), 1);
}
#[test]
fn native_checkin_requires_confirmation() {
    let f = fixture();
    f.app.update("cn2", json!({"enabled":false})).unwrap();
    let m = mock(|req| {
        respond(
            req,
            200,
            json!({"code":0,"data":{"active":true,"today_checked_in":false}}),
        )
    });
    bind(&f.app, &m);
    assert_eq!(f.app.checkins(true, None).unwrap()[0]["success"], false);
}
#[test]
fn native_pool_retries_cn_only_and_preserves_stream_alias() {
    let f = fixture();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let calls = seen.clone();
    let m = mock(move |req| {
        let uid = header(&req, "X-User-Id");
        calls.lock().unwrap().push(uid.clone());
        if uid == "cn1" {
            let _ = req.respond(Response::from_string(
                "data: {\"code\":1004,\"msg\":\"积分不足\"}\n\n",
            ));
        } else {
            ok_stream(req);
        }
    });
    bind(&f.app, &m);
    let url = proxy_url(&f.app);
    let r = chat(&f.app, &url, "deepseek-v4.1-flash-cn", true);
    assert_eq!(r.status().as_u16(), 200);
    let text = r.text().unwrap();
    assert!(text.contains("deepseek-v4.1-flash-cn"));
    assert!(text.contains("OK"));
    assert_eq!(*seen.lock().unwrap(), vec!["cn1", "cn2"]);
}
#[test]
fn native_stream_never_replays_after_content() {
    let f = fixture();
    let seen = Arc::new(AtomicUsize::new(0));
    let calls = seen.clone();
    let m = mock(move |req| {
        calls.fetch_add(1, Ordering::SeqCst);
        let _=req.respond(Response::from_string("data: {\"choices\":[{\"delta\":{\"content\":\"partial\"}}]}\n\ndata: {\"code\":1004,\"msg\":\"积分不足\"}\n\n"));
    });
    bind(&f.app, &m);
    let url = proxy_url(&f.app);
    let text = chat(&f.app, &url, "deepseek-v4.1-flash-cn", true)
        .text()
        .unwrap();
    assert!(text.contains("partial"));
    assert!(text.contains("流中断"));
    assert_eq!(seen.load(Ordering::SeqCst), 1);
}
#[test]
fn native_nonstream_tool_arguments_are_assembled() {
    let f = fixture();
    let m = mock(|req| {
        let _=req.respond(Response::from_string("data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call1\",\"function\":{\"name\":\"test\",\"arguments\":\"{\"}}]}}]}\n\ndata: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"arguments\":\"}\"}}]},\"finish_reason\":\"tool_calls\"}]}\n\ndata: [DONE]\n\n"));
    });
    bind(&f.app, &m);
    let url = proxy_url(&f.app);
    let j = chat(&f.app, &url, "deepseek-v4.1-flash-ai", false)
        .json::<V>()
        .unwrap();
    assert_eq!(
        j["choices"][0]["message"]["tool_calls"][0]["function"]["arguments"],
        "{}"
    );
    assert_eq!(j["choices"][0]["message"]["content"], V::Null);
}
#[test]
fn native_refresh_is_single_flight() {
    let f = fixture();
    f.app.update("cn1", json!({"expiresAt":1})).unwrap();
    let count = Arc::new(AtomicUsize::new(0));
    let c = count.clone();
    let m = mock(move |req| {
        c.fetch_add(1, Ordering::SeqCst);
        respond(
            req,
            200,
            json!({"code":0,"data":{"accessToken":"new-fake-token","refreshToken":"new-fake-refresh","expiresIn":3600}}),
        );
    });
    bind(&f.app, &m);
    let a = f.app.clone();
    let t = thread::spawn(move || a.ensure("cn1").unwrap());
    f.app.ensure("cn1").unwrap();
    t.join().unwrap();
    assert_eq!(count.load(Ordering::SeqCst), 1);
    let encrypted = fs::read_to_string(f.app.data.join("accounts.enc")).unwrap();
    assert!(!encrypted.contains("new-fake-token"));
}
#[test]
fn native_empty_pool_and_unknown_model_do_not_call_other_region() {
    let f = fixture();
    f.app.update("cn1", json!({"enabled":false})).unwrap();
    f.app.update("cn2", json!({"enabled":false})).unwrap();
    let count = Arc::new(AtomicUsize::new(0));
    let c = count.clone();
    let m = mock(move |req| {
        c.fetch_add(1, Ordering::SeqCst);
        ok_stream(req);
    });
    bind(&f.app, &m);
    let url = proxy_url(&f.app);
    assert_eq!(
        chat(&f.app, &url, "deepseek-v4.1-flash-cn", false)
            .status()
            .as_u16(),
        503
    );
    assert_eq!(chat(&f.app, &url, "bad-cn", false).status().as_u16(), 400);
    assert_eq!(
        f.app
            .http
            .get(format!("{url}/v1/models"))
            .send()
            .unwrap()
            .status()
            .as_u16(),
        401
    );
    assert_eq!(count.load(Ordering::SeqCst), 0);
}
