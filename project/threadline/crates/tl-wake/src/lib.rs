//! Threadline 唤醒入口的逻辑（`tl.exe` 的 lib 部分，便于单测）。
//!
//! 归属：**唤醒（Wake）**。拥有：一次"确保核心与助手在跑并打开面板"的决策与执行。
//! 提供：`plan`（纯决策）/ `probe_healthz` / `read_control` / `task_state` / `run`。
//! 允许调用：`std`、`tl_protocol`（发现契约）、`serde_json`。
//!
//! 硬约束（CAW I3 / AP2）：**每条路径都必须在秒级内终止并给出退出码**；
//! Wake 绝不驻留、绝不扮演监督者、绝不无限等待。
//!
//! 规模说明：本 crate 只有几百行，按 MSO「粒度小于模型时这套结构是纯开销」的判据，
//! 不拆层目录，用分节注释区分决策 / 探测 / 进程 / 注册层。

use std::path::{Path, PathBuf};
use std::time::Duration;

// ─────────────────────── 退出码（设计 §3.6） ───────────────────────

pub const EXIT_OK: u8 = 0;
pub const EXIT_BAD_ARGS: u8 = 2;
pub const EXIT_CORE_UNREACHABLE: u8 = 4;
pub const EXIT_AGENT_FAILED: u8 = 6;

/// 注册层任务名（与 `tl-core` 的 `foundation::registry::TASK_NAME` 保持一致）。
pub const TASK_NAME: &str = "ThreadlineCore";

/// 各步超时预算（合计 ≤ 20s，设计 §3.4）。
pub const PROBE_TIMEOUT: Duration = Duration::from_millis(1_500);
pub const TRIGGER_TIMEOUT: Duration = Duration::from_secs(3);
pub const SPAWN_TIMEOUT: Duration = Duration::from_secs(2);
pub const SETTLE_TIMEOUT: Duration = Duration::from_secs(5);
pub const AGENT_TIMEOUT: Duration = Duration::from_secs(3);

// ─────────────────────── 观测结果 ───────────────────────

/// 核心探测结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Probe {
    /// 可达且 `instance_id` 与 `control.json` 一致。
    Healthy,
    /// 端口有响应，但不是我们的核心（`instance_id` 不一致或缺少字段）。
    NotOurs,
    /// 完全不可达。
    Absent,
}

/// 注册层（任务计划）状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskState {
    /// 存在且启用（开机自启开着）。
    Enabled,
    /// 存在但被禁用。
    Disabled,
    /// 不存在。
    Missing,
}

// ─────────────────────── 决策（纯函数） ───────────────────────

/// 下一步该做什么。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// 核心已在跑，直接开面板。
    AlreadyRunning,
    /// 交给任务计划立即触发（保持 SYSTEM 身份）。
    TriggerTask,
    /// 以当前用户身份拉起手动核心。
    SpawnManual,
    /// 开机自启确实开着但系统核心没起来——拉起顶班核心（降级，需标记）。
    SpawnStandIn,
}

/// 第一层决策：核心可达就别动；不可达时按注册层状态分流。
pub fn plan(probe: &Probe, task: TaskState) -> Action {
    match probe {
        Probe::Healthy => Action::AlreadyRunning,
        // 端口被别人占着不算"核心在跑"，否则会误判成一切正常。
        Probe::NotOurs | Probe::Absent => match task {
            TaskState::Enabled => Action::TriggerTask,
            TaskState::Disabled | TaskState::Missing => Action::SpawnManual,
        },
    }
}

/// 第二层决策：`schtasks /run` 没救活时怎么办。
///
/// 注册层启用 → 拉起顶班（用户身份，属降级）；否则按正常手动启动。
pub fn plan_after_trigger_failed(task: TaskState) -> Action {
    match task {
        TaskState::Enabled => Action::SpawnStandIn,
        TaskState::Disabled | TaskState::Missing => Action::SpawnManual,
    }
}

// ─────────────────────── HTTP 探测 ───────────────────────

