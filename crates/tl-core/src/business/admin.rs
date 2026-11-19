//! 服务器管理的业务编排。
//!
//! 归属：**业务层**。只做"按顺序调度"：确认隧道存在 → 借它的会话 → 执行只读采集脚本 →
//! 交给 `server_admin` 模型解析 → 组装成契约里的概览。
//! **不含判定规则**：脚本内容、指标口径、模块是否可用全部由 `server_admin` 模型回答。
//!
//! 允许调用：`models::{server, server_admin, tunnel}`、`foundation`。

use std::collections::HashSet;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use anyhow::{anyhow, Result};
use tl_protocol::{
    ContainerAction, ContainerActionResult, ContainerDetail, ContainerList, ContainerLogs,
    ContainerStats, ServerOverview,
};

use crate::business::tunnel_runner::TunnelRunner;
use crate::foundation::clock;
use crate::models::server::ServerStore;
use crate::models::server_admin::{
    self, ConsoleMode, Probe, ACTION_BUDGET, EXEC_BUDGET, STATS_BUDGET,
};
use crate::models::tunnel::engine;
use crate::models::tunnel::engine::SessionHandle;

/// 取 stderr 的第一行：远端脚本报错时整段堆栈没有意义，一行就够定位。
fn first_line(text: &str) -> String {
    text.lines()
        .find(|line| !line.trim().is_empty())
        .unwrap_or("没有更多信息")
        .trim()
        .to_string()
}

/// 采集一次概览。
///
/// 按 `probe` 只采要的那几块：界面里 CPU/内存/流量每秒、磁盘 IO 每 5 秒、
/// 磁盘容量每 10 秒，各自独立请求，互不拖累。
pub async fn overview(
    servers: &ServerStore,
    runner: &Arc<TunnelRunner>,
    server_id: &str,
    probe: Probe,
) -> Result<ServerOverview> {
    // 先确认隧道存在：不存在的 id 会被 `ensure_session` 报成"连不上"，把错误藏起来。
    let started = Instant::now();
    let session: SessionHandle = admin_session(servers, runner, server_id).await?;

    let script = server_admin::overview_script(probe);
    let capture = engine::exec_capture(&session, &script, EXEC_BUDGET)
        .await
        .map_err(|e| anyhow!("{e}"))?;

    // 脚本没吐出任何东西 + 非零退出码：这时 stderr 才是唯一线索。
    if capture.stdout.trim().is_empty() {
        if let Some(code) = capture.code {
            if code != 0 {
                return Err(anyhow!(
                    "采集脚本执行失败（退出码 {code}）：{}",
                    first_line(&capture.stderr)
                ));
            }
        }
    }

    // 按需采集时缺的块是**预期**的（这一次根本没要），不能记成降级。
    let parsed = server_admin::interpret_with(
        &capture.stdout,
        if probe == Probe::Full {
            server_admin::Extras::Full
        } else {
            server_admin::Extras::Lite
        },
    );

    let mut overview = ServerOverview {
        collected_at: clock::now_iso8601(),
        elapsed_ms: started.elapsed().as_millis() as u64,
        system: parsed.system,
        cpu: parsed.cpu,
        load: parsed.load,
        memory: parsed.memory,
        mem_breakdown: parsed.mem_breakdown,
        gpus: parsed.gpus,
        disks: parsed.disks,
        block_disks: parsed.block_disks,
        zfs_pools: parsed.zfs_pools,
        network: parsed.network,
        disk_io: parsed.disk_io,
        processes: parsed.processes,
        containers: parsed.containers.clone(),
        modules: server_admin::modules(&parsed.containers),
        degraded: parsed.degraded,
    };

    // 只回本次采到的块：其余留空，前端按 probe 合并，不会把已有数据冲掉。
    match probe {
        Probe::Full => {}
        Probe::Live => {
            overview.disks = Vec::new();
            overview.block_disks = Vec::new();
            overview.zfs_pools = Vec::new();
            overview.processes = Vec::new();
            overview.containers = Default::default();
            overview.modules = Vec::new();
            overview.degraded = Vec::new();
        }
        Probe::DiskIo => {
            overview.system = Default::default();
            overview.cpu = Default::default();
            overview.load = Default::default();
            overview.memory = Default::default();
            overview.gpus = Vec::new();
            overview.disks = Vec::new();
            overview.block_disks = Vec::new();
            overview.zfs_pools = Vec::new();
            overview.network = Vec::new();
            overview.processes = Vec::new();
            overview.containers = Default::default();
            overview.modules = Vec::new();
            overview.degraded = Vec::new();
        }
        Probe::Disks => {
            overview.system = Default::default();
            overview.cpu = Default::default();
            overview.load = Default::default();
            overview.memory = Default::default();
            overview.gpus = Vec::new();
            overview.network = Vec::new();
            overview.disk_io = Vec::new();
            overview.processes = Vec::new();
            overview.containers = Default::default();
            overview.modules = Vec::new();
            overview.degraded = Vec::new();
        }
    }

    Ok(overview)
}

