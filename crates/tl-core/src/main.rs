//! Threadline 核心（壳层入口）。
//!
//! 归属：**应用壳**。只做四件事：解析命令行、初始化底座、按组合根装配三层、运行。
//! 不含业务规则。
//!
//! 硬约束（CAW I1/AP1）：本 crate 与任何 GUI/托盘/浏览器库**零依赖**，绝不渲染 UI。
//!
//! 退出码（设计 §3.6）：0 成功 · 2 参数错误 · 3 已有核心持锁 · 5 控制端口全占 ·
//! 7 数据目录不可用。`--autostart` 一次性模式不抢锁、不绑端口、不写 `control.json`。

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;

use tl_core::business::Services;
use tl_core::foundation::clock;
use tl_core::foundation::lock::{Acquire, InstanceLock};
use tl_core::foundation::registry;
use tl_core::models::credential::CredentialStore;
use tl_core::models::runtime::{new_instance_id, ProcessIdentity, RuntimeStore};
use tl_core::models::server::ServerStore;
use tl_core::models::tunnel::TunnelStore;
use tl_protocol::{AutostartResult, HostsSyncFileResult, RunMode};

const EXIT_OK: u8 = 0;
const EXIT_BAD_ARGS: u8 = 2;
const EXIT_LOCK_HELD: u8 = 3;
const EXIT_NO_PORT: u8 = 5;
const EXIT_DATA_DIR: u8 = 7;

const PORT_RANGE: std::ops::RangeInclusive<u16> = 18_921..=19_020;

/// 把 v1 的配置文件改名留档（不删，便于回查），让新版本从空白开始。
///
/// 新版把「凭据」收进隧道里，文件结构变了；用户确认不迁移。
fn retire_legacy_data(dir: &std::path::Path) {
    const CURRENT: u32 = tl_protocol::SCHEMA_VERSION;
    for name in ["servers.json", "tunnels.json"] {
        let path = dir.join(name);
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let version = serde_json::from_str::<serde_json::Value>(&text)
            .ok()
            .and_then(|v| v.get("schema_version").and_then(|n| n.as_u64()))
            .unwrap_or(0);
        if version < CURRENT as u64 {
            // 带上时间戳：重复留档不会把上一次的备份覆盖掉。
            let stamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            let backup = dir.join(format!("{name}.v{version}.{stamp}.bak"));
            let _ = std::fs::rename(&path, &backup);
            eprintln!("旧版数据已留档：{}", backup.display());
        }
    }
}