/// 从原始 HTTP 响应里取出状态码与 JSON 正文。
///
/// 手写解析而不是引 HTTP 客户端：Wake 只发一个 `GET /healthz`，
/// 为此拉一整套异步栈不值得。
pub fn parse_http_response(raw: &str) -> Option<(u16, String)> {
    let (head, body) = match raw.split_once("\r\n\r\n") {
        Some((head, body)) => (head, body.to_string()),
        None => (raw, String::new()),
    };
    let status_line = head.lines().next()?;
    let mut parts = status_line.split_whitespace();
    let version = parts.next()?;
    if !version.starts_with("HTTP/") {
        return None;
    }
    let code: u16 = parts.next()?.parse().ok()?;
    Some((code, body))
}

/// 用 `GET /healthz` 探测核心（有界超时）。
///
/// `expected_instance` 来自 `control.json`：只比端口不够——端口可能被别的程序占着。
pub fn probe_healthz(port: u16, expected_instance: &str, timeout: Duration) -> Probe {
    use std::io::{Read, Write};
    use std::net::{TcpStream, ToSocketAddrs};

    let Ok(mut addrs) = ("127.0.0.1", port).to_socket_addrs() else {
        return Probe::Absent;
    };
    let Some(addr) = addrs.next() else {
        return Probe::Absent;
    };
    let Ok(mut stream) = TcpStream::connect_timeout(&addr, timeout) else {
        return Probe::Absent;
    };
    let _ = stream.set_read_timeout(Some(timeout));
    let _ = stream.set_write_timeout(Some(timeout));

    let request = format!(
        "GET /api/v1/healthz HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n"
    );
    if stream.write_all(request.as_bytes()).is_err() {
        return Probe::Absent;
    }

    let mut raw = String::new();
    if stream.read_to_string(&mut raw).is_err() && raw.is_empty() {
        return Probe::Absent;
    }

    match parse_http_response(&raw) {
        Some((200, body)) => {
            let Ok(value) = serde_json::from_str::<serde_json::Value>(&body) else {
                return Probe::NotOurs;
            };
            match value.get("instance_id").and_then(|v| v.as_str()) {
                Some(id) if id == expected_instance => Probe::Healthy,
                _ => Probe::NotOurs,
            }
        }
        // 有响应但不是我们要的形状：端口被别的程序占着。
        _ => Probe::NotOurs,
    }
}

// ─────────────────────── 发现契约 ───────────────────────

/// 读取 `control.json` 的运行中信息；文件不存在或解析失败都返回 `None`。
pub fn read_control(data_dir: &Path) -> Option<tl_protocol::ControlInfo> {
    let text = std::fs::read_to_string(data_dir.join("control.json")).ok()?;
    let file: tl_protocol::ControlFile = serde_json::from_str(&text).ok()?;
    file.running().cloned()
}

// ─────────────────────── 注册层 ───────────────────────

/// 解析 `schtasks /query /xml` 的输出，判断任务是否启用。
pub fn parse_task_xml(xml: &str) -> TaskState {
    let trimmed = xml.trim();
    if trimmed.is_empty()
        || trimmed.contains("cannot find")
        || trimmed.contains("找不到")
        || trimmed.contains("ERROR:")
    {
        return TaskState::Missing;
    }
    // 任务计划 XML 省略 <Enabled> 时表示启用。
    if trimmed.contains("<Enabled>false</Enabled>") {
        TaskState::Disabled
    } else {
        TaskState::Enabled
    }
}

/// 执行 `schtasks.exe`，并确保**不创建控制台窗口**。
///
/// Wake 自身是 GUI 子系统（双击不闪窗），子进程若不加 `CREATE_NO_WINDOW`，
/// Windows 会为 `schtasks.exe` 新开一个控制台，表现为闪一下黑框。
fn schtasks(args: &[&str]) -> std::io::Result<std::process::Output> {
    let mut command = std::process::Command::new("schtasks.exe");
    command.args(args);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    command.output()
}

/// 查询任务计划状态（**只读**，不修改任何东西）。
pub fn task_state() -> TaskState {
    let output = schtasks(&["/query", "/tn", TASK_NAME, "/xml", "ONE"]);
    match output {
        Ok(out) if out.status.success() => parse_task_xml(&String::from_utf8_lossy(&out.stdout)),
        _ => TaskState::Missing,
    }
}

