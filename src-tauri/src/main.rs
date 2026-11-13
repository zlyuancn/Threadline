//! Threadline Agent：托盘 + 面板外壳。
//!
//! 归属：**Agent 壳**（CAW 里的"助手"）。它**没有业务状态**：
//! - 不持锁、不监听端口、不持久化服务状态；
//! - 一切管理动作都是调用核心的控制 API；
//! - 核心不可达时只把界面变成灰态，绝不崩溃、不弹错误框。
//!
//! 它做的三件事：把发现契约注入 WebView、提供托盘、保证每会话只有一个实例。

// 无条件使用 GUI 子系统：助手是托盘 + 窗口程序，任何构建形态都不该出现控制台黑窗。
// （原先写成 `cfg_attr(not(debug_assertions), ...)`，导致 debug 构建必带一个黑窗，
//   而 debug 构建恰恰是日常双击运行的那个。）
#![windows_subsystem = "windows"]

use std::path::PathBuf;
use std::time::Duration;

use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};

mod icon_import;
mod wininet;
use tauri::tray::{MouseButton, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, Manager};

const CONTROL_FILE: &str = "control.json";
const MAIN_WINDOW: &str = "main";

/// 数据目录：`--data-dir <绝对路径>` 优先（便于测试与绿色部署），否则 `%PUBLIC%\Threadline`。
///
/// 选 `%PUBLIC%` 而不是 `%ProgramData%`：前者给的是可继承的 `INTERACTIVE: Modify`，
/// 登录用户与 SYSTEM（登录前自启的核心）都能写同一个目录，不需要 icacls / UAC。
fn data_dir() -> PathBuf {
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        if arg == "--data-dir" {
            if let Some(value) = args.next() {
                let path = PathBuf::from(value);
                if path.is_absolute() {
                    return path;
                }
            }
        }
    }
    match std::env::var("PUBLIC") {
        Ok(dir) if !dir.is_empty() => PathBuf::from(dir).join("Threadline"),
        _ => PathBuf::from(r"C:\Users\Public\Threadline"),
    }
}

/// 从 `control.json` 读出发现契约（端口 + token）。
/// 解析失败一律视为"核心不存在"，不报错中断——灰态由前端展示。
fn read_bootstrap() -> Option<(u16, String)> {
    let text = std::fs::read_to_string(data_dir().join(CONTROL_FILE)).ok()?;
    let parsed: serde_json::Value = serde_json::from_str(&text).ok()?;
    if parsed.get("status")?.as_str()? != "running" {
        return None;
    }
    let port = parsed.get("port")?.as_u64()?;
    if !(1..=65535).contains(&port) {
        return None;
    }
    // `control.json` 落在共享目录，内容可能被其它本机主体改写。这里只接受
    // 核心自己会写出的形状（32 字节 CSPRNG 的 Base64URL），别的一律当"核心不存在"。
    let token = parsed.get("token")?.as_str()?;
    if !is_plausible_token(token) {
        return None;
    }
    Some((port as u16, token.to_string()))
}

/// token 形状校验：Base64URL 字符集、长度 16–128。
///
/// 这不是完整性校验，而是**注入防线**：既保证拼进页面的值不含引号/换行/控制字符，
/// 也让被篡改的控制文件无法把任意脚本送进 WebView。
fn is_plausible_token(token: &str) -> bool {
    (16..=128).contains(&token.len())
        && token
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// 把端口与 token 注入页面（前端 `src/api/client.ts` 从这里取）。
fn inject(auth: &AppHandle) -> bool {
    let Some(window) = auth.get_webview_window(MAIN_WINDOW) else {
        return false;
    };
    match read_bootstrap() {
        Some((port, token)) => {
            // 用 JSON 序列化而不是字符串拼接：斜杠/引号/反斜杠都由序列化器处理，
            // 不给"控制文件被改写 → 注入任意 JS"留口子。
            let bootstrap = serde_json::json!({
                "baseUrl": format!("http://127.0.0.1:{port}"),
                "token": token,
            });
            let script = format!("window.__THREADLINE__ = {bootstrap};");
            window.eval(&script).is_ok()
        }
        None => {
            let _ = window.eval("window.__THREADLINE__ = null;");
            false
        }
    }
}

/// 把页面上的发现契约置空（核心已退出时用，避免留着失效 token）。
fn clear_bootstrap(app: &AppHandle) -> bool {
    match app.get_webview_window(MAIN_WINDOW) {
        Some(window) => window.eval("window.__THREADLINE__ = null;").is_ok(),
        None => false,
    }
}

/// `control.json` 里记录的端口（**不看 status**）。
///
/// 托盘判活不能只看这个文件：只有优雅退出才会把 status 改成 stopped，
/// 崩溃或被杀掉的核心会让文件一直停在 running。
fn control_port() -> Option<u16> {
    let text = std::fs::read_to_string(data_dir().join(CONTROL_FILE)).ok()?;
    parse_control_port(&text)
}

fn parse_control_port(text: &str) -> Option<u16> {
    let value: serde_json::Value = serde_json::from_str(text).ok()?;
    value.get("port")?.as_u64().map(|port| port as u16)
}

/// 核心状态变化事件名（payload 为 `bool`）。
const CORE_STATE_EVENT: &str = "core-state";

/// 助手的最小诊断日志：`%PUBLIC%\Threadline\logs\agent.log`。
///
/// 托盘动作没有界面回执，出问题时只能靠它回答"点了没反应到底卡在哪一步"。
fn log_line(message: &str) {
    use std::io::Write;

    let dir = data_dir().join("logs");
    if std::fs::create_dir_all(&dir).is_err() {
        return;
    }
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("agent.log"))
    {
        let _ = writeln!(file, "[{secs}] {message}");
    }
}

/// 面板启动时先取一次当前状态，避免依赖事件时序。
/// 用系统默认浏览器打开一个 http(s) 地址。
///
/// 走 rundll32 的 FileProtocolHandler 而不是 `cmd /c start`：后者要经过一层命令行
/// 解析，URL 里的 `&` 会被当成命令分隔符。这里先校验 scheme，非 http(s) 一律拒绝。
#[tauri::command]
fn open_external(url: String) -> Result<(), String> {
    let trimmed = url.trim();
    let lower = trimmed.to_ascii_lowercase();
    if !(lower.starts_with("http://") || lower.starts_with("https://")) {
        return Err("只允许打开 http/https 地址".to_string());
    }
    std::process::Command::new("rundll32.exe")
        .args(["url.dll,FileProtocolHandler", trimmed])
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("打不开浏览器：{e}"))
}

#[tauri::command]
fn core_state() -> bool {
    core_alive()
}

/// 发现契约快照（端口 + token）。
///
/// 页面刷新（F5）会把 `eval` 注入的 `window.__THREADLINE__` 抹掉，
/// 所以前端必须在每次加载时**主动拉一次**，而不是依赖一次性的推送。
#[derive(serde::Serialize)]
struct BootstrapSnapshot {
    #[serde(rename = "baseUrl")]
    base_url: String,
    token: String,
}

#[tauri::command]
fn bootstrap_snapshot() -> Option<BootstrapSnapshot> {
    read_bootstrap().map(|(port, token)| BootstrapSnapshot {
        base_url: format!("http://127.0.0.1:{port}"),
        token,
    })
}

/// 核心是否真的在服务：直接探一次 `/healthz`。
///
/// 与面板用同一个判据（HTTP 可达），不再是"文件存在就算在跑"。
fn core_alive() -> bool {
    use std::io::{Read, Write};

    let Some(port) = control_port() else {
        return false;
    };
    let addr = std::net::SocketAddr::from(([127, 0, 0, 1], port));
    let Ok(mut stream) = std::net::TcpStream::connect_timeout(&addr, Duration::from_millis(400))
    else {
        return false;
    };
    let _ = stream.set_read_timeout(Some(Duration::from_millis(600)));
    let _ = stream.set_write_timeout(Some(Duration::from_millis(600)));

    let request = format!(
        "GET /api/v1/healthz HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n"
    );
    if stream.write_all(request.as_bytes()).is_err() {
        return false;
    }

    let mut buf = [0u8; 32];
    match stream.read(&mut buf) {
        Ok(n) if n > 0 => String::from_utf8_lossy(&buf[..n]).contains(" 200 "),
        _ => false,
    }
}

fn show_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window(MAIN_WINDOW) {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
        // 托盘、热键、再次双击启动都走这里。每次唤起都回默认页：
        // 用户「打开」应用时期望看到的是应用列表，而不是上次停留的设置页。
        let _ = app.emit("window:shown", ());
    }
}