// ─────────────────────── 容器管理 ───────────────────────

/// 管理页请求的公共前缀（概览 / 容器 / 终端共用）：确认隧道存在 → 取它的会话。
///
/// 不存在的 id 会被 `ensure_session` 报成「连不上」，把真正的错误藏起来。
async fn admin_session(
    servers: &ServerStore,
    runner: &Arc<TunnelRunner>,
    server_id: &str,
) -> Result<SessionHandle> {
    if !servers.list()?.items.iter().any(|s| s.id == server_id) {
        return Err(anyhow!("隧道不存在：{server_id}"));
    }
    runner
        .session_handle(server_id)
        .await
        .map_err(|e| anyhow!(e.to_string()))
}

/// 在会话上跑一段脚本。
async fn container_exec(
    session: &SessionHandle,
    script: String,
    budget: Duration,
) -> Result<engine::ExecCapture> {
    engine::exec_capture(session, &script, budget)
        .await
        .map_err(|e| anyhow!("{e}"))
}

/// 取某个 `@@section` 的原始文本（多行用 `\n` 连）。
fn section_text(raw: &str, name: &str) -> String {
    server_admin::split_sections(raw)
        .get(name)
        .map(|lines| lines.join("\n"))
        .unwrap_or_default()
}

/// 动作名给人看。
fn action_label(action: ContainerAction) -> &'static str {
    match action {
        ContainerAction::Start => "启动",
        ContainerAction::Stop => "停止",
        ContainerAction::Restart => "重启",
    }
}

/// 列表：`ps -a` + 批量 inspect，一次往返。
pub async fn containers(
    servers: &ServerStore,
    runner: &Arc<TunnelRunner>,
    server_id: &str,
) -> Result<ContainerList> {
    let started = Instant::now();
    let session = admin_session(servers, runner, server_id).await?;
    let capture = container_exec(&session, server_admin::list_script(), EXEC_BUDGET).await?;

    let mut list = server_admin::parse_list(&capture.stdout);
    // `ps` 给不了容器 IP 与卷，inspect 那一段补上；拿不到就只是少两列，不算失败。
    server_admin::merge_inspect(&mut list, &capture.stdout);
    list.collected_at = clock::now_iso8601();
    list.elapsed_ms = started.elapsed().as_millis() as u64;
    Ok(list)
}

/// 资源：`stats --no-stream`，前端按 3 秒调，列表本身不带它。
pub async fn container_stats(
    servers: &ServerStore,
    runner: &Arc<TunnelRunner>,
    server_id: &str,
) -> Result<Vec<ContainerStats>> {
    let session = admin_session(servers, runner, server_id).await?;
    let capture = container_exec(&session, server_admin::stats_script(), STATS_BUDGET).await?;
    Ok(server_admin::parse_stats(&capture.stdout))
}

/// 详情：只读，白名单校验即可（不必为了看一眼详情再跑一次 `ps`）。
pub async fn container_detail(
    servers: &ServerStore,
    runner: &Arc<TunnelRunner>,
    server_id: &str,
    cid: &str,
) -> Result<ContainerDetail> {
    server_admin::validate_cid(cid).map_err(|e| anyhow!("{e}"))?;

    let session = admin_session(servers, runner, server_id).await?;
    let script = server_admin::detail_script(cid).map_err(|e| anyhow!("{e}"))?;
    let capture = container_exec(&session, script, EXEC_BUDGET).await?;

    let Some(parts) = server_admin::parse_detail(&capture.stdout) else {
        return Err(anyhow!("容器不存在：{cid}"));
    };

    let mut item = ContainerDetail {
        command: parts.command,
        restart_policy: parts.restart_policy,
        env: parts.env,
        ..Default::default()
    };
    item.item.exit_code = parts.exit_code;
    item.item.restarts = parts.restarts;
    Ok(item)
}