#[tokio::main]
async fn main() -> ExitCode {
    init_tracing();

    // 提权子命令必须最先分流：走到 create_dir_all / retire_legacy_data 会改名用户数据，
    // 也会在错误的数据目录里留下控制文件。
    if let Some(code) = hosts_sync_entry() {
        return code;
    }

    let args = match Args::parse(std::env::args().skip(1)) {
        Ok(args) => args,
        Err(message) => {
            eprintln!("参数错误：{message}");
            return ExitCode::from(EXIT_BAD_ARGS);
        }
    };

    let data_dir = args.data_dir.clone();
    if let Err(e) = std::fs::create_dir_all(&data_dir) {
        eprintln!("数据目录不可用（{}）：{e}", data_dir.display());
        return ExitCode::from(EXIT_DATA_DIR);
    }
    // 新版把「凭据」收进隧道里，文件结构变了。用户确认不迁移：旧数据改名留档，重新开始。
    retire_legacy_data(&data_dir);

    let log_dir = data_dir.join("logs");
    let _ = std::fs::create_dir_all(&log_dir);

    // 老位置（ProgramData）的数据搬到公共目录；只搬缺失的文件。
    if let Ok(program_data) = std::env::var("ProgramData") {
        if !program_data.is_empty() {
            migrate_legacy_data(&data_dir, &PathBuf::from(program_data).join("threadline"));
        }
    }

    // 一次性提权模式：修配置目录的 ACL（管理员身份创建目录后，交互用户只有只读权限）。
    if args.fix_acl {
        return run_fix_acl_once(&data_dir);
    }

    // 一次性提权模式：只改注册层，绝不抢锁 / 绑端口 / 写 control.json。
    if let Some(target) = args.autostart {
        return run_autostart_once(&data_dir, &log_dir, target);
    }

    // 单实例：抢不到锁说明已有核心在跑（正常退出，不是错误）。
    let lock_path = data_dir.join("core.lock");
    let lock = match InstanceLock::acquire(
        &lock_path,
        std::time::Duration::from_secs(2),
        std::time::Duration::from_millis(100),
    ) {
        Ok(Acquire::Acquired(lock)) => lock,
        Ok(Acquire::Held) => {
            tracing::info!("已有核心实例持锁（{}），本进程退出", lock_path.display());
            return ExitCode::from(EXIT_LOCK_HELD);
        }
        Err(e) => {
            eprintln!("打开锁文件失败：{e}");
            return ExitCode::from(EXIT_DATA_DIR);
        }
    };

    let identity = ProcessIdentity {
        instance_id: new_instance_id(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        run_mode: args.run_mode,
        identity: current_identity(),
    };

    let credentials = Arc::new(CredentialStore::new(data_dir.join("credentials.dat")));
    let servers = Arc::new(ServerStore::new(
        data_dir.join("servers.json"),
        Arc::clone(&credentials),
    ));
    let tunnels = Arc::new(TunnelStore::new(
        data_dir.join("tunnels.json"),
        Arc::clone(&servers),
    ));
    let runtime = Arc::new(RuntimeStore::new(&data_dir, identity));
    runtime.sync_run_mode_degradations();

    let exe_path = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("tl-core.exe"));
    let services = Services::new(
        Arc::clone(&credentials),
        Arc::clone(&servers),
        Arc::clone(&tunnels),
        Arc::clone(&runtime),
        &data_dir,
    );

    // 绑控制端口：范围内第一个可用的。
    let (listener, port) = match bind_control_port().await {
        Some(pair) => pair,
        None => {
            eprintln!("控制端口 {:?} 全部被占用", PORT_RANGE);
            return ExitCode::from(EXIT_NO_PORT);
        }
    };

    let token = tl_core::interface::new_token();
    if let Err(e) = runtime.write_control(port, &token, &clock::now_iso8601()) {
        eprintln!("写 control.json 失败：{e}");
        return ExitCode::from(EXIT_DATA_DIR);
    }
    tracing::info!(
        port,
        run_mode = ?args.run_mode,
        identity = %runtime.process().identity,
        "核心已启动"
    );

    let state = tl_core::interface::AppState::new(Arc::clone(&services), token, &log_dir, exe_path);
    let app = tl_core::interface::router(state);

    // 启动自启的隧道（失败只记日志，不影响控制面）。
    let services_for_startup = Arc::clone(&services);
    tokio::spawn(async move {
        for (id, reason) in services_for_startup.on_startup().await {
            tracing::warn!(tunnel = %id, reason, "开机自启的隧道启动失败");
        }
    });

    let server = axum::serve(listener, app);
    let result = tokio::select! {
        result = server => result.map_err(|e| e.to_string()),
        _ = tokio::signal::ctrl_c() => Ok(()),
    };

    // 无论正常退出还是 Ctrl+C，都先把发现契约改成 stopped。
    if let Err(e) = services.shutdown().await {
        tracing::warn!("退出时停止隧道失败：{e}");
    }
    drop(lock);

    match result {
        Ok(()) => ExitCode::from(EXIT_OK),
        Err(e) => {
            eprintln!("控制面异常退出：{e}");
            ExitCode::from(EXIT_DATA_DIR)
        }
    }
}

fn init_tracing() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();
}

/// 当前 Windows 身份（用于发现契约与提权判定）。
fn current_identity() -> String {
    match std::env::var("USERNAME") {
        Ok(user) if !user.is_empty() => {
            // SYSTEM 账户名固定为 `SYSTEM`（本机服务上下文）。
            if user.eq_ignore_ascii_case("SYSTEM") {
                "SYSTEM".to_string()
            } else {
                format!("user:{user}")
            }
        }
        _ => "unknown".to_string(),
    }
}

async fn bind_control_port() -> Option<(tokio::net::TcpListener, u16)> {
    for port in PORT_RANGE {
        if let Ok(listener) = tokio::net::TcpListener::bind(("127.0.0.1", port)).await {
            return Some((listener, port));
        }
    }
    None
}