/// 托盘菜单里的「停止服务」：直接调核心的 `POST /shutdown`。
/// 这是 CAW C4 的体现——低权限助手通过 API 让（可能的）提权核心优雅退出。
fn request_shutdown() -> Result<(), String> {
    let (port, token) =
        read_bootstrap().ok_or_else(|| "没有可用的发现契约（核心未运行）".to_string())?;
    send_shutdown(port, &token)
}

/// 真正发字节的部分（抽出来便于用本地服务器测）。
fn send_shutdown(port: u16, token: &str) -> Result<(), String> {
    use std::io::{Read, Write};

    let addr = std::net::SocketAddr::from(([127, 0, 0, 1], port));
    let mut stream = std::net::TcpStream::connect_timeout(&addr, Duration::from_millis(800))
        .map_err(|e| format!("连接核心 127.0.0.1:{port} 失败：{e}"))?;
    let _ = stream.set_read_timeout(Some(Duration::from_millis(1500)));
    let _ = stream.set_write_timeout(Some(Duration::from_millis(800)));

    let request = format!(
        "POST /api/v1/shutdown HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nAuthorization: Bearer {token}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
    );
    stream
        .write_all(request.as_bytes())
        .map_err(|e| format!("发送请求失败：{e}"))?;
    stream.flush().map_err(|e| format!("刷新连接失败：{e}"))?;

    // **必须读完响应再关闭**。写完立刻 drop，Windows 会补一个 RST，
    // 服务端可能连请求都还没读到——表现就是"点了停止没反应"。
    let mut buf = [0u8; 128];
    let read = stream
        .read(&mut buf)
        .map_err(|e| format!("等待核心响应失败：{e}"))?;
    let head = String::from_utf8_lossy(&buf[..read]);
    let status = head.lines().next().unwrap_or_default().to_string();

    if status.contains(" 200 ") || status.contains(" 202 ") {
        Ok(())
    } else {
        Err(format!("核心拒绝了停止请求：{status}"))
    }
}

/// 确保核心在跑（前端「启动服务」按钮，以及助手自身启动时都会走这里）。
///
/// 原先这一步由一个独立的 `tl.exe` 完成；现在直接调 `tl_wake::ensure_core`——
/// 用户双击的本来就是助手，没必要再经一个只活几百毫秒的中转进程。
#[tauri::command]
fn start_core() -> Result<(), String> {
    let dir = data_dir();
    tl_wake::ensure_core(&dir)
}

/// 核心每次启动都会轮换 token，拉起新核心后必须重新注入，否则前端拿着旧 token 会 401。
#[tauri::command]
fn refresh_bootstrap(app: AppHandle) -> bool {
    inject(&app)
}

/// 一键导入本机私钥：只在用户点按钮时读一次，不在运行时引用路径。
///
/// 这与"运行时去 ~/.ssh 取路径"是两件事：后者在 SYSTEM 身份下读不到，
/// 前者只是把用户本来就要手动粘贴的内容帮他填上，随后仍由核心加密托管。
#[tauri::command]
fn read_local_private_key(name: Option<String>) -> Result<LocalPrivateKey, String> {
    let ssh_dir = ssh_dir()?;
    let files = private_key_files(&ssh_dir);

    let path = match name.filter(|n| !n.trim().is_empty()) {
        Some(wanted) => {
            // 只允许 ~/.ssh 下的文件名，挡掉 `..\..\` 之类的路径穿越。
            if wanted.contains(['/', '\\', ':']) {
                return Err(format!("不是合法的私钥文件名：{wanted}"));
            }
            let path = ssh_dir.join(&wanted);
            if !files.iter().any(|p| p == &path) {
                return Err(format!("不是可用的私钥文件：{wanted}"));
            }
            path
        }
        None => files
            .first()
            .cloned()
            .ok_or_else(|| format!("在 {} 下没找到私钥，请手动粘贴内容", ssh_dir.display()))?,
    };

    let pem =
        std::fs::read_to_string(&path).map_err(|e| format!("读取 {} 失败：{e}", path.display()))?;
    Ok(LocalPrivateKey {
        path: path.display().to_string(),
        pem,
    })
}

/// 列出 `~/.ssh` 下可用的私钥文件，界面有多把时让用户选。
#[tauri::command]
fn list_local_private_keys() -> Result<Vec<LocalKeyFile>, String> {
    let ssh_dir = ssh_dir()?;
    Ok(private_key_files(&ssh_dir)
        .into_iter()
        .map(|path| LocalKeyFile {
            name: path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or_default()
                .to_string(),
            path: path.display().to_string(),
        })
        .collect())
}

fn ssh_dir() -> Result<std::path::PathBuf, String> {
    let home = std::env::var("USERPROFILE").map_err(|_| "取不到用户目录".to_string())?;
    Ok(std::path::PathBuf::from(home).join(".ssh"))
}

/// `~/.ssh` 下看起来是私钥的文件（排除 `.pub` / `known_hosts` / `config`）。
fn private_key_files(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut files: Vec<std::path::PathBuf> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            if !path.is_file() {
                return false;
            }
            let name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or_default();
            if name.ends_with(".pub")
                || name == "known_hosts"
                || name == "known_hosts.old"
                || name == "config"
                || name == "authorized_keys"
            {
                return false;
            }
            // 内容里含 "PRIVATE KEY" 的才算（顺带排除 socket 之类的非文件内容）。
            std::fs::read_to_string(path)
                .map(|text| text.contains("PRIVATE KEY"))
                .unwrap_or(false)
        })
        .collect();
    files.sort();
    files
}

#[derive(serde::Serialize)]
struct LocalKeyFile {
    name: String,
    path: String,
}

#[derive(serde::Serialize)]
struct LocalPrivateKey {
    path: String,
    pem: String,
}

/// 本机 `~/.ssh/known_hosts` 里对某台主机的记录。
///
/// 这是用户真正能用的参照物：他平时用终端 SSH 过这台机器，信任决策已经在那里做过了。
#[derive(serde::Serialize)]
struct KnownHostLookup {
    /// 找到了可直接比对的记录。
    found: bool,
    /// 记录是 `|1|` 哈希格式，无法反查（OpenSSH 的 HashKnownHosts）。
    hashed: bool,
    /// `SHA256:...`，与核心观测到的指纹同一格式。
    fingerprint: Option<String>,
}

/// 读本机 known_hosts，算出该主机的指纹供界面比对。
#[tauri::command]
fn known_hosts_fingerprint(host: String, port: u16) -> Result<KnownHostLookup, String> {
    use base64::Engine;
    use sha2::{Digest, Sha256};

    let home = std::env::var("USERPROFILE").map_err(|_| "取不到用户目录".to_string())?;
    let path = std::path::Path::new(&home).join(".ssh").join("known_hosts");
    let Ok(text) = std::fs::read_to_string(&path) else {
        return Ok(KnownHostLookup {
            found: false,
            hashed: false,
            fingerprint: None,
        });
    };

    // OpenSSH 的非默认端口写成 [host]:port。
    let target = if port == 22 {
        host.clone()
    } else {
        format!("[{host}]:{port}")
    };

    let mut saw_hashed = false;
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut parts = line.split_whitespace();
        let (Some(hosts), Some(_key_type), Some(key_b64)) =
            (parts.next(), parts.next(), parts.next())
        else {
            continue;
        };

        if hosts.starts_with("|1|") {
            // 哈希条目无法反查主机名；只能告诉界面"有记录但不可比对"。
            saw_hashed = true;
            continue;
        }
        if !hosts.split(',').any(|h| h == target || h == host) {
            continue;
        }

        let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(key_b64) else {
            continue;
        };
        let digest = Sha256::digest(&bytes);
        let encoded = base64::engine::general_purpose::STANDARD_NO_PAD.encode(digest);
        return Ok(KnownHostLookup {
            found: true,
            hashed: false,
            fingerprint: Some(format!("SHA256:{encoded}")),
        });
    }

    Ok(KnownHostLookup {
        found: false,
        hashed: saw_hashed,
        fingerprint: None,
    })
}

/// 改开机自启：核心以用户身份跑，改 per-machine 任务计划必然 409 `privilege_required`。
/// 这里代它弹**一次** UAC，跑 `tl-core --autostart on|off`（一次性进程，不抢锁、不绑端口），
/// 再等它把结果写进 `autostart.result.json`。
#[derive(serde::Serialize)]
struct DataDirStatus {
    path: String,
    writable: bool,
}

