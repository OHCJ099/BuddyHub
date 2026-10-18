//! Local administrator operations for the optional public gateway. Never mounted on its listener.
use super::*;
use std::{net::TcpStream, os::windows::process::CommandExt, process::{Command, Stdio}};
static LOCK: Mutex<()> = Mutex::new(());
const MODELS: [&str; 3] = ["deepseek-v4.1-flash-cn", "deepseek-v4.1-flash-ai", "deepseek-v4.1-flash-auto"];
fn read(path: &Path) -> Result<V> {
    if !path.exists() { return Ok(json!({})); }
    serde_json::from_slice(&fs::read(path).map_err(|_|"无法读取共享设置")?).map_err(|_|"共享数据格式异常，请保留文件并恢复备份".into())
}
fn write(path: &Path, v: &V) -> Result<()> { atomic(path, &serde_json::to_vec_pretty(v).unwrap()) }
fn digest(key: &str) -> String { hex(&Sha256::digest(key.as_bytes())) }
fn secret_read(app: &App) -> Result<V> {
    let path=app.data.join("sharing-secrets.enc");
    if !path.exists() { return Ok(json!({})); }
    let blob=fs::read(path).map_err(|_|"无法读取共享密钥")?;
    if blob.len()<28 { return Err("共享密钥存储异常".into()); }
    let plain=Aes256Gcm::new_from_slice(&app.key).map_err(|_|"共享密钥加密失败")?
        .decrypt(Nonce::from_slice(&blob[..12]), &blob[12..]).map_err(|_|"共享密钥解密失败")?;
    serde_json::from_slice(&plain).map_err(|_|"共享密钥格式异常".into())
}
fn secret_write(app: &App, value: &V) -> Result<()> {
    let iv=random(12);
    let bytes=Aes256Gcm::new_from_slice(&app.key).map_err(|_|"共享密钥加密失败")?
        .encrypt(Nonce::from_slice(&iv),serde_json::to_vec(value).unwrap().as_ref()).map_err(|_|"共享密钥加密失败")?;
    let mut data=iv; data.extend(bytes); atomic(&app.data.join("sharing-secrets.enc"), &data)
}
fn load(app: &App) -> Result<V> {
    let path=app.data.join("sharing.json"); let mut cfg=read(&path)?; let original=cfg.clone();
    if cfg.get("enabled").is_none() { cfg["enabled"]=json!(false); }
    if cfg.get("autoStart").is_none() { cfg["autoStart"]=json!(true); }
    if cfg.get("maxBodyMb").is_none() { cfg["maxBodyMb"]=json!(64); }
    if cfg.get("keys").is_none() { cfg["keys"]=json!([]); }
    let keys=cfg["keys"].as_array_mut().ok_or("共享 Key 列表格式异常")?;
    for key in keys {
        if s(key,"id").is_empty() { key["id"]=key["sha256"].clone(); }
        if key.get("monthlyRequests").is_none() { key["monthlyRequests"]=json!(0); }
        if key.get("totalRequests").is_none() { key["totalRequests"]=json!(0); }
        if key.get("models").is_none() { key["models"]=json!(MODELS); }
    }
    let mut secrets=secret_read(app)?;
    let old=read(&app.data.join("share/friend-1.json"))?;
    if s(&cfg,"publicUrl").is_empty() && s(&old,"url").starts_with("https://") {cfg["publicUrl"]=old["url"].clone();}
    if let Some(key)=cfg["keys"].as_array_mut().unwrap().iter_mut().find(|k| !s(&old,"key").is_empty() && s(k,"sha256")==digest(&s(&old,"key"))) {
        let hash=s(key,"sha256");
        if secrets.get(&hash).is_none() { secrets[&hash]=old["key"].clone(); secret_write(app,&secrets)?; }
    }
    for key in cfg["keys"].as_array_mut().unwrap() {
        if let Some(secret)=secrets[&s(key,"sha256")].as_str() { key["masked"]=json!(format!("sk-bh-••••{}",&secret[secret.len().saturating_sub(4)..])); }
    }
    if cfg!=original { write(&path,&cfg)?; }
    Ok(cfg)
}
fn usage_counts(usage: &V, id: &str) -> (u64,u64,u64) {
    let day=(Utc::now()+Duration::hours(8)).format("%Y-%m-%d").to_string();
    let old=usage["keys"][id].as_u64().unwrap_or(0);
    let today=if s(usage,"date")==day {old}else{0};
    let month=if usage.get("month").is_none() {if s(usage,"date").starts_with(&day[..7]) {old}else{0}}
        else if s(usage,"month")==day[..7] {usage["monthlyKeys"][id].as_u64().unwrap_or(0)} else {0};
    let total=usage["totalKeys"][id].as_u64().unwrap_or(old);
    (today,month,total)
}
fn public_key(k: &V, usage: &V, can_copy: bool) -> V {
    let (today,month,total)=usage_counts(usage,&s(k,"id"));
    let mut out=k.clone(); out.as_object_mut().unwrap().remove("sha256");
    out["usedToday"]=json!(today); out["usedMonth"]=json!(month); out["usedTotal"]=json!(total); out["canCopy"]=json!(can_copy);
    out["expired"]=json!(k["expiresAt"].as_i64().is_some_and(|x|x>0&&x<=now())); out
}
fn snapshot(app: &App, cfg: &V) -> Result<V> {
    let usage=read(&app.data.join("sharing-usage.json"))?; let secrets=secret_read(app)?;
    let gateway=TcpStream::connect_timeout(&"127.0.0.1:8791".parse().unwrap(),Wait::from_millis(150)).is_ok();
    let client=Client::builder().no_proxy().timeout(Wait::from_millis(700)).build().map_err(|_|"无法查询隧道")?;
    let tunnels=client.get("http://127.0.0.1:4041/api/tunnels").send().ok().and_then(|r|r.json::<V>().ok());
    let url=tunnels.as_ref().and_then(|v|v["tunnels"].as_array()).and_then(|a|a.iter().find(|v|s(v,"proto")=="https"&&s(&v["config"],"addr")=="http://127.0.0.1:8791")).map(|v|format!("{}/v1",s(v,"public_url"))).unwrap_or_default();
    let live=!url.is_empty();
    if live && url!=s(cfg,"publicUrl") {let mut update=cfg.clone();update["publicUrl"]=json!(url);write(&app.data.join("sharing.json"),&update)?;}
    let url=if live {url}else{s(cfg,"publicUrl")};
    let keys=cfg["keys"].as_array().unwrap().iter().map(|k|public_key(k,&usage,secrets.get(s(k,"sha256")).is_some())).collect::<Vec<_>>();
    Ok(json!({"enabled":cfg["enabled"],"autoStart":cfg["autoStart"],"maxBodyMb":cfg["maxBodyMb"],"gatewayRunning":gateway,"tunnelRunning":live,"url":url,"tokenConfigured":app.data.join("share/ngrok.yml").is_file(),"agentInstalled":app.data.join("share/ngrok.exe").is_file(),"keys":keys}))
}
fn validate_key(d: &V) -> Result<()> {
    let name=s(d,"name");
    if name.trim().is_empty() || name.chars().count()>60 || name.chars().any(char::is_control) { return Err("名称需为 1–60 个可见字符".into()); }
    for (field,max) in [("dailyRequests",10_000_000),("monthlyRequests",100_000_000),("totalRequests",1_000_000_000),("requestsPerMinute",100_000),("concurrency",16)] {
        let n=d[field].as_u64().ok_or("限额必须是非负整数")?;
        if n>max || (field=="concurrency"&&n==0) { return Err("限额超出允许范围；并发应为 1–16".into()); }
    }
    if !d["enabled"].is_boolean() { return Err("启用状态无效".into()); }
    if let Some(v)=d.get("expiresAt") {if v.as_i64().is_none_or(|n|n<0) {return Err("有效期无效".into());}}
    let models=d["models"].as_array().ok_or("请选择可用模型")?;
    if models.is_empty() || models.iter().any(|m|!m.as_str().is_some_and(|s|MODELS.contains(&s))) { return Err("请选择至少一个可用的 4.1 模型".into()); }
    let secret=s(d,"key");
    if !secret.is_empty() && (secret.len()<16 || secret.len()>256 || !secret.bytes().all(|b|b.is_ascii_alphanumeric() || b"-_.".contains(&b))) { return Err("自定义 Key 需为 16–256 位字母、数字、减号、下划线或点".into()); }
    Ok(())
}
fn control(app: &App, enabled: bool, auto: bool) -> Result<()> {
    let script=if enabled {
        if !app.data.join("share/ngrok.exe").is_file() || !app.data.join("share/ngrok.yml").is_file() || !app.data.join("share/buddyhub-share.exe").is_file() { return Err("请先安装 ngrok 并保存隧道凭据".into()); }
        format!("& {{ {} }} -AutoStart ${}",include_str!("../../tools/install-sharing-tasks.ps1"),if auto {"true"} else {"false"})
    } else {
        r#"$ErrorActionPreference='Stop'
foreach($name in @('BuddyHub Sharing Gateway','BuddyHub Sharing Tunnel')) {
 $task=Get-ScheduledTask -TaskName $name -ErrorAction SilentlyContinue
 if($task){Disable-ScheduledTask -TaskName $name | Out-Null; Stop-ScheduledTask -TaskName $name}
}
$folder=[IO.Path]::GetFullPath((Join-Path $env:BUDDYHUB_DATA 'share')).TrimEnd('\')
Get-CimInstance Win32_Process -Filter "Name='buddyhub-share.exe' OR Name='ngrok.exe'" | Where-Object { $_.ExecutablePath -and [IO.Path]::GetDirectoryName($_.ExecutablePath) -eq $folder } | ForEach-Object {Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue}
"#.to_owned()
    };
    let script_path=app.data.join("share/control.ps1");
    let mut encoded=vec![0xef,0xbb,0xbf]; encoded.extend(script.as_bytes()); atomic(&script_path,&encoded)?;
    let mut child=Command::new("powershell.exe").args(["-NoProfile","-NonInteractive","-ExecutionPolicy","Bypass","-File"]).arg(&script_path)
        .env("BUDDYHUB_DATA",&app.data).creation_flags(0x08000000).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn().map_err(|_|"无法调整共享后台")?;
    let start=Instant::now();
    loop { match child.try_wait() {
        Ok(Some(status))=>return if status.success(){Ok(())}else{Err("共享后台操作失败，请点击重试；原有反代不受影响".into())},
        Ok(None) if start.elapsed()<Wait::from_secs(25)=>thread::sleep(Wait::from_millis(50)),
        _=>{let _=child.kill(); let _=child.wait();return Err("调整共享后台超时，请重试".into());}
    }}
}
pub fn api(app: &App, route: &str, d: &V) -> Result<V> {
    let _guard=LOCK.lock().map_err(|_|"共享设置忙，请重试")?;
    let mut cfg=load(app)?;
    match route {
        "/admin/sharing"=>snapshot(app,&cfg),
        "/admin/sharing/settings"=>{
            for field in ["enabled","autoStart"] { if let Some(v)=d.get(field) {if !v.is_boolean(){return Err("共享开关格式无效".into());}cfg[field]=v.clone();} }
            if let Some(v)=d.get("maxBodyMb") {if v.as_u64().is_none_or(|n|!(1..=128).contains(&n)){return Err("请求大小应为 1–128 MB".into());}cfg["maxBodyMb"]=v.clone();}
            let token=s(d,"authtoken");
            if !token.is_empty() {
                if !(20..=200).contains(&token.len()) || !token.bytes().all(|b|b.is_ascii_alphanumeric()||b"_-".contains(&b)){return Err("ngrok Authtoken 格式无效".into());}
                let yml=format!("version: 3\nagent:\n  authtoken: {}\n  web_addr: 127.0.0.1:4041\n",serde_json::to_string(&token).unwrap());
                atomic(&app.data.join("share/ngrok.yml"),yml.as_bytes())?;
            }
            // Disabled config is visible to in-flight admission before any processes are stopped.
            write(&app.data.join("sharing.json"),&cfg)?;
            if (!token.is_empty() || b(d,"restart")) && b(&cfg,"enabled") { control(app,false,false)?; }
            control(app,b(&cfg,"enabled"),b(&cfg,"autoStart"))?;
            app.log("公网共享",if b(&cfg,"enabled"){"已开启或更新共享设置"}else{"已关闭隧道和共享入口"});
            snapshot(app,&cfg)
        }
        "/admin/sharing/key"=>{
            validate_key(d)?;
            let requested_id=s(d,"id"); let mut secrets=secret_read(app)?;
            let keys=cfg["keys"].as_array_mut().unwrap();
            let index=if requested_id.is_empty() {
                if keys.len()>=100 {return Err("最多管理 100 个 Key".into());}
                keys.push(json!({"id":hex(&random(16))})); keys.len()-1
            } else {keys.iter().position(|k|s(k,"id")==requested_id).ok_or("未找到这个 Key")?};
            let k=&mut keys[index]; let id=s(k,"id");
            let mut secret=s(d,"key");
            if secret.is_empty() && (b(d,"rotate") || s(k,"sha256").is_empty()) { secret=format!("sk-bh-{}",hex(&random(24))); }
            if !secret.is_empty() {
                let hash=digest(&secret);k["sha256"]=json!(hash); k["masked"]=json!(format!("sk-bh-••••{}",&secret[secret.len()-4..])); secrets[&hash]=json!(secret);
            }
            for field in ["name","enabled","dailyRequests","monthlyRequests","totalRequests","requestsPerMinute","concurrency","models"] { k[field]=d[field].clone(); }
            k["expiresAt"]=json!(d["expiresAt"].as_i64().unwrap_or(0));
            let hashes=keys.iter().map(|k|s(k,"sha256")).collect::<std::collections::HashSet<_>>();
            if hashes.len()!=keys.len() {return Err("这个 Key 已被使用，请换一个".into());}
            secret_write(app,&secrets)?; write(&app.data.join("sharing.json"),&cfg)?;
            app.log("共享 Key 已更新","密钥内容未记入日志；已有用量保留");
            Ok(json!({"ok":true,"id":id}))
        }
        "/admin/sharing/key-copy"=>{
            let id=s(d,"id"); let key=cfg["keys"].as_array().unwrap().iter().find(|k|s(k,"id")==id).ok_or("未找到这个 Key")?;
            let secrets=secret_read(app)?; let secret=secrets[&s(key,"sha256")].as_str().ok_or("旧 Key 无法读取，请重置生成新 Key")?;
            let status=snapshot(app,&cfg)?;
            Ok(json!({"key":secret,"url":status["url"],"models":key["models"]}))
        }
        _=>Err("共享接口不存在".into())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn validates_sharing_limits_and_models() {
        let mut d=json!({"name":"friend","enabled":true,"dailyRequests":0,"monthlyRequests":100,"totalRequests":1000,"requestsPerMinute":10,"concurrency":2,"models":[MODELS[2]]});
        assert!(validate_key(&d).is_ok()); d["concurrency"]=json!(0); assert!(validate_key(&d).is_err());
        d["concurrency"]=json!(2);d["models"]=json!(["other"]);assert!(validate_key(&d).is_err());
    }
    #[test]
    fn public_snapshot_never_contains_key_digest() {
        let key=json!({"id":"id","sha256":"private-digest","enabled":true});
        let out=public_key(&key,&json!({}),true).to_string(); assert!(!out.contains("sha256")); assert!(!out.contains("private-digest"));
    }
}