/// `--autostart on|off`：一次性提权进程，结果写 `autostart.result.json`。
fn run_autostart_once(data_dir: &Path, log_dir: &Path, enabled: bool) -> ExitCode {
    let exe_path = match std::env::current_exe() {
        Ok(path) => path,
        Err(e) => {
            write_autostart_result(
                data_dir,
                AutostartResult {
                    ok: false,
                    code: EXIT_DATA_DIR as u32,
                    field: Some("exe_path".to_string()),
                    message: format!("无法获取自身路径：{e}"),
                    at: clock::now_iso8601(),
                },
            );
            return ExitCode::from(EXIT_DATA_DIR);
        }
    };

    let outcome = if enabled {
        registry::create(&exe_path, log_dir).map_err(|e| (e.field, e.message))
    } else {
        registry::delete(registry::TASK_NAME).map_err(|e| (e.field, e.message))
    };

    match outcome {
        Ok(()) => {
            write_autostart_result(
                data_dir,
                AutostartResult {
                    ok: true,
                    code: EXIT_OK as u32,
                    field: None,
                    message: if enabled {
                        "已开启开机自启".to_string()
                    } else {
                        "已关闭开机自启".to_string()
                    },
                    at: clock::now_iso8601(),
                },
            );
            ExitCode::from(EXIT_OK)
        }
        Err((field, message)) => {
            let code = if field.is_some() {
                EXIT_BAD_ARGS
            } else {
                EXIT_DATA_DIR
            };
            write_autostart_result(
                data_dir,
                AutostartResult {
                    ok: false,
                    code: code as u32,
                    field: field.map(str::to_string),
                    message,
                    at: clock::now_iso8601(),
                },
            );
            ExitCode::from(code)
        }
    }
}

/// 给"当前交互用户"授予数据目录的修改权限。
///
/// 背景：`C:\ProgramData\threadline` 是由管理员/SYSTEM 创建的，创建者的显式 ACL 只留下
/// SYSTEM / Administrators 完全控制，交互用户只有继承来的只读权限。核心以 SYSTEM 跑没问题，
/// 但用户双击助手（普通权限）后改配置就会"拒绝访问"。
///
/// UAC 提权**不换用户**，所以提权进程里的 USERDOMAIN\USERNAME 仍然是那个交互用户。
fn run_fix_acl_once(data_dir: &Path) -> ExitCode {
    // 与 `hosts sync` 同级校验：目录不对就什么都不做。
    // 尤其不能往目标目录写结果文件——它由调用方给，可能是任意系统目录。
    let expected = trusted_data_dir();
    if !same_dir(data_dir, &expected) {
        eprintln!(
            "数据目录不在允许范围：{}（只允许 {}）",
            data_dir.display(),
            expected.display()
        );
        return ExitCode::from(EXIT_BAD_ARGS);
    }

    let who = match (std::env::var("USERDOMAIN"), std::env::var("USERNAME")) {
        (Ok(domain), Ok(user)) if !user.is_empty() => format!("{domain}\\{user}"),
        (_, Ok(user)) if !user.is_empty() => user,
        _ => {
            write_acl_result(data_dir, false, "取不到当前用户名");
            return ExitCode::from(EXIT_DATA_DIR);
        }
    };

    if let Err(e) = std::fs::create_dir_all(data_dir) {
        write_acl_result(data_dir, false, &format!("创建数据目录失败：{e}"));
        return ExitCode::from(EXIT_DATA_DIR);
    }

    // /T 覆盖已存在的文件：它们带的是创建者(管理员)的显式 ACL，不会自动继承。
    let output = std::process::Command::new("icacls.exe")
        .arg(data_dir)
        .arg("/grant")
        .arg(format!("{who}:(OI)(CI)M"))
        .arg("/T")
        .arg("/C")
        .arg("/Q")
        .output();

    let (ok, message) = match output {
        Ok(out) if out.status.success() => (true, format!("已授予 {who} 修改权限")),
        Ok(out) => (
            false,
            format!(
                "icacls 失败（{}）：{}",
                out.status.code().unwrap_or(-1),
                String::from_utf8_lossy(&out.stderr).trim()
            ),
        ),
        Err(e) => (false, format!("无法执行 icacls：{e}")),
    };
    write_acl_result(data_dir, ok, &message);

    if ok {
        ExitCode::from(EXIT_OK)
    } else {
        ExitCode::from(EXIT_DATA_DIR)
    }
}