/// 数据目录能不能写。
///
/// 自定义 `--data-dir`（例如指回 ProgramData）时，交互用户可能只有读权限；
/// 界面需要在"改配置注定失败"之前就告诉用户，而不是等他点保存才报错。
#[tauri::command]
fn data_dir_status() -> DataDirStatus {
    let dir = data_dir();
    let probe = dir.join(".write-probe");
    let writable = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(&probe)
        .is_ok();
    let _ = std::fs::remove_file(&probe);
    DataDirStatus {
        path: dir.display().to_string(),
        writable,
    }
}

/// 修数据目录权限：弹一次 UAC，由核心的一次性模式跑 icacls。
#[tauri::command]
async fn fix_data_dir_acl() -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(elevate_fix_acl)
        .await
        .map_err(|e| format!("提权任务异常：{e}"))?
}

fn elevate_fix_acl() -> Result<String, String> {
    let core = sibling_core().ok_or_else(|| "找不到 tl-core.exe，无法修复权限".to_string())?;
    let dir = data_dir();
    let result_path = dir.join("acl.result.json");
    // 先删旧结果，否则会读到上一次的"成功"。
    let _ = std::fs::remove_file(&result_path);

    shell_execute_run_as(
        &core,
        &format!("--fix-acl --data-dir \"{}\"", dir.display()),
    )?;

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(120);
    while std::time::Instant::now() < deadline {
        if let Ok(text) = std::fs::read_to_string(&result_path) {
            let value: serde_json::Value =
                serde_json::from_str(&text).map_err(|e| format!("结果文件无法解析：{e}"))?;
            let message = value
                .get("message")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let ok = value.get("ok").and_then(|v| v.as_bool()).unwrap_or(false);
            return if ok { Ok(message) } else { Err(message) };
        }
        std::thread::sleep(std::time::Duration::from_millis(300));
    }
    Err("等待授权超时（可能取消了 UAC）".to_string())
}

/// 登录启动项：写 `HKCU\...\Run`，登录后把助手（托盘）拉起来。
///
/// 为什么必须由助手自己写：`HKCU` 是**每用户**的。开机自启的核心以 SYSTEM 运行，
/// 它去写 HKCU 会写进 SYSTEM 自己的 hive（见 CAW AP9/AP10）——表面成功、实际不生效。
/// 只有跑在用户会话里的助手，才有资格改用户级的启动项。
#[cfg(target_os = "windows")]
mod logon_entry {
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;

    use windows_sys::Win32::System::Registry::{
        RegCloseKey, RegDeleteValueW, RegOpenKeyExW, RegQueryValueExW, RegSetValueExW, HKEY,
        HKEY_CURRENT_USER, KEY_QUERY_VALUE, KEY_SET_VALUE, REG_SZ,
    };

    const SUBKEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
    pub const VALUE: &str = "Threadline";
    /// ERROR_FILE_NOT_FOUND
    const NOT_FOUND: u32 = 2;

    fn wide(text: &str) -> Vec<u16> {
        OsStr::new(text)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect()
    }

    fn open(write: bool) -> Option<HKEY> {
        let mut key: HKEY = std::ptr::null_mut();
        let access = if write {
            KEY_SET_VALUE | KEY_QUERY_VALUE
        } else {
            KEY_QUERY_VALUE
        };
        let rc = unsafe {
            RegOpenKeyExW(
                HKEY_CURRENT_USER,
                wide(SUBKEY).as_ptr(),
                0,
                access,
                &mut key,
            )
        };
        (rc == 0).then_some(key)
    }

    pub fn read(name: &str) -> Option<String> {
        let key = open(false)?;
        let mut kind: u32 = 0;
        let mut size: u32 = 0;
        let rc = unsafe {
            RegQueryValueExW(
                key,
                wide(name).as_ptr(),
                std::ptr::null_mut(),
                &mut kind,
                std::ptr::null_mut(),
                &mut size,
            )
        };
        if rc != 0 || size == 0 {
            unsafe { RegCloseKey(key) };
            return None;
        }

        let mut buffer = vec![0u8; size as usize];
        let rc = unsafe {
            RegQueryValueExW(
                key,
                wide(name).as_ptr(),
                std::ptr::null_mut(),
                &mut kind,
                buffer.as_mut_ptr(),
                &mut size,
            )
        };
        unsafe { RegCloseKey(key) };
        if rc != 0 {
            return None;
        }

        // 同 wininet.rs：as_chunks 要 Rust 1.88，MSRV 是 1.80。
        #[allow(clippy::chunks_exact_to_as_chunks)]
        let units: Vec<u16> = buffer
            .chunks_exact(2)
            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
            .collect();
        Some(
            String::from_utf16_lossy(&units)
                .trim_end_matches('\0')
                .to_string(),
        )
    }

    pub fn write(name: &str, value: &str) -> Result<(), String> {
        let key = open(true).ok_or_else(|| "打不开 HKCU 的 Run 键".to_string())?;
        let data = wide(value);
        let bytes =
            unsafe { std::slice::from_raw_parts(data.as_ptr() as *const u8, data.len() * 2) };
        let rc = unsafe {
            RegSetValueExW(
                key,
                wide(name).as_ptr(),
                0,
                REG_SZ,
                bytes.as_ptr(),
                bytes.len() as u32,
            )
        };
        unsafe { RegCloseKey(key) };
        if rc == 0 {
            Ok(())
        } else {
            Err(format!("写登录启动项失败（错误码 {rc}）"))
        }
    }

    pub fn delete(name: &str) -> Result<(), String> {
        let Some(key) = open(true) else {
            return Ok(());
        };
        let rc = unsafe { RegDeleteValueW(key, wide(name).as_ptr()) };
        unsafe { RegCloseKey(key) };
        if rc == 0 || rc == NOT_FOUND {
            Ok(())
        } else {
            Err(format!("删登录启动项失败（错误码 {rc}）"))
        }
    }
}

/// 按"是否开机自启"同步登录启动项。
///
/// 幂等：每次助手启动都调一次，所以用户手动删掉注册表项后，下次启动会自愈。
#[tauri::command]
fn sync_logon_entry(enabled: bool) -> Result<String, String> {
    let exe = std::env::current_exe().map_err(|e| format!("取不到自身路径：{e}"))?;
    let quoted = logon_command(&exe);

    if !enabled {
        logon_entry::delete(logon_entry::VALUE)?;
        return Ok("已移除登录启动项".to_string());
    }
    if logon_entry::read(logon_entry::VALUE).as_deref() == Some(quoted.as_str()) {
        return Ok("登录启动项已就绪".to_string());
    }
    logon_entry::write(logon_entry::VALUE, &quoted)?;
    Ok("已添加登录启动项".to_string())
}

#[tauri::command]
async fn set_autostart_elevated(enabled: bool) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || elevate_autostart(enabled))
        .await
        .map_err(|e| format!("提权任务异常：{e}"))?
}

fn elevate_autostart(enabled: bool) -> Result<(), String> {
    let core = sibling_core().ok_or_else(|| "找不到 tl-core.exe，无法修改开机自启".to_string())?;
    let dir = data_dir();
    let result_path = dir.join("autostart.result.json");

    // 先删旧结果：否则会读到上一次的"成功"。
    let _ = std::fs::remove_file(&result_path);

    let params = format!(
        "--autostart {} --data-dir \"{}\"",
        if enabled { "on" } else { "off" },
        dir.display()
    );
    shell_execute_run_as(&core, &params)?;

    // UAC 弹窗可能要等用户几十秒；一次性进程写完结果文件即结束。
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(120);
    while std::time::Instant::now() < deadline {
        if let Ok(text) = std::fs::read_to_string(&result_path) {
            let value: serde_json::Value =
                serde_json::from_str(&text).map_err(|e| format!("结果文件无法解析：{e}"))?;
            if value.get("ok").and_then(|v| v.as_bool()).unwrap_or(false) {
                return Ok(());
            }
            return Err(value
                .get("message")
                .and_then(|v| v.as_str())
                .unwrap_or("提权执行失败")
                .to_string());
        }
        std::thread::sleep(std::time::Duration::from_millis(300));
    }
    Err("等待提权结果超时（可能取消了授权）".to_string())
}

/// 域名写 hosts 的提权路径：Core 的 `POST /hosts/sync` 返回 409 后由 Agent 拉起
/// `tl-core hosts sync --data-dir <dir>`（一次性进程，不抢锁、不绑端口），
/// 再等它把结果写进 `hosts.result.json`。
#[tauri::command]
async fn sync_hosts() -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(elevate_hosts_sync)
        .await
        .map_err(|e| format!("提权任务异常：{e}"))?
}

