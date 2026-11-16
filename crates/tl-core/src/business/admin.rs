//! 服务器管理的业务编排。
//!
//! 归属：**业务层**。只做"按顺序调度"：确认隧道存在 → 借它的会话 → 执行只读采集脚本 →
//! 交给 `server_admin` 模型解析 → 组装成契约里的概览。
//! **不含判定规则**：脚本内容、指标口径、模块是否可用全部由 `server_admin` 模型回答。
//!
//! 允许调用：`models::{server, server_admin, tunnel}`、`foundation`。

use std::sync::Arc;
use std::time::Instant;

use anyhow::{anyhow, Result};
use tl_protocol::ServerOverview;

use crate::business::tunnel_runner::TunnelRunner;
use crate::foundation::clock;
use crate::models::server::ServerStore;
use crate::models::server_admin::{self, Probe, EXEC_BUDGET};
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
    if !servers.list()?.items.iter().any(|s| s.id == server_id) {
        return Err(anyhow!("隧道不存在：{server_id}"));
    }

    let started = Instant::now();
    let session: SessionHandle = runner
        .session_handle(server_id)
        .await
        .map_err(|e| anyhow!(e.to_string()))?;

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