fn write_acl_result(data_dir: &Path, ok: bool, message: &str) {
    let payload = serde_json::json!({
        "ok": ok,
        "message": message,
        "at": clock::now_iso8601(),
    });
    let path = data_dir.join("acl.result.json");
    if let Err(e) = tl_core::foundation::store::write_json_atomic(&path, &payload) {
        eprintln!("写 acl.result.json 失败：{e}");
    }
}

fn write_autostart_result(data_dir: &Path, result: AutostartResult) {
    let path = data_dir.join("autostart.result.json");
    if let Err(e) = tl_core::foundation::store::write_json_atomic(&path, &result) {
        eprintln!("写 autostart.result.json 失败：{e}");
    }
}

/// 命令行参数。
#[derive(Debug, Clone, PartialEq, Eq)]
struct Args {
    fix_acl: bool,
    run_mode: RunMode,
    data_dir: PathBuf,
    autostart: Option<bool>,
}

impl Args {
    fn parse(args: impl Iterator<Item = String>) -> Result<Self, String> {
        let mut run_mode = RunMode::Manual;
        let mut data_dir = default_data_dir();
        let mut autostart = None;
        let mut fix_acl = false;

        let mut iter = args.peekable();
        while let Some(arg) = iter.next() {
            match arg.as_str() {
                "--boot" => run_mode = RunMode::Boot,
                "--stand-in" => run_mode = RunMode::StandIn,
                "--fallback" => run_mode = RunMode::Fallback,
                "--manual" => run_mode = RunMode::Manual,
                "--autostart" => {
                    let value = iter.next().ok_or("--autostart 需要一个值：on|off")?;
                    autostart = Some(match value.as_str() {
                        "on" => true,
                        "off" => false,
                        other => return Err(format!("--autostart 只接受 on|off，收到 {other}")),
                    });
                }
                "--fix-acl" => fix_acl = true,
                "--data-dir" => {
                    let value = iter.next().ok_or("--data-dir 需要一个绝对路径")?;
                    let path = PathBuf::from(value);
                    if !path.is_absolute() {
                        return Err("--data-dir 必须是绝对路径".to_string());
                    }
                    data_dir = path;
                }
                "--log-dir" => {
                    // 兼容任务计划命令行里的日志目录参数（与数据目录下的 logs 等价）。
                    let value = iter.next().ok_or("--log-dir 需要一个绝对路径")?;
                    if !PathBuf::from(&value).is_absolute() {
                        return Err("--log-dir 必须是绝对路径".to_string());
                    }
                }
                "--help" | "-h" => {
                    return Err(
                        "用法：tl-core [--boot|--manual|--stand-in|--fallback] [--data-dir <绝对路径>] [--autostart on|off] [--fix-acl]"
                            .to_string(),
                    )
                }
                other => return Err(format!("未知参数：{other}")),
            }
        }

        Ok(Self {
            fix_acl,
            run_mode,
            data_dir,
            autostart,
        })
    }
}

/// 提权后的一次性入口：只重算并写入 hosts 托管区块。
///
/// 安全约束：
/// - 只接受 `--data-dir`，且**只接受默认数据目录**（不用 `install.json` 做白名单——
///   它就在候选目录里，调用方放一份自己写的就能通过，等于自证）；
/// - 目标目录与 `logs` 子路径不得含重解析点；
/// - 不抢锁、不绑端口、不写 `control.json`；
/// - 结果写 `hosts.result.json`：`ShellExecuteW` 拿不到退出码，Agent 靠它判断。
fn hosts_sync_entry() -> Option<ExitCode> {
    let mut args = std::env::args().skip(1);
    if args.next().as_deref() != Some("hosts") {
        return None;
    }
    match args.next().as_deref() {
        Some("sync") => Some(ExitCode::from(run_hosts_sync_once(args))),
        Some("install-task") => Some(ExitCode::from(run_hosts_task_install(args))),
        Some("remove-task") => Some(ExitCode::from(run_hosts_task_remove())),
        other => {
            eprintln!("未知子命令：hosts {}", other.unwrap_or_default());
            Some(ExitCode::from(EXIT_BAD_ARGS))
        }
    }
}

/// 计划任务名。注册为 SYSTEM 身份后，写 hosts 不再需要每次弹 UAC。
const HOSTS_TASK: &str = "ThreadlineHostsSync";