fn elevate_hosts_sync() -> Result<(), String> {
    let core = sibling_core().ok_or_else(|| "找不到 tl-core.exe，无法写入 hosts".to_string())?;
    let dir = data_dir();
    let result_path = dir.join("hosts.result.json");

    // 先删旧结果：否则会读到上一次的"成功"。
    let _ = std::fs::remove_file(&result_path);

    shell_execute_run_as(
        &core,
        &format!("hosts sync --data-dir \"{}\"", dir.display()),
    )?;

    // 参数被拒（非默认数据目录）时子进程直接退出、不写结果，所以超时不能像 autostart 那样放到 120s。
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    while std::time::Instant::now() < deadline {
        if let Ok(text) = std::fs::read_to_string(&result_path) {
            let value: serde_json::Value =
                serde_json::from_str(&text).map_err(|e| format!("结果文件无法解析：{e}"))?;
            if value.get("ok").and_then(|v| v.as_bool()).unwrap_or(false) {
                return Ok(());
            }
            return Err(value
                .get("message")
                .and_then(|v| v.as_str())
                .unwrap_or("写入 hosts 失败")
                .to_string());
        }
        std::thread::sleep(std::time::Duration::from_millis(300));
    }
    Err("等待提权结果超时（可能取消了授权，或数据目录不是默认位置）".to_string())
}

#[derive(serde::Serialize)]
struct IconPayload {
    mime: String,
    base64: String,
}

/// 下载远程图标并把原始字节交给前端；缩放/转 data URI 在前端做。
///
/// 不走 WebView 的 fetch：跨域图片会把 canvas 污染，`toDataURL` 直接抛错。
#[tauri::command]
async fn import_icon(url: String) -> Result<IconPayload, String> {
    tauri::async_runtime::spawn_blocking(move || icon_import::download(&url))
        .await
        .map_err(|e| format!("下载任务异常：{e}"))?
        .map(|icon| IconPayload {
            mime: icon.mime,
            base64: icon.base64,
        })
}

/// 代理例外：Agent 以登录用户身份读写自己的 HKCU（Core 是 SYSTEM 时写的是 S-1-5-18，无效）。
///
/// `desired` / `managed` 由前端从 `GET /hosts` 的 `proxy` 字段读出后传入；
/// 返回值交给前端转成 `ProxySyncResult` 回报 Core —— Agent 无状态，不直接调 Core。
/// 让 Windows 标题栏跟着应用主题走。
///
/// `setTheme` 只能二选一（深/浅）；自定义配色要靠 DWM 的
/// `DWMWA_CAPTION_COLOR` / `DWMWA_TEXT_COLOR`（Windows 11 起支持）。
/// Windows 10 会返回错误，忽略即可：那种系统上标题栏只能跟随深浅。
#[tauri::command]
fn set_window_caption(
    window: tauri::WebviewWindow,
    caption: String,
    text: String,
) -> Result<(), String> {
    #[cfg(windows)]
    {
        let parse = |value: &str| -> Result<u32, String> {
            let hex = value.trim().trim_start_matches('#');
            if hex.len() != 6 {
                return Err(format!("颜色格式应为 #rrggbb：{value}"));
            }
            let r = u8::from_str_radix(&hex[0..2], 16).map_err(|e| e.to_string())? as u32;
            let g = u8::from_str_radix(&hex[2..4], 16).map_err(|e| e.to_string())? as u32;
            let b = u8::from_str_radix(&hex[4..6], 16).map_err(|e| e.to_string())? as u32;
            // DWM 要的是 0x00BBGGRR，不是 RGB。
            Ok((b << 16) | (g << 8) | r)
        };
        let caption = parse(&caption)?;
        let text = parse(&text)?;

        // Tauri 交出来的是 `windows` crate 的 HWND（newtype），
        // windows-sys 的 DwmSetWindowAttribute 要裸指针，这里拆一次。
        let hwnd: *mut core::ffi::c_void =
            window.hwnd().map_err(|e| format!("取窗口句柄失败：{e}"))?.0;
        unsafe {
            use windows_sys::Win32::Graphics::Dwm::{
                DwmSetWindowAttribute, DWMWA_CAPTION_COLOR, DWMWA_TEXT_COLOR,
            };
            let size = std::mem::size_of::<u32>() as u32;
            // 失败通常是系统不支持（Windows 10），不必让界面报错。
            let _ = DwmSetWindowAttribute(
                hwnd,
                DWMWA_CAPTION_COLOR as u32,
                &caption as *const u32 as *const core::ffi::c_void,
                size,
            );
            let _ = DwmSetWindowAttribute(
                hwnd,
                DWMWA_TEXT_COLOR as u32,
                &text as *const u32 as *const core::ffi::c_void,
                size,
            );
        }
        Ok(())
    }
    #[cfg(not(windows))]
    {
        let _ = (window, caption, text);
        Ok(())
    }
}

#[tauri::command]
fn sync_proxy_override(desired: Vec<String>, managed: Vec<String>) -> wininet::ProxyReport {
    // 这个命令没有界面回执，排查「自动同步到底有没有被调用」只能靠日志。
    log_line(&format!(
        "sync_proxy_override called desired={} managed={}",
        desired.len(),
        managed.len()
    ));
    let report = wininet::sync(&desired, &managed);
    log_line(&format!(
        "sync_proxy_override done ok={} enabled={:?} pac={:?} msg={:?}",
        report.ok, report.enabled, report.pac, report.message
    ));
    report
}

/// 把本地根证书装进**当前用户**的根存储。
///
/// `certutil -user` 免管理员，但会弹**系统安全警告**——调用方必须先弹自己的确认；
/// 用户点「否」时 certutil 返回非 0，这里如实回传，不重试。
#[tauri::command]
fn install_gateway_ca(thumbprint_sha1: Option<String>) -> Result<serde_json::Value, String> {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    let path = data_dir().join("gateway-ca").join("root.pem");
    if !path.exists() {
        return Err("根证书还没生成（Core 未启动过）".into());
    }
    let output = std::process::Command::new("certutil")
        .args(["-user", "-addstore", "Root"])
        .arg(&path)
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .map_err(|e| format!("启动 certutil 失败：{e}"))?;

    let added = output.status.success();
    // 退出码 0 不代表一定进库；有指纹就以「查库结果」为准，避免界面显示已装实际没装。
    let in_store = match thumbprint_sha1.as_deref().filter(|t| !t.is_empty()) {
        Some(sha1) => certificate_in_root(sha1),
        None => added,
    };
    let ok = added && in_store;

    let message = if !added {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        Some(if stderr.is_empty() {
            format!(
                "certutil 退出码 {:?}（通常是取消了系统安全警告）",
                output.status.code()
            )
        } else {
            stderr
        })
    } else if !in_store {
        Some("certutil 返回成功，但当前用户根库中查不到该证书（可能被组策略拦截）".to_string())
    } else {
        None
    };
    Ok(serde_json::json!({
        "ok": ok,
        "code": output.status.code(),
        "message": message,
    }))
}

/// 查证书是否在**当前用户**根库里。
///
/// 必须传 **SHA-1**：Windows 证书存储默认按 SHA-1 匹配，传 SHA-256 会返回
/// `CRYPT_E_NOT_FOUND`（真机实测）。
fn certificate_in_root(thumbprint: &str) -> bool {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    std::process::Command::new("certutil")
        .args(["-user", "-store", "Root", thumbprint])
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false)
}

/// 查当前用户根库里有没有这张根。
///
/// 必须由 Agent 做：开机自启档的 Core 是 SYSTEM，查的是 `S-1-5-18` 的根库，
/// 自己判断会永远显示「未信任」。
#[tauri::command]
fn gateway_ca_trust(thumbprint_sha1: String) -> Result<bool, String> {
    Ok(certificate_in_root(&thumbprint_sha1))
}

/// 导出根证书 PEM：无可见桌面会话时前端退化成「下载」，用户自己导入。
#[tauri::command]
fn export_gateway_ca() -> Result<String, String> {
    std::fs::read_to_string(data_dir().join("gateway-ca").join("root.pem"))
        .map_err(|e| format!("读取根证书失败：{e}"))
}

/// 读回来的配置包。
///
/// `data` 用 base64 而不是 `Vec<u8>`：Tauri 的 IPC 是 JSON，字节数组会膨胀约 4 倍，
/// 一个几百 KB 的包能涨到几 MB 的字符串。
#[derive(serde::Serialize)]
struct ProfilePackage {
    name: String,
    data: String,
}