/// 日志：跟随会 2 秒一次，只做白名单校验，不额外跑 `ps`。
pub async fn container_logs(
    servers: &ServerStore,
    runner: &Arc<TunnelRunner>,
    server_id: &str,
    cid: &str,
    tail: usize,
    since: Option<&str>,
) -> Result<ContainerLogs> {
    let session = admin_session(servers, runner, server_id).await?;
    let script = server_admin::logs_script(cid, tail, since).map_err(|e| anyhow!("{e}"))?;
    let capture = container_exec(&session, script, EXEC_BUDGET).await?;
    Ok(server_admin::parse_logs(&capture.stdout, tail))
}

/// 生命周期动作。
///
/// 写操作必须走**清单校验**：`cid` 得是本次真实采到的容器，不能凭一个名字就动手。
pub async fn container_action(
    servers: &ServerStore,
    runner: &Arc<TunnelRunner>,
    server_id: &str,
    cid: &str,
    action: ContainerAction,
) -> Result<ContainerActionResult> {
    let list = containers(servers, runner, server_id).await?;
    let name = server_admin::resolve_cid(&list, cid).map_err(|e| anyhow!("{e}"))?;
    let item = list
        .items
        .iter()
        .find(|i| i.name == name)
        .ok_or_else(|| anyhow!("容器不存在：{cid}"))?;

    // 菜单不是安全边界：状态允许哪些动作，由模型再判一次。
    if !server_admin::allowed_actions(&item.state).contains(&action) {
        return Err(anyhow!(
            "容器当前状态（{}）不允许{}",
            item.state,
            action_label(action)
        ));
    }

    let session = admin_session(servers, runner, server_id).await?;
    let script = server_admin::action_script(action, &name).map_err(|e| anyhow!("{e}"))?;
    let capture = container_exec(&session, script, ACTION_BUDGET).await?;

    // 不拿命令退出码当成功依据：脚本里回读了一次 `.State.Status`。
    let state = section_text(&capture.stdout, "c_state").trim().to_string();
    if state.is_empty() {
        return Err(anyhow!(
            "{}失败：{}",
            action_label(action),
            first_line(&capture.stderr)
        ));
    }

    Ok(ContainerActionResult {
        ok: true,
        message: format!("已{}容器 {name}", action_label(action)),
        state,
        image_removed: false,
    })
}

/// 删除容器（可选一并删镜像）。
///
/// 顺序是**先删容器、再处理镜像**：
///
/// 1. `docker rm [-f]`，失败就到此为止（容器还在，谈不上删镜像）；
/// 2. 勾了删镜像，再查这个 tag 还有谁用——此刻它自己已不在 `docker ps -a` 里，
///    不必再做「排除自己」那层手工逻辑；
/// 3. 还有人就**不删**，回执列出是谁；
/// 4. 没人了才 `docker rmi`。
///
/// 为什么不靠 `docker rmi` 自己报错：它删的是 tag，多个容器共用一个 tag 时会把
/// tag 删掉并返回成功，别人的「镜像」随即变成悬空引用。真机验收时撞到过。
pub async fn container_remove(
    servers: &ServerStore,
    runner: &Arc<TunnelRunner>,
    server_id: &str,
    cid: &str,
    force: bool,
    remove_image: bool,
) -> Result<ContainerActionResult> {
    let list = containers(servers, runner, server_id).await?;
    let name = server_admin::resolve_cid(&list, cid).map_err(|e| anyhow!("{e}"))?;
    let item = list
        .items
        .iter()
        .find(|i| i.name == name)
        .ok_or_else(|| anyhow!("容器不存在：{cid}"))?;

    if item.state == "running" && !force {
        return Err(anyhow!("容器正在运行，请先停止或使用强制删除"));
    }

    let image = remove_image.then(|| item.image.clone());
    let session = admin_session(servers, runner, server_id).await?;

    let script = server_admin::remove_script(&name, force).map_err(|e| anyhow!("{e}"))?;
    let capture = container_exec(&session, script, ACTION_BUDGET).await?;

    // 脚本里 `2>&1` 把 stderr 合进 stdout，成功时输出是容器名，失败时带 Error。
    let removed = section_text(&capture.stdout, "c_remove");
    if removed.to_lowercase().contains("error") || removed.trim().is_empty() {
        return Err(anyhow!("删除容器失败：{}", first_line(&removed)));
    }

    let mut message = format!("已删除容器 {name}");
    let mut image_removed = false;
    if let Some(image) = image {
        // 容器已经没了，这时看到的占用才是真的「别人还在用」。
        let users =
            container_exec(&session, server_admin::image_users_script(), EXEC_BUDGET).await?;
        let shared_with = server_admin::parse_image_users(&users.stdout, &image);

        if !shared_with.is_empty() {
            message.push_str(&format!(
                "，镜像 {image} 仍被 {} 个容器使用（{}），未删除",
                shared_with.len(),
                shared_with.join("、")
            ));
        } else {
            let rmi_script = server_admin::rmi_script(&image).map_err(|e| anyhow!("{e}"))?;
            let rmi = container_exec(&session, rmi_script, ACTION_BUDGET).await?;
            let text = section_text(&rmi.stdout, "c_rmi");
            let lower = text.to_lowercase();
            if lower.contains("no such image") {
                // tag 之前就被删过：容器照样删掉了，不该报成失败
                message.push_str(&format!("，镜像 {image} 已不存在"));
            } else if lower.contains("error") {
                message.push_str(&format!("，镜像 {image} 未删除：{}", first_line(&text)));
            } else {
                image_removed = true;
                message.push_str(&format!("，镜像 {image} 已删除"));
            }
        }
    }

    Ok(ContainerActionResult {
        ok: true,
        message,
        state: "removed".to_string(),
        image_removed,
    })
}

