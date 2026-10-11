use super::*;
use std::{
    net::TcpStream,
    os::windows::process::CommandExt,
    process::{Command, Stdio},
};

const HIDDEN: u32 = 0x08000000;
fn command(exe: &str, args: &[String]) -> Result<String> {
    let out = Command::new(exe)
        .args(args)
        .creation_flags(HIDDEN)
        .output()
        .map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err("本地服务操作失败".into());
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_owned())
}
pub fn launch(exe: &Path, args: &[String], cwd: Option<&str>, log: Option<&Path>) -> Result<u32> {
    let mut c = Command::new(exe);
    c.args(args).creation_flags(HIDDEN).stdin(Stdio::null());
    if let Some(d) = cwd {
        c.current_dir(d);
    }
    if let Some(p) = log {
        fs::create_dir_all(p.parent().unwrap()).map_err(|e| e.to_string())?;
        let f = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(p)
            .map_err(|e| e.to_string())?;
        c.stdout(f.try_clone().map_err(|e| e.to_string())?)
            .stderr(f);
    } else {
        c.stdout(Stdio::null()).stderr(Stdio::null());
    }
    let mut child = c.spawn().map_err(|_| "启动程序失败")?;
    let id = child.id();
    thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(id)
}
#[repr(C)]
struct Counters {
    cb: u32,
    faults: u32,
    peak: usize,
    working: usize,
    peak_paged: usize,
    paged: usize,
    peak_nonpaged: usize,
    nonpaged: usize,
    pagefile: usize,
    peak_pagefile: usize,
    private: usize,
}
#[link(name = "psapi")]
unsafe extern "system" {
    fn GetProcessMemoryInfo(process: *mut std::ffi::c_void, out: *mut Counters, size: u32) -> i32;
}
#[link(name = "kernel32")]
unsafe extern "system" {
    fn GetCurrentProcess() -> *mut std::ffi::c_void;
}
pub fn memory() -> (usize, usize) {
    unsafe {
        let mut c: Counters = std::mem::zeroed();
        c.cb = std::mem::size_of::<Counters>() as u32;
        GetProcessMemoryInfo(
            GetCurrentProcess(),
            &mut c,
            std::mem::size_of::<Counters>() as u32,
        );
        (c.working, c.private)
    }
}
type Handle = *mut std::ffi::c_void;
#[link(name = "advapi32")]
unsafe extern "system" {
    fn RegOpenKeyExW(
        key: Handle,
        path: *const u16,
        options: u32,
        access: u32,
        result: *mut Handle,
    ) -> i32;
    fn RegCreateKeyExW(
        key: Handle,
        path: *const u16,
        reserved: u32,
        class: *mut u16,
        options: u32,
        access: u32,
        security: Handle,
        result: *mut Handle,
        disposition: *mut u32,
    ) -> i32;
    fn RegQueryValueExW(
        key: Handle,
        name: *const u16,
        reserved: *mut u32,
        kind: *mut u32,
        data: *mut u8,
        length: *mut u32,
    ) -> i32;
    fn RegSetValueExW(
        key: Handle,
        name: *const u16,
        reserved: u32,
        kind: u32,
        data: *const u8,
        length: u32,
    ) -> i32;
    fn RegDeleteValueW(key: Handle, name: *const u16) -> i32;
    fn RegCloseKey(key: Handle) -> i32;
}
fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}
fn guard_registry(app: &App) -> Vec<String> {
    let mut changes = Vec::new();
    unsafe {
        let hkcu = 0x80000001u32 as i32 as isize as Handle;
        let mut run = std::ptr::null_mut();
        if RegOpenKeyExW(
            hkcu,
            wide("Software\\Microsoft\\Windows\\CurrentVersion\\Run").as_ptr(),
            0,
            0x0001 | 0x0002,
            &mut run,
        ) != 0
        {
            return changes;
        }
        for name in [
            "WorkBuddy.WorkBuddy",
            "WorkBuddy.WorkBuddyAI",
            "WorkBuddy AI",
            "WorkBuddyAI",
            "WorkBuddy",
        ] {
            let mut len = 0;
            let mut kind = 0;
            let wn = wide(name);
            if RegQueryValueExW(
                run,
                wn.as_ptr(),
                std::ptr::null_mut(),
                &mut kind,
                std::ptr::null_mut(),
                &mut len,
            ) != 0
                || kind != 1
                || len > 65536
            {
                continue;
            }
            let mut data = vec![0u8; len as usize];
            if RegQueryValueExW(
                run,
                wn.as_ptr(),
                std::ptr::null_mut(),
                &mut kind,
                data.as_mut_ptr(),
                &mut len,
            ) != 0
            {
                continue;
            }
            let val = String::from_utf16_lossy(
                &data
                    .chunks_exact(2)
                    .map(|x| u16::from_le_bytes([x[0], x[1]]))
                    .collect::<Vec<_>>(),
            )
            .trim_end_matches('\0')
            .to_owned();
            let backup = app.data.join("backups").join(format!(
                "startup-{}.txt",
                name.chars()
                    .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
                    .collect::<String>()
            ));
            if !backup.exists() {
                let _ = atomic(&backup, val.as_bytes());
            }
            let mut approved = std::ptr::null_mut();
            if RegCreateKeyExW(
                hkcu,
                wide(
                    "Software\\Microsoft\\Windows\\CurrentVersion\\Explorer\\StartupApproved\\Run",
                )
                .as_ptr(),
                0,
                std::ptr::null_mut(),
                0,
                0x0002,
                std::ptr::null_mut(),
                &mut approved,
                std::ptr::null_mut(),
            ) != 0
            {
                continue;
            }
            let disabled = [3u8, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
            let ok = RegSetValueExW(approved, wn.as_ptr(), 0, 3, disabled.as_ptr(), 12) == 0;
            RegCloseKey(approved);
            if ok && RegDeleteValueW(run, wn.as_ptr()) == 0 {
                changes.push(name.into());
            }
        }
        RegCloseKey(run);
    }
    changes
}
fn listening(port: u16) -> bool {
    TcpStream::connect_timeout(
        &format!("127.0.0.1:{port}").parse().unwrap(),
        Wait::from_millis(150),
    )
    .is_ok()
}
impl App {
    pub fn guard(&self, force: bool) -> Result<V> {
        if !b(&self.config(), "startupGuard") && !force {
            return Ok(json!({"enabled":false}));
        }
        let mut changed = Vec::new();
        let home = PathBuf::from(std::env::var_os("USERPROFILE").unwrap());
        for folder in [".workbuddy", ".workbuddy-ai"] {
            let p = home.join(folder).join("settings.json");
            if let Ok(raw) = fs::read_to_string(&p) {
                if let Ok(mut j) = serde_json::from_str::<V>(raw.trim_start_matches('\u{feff}')) {
                    if j["autoLaunchDesired"] != false {
                        let backup = self
                            .data
                            .join("backups")
                            .join(format!("{folder}-settings-before.json"));
                        if !backup.exists() {
                            atomic(&backup, raw.as_bytes())?;
                        }
                        j["autoLaunchDesired"] = json!(false);
                        atomic(&p, &serde_json::to_vec_pretty(&j).unwrap())?;
                        changed.push(format!("{folder} 偏好"));
                    }
                }
            }
        }
        changed.extend(guard_registry(self));
        if !changed.is_empty() {
            self.log("自启动已关闭", &changed.join("，"));
        }
        Ok(json!({"enabled":true,"changed":changed}))
    }
    pub fn service_snapshot(&self) -> V {
        let mut cache = self.snapshot.lock().unwrap();
        if cache.0.elapsed() < Wait::from_secs(4) {
            return cache.1.clone();
        }
        let cfg = self.config();
        let children = self.state.lock().unwrap().children.clone();
        let services = cfg["services"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| {
                let mut x = s.clone();
                let internal = super::s(s, "kind") == "internal";
                let running = if internal {
                    self.proxy.lock().unwrap().is_some()
                } else {
                    listening(n(s, "port") as u16)
                };
                x["running"] = json!(running);
                x["status"] = json!(if running { "运行中" } else { "已停止" });
                x["canStop"] = json!(
                    internal
                        || super::s(s, "kind") == "docker"
                        || children.contains_key(&super::s(s, "id"))
                );
                x["portConflict"] = json!(false);
                x["pids"] = json!(
                    children
                        .get(&super::s(s, "id"))
                        .map(|id| vec![*id])
                        .unwrap_or_default()
                );
                x
            })
            .collect::<Vec<_>>();
        cache.0 = Instant::now();
        cache.1 = json!(services);
        cache.1.clone()
    }
    pub fn service_action(self: &Arc<Self>, id: &str, action: &str) -> Result<()> {
        let _g = self.services.lock().unwrap();
        if !["start", "stop", "restart"].contains(&action) {
            return Err("操作无效".into());
        }
        let cfg = self.config();
        let service = cfg["services"]
            .as_array()
            .unwrap()
            .iter()
            .find(|x| s(x, "id") == id)
            .ok_or("没有这个服务")?
            .clone();
        if action == "restart" {
            self.service_once(&service, "stop")?;
            for _ in 0..20 {
                if !listening(n(&service, "port") as u16) {
                    break;
                }
                thread::sleep(Wait::from_millis(100));
            }
            self.service_once(&service, "start")
        } else {
            self.service_once(&service, action)
        }
    }
    fn service_once(self: &Arc<Self>, svc: &V, action: &str) -> Result<()> {
        let id = s(svc, "id");
        let kind = s(svc, "kind");
        if kind == "internal" {
            if action == "start" {
                self.start_proxy()?;
            } else {
                if let Some(p) = self.proxy.lock().unwrap().take() {
                    p.unblock();
                }
                let mut state = self.state.lock().unwrap();
                state.config["proxyEnabled"] = json!(false);
                self.save_config(&state)?;
            }
        } else if kind == "docker" {
            let compose = s(svc, "compose");
            let mut args = vec!["compose".into(), "-f".into(), compose];
            args.extend(
                if action == "start" {
                    vec!["up", "-d", "--no-build", "--pull", "never"]
                } else {
                    vec!["stop"]
                }
                .iter()
                .map(|s| s.to_string()),
            );
            if let Some(a) = svc["dockerServices"].as_array() {
                args.extend(a.iter().filter_map(V::as_str).map(str::to_owned));
            }
            command(
                "C:\\Program Files\\Docker\\Docker\\resources\\bin\\docker.exe",
                &args,
            )
            .map_err(|_| "Docker 操作失败，请先启动 Docker Desktop 并确认镜像已安装")?;
        } else if action == "start" {
            if listening(n(svc, "port") as u16) {
                return Ok(());
            }
            let exe = s(svc, "executable");
            if !Path::new(&exe).exists() {
                return Err("找不到启动程序".into());
            }
            let args: Vec<String> = svc["args"]
                .as_array()
                .map(|a| a.iter().filter_map(V::as_str).map(str::to_owned).collect())
                .unwrap_or_default();
            let pid = launch(
                Path::new(&exe),
                &args,
                svc["cwd"].as_str(),
                Some(&self.data.join("logs").join(format!("{id}.log"))),
            )?;
            self.state.lock().unwrap().children.insert(id, pid);
        } else {
            let owned = self.state.lock().unwrap().children.remove(&id);
            if owned.is_some() || listening(n(svc, "port") as u16) {
                let expected = s(svc, "match").replace('\'', "''");
                let selection = if let Some(pid) = owned {
                    format!("@({pid})")
                } else {
                    format!(
                        "@(Get-NetTCPConnection -State Listen -LocalPort {} -ErrorAction SilentlyContinue|Select-Object -ExpandProperty OwningProcess -Unique)",
                        n(svc, "port")
                    )
                };
                let script = format!(
                    "$expected='{expected}';$pids={selection};$stopped=0;foreach($procId in $pids){{$p=Get-CimInstance Win32_Process -Filter ('ProcessId = '+$procId);if($p -and $p.CommandLine -and $p.CommandLine.Contains($expected)){{Stop-Process -Id $procId -ErrorAction Stop;$stopped++}}}};Write-Output $stopped"
                );
                let bytes = script
                    .encode_utf16()
                    .flat_map(u16::to_le_bytes)
                    .collect::<Vec<_>>();
                let out = command(
                    "powershell.exe",
                    &[
                        "-NoProfile".into(),
                        "-NonInteractive".into(),
                        "-EncodedCommand".into(),
                        B64.encode(bytes),
                    ],
                )?;
                if out.parse::<usize>().unwrap_or(0) < 1 {
                    return Err("该端口进程无法确认属于本服务，已拒绝停止".into());
                }
            }
        }
        self.snapshot.lock().unwrap().0 = Instant::now() - Wait::from_secs(60);
        self.log(
            if action == "start" {
                "服务启动"
            } else {
                "服务停止"
            },
            &s(svc, "name"),
        );
        Ok(())
    }
}