/// 导出配置包：弹**系统保存对话框**，返回用户最终选的路径（取消为 `None`）。
///
/// 不能用 WebView 的 `<a download>`：那条路在 Tauri 里是静默下载，文件落进默认下载
/// 目录，用户既看不到落点也没有选择余地——分享配置这件事必须让人知道存哪。
#[tauri::command]
async fn save_profile_package(
    bytes_base64: String,
    default_name: String,
) -> Result<Option<String>, String> {
    use base64::Engine;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(bytes_base64.as_bytes())
        .map_err(|e| format!("数据损坏：{e}"))?;

    tauri::async_runtime::spawn_blocking(move || {
        let picked = rfd::FileDialog::new()
            .set_title("导出配置")
            .set_file_name(&default_name)
            .add_filter("Threadline 配置", &["threadline"])
            .save_file();
        let Some(path) = picked else {
            return Ok::<Option<String>, String>(None);
        };
        std::fs::write(&path, &bytes).map_err(|e| format!("写入失败：{e}"))?;
        Ok(Some(path.to_string_lossy().to_string()))
    })
    .await
    .map_err(|e| format!("保存对话框失败：{e}"))?
}

/// 导入配置包：弹**系统打开对话框**，读回文件名与内容（取消为 `None`）。
#[tauri::command]
async fn open_profile_package() -> Result<Option<ProfilePackage>, String> {
    use base64::Engine;
    tauri::async_runtime::spawn_blocking(move || {
        let picked = rfd::FileDialog::new()
            .set_title("导入配置")
            .add_filter("Threadline 配置", &["threadline"])
            .pick_file();
        let Some(path) = picked else {
            return Ok::<Option<ProfilePackage>, String>(None);
        };
        let meta = std::fs::metadata(&path).map_err(|e| format!("读取失败：{e}"))?;
        // 与前端同一上限：比这更大的多半不是配置包
        if meta.len() > 10 * 1024 * 1024 {
            return Err("文件太大（超过 10MB），不是有效的配置包".to_string());
        }
        let bytes = std::fs::read(&path).map_err(|e| format!("读取失败：{e}"))?;
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "config.threadline".to_string());
        Ok(Some(ProfilePackage {
            name,
            data: base64::engine::general_purpose::STANDARD.encode(&bytes),
        }))
    })
    .await
    .map_err(|e| format!("打开对话框失败：{e}"))?
}

/* ───────────── 检查更新 / 主动升级 ───────────── */

const UPDATE_REPO: &str = "zlyuancn/Threadline";

/// 更新信息：由 GitHub Releases 的 `latest` 接口拼出来。
#[derive(serde::Serialize)]
struct UpdateInfo {
    current: String,
    latest: String,
    available: bool,
    /// Release 正文（Markdown 原文），界面按纯文本展示
    notes: String,
    /// `*_x64-setup.exe` 的下载地址
    url: String,
    size: u64,
    /// 当前进程是否处于「已安装」状态：开发态只能跳转下载页，不能一键装
    installable: bool,
}

/// 逐段比数字：`0.10.0` 必须大于 `0.9.9`，按字符串比会判反。
fn version_is_newer(candidate: &str, current: &str) -> bool {
    let parse = |text: &str| -> Vec<u64> {
        text.trim_start_matches(['v', 'V'])
            .split(['.', '-', '+'])
            .map(|part| part.parse::<u64>().unwrap_or(0))
            .collect()
    };
    let (a, b) = (parse(candidate), parse(current));
    for i in 0..a.len().max(b.len()) {
        let x = a.get(i).copied().unwrap_or(0);
        let y = b.get(i).copied().unwrap_or(0);
        if x != y {
            return x > y;
        }
    }
    false
}

/// 安装目录。判定依据是同目录有 NSIS 生成的 `uninstall.exe`——
/// 开发态 `target\debug` 没有，因此不会被误判成「可以就地升级」。
fn install_dir() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?.to_path_buf();
    if dir.join("uninstall.exe").exists() {
        Some(dir)
    } else {
        None
    }
}

/// 代理地址。环境变量优先（终端里跑时更明确），否则读 Windows 系统代理。
///
/// 这台机器的网络里 GitHub 只能经企业代理访问：直连时 DNS 直接解析失败
/// （`os error 11001`），而浏览器/PowerShell 因为自动读系统代理所以是通的。
fn proxy_url() -> Option<String> {
    for key in ["HTTPS_PROXY", "https_proxy", "ALL_PROXY", "all_proxy"] {
        if let Ok(value) = std::env::var(key) {
            let value = value.trim();
            if !value.is_empty() {
                return Some(if value.contains("://") {
                    value.to_string()
                } else {
                    format!("http://{value}")
                });
            }
        }
    }
    system_proxy()
}

/// 读 `HKCU\...\Internet Settings`：`ProxyEnable=1` 时取 `ProxyServer`。
/// 值可能是 `host:port`，也可能是 `http=h:p;https=h:p` 的分协议写法。
#[cfg(windows)]
fn system_proxy() -> Option<String> {
    let settings = winreg::RegKey::predef(winreg::enums::HKEY_CURRENT_USER)
        .open_subkey("Software\\Microsoft\\Windows\\CurrentVersion\\Internet Settings")
        .ok()?;
    let enabled: u32 = settings.get_value("ProxyEnable").unwrap_or(0);
    if enabled == 0 {
        return None;
    }
    let server: String = settings.get_value("ProxyServer").unwrap_or_default();
    if server.is_empty() {
        return None;
    }
    let host = if server.contains('=') {
        server
            .split(';')
            .find_map(|part| part.strip_prefix("https=").or_else(|| part.strip_prefix("http=")))
            .unwrap_or_default()
    } else {
        server.as_str()
    };
    if host.is_empty() {
        return None;
    }
    Some(format!("http://{host}"))
}

#[cfg(not(windows))]
fn system_proxy() -> Option<String> {
    None
}

/// 带代理与超时的 HTTP agent。`ureq` 不会自己读系统代理，必须显式传。
fn http_agent(overall: Duration) -> ureq::Agent {
    let mut builder = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(10))
        .timeout(overall);
    if let Some(url) = proxy_url() {
        if let Ok(proxy) = ureq::Proxy::new(&url) {
            builder = builder.proxy(proxy);
        }
    }
    builder.build()
}

/// 查 GitHub Releases 的最新版。失败时返回 `Err`，由前端决定是否弹提示。
///
/// 没有 token：公开仓的 `latest` 接口未认证也能用（每小时 60 次，
/// 启动时一次 + 手动点击，远够）。
#[tauri::command]
async fn check_update(app: AppHandle) -> Result<UpdateInfo, String> {
    let current = app.package_info().version.to_string();
    let installable = install_dir().is_some();
    let url = format!("https://api.github.com/repos/{UPDATE_REPO}/releases/latest");

    tauri::async_runtime::spawn_blocking(move || {
        let body: serde_json::Value = http_agent(Duration::from_secs(20))
            .get(&url)
            .set("User-Agent", "Threadline-Updater")
            .set("Accept", "application/vnd.github+json")
            .call()
            .map_err(|e| format!("请求失败：{e}"))?
            .into_json()
            .map_err(|e| format!("响应解析失败：{e}"))?;

        let latest = body
            .get("tag_name")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .trim_start_matches(['v', 'V'])
            .to_string();
        if latest.is_empty() {
            return Err("响应里没有版本号".to_string());
        }

        // 只认 NSIS 安装包：便携包解压即用，没法覆盖正在运行的文件
        let asset = body
            .get("assets")
            .and_then(|v| v.as_array())
            .and_then(|items| {
                items.iter().find(|item| {
                    item.get("name")
                        .and_then(|n| n.as_str())
                        .is_some_and(|n| n.ends_with("-setup.exe"))
                })
            });
        let url = asset
            .and_then(|a| a.get("browser_download_url"))
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string();
        let size = asset
            .and_then(|a| a.get("size"))
            .and_then(|v| v.as_u64())
            .unwrap_or(0);

        Ok(UpdateInfo {
            available: version_is_newer(&latest, &current),
            notes: body
                .get("body")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string(),
            current,
            latest,
            url,
            size,
            installable,
        })
    })
    .await
    .map_err(|e| format!("检查更新失败：{e}"))?
}