/// 注册（或更新）计划任务，并立刻同步一次。
///
/// 这个子命令**必须提权**运行：只有管理员能创建 SYSTEM 身份的定时任务。
/// 首次点「写入」时由 Agent 拉一次 UAC 跑它，之后 Agent 直接 `schtasks /Run`，
/// 不再打扰用户。
fn run_hosts_task_install(mut args: impl Iterator<Item = String>) -> u8 {
    let mut data_dir: Option<PathBuf> = None;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--data-dir" => {
                let Some(value) = args.next() else {
                    eprintln!("--data-dir 需要一个路径");
                    return EXIT_BAD_ARGS;
                };
                data_dir = Some(PathBuf::from(value));
            }
            other => {
                eprintln!("不支持的参数：{other}（只允许 --data-dir）");
                return EXIT_BAD_ARGS;
            }
        }
    }
    let Some(dir) = data_dir else {
        eprintln!("缺少 --data-dir");
        return EXIT_BAD_ARGS;
    };

    // 与 hosts sync 同一套校验：任务动作里写的是这个目录，不能让它指向任意位置。
    let expected = trusted_data_dir();
    if !same_dir(&dir, &expected) {
        eprintln!(
            "数据目录不在允许范围：{}（只允许 {}）",
            dir.display(),
            expected.display()
        );
        return EXIT_BAD_ARGS;
    }
    if has_reparse_point(&dir) || has_reparse_point(&dir.join("logs")) {
        eprintln!("数据目录包含重解析点，拒绝注册：{}", dir.display());
        return EXIT_BAD_ARGS;
    }

    if let Err(e) = register_hosts_task(&dir) {
        eprintln!("注册计划任务失败：{e}");
        return 1;
    }

    // 任务刚建好，顺手把这次请求也做掉：用户点一次就生效，不必等下一次。
    run_hosts_sync_once(["--data-dir".to_string(), dir.display().to_string()].into_iter())
}

fn run_hosts_task_remove() -> u8 {
    let output = std::process::Command::new("schtasks")
        .args(["/Delete", "/F", "/TN", HOSTS_TASK])
        .output();
    match output {
        Ok(out) if out.status.success() => 0,
        // 任务本来就不存在（退出码 1）也算成功：卸载时不该因此报错。
        Ok(_) => 0,
        Err(e) => {
            eprintln!("调用 schtasks 失败：{e}");
            1
        }
    }
}

/// 写一份任务 XML 交给 `schtasks /Create`。
///
/// 用 XML 而不是 `/TR`：命令行里带引号的数据目录会把 `/TR` 拆坏，
/// XML 能精确表达 `Command` + `Arguments`。
fn register_hosts_task(dir: &Path) -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| format!("取自身路径失败：{e}"))?;
    let action = format!("hosts sync --data-dir \"{}\"", dir.display());
    let xml = format!(
        r#"<?xml version="1.0" encoding="UTF-16"?>
<Task version="1.4" xmlns="http://schemas.microsoft.com/windows/2004/02/mit/task">
  <RegistrationInfo>
    <Description>Threadline: write the managed block into the hosts file</Description>
  </RegistrationInfo>
  <Triggers />
  <Principals>
    <Principal id="Author">
      <UserId>S-1-5-18</UserId>
      <RunLevel>HighestAvailable</RunLevel>
    </Principal>
  </Principals>
  <Settings>
    <MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy>
    <DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries>
    <StopIfGoingOnBatteries>false</StopIfGoingOnBatteries>
    <AllowHardTerminate>true</AllowHardTerminate>
    <StartWhenAvailable>true</StartWhenAvailable>
    <RunOnlyIfNetworkAvailable>false</RunOnlyIfNetworkAvailable>
    <IdleSettings>
      <StopOnIdleEnd>false</StopOnIdleEnd>
      <RestartOnIdle>false</RestartOnIdle>
    </IdleSettings>
    <AllowStartOnDemand>true</AllowStartOnDemand>
    <Enabled>true</Enabled>
    <Hidden>false</Hidden>
    <RunOnlyIfIdle>false</RunOnlyIfIdle>
    <WakeToRun>false</WakeToRun>
    <ExecutionTimeLimit>PT5M</ExecutionTimeLimit>
    <Priority>7</Priority>
  </Settings>
  <Actions Context="Author">
    <Exec>
      <Command>{command}</Command>
      <Arguments>{action}</Arguments>
    </Exec>
  </Actions>
</Task>
"#,
        command = xml_escape(&exe.display().to_string()),
        action = xml_escape(&action)
    );

    // schtasks 只接受 UTF-16（带 BOM）的 XML。
    let path = std::env::temp_dir().join("threadline-hosts-task.xml");
    let mut bytes = vec![0xFF, 0xFE];
    for unit in xml.encode_utf16() {
        bytes.extend_from_slice(&unit.to_le_bytes());
    }
    std::fs::write(&path, &bytes).map_err(|e| format!("写任务 XML 失败：{e}"))?;

    let output = std::process::Command::new("schtasks")
        .args(["/Create", "/F", "/TN", HOSTS_TASK, "/XML"])
        .arg(&path)
        .output()
        .map_err(|e| format!("调用 schtasks 失败：{e}"))?;
    let _ = std::fs::remove_file(&path);

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let detail = if stderr.trim().is_empty() {
            stdout.trim().to_string()
        } else {
            stderr.trim().to_string()
        };
        return Err(format!("schtasks 返回失败：{detail}"));
    }
    Ok(())
}