// ─────────────────────── 进程 ───────────────────────

/// 与 `tl-core.exe` / `tl-agent.exe` 同目录（安装后三者并列）。
pub fn sibling_exe(name: &str) -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let path = exe.parent()?.join(name);
    path.exists().then_some(path)
}

/// 按候选名依次找同目录的可执行文件。
///
/// 打包后 Agent 的主程序名由 Tauri 决定（`Threadline.exe`），开发态是 `tl-agent.exe`，
/// 两者都要认，否则安装版会出现"核心起来了但面板打不开"。
pub fn sibling_exe_any(names: &[&str]) -> Option<PathBuf> {
    names.iter().find_map(|name| sibling_exe(name))
}

/// 拉起一个子进程并**不等待**（Wake 必须自己退出，不能等子进程）。
pub fn spawn_detached(exe: &Path, args: &[&str]) -> std::io::Result<()> {
    let mut command = std::process::Command::new(exe);
    command.args(args);
    // Windows 上：新进程不继承控制台，父进程退出后继续跑。
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        command.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP);
    }
    command.spawn().map(|_| ())
}

/// `schtasks /run` 立即触发任务（不等待任务内部逻辑）。
pub fn trigger_task() -> bool {
    schtasks(&["/run", "/tn", TASK_NAME])
        .map(|out| out.status.success())
        .unwrap_or(false)
}