/// 下载安装包到 `%TEMP%`，边下边发 `update:progress`（已下载字节 / 总字节）。
#[tauri::command]
async fn download_update(app: AppHandle, url: String, version: String) -> Result<String, String> {
    let target = std::env::temp_dir().join(format!("Threadline-{version}-setup.exe"));

    tauri::async_runtime::spawn_blocking(move || {
        use std::io::{Read, Write};

        let resp = http_agent(Duration::from_secs(600))
            .get(&url)
            .set("User-Agent", "Threadline-Updater")
            .call()
            .map_err(|e| format!("下载失败：{e}"))?;
        let total: u64 = resp
            .header("Content-Length")
            .and_then(|v| v.parse().ok())
            .unwrap_or(0);

        let mut reader = resp.into_reader();
        let mut file = std::fs::File::create(&target).map_err(|e| format!("建文件失败：{e}"))?;
        let mut buf = vec![0u8; 64 * 1024];
        let mut written: u64 = 0;
        loop {
            let n = reader.read(&mut buf).map_err(|e| format!("读取失败：{e}"))?;
            if n == 0 {
                break;
            }
            file.write_all(&buf[..n])
                .map_err(|e| format!("写入失败：{e}"))?;
            written += n as u64;
            // 失败（窗口已关）不算错误，下载继续
            let _ = app.emit("update:progress", (written, total));
        }
        Ok::<String, String>(target.to_string_lossy().to_string())
    })
    .await
    .map_err(|e| format!("下载任务失败：{e}"))?
}

/// 启动安装器并退出：静默安装（`/S`），装完由 NSIS 的 POSTINSTALL 重新拉起应用。
///
/// `/D=` 必须是最后一个参数，且 NSIS 不认带引号的写法——用 `raw_arg` 绕开
/// `Command` 的自动加引号，否则路径带空格时会被解析成一堆碎片参数。
#[tauri::command]
async fn install_update(app: AppHandle, path: String) -> Result<(), String> {
    use std::os::windows::process::CommandExt;

    let dir = install_dir().ok_or("当前不是安装版，无法自动升级")?;
    let mut cmd = std::process::Command::new(&path);
    cmd.raw_arg("/S");
    cmd.raw_arg(format!("/D={}", dir.display()));
    cmd.spawn().map_err(|e| format!("启动安装器失败：{e}"))?;

    // 给安装器一点时间接管，再退出；hook 会结束残留的 tl-core
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(1500));
        app.exit(0);
    });
    Ok(())
}

/// 找到同目录下的 `tl-core.exe`（安装后两者同处一个目录）。
///
/// 开发态额外允许 `<repo>/target/debug/tl-core.exe` 兜底。这条分支**只在 debug
/// 构建里存在**：release 包也保留它，等于把"以管理员身份执行用户目录下任意
/// 同名文件"的入口写死进产品（`runas` 不校验签名）。
fn sibling_core() -> Option<std::path::PathBuf> {
    let dir = std::env::current_exe().ok()?.parent()?.to_path_buf();
    let sibling = dir.join("tl-core.exe");
    if sibling.exists() {
        return Some(sibling);
    }

    #[cfg(debug_assertions)]
    {
        if let Some(root) = dir.ancestors().nth(3) {
            let dev = root.join("target").join("debug").join("tl-core.exe");
            if dev.exists() {
                return Some(dev);
            }
        }
    }

    None
}

/// `ShellExecuteW(..., "runas", ...)`：弹一次 UAC 并以管理员身份运行。
#[cfg(windows)]
fn shell_execute_run_as(exe: &std::path::Path, params: &str) -> Result<(), String> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::UI::Shell::ShellExecuteW;

    fn wide(value: &std::ffi::OsStr) -> Vec<u16> {
        value.encode_wide().chain(std::iter::once(0)).collect()
    }

    let verb: Vec<u16> = "runas\0".encode_utf16().collect();
    let file = wide(exe.as_os_str());
    let params: Vec<u16> = params.encode_utf16().chain(std::iter::once(0)).collect();

    // SW_SHOWNORMAL = 1；HINSTANCE <= 32 表示失败（最常见是用户取消了 UAC）。
    let code = unsafe {
        ShellExecuteW(
            std::ptr::null_mut(),
            verb.as_ptr(),
            file.as_ptr(),
            params.as_ptr(),
            std::ptr::null(),
            1,
        )
    } as isize;

    if code <= 32 {
        return Err(format!("提权启动失败（可能取消了 UAC），代码 {code}"));
    }
    Ok(())
}

#[cfg(not(windows))]
fn shell_execute_run_as(_exe: &std::path::Path, _params: &str) -> Result<(), String> {
    Err("仅支持 Windows".to_string())
}

/// 托盘菜单的统一处理。
///
/// 只在 tray 级注册一次：实测 app 级与 tray 级**会同时触发**，
/// 两处都注册等于一次点击执行两遍（启动会拉起两个 Wake）。
fn handle_menu(app: &AppHandle, id: &str) {
    log_line(&format!("menu action: {id}"));
    match id {
        "open" => show_window(app),
        // 两个动作都是幂等的：启动 = 确保在跑，停止 = 确保停下。
        "start" => {
            let alive = core_alive();
            log_line(&format!("start: core_alive={alive}"));
            if alive {
                show_window(app);
            } else {
                match start_core() {
                    Ok(()) => log_line("start: launched wake"),
                    Err(e) => log_line(&format!("start: FAILED {e}")),
                }
            }
        }
        "stop" => match request_shutdown() {
            Ok(()) => log_line("stop: shutdown request sent"),
            Err(e) => log_line(&format!("stop: FAILED {e}")),
        },
        // 「退出助手」= 用户想关掉这个程序。核心是它拉起的后台服务，
        // 留着会变成一个没有托盘、没有界面的孤儿进程——先停核心再退出。
        "quit" => {
            match request_shutdown() {
                Ok(()) => log_line("quit: shutdown request sent"),
                Err(e) => log_line(&format!("quit: core not stopped ({e})")),
            }
            // 给核心一点时间落盘 stopped，避免退出太快把它变成"崩溃"。
            std::thread::sleep(Duration::from_millis(400));
            app.exit(0);
        }
        other => log_line(&format!("menu action: unknown id {other}")),
    }
}

fn build_tray(app: &AppHandle) -> tauri::Result<()> {
    let open = MenuItem::with_id(app, "open", "打开面板", true, None::<&str>)?;
    // 启停是两个独立动作，不做"一个按钮两种含义"——语义固定，当前状态决定哪个可点。
    let start = MenuItem::with_id(app, "start", "启动服务", true, None::<&str>)?;
    let stop = MenuItem::with_id(app, "stop", "停止服务", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "退出助手", true, None::<&str>)?;
    let separator = PredefinedMenuItem::separator(app)?;
    let menu = Menu::with_items(app, &[&open, &start, &stop, &separator, &quit])?;

    let mut builder = TrayIconBuilder::with_id("threadline")
        .tooltip("Threadline · 引线")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| {
            log_line("menu event (tray)");
            handle_menu(app, event.id.as_ref());
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                ..
            } = event
            {
                show_window(tray.app_handle());
            }
        });

    if let Some(icon) = app.default_window_icon() {
        builder = builder.icon(icon.clone());
    }
    builder.build(app)?;

    // 托盘与面板在同一个进程里，就**只能有一个状态源**：这个线程探活、
    // 更新托盘项、并在状态变化时通过事件通知 WebView。面板不再自己判"在不在跑"。
    let start_probe = start.clone();
    let stop_probe = stop.clone();
    let handle = app.clone();
    std::thread::spawn(move || {
        let mut last: Option<bool> = None;
        loop {
            let alive = core_alive();
            let _ = start_probe.set_enabled(!alive);
            let _ = stop_probe.set_enabled(alive);
            if last != Some(alive) {
                // 核心每次启动都会轮换 token：状态一变就必须重新注入，
                // 否则前端手里是旧 token，表现为"起来了但一直 401"。
                // 核心已死时 control.json 仍是 running（崩溃不改写），
                // 直接读它会注入一个已经失效的 token，所以显式清空。
                let injected = if alive {
                    inject(&handle)
                } else {
                    clear_bootstrap(&handle)
                };
                let _ = handle.emit(CORE_STATE_EVENT, alive);
                log_line(&format!(
                    "core alive -> {alive} (menu: start={}, stop={}, bootstrap_injected={injected})",
                    !alive,
                    alive
                ));
                last = Some(alive);
            }
            std::thread::sleep(Duration::from_secs(2));
        }
    });

    Ok(())
}

/// 页面加载后注入发现契约；核心晚一点起来也能补上（有界重试，绝不无限等）。
fn spawn_bootstrap_injection(app: AppHandle) {
    std::thread::spawn(move || {
        for _ in 0..20 {
            std::thread::sleep(Duration::from_millis(500));
            if inject(&app) {
                return;
            }
        }
    });
}

/// 全局快捷键：注册、改绑、冲突检测。
///
/// 不引入 tauri-plugin-global-shortcut：RegisterHotKey 直接用 user32，少一个依赖。
/// 注册与消息循环必须在**同一个线程**（hwnd 传 0 时热键归属调用线程），
/// 所以改绑不是"注销再注册"，而是往那个线程投一条消息，由它自己换。
#[cfg(target_os = "windows")]
mod hotkey {
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::mpsc::{self, Sender};
    use std::time::Duration;