fn xml_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

fn run_hosts_sync_once(mut args: impl Iterator<Item = String>) -> u8 {
    let mut data_dir: Option<PathBuf> = None;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--data-dir" => {
                let Some(value) = args.next() else {
                    eprintln!("--data-dir 需要一个路径");
                    return EXIT_BAD_ARGS;
                };
                data_dir = Some(PathBuf::from(value));
            }
            other => {
                eprintln!("不支持的参数：{other}（只允许 --data-dir）");
                return EXIT_BAD_ARGS;
            }
        }
    }

    let Some(dir) = data_dir else {
        eprintln!("缺少 --data-dir");
        return EXIT_BAD_ARGS;
    };

    let expected = trusted_data_dir();
    if !same_dir(&dir, &expected) {
        eprintln!(
            "数据目录不在允许范围：{}（只允许 {}）",
            dir.display(),
            expected.display()
        );
        return EXIT_BAD_ARGS;
    }
    if has_reparse_point(&dir) || has_reparse_point(&dir.join("logs")) {
        eprintln!("数据目录包含重解析点，拒绝写入：{}", dir.display());
        return EXIT_BAD_ARGS;
    }

    let hosts_path = tl_core::foundation::hosts::hosts_path();
    let existing = tl_core::foundation::hosts::read_raw(&hosts_path).unwrap_or_default();
    // 读失败**绝不能**当成「没有配置」：主进程正在原子替换这个文件时，
    // 提权进程可能读到半截 JSON。把解析失败当空集合，会把整块域名从 hosts 抹掉。
    let items = match tl_core::foundation::store::read_json::<tl_protocol::TunnelsFile>(
        &dir.join("tunnels.json"),
    ) {
        Ok(Some(file)) => file.items,
        Ok(None) => Vec::new(),
        Err(e) => {
            eprintln!("读取 tunnels.json 失败，拒绝改动 hosts：{e}");
            let result = HostsSyncFileResult {
                ok: false,
                code: EXIT_DATA_DIR as u32,
                message: Some(format!("读取配置失败，未改动 hosts：{e}")),
                at: clock::now_iso8601(),
            };
            let _ = tl_core::foundation::store::write_json_atomic(
                &dir.join("hosts.result.json"),
                &result,
            );
            return EXIT_DATA_DIR;
        }
    };

    // 这里读到的是**磁盘上的文件**，不能假定它一定由业务层写过：提权进程写 hosts
    // 之前必须自己再过一遍校验（域名规则同 `models::tunnel::validate_domain`，
    // 地址只允许回环池），否则篡改 tunnels.json 就能往系统 hosts 里注入任意条目。
    let mut entries: Vec<tl_core::foundation::hosts::Entry> = items
        .into_iter()
        .filter(|dto| !dto.domain.is_empty() && !dto.domain_host.is_empty())
        .filter_map(|dto| {
            let domain = match tl_core::models::tunnel::validate_domain(&dto.domain) {
                Ok(domain) => domain,
                Err(e) => {
                    eprintln!("跳过非法域名 {}：{e}", dto.domain_host);
                    return None;
                }
            };
            let address: std::net::IpAddr = match dto.domain_host.parse() {
                Ok(address) => address,
                Err(_) => {
                    eprintln!("跳过非 IP 地址：{}", dto.domain_host);
                    return None;
                }
            };
            if !address.is_loopback() {
                eprintln!("跳过非回环地址：{address}");
                return None;
            }
            Some(tl_core::foundation::hosts::Entry {
                address: address.to_string(),
                domain,
            })
        })
        .collect();
    entries.sort();

    let desired = tl_core::foundation::hosts::render(&existing, &entries);
    let result = match tl_core::foundation::hosts::write_raw(&hosts_path, &desired) {
        Ok(()) => {
            let _ = tl_core::foundation::hosts::flush_dns();
            HostsSyncFileResult {
                ok: true,
                code: 0,
                message: None,
                at: clock::now_iso8601(),
            }
        }
        Err(e) => HostsSyncFileResult {
            ok: false,
            code: EXIT_DATA_DIR as u32,
            message: Some(e.to_string()),
            at: clock::now_iso8601(),
        },
    };

    let result_path = dir.join("hosts.result.json");
    if let Err(e) = tl_core::foundation::store::write_json_atomic(&result_path, &result) {
        eprintln!("写 hosts.result.json 失败：{e}");
    }

    if result.ok {
        EXIT_OK
    } else {
        EXIT_DATA_DIR
    }
}