/// 在预算内轮询，直到探测到健康核心或超时。
///
/// **每轮都重新读 `control.json`**：拉起新核心时，磁盘上可能还留着上一个核心的
/// 契约（同端口、旧 `instance_id`）。若拿一次性快照去比对，会一直 `NotOurs`
/// 直到预算耗尽——核心明明起来了却报"不可达"。
pub fn wait_until_healthy(data_dir: &Path, budget: Duration) -> Probe {
    let deadline = std::time::Instant::now() + budget;
    let mut last = Probe::Absent;
    loop {
        if let Some(control) = read_control(data_dir) {
            last = probe_healthz(control.port, &control.instance_id, PROBE_TIMEOUT);
            if last == Probe::Healthy {
                return Probe::Healthy;
            }
        }
        if std::time::Instant::now() >= deadline {
            return last;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}

// ─────────────────────── 编排 ───────────────────────

/// 确保核心在跑：探测 → 按注册层分流 → 等它就绪。
///
/// 这是原先 `tl.exe` 的全部"唤醒"职责。现在由**助手**直接调用——
/// 用户双击的就是助手，没必要再经一个只活几百毫秒的中转进程。
///
/// 返回 `Err` 时带上诊断信息（数据目录 / 任务计划状态），调用方直接展示给用户。
pub fn ensure_core(data_dir: &Path) -> Result<(), String> {
    let mut probe = match read_control(data_dir) {
        Some(control) => probe_healthz(control.port, &control.instance_id, PROBE_TIMEOUT),
        None => Probe::Absent,
    };
    let task = task_state();

    if probe != Probe::Healthy {
        let action = plan(&probe, task);
        match action {
            Action::AlreadyRunning => {}
            Action::TriggerTask => {
                let _ = trigger_task();
                probe = wait_until_healthy(data_dir, SETTLE_TIMEOUT);
                if probe != Probe::Healthy {
                    launch(&plan_after_trigger_failed(task), data_dir);
                    probe = wait_until_healthy(data_dir, SETTLE_TIMEOUT);
                }
            }
            other => {
                launch(&other, data_dir);
                probe = wait_until_healthy(data_dir, SETTLE_TIMEOUT);
            }
        }
    }

    if probe == Probe::Healthy {
        return Ok(());
    }
    Err(format!(
        "核心不可达（数据目录 {}，任务计划 {:?}）",
        data_dir.display(),
        task
    ))
}

fn launch(action: &Action, data_dir: &Path) {
    let Some(core) = sibling_exe("tl-core.exe") else {
        return;
    };
    let data_arg = data_dir.to_string_lossy().to_string();
    let mode = match action {
        Action::SpawnStandIn => "--stand-in",
        Action::SpawnManual => "--manual",
        _ => return,
    };
    let _ = spawn_detached(&core, &[mode, "--data-dir", &data_arg]);
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── 决策矩阵 ──

    #[test]
    fn healthy_core_needs_no_spawn_regardless_of_registration() {
        for task in [TaskState::Enabled, TaskState::Disabled, TaskState::Missing] {
            assert_eq!(plan(&Probe::Healthy, task), Action::AlreadyRunning);
        }
    }

    #[test]
    fn absent_core_with_enabled_task_triggers_the_task_first() {
        assert_eq!(
            plan(&Probe::Absent, TaskState::Enabled),
            Action::TriggerTask
        );
    }

    #[test]
    fn absent_core_without_enabled_task_starts_manual() {
        assert_eq!(
            plan(&Probe::Absent, TaskState::Disabled),
            Action::SpawnManual
        );
        assert_eq!(
            plan(&Probe::Absent, TaskState::Missing),
            Action::SpawnManual
        );
    }

    #[test]
    fn foreign_listener_is_treated_as_absent() {
        // 端口被别人占着不算"核心在跑"：必须走拉起分支，不能误判。
        assert_eq!(
            plan(&Probe::NotOurs, TaskState::Enabled),
            Action::TriggerTask
        );
        assert_eq!(
            plan(&Probe::NotOurs, TaskState::Missing),
            Action::SpawnManual
        );
    }

    #[test]
    fn failed_trigger_falls_back_to_stand_in_only_when_autostart_is_on() {
        assert_eq!(
            plan_after_trigger_failed(TaskState::Enabled),
            Action::SpawnStandIn,
            "自启开着却起不来 = 顶班（降级）"
        );
        assert_eq!(
            plan_after_trigger_failed(TaskState::Disabled),
            Action::SpawnManual
        );
        assert_eq!(
            plan_after_trigger_failed(TaskState::Missing),
            Action::SpawnManual
        );
    }

    // ── HTTP 解析 ──

    #[test]
    fn parses_status_and_json_body() {
        let raw = "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\n\r\n{\"status\":\"ok\",\"instance_id\":\"abc\"}";
        let (status, body) = parse_http_response(raw).expect("parse");
        assert_eq!(status, 200);
        assert!(body.contains("\"instance_id\":\"abc\""));
    }

    #[test]
    fn parses_non_200_status() {
        let raw = "HTTP/1.1 401 Unauthorized\r\ncontent-length: 2\r\n\r\n{}";
        let (status, _) = parse_http_response(raw).expect("parse");
        assert_eq!(status, 401);
    }

    #[test]
    fn rejects_garbage_that_is_not_http() {
        assert!(parse_http_response("not http at all").is_none());
        assert!(parse_http_response("HTTP/1.1 abc OK\r\n\r\n").is_none());
        assert!(parse_http_response("").is_none());
    }

    #[test]
    fn tolerates_body_without_headers_separator() {
        let (status, body) = parse_http_response("HTTP/1.1 204 No Content").expect("parse");
        assert_eq!(status, 204);
        assert!(body.is_empty());
    }

    // ── 任务 XML ──

    #[test]
    fn task_xml_detects_enabled_and_disabled() {
        assert_eq!(
            parse_task_xml("<Task><Settings><Enabled>true</Enabled></Settings></Task>"),
            TaskState::Enabled
        );
        assert_eq!(
            parse_task_xml("<Task><Settings><Enabled>false</Enabled></Settings></Task>"),
            TaskState::Disabled
        );
    }

    #[test]
    fn task_xml_without_enabled_flag_is_treated_as_enabled() {
        assert_eq!(
            parse_task_xml("<Task><Settings></Settings></Task>"),
            TaskState::Enabled
        );
    }

    #[test]
    fn empty_task_xml_means_missing() {
        assert_eq!(parse_task_xml(""), TaskState::Missing);
        assert_eq!(
            parse_task_xml("ERROR: The system cannot find"),
            TaskState::Missing
        );
    }

    // ── 退出码 ──

    #[test]
    fn exit_codes_are_stable() {
        assert_eq!(EXIT_OK, 0);
        assert_eq!(EXIT_BAD_ARGS, 2);
        assert_eq!(EXIT_CORE_UNREACHABLE, 4);
        assert_eq!(EXIT_AGENT_FAILED, 6);
    }

    // ── 探测（真实 socket，不依赖核心） ──

    #[test]
    fn probe_reports_absent_on_closed_port() {
        // 绑定后立刻释放，得到一个几乎肯定没人监听的端口。
        let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).expect("bind");
        let port = listener.local_addr().expect("addr").port();
        drop(listener);
        let probe = probe_healthz(port, "whatever", Duration::from_millis(300));
        assert_eq!(probe, Probe::Absent);
    }

    fn write_control_json(dir: &Path, port: u16, instance_id: &str) {
        let json = format!(
            r#"{{"status":"running","version":"0.1.0","instance_id":"{instance_id}","pid":123,"identity":"user:dev","run_mode":"manual","host":"127.0.0.1","port":{port},"token":"tok","started_at":"2026-10-20T00:00:00Z"}}"#
        );
        std::fs::write(dir.join("control.json"), json).expect("write control.json");
    }

    /// 起一个假的健康端点：对每个连接回一次带 `instance_id` 的 200。
    fn spawn_fake_core(instance_id: &'static str) -> u16 {
        use std::io::{Read, Write};

        let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).expect("bind fake core");
        let port = listener.local_addr().expect("addr").port();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { break };
                let mut buf = [0u8; 1024];
                let _ = stream.read(&mut buf);
                let body = format!(r#"{{"status":"ok","instance_id":"{instance_id}"}}"#);
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = stream.write_all(response.as_bytes());
            }
        });
        port
    }

    #[test]
    fn wait_until_healthy_survives_a_stale_control_file() {
        // 回归：磁盘上还留着上一个核心的契约（同端口、旧 instance_id）时，
        // 必须先判 NotOurs 再重读，直到新核心把契约改写成自己的。
        let dir = std::env::temp_dir().join(format!("threadline-stale-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("mkdir");

        let port = spawn_fake_core("real");
        write_control_json(&dir, port, "stale");

        let writer_dir = dir.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(400));
            write_control_json(&writer_dir, port, "real");
        });

        let started = std::time::Instant::now();
        assert_eq!(
            wait_until_healthy(&dir, Duration::from_secs(3)),
            Probe::Healthy,
            "契约被新核心改写后必须判为可达，而不是死盯旧快照"
        );
        assert!(
            started.elapsed() >= Duration::from_millis(300),
            "必须真的等过重写，而不是第一次就返回"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn wait_until_healthy_times_out_without_core() {
        let dir = std::env::temp_dir().join(format!("threadline-none-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("mkdir");

        let started = std::time::Instant::now();
        assert_eq!(
            wait_until_healthy(&dir, Duration::from_millis(300)),
            Probe::Absent
        );
        assert!(
            started.elapsed() >= Duration::from_millis(250),
            "超时必须等满预算"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn wait_until_healthy_reports_not_ours_when_instance_never_matches() {
        let dir = std::env::temp_dir().join(format!("threadline-notours-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("mkdir");

        let port = spawn_fake_core("someone-else");
        write_control_json(&dir, port, "expected");

        assert_eq!(
            wait_until_healthy(&dir, Duration::from_millis(400)),
            Probe::NotOurs,
            "端口活着但不是我们的核心，必须如实报 NotOurs"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn read_control_returns_none_for_missing_or_broken_file() {
        let dir = std::env::temp_dir().join(format!("threadline-wake-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("mkdir");
        assert!(read_control(&dir).is_none(), "文件不存在应为 None");

        std::fs::write(dir.join("control.json"), "{ not json").expect("write");
        assert!(read_control(&dir).is_none(), "解析失败应为 None");

        let _ = std::fs::remove_dir_all(&dir);
    }
}