    use tauri::Manager;

    const MOD_ALT: u32 = 0x0001;
    const MOD_CONTROL: u32 = 0x0002;
    const MOD_SHIFT: u32 = 0x0004;
    const MOD_WIN: u32 = 0x0008;
    const MOD_NOREPEAT: u32 = 0x4000;
    const WM_HOTKEY: u32 = 0x0312;
    const WM_APP_REBIND: u32 = 0x8000 + 1;
    const HOTKEY_ID: i32 = 1;
    /// ERROR_HOTKEY_ALREADY_REGISTERED
    const ERR_ALREADY_REGISTERED: u32 = 1409;

    pub const DEFAULT: &str = "Ctrl+Alt+T";

    pub struct Bindings {
        tx: Sender<Command>,
    }

    enum Command {
        Rebind(String, Sender<Result<String, String>>),
    }

    #[repr(C)]
    struct Point {
        x: i32,
        y: i32,
    }

    #[repr(C)]
    struct Msg {
        hwnd: isize,
        message: u32,
        wparam: usize,
        lparam: isize,
        time: u32,
        pt: Point,
        private: u32,
    }

    #[link(name = "user32")]
    unsafe extern "system" {
        fn RegisterHotKey(hwnd: isize, id: i32, modifiers: u32, vk: u32) -> i32;
        fn UnregisterHotKey(hwnd: isize, id: i32) -> i32;
        fn GetMessageW(msg: *mut Msg, hwnd: isize, min: u32, max: u32) -> i32;
        fn PostThreadMessageW(id: u32, msg: u32, wparam: usize, lparam: isize) -> i32;
        fn GetCurrentThreadId() -> u32;
    }

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetLastError() -> u32;
    }

    /// 注册与改绑都在这个线程里，PostThreadMessage 需要它的 id。
    static THREAD_ID: AtomicU32 = AtomicU32::new(0);

    /// "Ctrl+Alt+T" → (修饰键位掩码, 虚拟键码)。
    pub fn parse_spec(spec: &str) -> Result<(u32, u32), String> {
        let parts: Vec<&str> = spec
            .split('+')
            .map(|part| part.trim())
            .filter(|part| !part.is_empty())
            .collect();
        if parts.is_empty() {
            return Err("快捷键不能为空".to_string());
        }

        let mut modifiers = 0u32;
        let mut key: Option<u32> = None;
        for (index, part) in parts.iter().enumerate() {
            let lower = part.to_ascii_lowercase();
            let is_last = index + 1 == parts.len();
            if is_last {
                key = match lower.as_str() {
                    k if k.chars().count() == 1 => {
                        let ch = k.chars().next().unwrap().to_ascii_uppercase();
                        if ch.is_ascii_alphanumeric() {
                            Some(ch as u32)
                        } else {
                            return Err(format!("不支持的按键：{part}"));
                        }
                    }
                    k if k.starts_with('f') && k[1..].parse::<u8>().is_ok() => {
                        let n: u8 = k[1..].parse().unwrap();
                        if (1..=24).contains(&n) {
                            Some(0x70 + u32::from(n) - 1)
                        } else {
                            return Err("功能键只支持 F1~F24".to_string());
                        }
                    }
                    _ => return Err(format!("不支持的按键：{part}")),
                };
            } else {
                match lower.as_str() {
                    "ctrl" | "control" => modifiers |= MOD_CONTROL,
                    "alt" => modifiers |= MOD_ALT,
                    "shift" => modifiers |= MOD_SHIFT,
                    "win" | "meta" | "super" => modifiers |= MOD_WIN,
                    _ => return Err(format!("不支持的修饰键：{part}")),
                }
            }
        }

        let key = key.ok_or_else(|| "缺少主键（例如 D）".to_string())?;
        if modifiers == 0 {
            // 没有修饰键的全局热键会把普通打字吞掉，绝大多数场景都是误配。
            return Err("至少要带一个修饰键（Ctrl / Alt / Shift / Win）".to_string());
        }
        Ok((modifiers | MOD_NOREPEAT, key))
    }

    fn describe_failure(code: u32) -> String {
        if code == ERR_ALREADY_REGISTERED {
            "这个组合已被其他程序占用，换一个试试".to_string()
        } else {
            format!("注册失败（错误码 {code}）")
        }
    }

    pub fn start(app: tauri::AppHandle, initial: String) -> Bindings {
        let (tx, rx) = mpsc::channel::<Command>();

        std::thread::spawn(move || unsafe {
            THREAD_ID.store(GetCurrentThreadId(), Ordering::SeqCst);

            let mut current_spec = initial.clone();
            let (mut mods, mut vk) = match parse_spec(&initial) {
                Ok(pair) => pair,
                Err(_) => parse_spec(DEFAULT).expect("默认快捷键必须合法"),
            };
            if RegisterHotKey(0, HOTKEY_ID, mods, vk) == 0 {
                let code = GetLastError();
                super::log_line(&format!("hotkey: {current_spec} 注册失败（错误码 {code}）"));
            }

            loop {
                let mut msg: Msg = std::mem::zeroed();
                if GetMessageW(&mut msg, 0, 0, 0) <= 0 {
                    break;
                }
                match msg.message {
                    WM_HOTKEY => toggle(&app),
                    WM_APP_REBIND => {
                        while let Ok(Command::Rebind(spec, reply)) = rx.try_recv() {
                            let parsed = match parse_spec(&spec) {
                                Ok(pair) => pair,
                                Err(e) => {
                                    let _ = reply.send(Err(e));
                                    continue;
                                }
                            };
                            if parsed == (mods, vk) {
                                let _ = reply.send(Ok(current_spec.clone()));
                                continue;
                            }
                            // 先试新的：失败就回滚，不能出现"改失败后旧的也没了"。
                            UnregisterHotKey(0, HOTKEY_ID);
                            if RegisterHotKey(0, HOTKEY_ID, parsed.0, parsed.1) != 0 {
                                (mods, vk) = parsed;
                                current_spec = spec;
                                super::save_hotkey(&current_spec);
                                super::log_line(&format!("hotkey: 改为 {current_spec}"));
                                let _ = reply.send(Ok(current_spec.clone()));
                            } else {
                                let code = GetLastError();
                                let _ = RegisterHotKey(0, HOTKEY_ID, mods, vk);
                                let _ = reply.send(Err(describe_failure(code)));
                            }
                        }
                    }
                    _ => {}
                }
            }
        });

        Bindings { tx }
    }

    /// 已在前台就收起，否则呼出——避免按一下反而把面板顶到脸上。
    fn toggle(app: &tauri::AppHandle) {
        if let Some(window) = app.get_webview_window("main") {
            let visible = window.is_visible().unwrap_or(false);
            let focused = window.is_focused().unwrap_or(false);
            if visible && focused {
                let _ = window.hide();
            } else {
                super::show_window(app);
            }
        }
    }

    impl Bindings {
        pub fn rebind(&self, spec: &str) -> Result<String, String> {
            let (reply_tx, reply_rx) = mpsc::channel();
            self.tx
                .send(Command::Rebind(spec.to_string(), reply_tx))
                .map_err(|_| "快捷键线程已退出".to_string())?;
            unsafe {
                let tid = THREAD_ID.load(Ordering::SeqCst);
                if tid == 0 || PostThreadMessageW(tid, WM_APP_REBIND, 0, 0) == 0 {
                    return Err("无法通知快捷键线程".to_string());
                }
            }
            reply_rx
                .recv_timeout(Duration::from_secs(3))
                .map_err(|_| "快捷键没有响应（超时）".to_string())?
        }
    }

    /// Tauri 的托管状态：命令层从这里拿到 Manager。
    pub struct State(pub std::sync::Mutex<Option<Bindings>>);

    #[tauri::command]
    pub fn get_hotkey() -> String {
        super::load_hotkey()
    }

    #[tauri::command]
    pub fn set_hotkey(spec: String, state: tauri::State<'_, State>) -> Result<String, String> {
        let guard = state.0.lock().map_err(|_| "状态锁中毒".to_string())?;
        let manager = guard
            .as_ref()
            .ok_or_else(|| "快捷键还没初始化".to_string())?;
        manager.rebind(&spec)
    }
}

/// 快捷键存在助手自己的设置文件里（核心不关心界面偏好）。
fn settings_path() -> std::path::PathBuf {
    data_dir().join("agent.json")
}