/// 比较两个目录是否指向同一处（大小写不敏感，忽略末尾分隔符）。
fn same_dir(a: &Path, b: &Path) -> bool {
    let normalize = |path: &Path| {
        path.to_string_lossy()
            .trim_end_matches(['\\', '/'])
            .to_ascii_lowercase()
    };
    normalize(a) == normalize(b)
}

#[cfg(windows)]
fn has_reparse_point(path: &Path) -> bool {
    use std::os::windows::fs::MetadataExt;
    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
    std::fs::symlink_metadata(path)
        .map(|meta| meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0)
        .unwrap_or(false)
}

#[cfg(not(windows))]
fn has_reparse_point(_path: &Path) -> bool {
    false
}

/// `%PUBLIC%\Threadline`。
///
/// 为什么不是 `%ProgramData%`：ProgramData 的子目录由谁创建就归谁——管理员创建时交互用户只剩
/// 只读，SYSTEM 创建时用户拿到的是没有 Delete 的 Write（覆盖不了已有文件）。而 `%PUBLIC%`
/// 给的是 `INTERACTIVE: Modify` 的**可继承** ACE，SYSTEM（登录前自启）与登录用户都能读写，
/// 不需要任何 icacls / UAC。代价是同机其他交互用户也在这个权限里。
fn default_data_dir() -> PathBuf {
    match std::env::var("PUBLIC") {
        Ok(dir) if !dir.is_empty() => PathBuf::from(dir).join("Threadline"),
        _ => PathBuf::from(r"C:\Users\Public\Threadline"),
    }
}

/// 提权入口（`hosts sync` / `--fix-acl`）认的**唯一**合法数据目录。
///
/// 不能用 [`default_data_dir`]：它读 `%PUBLIC%`，而环境变量由发起 `runas` 的
/// 父进程决定——低权用户设 `PUBLIC=D:\mine` 再借一次 UAC，就能让提权进程去写
/// 那个目录（甚至 `C:\Windows\System32`）。这里钉死系统公共目录。
fn trusted_data_dir() -> PathBuf {
    PathBuf::from(r"C:\Users\Public\Threadline")
}