// ─────────────────────── 控制台（PTY） ───────────────────────

/// 正在开着的控制台会话（`server_id:容器名`）。
///
/// 两个人同时 `docker exec` 进同一个容器，输入会互相串到对方的终端上
/// （共用一份 TTY 输出），看起来像「键盘失灵」。用一张表挡住，
/// 换一个容器仍然可以并行开。
fn console_registry() -> &'static Mutex<HashSet<String>> {
    static REGISTRY: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(HashSet::new()))
}

/// 控制台占用凭据：连接断开（或任务 panic）时自动从表里摘掉。
pub struct ConsoleGuard {
    key: String,
}

impl Drop for ConsoleGuard {
    fn drop(&mut self) {
        if let Ok(mut set) = console_registry().lock() {
            set.remove(&self.key);
        }
    }
}

/// 开一条控制台。
///
/// 顺序有讲究：先核对容器存在且在运行，再占位，最后才开会话——
/// 反过来会出现「占着位但连不上」，下次进来还得等它超时。
pub async fn container_console(
    servers: &ServerStore,
    runner: &Arc<TunnelRunner>,
    server_id: &str,
    cid: &str,
    mode: ConsoleMode,
    cols: u32,
    rows: u32,
) -> Result<(engine::PtyChannel, String, ConsoleGuard)> {
    let list = containers(servers, runner, server_id).await?;
    let name = server_admin::resolve_cid(&list, cid).map_err(|e| anyhow!("{e}"))?;
    let item = list
        .items
        .iter()
        .find(|i| i.name == name)
        .ok_or_else(|| anyhow!("容器不存在：{cid}"))?;

    if item.state != "running" {
        return Err(anyhow!(
            "容器当前状态是 {}，只有运行中的容器能进控制台",
            item.state
        ));
    }

    let key = format!("{server_id}:{name}");
    {
        let mut set = console_registry()
            .lock()
            .map_err(|_| anyhow!("控制台注册表不可用"))?;
        if !set.insert(key.clone()) {
            return Err(anyhow!("这个容器已经有一条控制台会话了"));
        }
    }
    let guard = ConsoleGuard { key };

    let command = server_admin::console_command(mode, &name).map_err(|e| anyhow!("{e}"))?;
    let session = admin_session(servers, runner, server_id).await?;
    let pty = engine::open_pty(&session, &command, cols, rows)
        .await
        .map_err(|e| anyhow!("{e}"))?;

    Ok((pty, command, guard))
}

// ─────────────────────── 服务器终端 ───────────────────────

/// 开一条服务器登录 shell。
///
/// 不占占用表：SSH shell 各自独立 PTY，两条会话不会串输入
/// （容器控制台要互斥，是因为 `docker exec` 共享同一个容器 TTY）。
pub async fn server_terminal(
    servers: &ServerStore,
    runner: &Arc<TunnelRunner>,
    server_id: &str,
    cols: u32,
    rows: u32,
) -> Result<(engine::PtyChannel, String)> {
    let session = admin_session(servers, runner, server_id).await?;
    let command = server_admin::login_shell_command();
    let pty = engine::open_pty(&session, &command, cols, rows)
        .await
        .map_err(|e| anyhow!("{e}"))?;
    Ok((pty, command))
}