fn load_hotkey() -> String {
    let raw = std::fs::read_to_string(settings_path()).unwrap_or_default();
    let value: serde_json::Value = serde_json::from_str(&raw).unwrap_or(serde_json::Value::Null);
    value
        .get("hotkey")
        .and_then(|v| v.as_str())
        .filter(|v| !v.is_empty())
        .unwrap_or(hotkey::DEFAULT)
        .to_string()
}

fn save_hotkey(spec: &str) {
    let path = settings_path();
    let mut value: serde_json::Value = std::fs::read_to_string(&path)
        .ok()
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_else(|| serde_json::json!({}));
    if let Some(map) = value.as_object_mut() {
        map.insert("hotkey".to_string(), serde_json::json!(spec));
    }
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(
        path,
        serde_json::to_string_pretty(&value).unwrap_or_default(),
    );
}

/// 是否"只进托盘、不弹窗"。
///
/// 登录自启走这条：用户登录后应该看到图标，而不是一个突然弹出来的窗口。
/// 双击图标（无参数）仍然是"我要看界面"，两者必须能区分。
fn tray_only_requested() -> bool {
    std::env::args().skip(1).any(|arg| arg == "--tray")
}

/// 登录启动项里要写的命令：带 `--tray`，保证登录后只出托盘。
fn logon_command(exe: &std::path::Path) -> String {
    format!("\"{}\" --tray", exe.display())
}

/// `--sync-logon on|off`：只维护登录启动项，不启界面、不拉起核心。
///
/// 存在的意义有两个：让这条能力能被脚本/自动化验证；以及用户在外部删掉注册表项后
/// 能一键修回，而不必先开一次界面。
fn logon_cli_request() -> Option<bool> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let index = args.iter().position(|arg| arg == "--sync-logon")?;
    match args.get(index + 1).map(String::as_str) {
        Some("on") => Some(true),
        Some("off") => Some(false),
        _ => {
            eprintln!("用法：tl-agent.exe --sync-logon on|off");
            std::process::exit(2);
        }
    }
}

fn main() {
    if let Some(enabled) = logon_cli_request() {
        match sync_logon_entry(enabled) {
            Ok(message) => {
                println!("{message}");
                std::process::exit(0);
            }
            Err(e) => {
                eprintln!("{e}");
                std::process::exit(1);
            }
        }
    }

    tauri::Builder::default()
        // 每会话至多一个助手：第二个实例把 `--show` 转发给已运行的，然后自己退出。
        .plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            show_window(app);
        }))
        .invoke_handler(tauri::generate_handler![
            start_core,
            refresh_bootstrap,
            set_autostart_elevated,
            read_local_private_key,
            list_local_private_keys,
            set_window_caption,
            known_hosts_fingerprint,
            open_external,
            data_dir_status,
            fix_data_dir_acl,
            sync_hosts,
            import_icon,
            sync_proxy_override,
            install_gateway_ca,
            gateway_ca_trust,
            export_gateway_ca,
            save_profile_package,
            open_profile_package,
            check_update,
            download_update,
            install_update,
            sync_logon_entry,
            hotkey::get_hotkey,
            hotkey::set_hotkey,
            core_state,
            bootstrap_snapshot
        ])
        .setup(|app| {
            let handle = app.handle().clone();
            build_tray(&handle)?;

            // 登录自启（--tray）只留托盘；双击图标才把界面弹出来。
            if !tray_only_requested() {
                show_window(&handle);
            }
            spawn_bootstrap_injection(handle.clone());
            let manager = hotkey::start(handle, load_hotkey());
            app.manage(hotkey::State(std::sync::Mutex::new(Some(manager))));

            // 双击助手 = 想要这个服务：后台确保核心在跑，不阻塞窗口出现。
            std::thread::spawn(|| {
                let dir = data_dir();
                match tl_wake::ensure_core(&dir) {
                    Ok(()) => log_line("startup: core ensured"),
                    Err(e) => log_line(&format!("startup: core not ready ({e})")),
                }
            });

            Ok(())
        })
        // 关窗 = 收进托盘，不退出（退出服务与退出助手是两件事）。
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .run(tauri::generate_context!())
        .expect("运行 Threadline Agent 失败");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn logon_command_starts_in_the_tray_only() {
        // 登录自启绝不能弹窗：命令里必须带 --tray，且路径要加引号（可能含空格）。
        let cmd = logon_command(std::path::Path::new(
            r"C:\Program Files\Threadline\tl-agent.exe",
        ));
        assert_eq!(
            cmd,
            "\"C:\\Program Files\\Threadline\\tl-agent.exe\" --tray"
        );
        assert!(cmd.ends_with("--tray"));
    }

    #[test]
    fn logon_entry_round_trips_with_a_test_value() {
        // 用独立值名，不碰正式的 Threadline 项，避免测试改掉用户的自启配置。
        let name = "ThreadlineTestValue";
        let value = "\"C:\\path with space\\tl-agent.exe\"";

        logon_entry::write(name, value).expect("write");
        assert_eq!(logon_entry::read(name).as_deref(), Some(value));

        logon_entry::delete(name).expect("delete");
        assert!(logon_entry::read(name).is_none(), "删除后必须读不到");

        // 幂等：重复删除不报错（用户在外部删过注册表项时也会走到这里）
        logon_entry::delete(name).expect("delete twice");
    }

    #[test]
    fn hotkey_spec_parsing_matches_what_the_ui_sends() {
        // 界面按下 Ctrl+Alt+T 送来 "Ctrl+Alt+T"，必须解析成 (修饰键, 主键)。
        let (mods, vk) = hotkey::parse_spec("Ctrl+Alt+T").expect("parse");
        assert_eq!(mods, 0x0001 | 0x0002 | 0x4000, "Alt+Ctrl+ NOREPEAT");
        assert_eq!(vk, 'T' as u32);

        // 大小写、顺序都不该影响结果
        assert_eq!(hotkey::parse_spec("alt+ctrl+t").expect("parse"), (mods, vk));

        assert!(hotkey::parse_spec("Ctrl+Shift+F12").is_ok());
    }

    #[test]
    fn hotkey_spec_rejects_combinations_that_would_break_typing() {
        // 没有修饰键的全局热键会把普通打字吞掉，必须拒绝。
        assert!(hotkey::parse_spec("D").is_err());
        // 拼错的键名要报错，而不是悄悄注册成别的东西。
        assert!(hotkey::parse_spec("Ctrl+Foo").is_err());
        assert!(hotkey::parse_spec("").is_err());
        assert!(hotkey::parse_spec("Ctrl+F25").is_err());
    }

    #[test]
    fn control_port_is_read_even_when_status_is_stale() {
        // 崩溃的核心会把 status 留在 running；端口仍然要能读出来用于探活。
        let running = r#"{"status":"running","port":18921,"token":"t"}"#;
        assert_eq!(parse_control_port(running), Some(18921));

        let stopped = r#"{"status":"stopped","port":18922,"token":""}"#;
        assert_eq!(
            parse_control_port(stopped),
            Some(18922),
            "探活不依赖 status，否则停掉的核心会被误判为从未存在"
        );
    }

    #[test]
    fn send_shutdown_waits_for_and_accepts_a_202() {
        use std::io::{Read, Write};

        let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).expect("bind");
        let port = listener.local_addr().expect("addr").port();
        let server = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().expect("accept");
            let mut buf = [0u8; 512];
            let n = socket.read(&mut buf).expect("read request");
            let text = String::from_utf8_lossy(&buf[..n]).to_string();
            assert!(
                text.starts_with("POST /api/v1/shutdown"),
                "请求行不对：{text}"
            );
            socket
                .write_all(
                    b"HTTP/1.1 202 Accepted\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                )
                .expect("write response");
        });

        assert!(send_shutdown(port, "tok").is_ok());
        server.join().expect("server thread");
    }

    #[test]
    fn send_shutdown_reports_a_rejection() {
        use std::io::{Read, Write};

        let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).expect("bind");
        let port = listener.local_addr().expect("addr").port();
        std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().expect("accept");
            let mut buf = [0u8; 512];
            let _ = socket.read(&mut buf);
            let _ = socket.write_all(
                b"HTTP/1.1 401 Unauthorized\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            );
        });

        let result = send_shutdown(port, "stale-token");
        assert!(result.is_err(), "非 2xx 必须报错，不能假装成功");
        assert!(result.unwrap_err().contains("401"));
    }

    #[test]
    fn control_port_missing_or_broken_is_none() {
        assert_eq!(parse_control_port("{ not json"), None);
        assert_eq!(parse_control_port(r#"{"status":"running"}"#), None);
    }
}