/// 老位置（`%ProgramData%\threadline`）的数据搬一次，旧目录原样留着备查。
///
/// 只搬"新位置还不存在"的文件，所以重复执行是安全的；用户后来在新位置改过的东西不会被覆盖。
fn migrate_legacy_data(new_dir: &Path, legacy: &Path) {
    if legacy == new_dir || !legacy.is_dir() {
        return;
    }
    let _ = std::fs::create_dir_all(new_dir);

    for name in [
        "servers.json",
        "tunnels.json",
        "credentials.dat",
        "agent.json",
    ] {
        let src = legacy.join(name);
        let dst = new_dir.join(name);
        if src.is_file() && !dst.exists() {
            match std::fs::copy(&src, &dst) {
                Ok(_) => eprintln!("已从老位置迁移 {name}"),
                Err(e) => eprintln!("迁移 {name} 失败：{e}"),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Result<Args, String> {
        Args::parse(args.iter().map(|s| s.to_string()))
    }

    #[test]
    fn same_dir_ignores_case_and_trailing_separator() {
        assert!(same_dir(
            Path::new(r"C:\Users\Public\Threadline\"),
            Path::new(r"c:\users\public\threadline")
        ));
        assert!(!same_dir(
            Path::new(r"C:\Temp\Threadline"),
            Path::new(r"C:\Users\Public\Threadline")
        ));
    }

    #[test]
    fn reparse_point_is_false_for_normal_directory() {
        let dir = std::env::temp_dir().join(format!("tl-reparse-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        assert!(!has_reparse_point(&dir));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn legacy_migration_copies_missing_files_and_keeps_existing() {
        let root = std::env::temp_dir().join(format!("tl-migrate-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let legacy = root.join("old");
        let new = root.join("new");
        std::fs::create_dir_all(&legacy).expect("legacy dir");
        std::fs::create_dir_all(&new).expect("new dir");

        std::fs::write(legacy.join("servers.json"), "old-servers").expect("write");
        std::fs::write(legacy.join("credentials.dat"), "old-cred").expect("write");
        // 新位置已经有同名文件：迁移不能覆盖用户后来改过的东西
        std::fs::write(new.join("servers.json"), "new-servers").expect("write");

        migrate_legacy_data(&new, &legacy);

        assert_eq!(
            std::fs::read_to_string(new.join("servers.json")).expect("read"),
            "new-servers",
            "已存在的配置不能被老文件覆盖"
        );
        assert_eq!(
            std::fs::read_to_string(new.join("credentials.dat")).expect("read"),
            "old-cred",
            "缺失的凭据要被搬过来"
        );

        // 同一个目录当参数时直接返回，不做任何事
        migrate_legacy_data(&legacy, &legacy);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn fix_acl_flag_is_parsed_and_defaults_off() {
        let args = Args::parse(vec!["--fix-acl".to_string()].into_iter()).expect("parse");
        assert!(args.fix_acl, "带 --fix-acl 时必须是修复模式");

        let normal = Args::parse(Vec::new().into_iter()).expect("parse");
        assert!(!normal.fix_acl, "默认不能进一次性提权模式");
        assert!(normal.autostart.is_none());
    }

    #[test]
    fn defaults_to_manual_mode() {
        let args = parse(&[]).expect("parse");
        assert_eq!(args.run_mode, RunMode::Manual);
        assert!(args.autostart.is_none());
    }

    #[test]
    fn recognises_all_run_modes() {
        assert_eq!(parse(&["--boot"]).unwrap().run_mode, RunMode::Boot);
        assert_eq!(parse(&["--stand-in"]).unwrap().run_mode, RunMode::StandIn);
        assert_eq!(parse(&["--fallback"]).unwrap().run_mode, RunMode::Fallback);
        assert_eq!(parse(&["--manual"]).unwrap().run_mode, RunMode::Manual);
    }

    #[test]
    fn autostart_accepts_only_on_off() {
        assert_eq!(parse(&["--autostart", "on"]).unwrap().autostart, Some(true));
        assert_eq!(
            parse(&["--autostart", "off"]).unwrap().autostart,
            Some(false)
        );
        assert!(parse(&["--autostart"]).is_err());
        assert!(parse(&["--autostart", "maybe"]).is_err());
    }

    #[test]
    fn data_dir_must_be_absolute() {
        assert!(parse(&["--data-dir", "relative/path"]).is_err());
        let args = parse(&["--data-dir", "C:\\ProgramData\\threadline"]).expect("absolute ok");
        assert_eq!(args.data_dir, PathBuf::from("C:\\ProgramData\\threadline"));
    }

    #[test]
    fn unknown_argument_is_rejected() {
        assert!(parse(&["--nope"]).is_err());
    }

    #[test]
    fn boot_flag_with_autostart_is_parsed_together() {
        // 任务计划命令行形态：`--boot --log-dir <绝对路径>`
        let args =
            parse(&["--boot", "--log-dir", "C:\\ProgramData\\threadline\\logs"]).expect("parse");
        assert_eq!(args.run_mode, RunMode::Boot);
    }
}
