//! 业务层：用例编排。
//!
//! 归属：**业务层**。只做"按顺序调度几个模型完成一个动作"，**不藏判定规则**——
//! 「能不能启动」「什么时候进 Error」「要不要提权」一律由模型回答。
//! 允许调用：`models`（各模型的公开操作）、`foundation`。

pub mod gateway;
pub mod hosts_sync;
pub mod profile;
pub mod tunnel_runner;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{anyhow, Result};
use tl_protocol::{
    AutostartStatus, CreateCredentialRequest, CreateServerRequest, CreateTunnelRequest,
    CredentialKind, PatchServerRequest, PatchTunnelRequest, ProbeDraftRequest, TunnelStatus,
    WhoamiResponse,
};
use tokio::sync::broadcast;

use crate::foundation::tls_ca::GatewayCa;
use crate::models::credential::CredentialStore;
use crate::models::runtime::{AutostartError, RuntimeStore};
use crate::models::server::ServerStore;
use crate::models::tunnel::TunnelStore;

pub use gateway::GatewayService;
pub use hosts_sync::{HostsSyncService, SyncOutcome};
pub use tunnel_runner::TunnelRunner;

/// 组合根装配出来的服务集合（由壳层创建一次，注入给接口层）。
pub struct Services {
    pub credentials: Arc<CredentialStore>,
    pub servers: Arc<ServerStore>,
    pub tunnels: Arc<TunnelStore>,
    pub runtime: Arc<RuntimeStore>,
    pub runner: Arc<TunnelRunner>,
    pub hosts: Arc<HostsSyncService>,
    pub gateway: Arc<GatewayService>,
    /// `/events` 的唯一来源：隧道变更、hosts 同步结果都从这里敲。
    changes: broadcast::Sender<()>,
}

/// 隧道自建登录材料的内部命名（用户看不到）。
fn inline_credential_name(server_name: &str) -> String {
    format!("__inline_{server_name}")
}

/// 编辑连接时对登录材料的处理方式。
///
/// 抽成纯函数是为了能单独测：这里判错会「每次改端口都重建凭据」，
/// 私钥口令会被静默丢掉。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CredentialChoice {
    /// 给了新的密码/私钥：建新材料并切过去
    Rebuild,
    /// 没给新密钥，但已有材料：沿用原密钥（只改用户名）
    Reuse,
    /// 待填凭据且没给密钥：只更新字段，不碰材料
    Skip,
}

fn credential_choice(has_new_secret: bool, credential_id: &str) -> CredentialChoice {
    if has_new_secret {
        CredentialChoice::Rebuild
    } else if credential_id.trim().is_empty() {
        CredentialChoice::Skip
    } else {
        CredentialChoice::Reuse
    }
}

impl Services {
    /// 试连一次：回答"这条隧道配置真的连得上吗"。
    ///
    /// 只做 TCP → 握手 → 指纹比对 → 认证，**不绑端口、不转发**。
    /// 代价是会在服务器上留下一条 SSH 登录记录（认证阶段），这是诊断必须付出的。
    pub async fn probe_server(
        &self,
        server_id: &str,
    ) -> Result<crate::models::tunnel::engine::ProbeReport> {
        use crate::models::tunnel::engine::{self, ServeConfig};
        use std::time::Duration;

        let params = self.servers.connect_params(server_id)?;
        let material = self.credentials.material(&params.credential_id)?;
        let login = crate::business::tunnel_runner::to_login(material);

        let config = ServeConfig {
            server_host: params.host.clone(),
            server_port: params.port,
            // 探测不转发，这两个字段只是结构要求。
            remote_host: "127.0.0.1".to_string(),
            remote_port: 0,
            trusted_fingerprint: params.trusted_fingerprint.clone(),
            keepalive_secs: params.keepalive_secs as u64,
        };

        let report = engine::probe(&config, login, Duration::from_secs(8)).await;

        // 握手观测到的指纹只作为"待确认"记下（不信任）。
        // 这样用户测试连接后就能在列表里直接确认指纹，不必先启动一条转发。
        if let Some(fingerprint) = &report.fingerprint {
            let _ = self.servers.observe_host_key(
                server_id,
                fingerprint,
                &crate::foundation::clock::now_iso8601(),
            );
        }

        Ok(report)
    }

    /// 用还没保存的表单内容试连一次。
    ///
    /// 新建连接时用户还没点「保存」，但「测试」不该因此失效。凭据只在本次调用里
    /// 使用：不落库、不写日志、不留待确认指纹（那属于已保存的连接）。
    pub async fn probe_draft(
        &self,
        draft: &ProbeDraftRequest,
    ) -> Result<crate::models::tunnel::engine::ProbeReport> {
        use crate::models::credential::CredentialMaterial;
        use crate::models::tunnel::engine::{self, ServeConfig};
        use std::time::Duration;

        if draft.host.trim().is_empty() {
            anyhow::bail!("先填主机地址");
        }
        if draft.secret.trim().is_empty() {
            anyhow::bail!("先填密码，或粘贴私钥");
        }

        let login = crate::business::tunnel_runner::to_login(CredentialMaterial {
            kind: draft.credential_kind,
            username: draft.username.clone(),
            secret: draft.secret.clone(),
            passphrase: draft.passphrase.clone(),
        });

        let config = ServeConfig {
            server_host: draft.host.trim().to_string(),
            server_port: draft.port,
            // 探测不转发，这两个字段只是结构要求。
            remote_host: "127.0.0.1".to_string(),
            remote_port: 0,
            trusted_fingerprint: draft.host_key_fingerprint.clone(),
            keepalive_secs: 30,
        };

        Ok(engine::probe(&config, login, Duration::from_secs(8)).await)
    }

    pub fn new(
        credentials: Arc<CredentialStore>,
        servers: Arc<ServerStore>,
        tunnels: Arc<TunnelStore>,
        runtime: Arc<RuntimeStore>,
        data_dir: impl Into<PathBuf>,
    ) -> Arc<Self> {
        let data_dir = data_dir.into();
        // ② 的根 CA 在启动时加载一次（同步 IO 几十毫秒）；失败只降级 ②，不影响其它转发。
        let gateway = match GatewayCa::load_or_create(data_dir.join("gateway-ca")) {
            Ok(ca) => Some(Arc::new(ca)),
            Err(e) => {
                tracing::warn!("本地网关证书初始化失败（② 将不可用）：{e}");
                None
            }
        };
        let gateway_service = GatewayService::new(gateway.clone(), &data_dir);
        let runner = Arc::new(TunnelRunner::new(
            Arc::clone(&tunnels),
            Arc::clone(&servers),
            Arc::clone(&credentials),
            gateway,
        ));
        let hosts = HostsSyncService::new(Arc::clone(&tunnels), &data_dir);
        let (changes, _) = broadcast::channel(64);
        Arc::new(Self {
            credentials,
            servers,
            tunnels,
            runtime,
            runner,
            hosts,
            gateway: Arc::new(gateway_service),
            changes,
        })
    }

    /// 订阅「任一可观察状态变了」。
    pub fn subscribe(&self) -> broadcast::Receiver<()> {
        self.changes.subscribe()
    }

    pub fn notify_changed(&self) {
        let _ = self.changes.send(());
    }

    /// 进程启动后的编排：补齐运行档位带来的降级标记，再拉起 `auto_start` 的隧道。
    pub async fn on_startup(self: &Arc<Self>) -> Vec<(String, String)> {
        self.runtime.sync_run_mode_degradations();

        // 隧道模型自己的广播转发到 Services 级：`/events` 只订阅一个流，
        // 否则前端要开两条 SSE 才能同时感知隧道与 hosts 的变化。
        let mut tunnel_changes = self.tunnels.subscribe();
        let changes = self.changes.clone();
        tokio::spawn(async move {
            // Lagged 也要转发：漏事件时让界面重拉一次，自动对齐；
            // 收到 Closed（发送端全部释放）时自然退出。
            while let Ok(()) | Err(broadcast::error::RecvError::Lagged(_)) =
                tunnel_changes.recv().await
            {
                let _ = changes.send(());
            }
        });

        // 启动自检：上次异常退出可能留下已删除域名的旧条目。
        // 能写就静默修，不能写只标记，不在这里弹 UAC。
        self.hosts.startup_check();
        self.notify_changed();

        self.runner.startup_autostart().await
    }

    /// 按当前配置重写 hosts 托管区块（接口层「写入」按钮）。
    pub fn sync_hosts(&self) -> Result<SyncOutcome> {
        let outcome = self.hosts.sync()?;
        self.notify_changed();
        Ok(outcome)
    }

    /// 清空托管区块。
    pub fn clear_hosts(&self) -> Result<SyncOutcome> {
        let outcome = self.hosts.clear()?;
        self.notify_changed();
        Ok(outcome)
    }

    /// 配置变了就顺手同步一次；失败只记录状态，不阻塞保存动作。
    fn sync_hosts_quietly(&self) {
        let _ = self.hosts.sync();
        self.notify_changed();
    }

    /// 优雅退出：停全部隧道 → 写发现契约的 stopped 标记。
    ///
    /// 顺序不可颠倒：先停数据面，再宣布停止，否则 Wake 可能对着还在服务的端口判"已停"。
    pub async fn shutdown(&self) -> Result<()> {
        self.runner.stop_all().await?;
        self.runtime.mark_stopped()?;
        Ok(())
    }

    pub fn whoami(&self) -> Result<WhoamiResponse, crate::foundation::registry::RegistryError> {
        self.runtime.whoami()
    }

    pub fn autostart_status(
        &self,
    ) -> Result<AutostartStatus, crate::foundation::registry::RegistryError> {
        self.runtime.autostart_status()
    }

    /// 改开机自启：只动注册层，**不停正在跑的隧道**（I5）。
    pub fn set_autostart(
        &self,
        enabled: bool,
        exe_path: &Path,
        log_dir: &Path,
    ) -> Result<AutostartStatus, AutostartError> {
        self.runtime.set_autostart(enabled, exe_path, log_dir)
    }

    /// 启动单条隧道（接口层调用）。
    /// 创建隧道时**顺带创建它的登录材料**。
    ///
    /// 界面上没有「凭据」这一层：用户填用户名 + 密码/私钥，隧道自己持有。
    /// 内部仍用加密存储（DPAPI），只是不再对外暴露为独立实体。
    /// 重排隧道显示顺序。只影响列表顺序，不动运行态。
    pub fn reorder_servers(&self, ids: &[String]) -> Result<()> {
        self.servers.reorder(ids)
    }

    pub fn create_server(&self, mut req: CreateServerRequest) -> Result<tl_protocol::ServerDto> {
        let mut created: Option<String> = None;
        if req.credential_id.trim().is_empty() {
            let secret = req
                .secret
                .take()
                .filter(|s| !s.trim().is_empty())
                .ok_or_else(|| anyhow!("请填写密码或私钥"))?;
            let username = req.username.trim().to_string();
            if username.is_empty() {
                return Err(anyhow!("登录用户名不能为空"));
            }
            let summary = self.credentials.add(CreateCredentialRequest {
                name: inline_credential_name(&req.name),
                kind: req.kind.unwrap_or(CredentialKind::Password),
                username,
                secret,
                passphrase: req.passphrase.take(),
            })?;
            created = Some(summary.id.clone());
            req.credential_id = summary.id;
        }

        match self.servers.create(req) {
            Ok(dto) => Ok(dto),
            Err(e) => {
                // 隧道没建成，别留下一条孤儿登录材料。
                if let Some(id) = created {
                    let _ = self.credentials.remove(&id);
                }
                Err(e)
            }
        }
    }

    /// 编辑隧道。改用户名或换密钥时**重建登录材料**并切过去。
    pub fn patch_server(
        &self,
        id: &str,
        mut req: PatchServerRequest,
    ) -> Result<tl_protocol::ServerDto> {
        let username = req.username.take();
        let kind = req.kind.take();
        let secret = req.secret.take();
        let passphrase = req.passphrase.take();

        if username.is_some() || secret.is_some() {
            let current = self
                .servers
                .list()?
                .items
                .into_iter()
                .find(|s| s.id == id)
                .ok_or_else(|| anyhow!("服务器不存在：{id}"))?;
            let has_new_secret = secret
                .as_deref()
                .map(|s| !s.trim().is_empty())
                .unwrap_or(false);

            // 待填凭据（导入的连接还没有密码）且这次没给新密钥：只更新字段。
            // 既不能去读原材料（credential_id 是空的），也不能凭空造一条空凭据。
            // username/kind 已被 take()，必须放回去，否则改名会静默失效。
            if credential_choice(has_new_secret, &current.credential_id) == CredentialChoice::Skip {
                req.username = username;
                req.kind = kind;
                return self.servers.patch(id, req);
            }

            let login_user = username.unwrap_or_else(|| current.username.clone());
            let material = if has_new_secret {
                secret.unwrap_or_default()
            } else {
                // 只改用户名：沿用原密钥。
                self.credentials.material(&current.credential_id)?.secret
            };
            if login_user.trim().is_empty() {
                return Err(anyhow!("登录用户名不能为空"));
            }

            let summary = self.credentials.add(CreateCredentialRequest {
                name: inline_credential_name(&current.name),
                kind: kind.unwrap_or(CredentialKind::Password),
                username: login_user.clone(),
                secret: material,
                passphrase,
            })?;
            req.credential_id = Some(summary.id.clone());
            req.username = Some(login_user);

            let result = self.servers.patch(id, req);
            // 换成新登录材料后，把旧的清掉（名称带内部前缀的才是我们建的）。
            if result.is_ok() && current.credential_id.starts_with("cred_") {
                let _ = self.credentials.remove(&current.credential_id);
            }
            return result;
        }

        self.servers.patch(id, req)
    }

    /// 该隧道的 SSH 会话是否连着（界面显示"已连接/未连接"）。
    pub fn session_connected(&self, server_id: &str) -> bool {
        self.runner.session_connected(server_id)
    }

    /// 确认主机密钥之后：如果这条隧道是「启用」的，把会话与端口重新拉起来。
    pub async fn after_host_key_accepted(self: &Arc<Self>, server_id: &str) -> Result<()> {
        let enabled = self
            .servers
            .list()?
            .items
            .into_iter()
            .any(|s| s.id == server_id && s.enabled);
        if !enabled {
            return Ok(());
        }
        self.runner.ensure_keeper(server_id);
        let _ = self.runner.connect_session(server_id).await;
        for view in self.tunnels.list()?.items {
            if view.definition.server_id == server_id && view.definition.auto_start {
                let _ = self.start_tunnel(&view.definition.id).await;
            }
        }
        Ok(())
    }

    /// 打开/关闭一条隧道的「启用」。
    ///
    /// 与端口转发同一套语义：开关只表示**用户意图**（持久化），
    /// 连不上不会把开关弹回去；打开时顺带恢复该隧道下"启用"的转发。
    pub async fn set_server_enabled(
        self: &Arc<Self>,
        server_id: &str,
        enabled: bool,
    ) -> Result<usize> {
        self.servers.set_enabled(server_id, enabled)?;

        if !enabled {
            self.runner.stop_keeper(server_id);
            return self.runner.disconnect_session(server_id).await;
        }

        // 先把守护挂上，再试第一次连接：第一次失败（网络抖动 / 本机策略拦截）
        // 也会继续按退避重试，而不是停在"启用着却一直未连接"。
        self.runner.ensure_keeper(server_id);
        self.runner.connect_session(server_id).await?;

        // 会话通了：把这条隧道下"启用"的端口自动恢复。
        let mut started = 0;
        for view in self.tunnels.list()?.items {
            if view.definition.server_id != server_id || !view.definition.auto_start {
                continue;
            }
            if self.start_tunnel(&view.definition.id).await.is_ok() {
                started += 1;
            }
        }
        Ok(started)
    }

    /// 断开某条隧道的 SSH 会话：挂在上面的所有端口一起关。
    pub async fn disconnect_session(&self, server_id: &str) -> Result<usize> {
        self.runner.disconnect_session(server_id).await
    }

    /// 新建端口转发。
    ///
    /// 级联规则：这条转发是「启用」的，就顺手把它连上——用户不需要再点一次开关。
    /// 隧道没启用时 `start` 会自己写清原因，不会静默失败。
    pub async fn create_tunnel(self: &Arc<Self>, req: CreateTunnelRequest) -> Result<TunnelStatus> {
        let auto_start = req.auto_start;
        let status = self.tunnels.create(req)?;
        self.sync_hosts_quietly();
        if auto_start {
            let _ = self.start_tunnel(&status.id).await;
        }
        Ok(status)
    }

    /// 修改端口转发。
    ///
    /// 之前只改模型、不动运行态：改一个正在跑的转发，监听还按老配置在跑，状态却回到 Stopped
    /// （又一个"界面和实际不一致"）。现在按"先停 → 改 → 需要就重启"的状态机走。
    pub async fn patch_tunnel(
        self: &Arc<Self>,
        id: &str,
        req: PatchTunnelRequest,
    ) -> Result<TunnelStatus> {
        let was_running = self.runner.is_running(id);
        if was_running {
            let _ = self.runner.stop(id).await;
        }

        if let Err(e) = self.tunnels.patch(id, req) {
            // 改失败（端口冲突之类）：把原来那条按旧配置接回去，别让用户白丢一条连接。
            if was_running {
                let _ = self.start_tunnel(id).await;
            }
            return Err(e);
        }

        self.sync_hosts_quietly();
        let view = self.tunnels.view(id)?;
        if view.definition.auto_start {
            let _ = self.start_tunnel(id).await;
        }
        Ok(view.status)
    }

    /// 删除端口转发：停监听 → 删配置 → 同步 hosts（域名从区块里移除）。
    pub async fn remove_tunnel(&self, id: &str) -> Result<()> {
        let _ = self.runner.stop(id).await;
        self.tunnels.remove(id)?;
        self.sync_hosts_quietly();
        Ok(())
    }

    /// 删除连接前把运行时收拾干净：停守护、断开会话。
    ///
    /// 只删配置会留下孤儿 SSH 会话：界面顶部继续显示「已连接」、流量还在跳，
    /// 而库里这条连接已经没了（或导入后被同名新连接顶替，凭据是空的）。
    /// 之后再「启动」转发也解释不通——会话在、`enabled` 却是 false。
    pub async fn forget_server(&self, server_id: &str) -> Result<()> {
        self.runner.stop_keeper(server_id);
        self.runner.disconnect_session(server_id).await?;
        Ok(())
    }

    /// 删掉一条连接下的全部转发。
    ///
    /// 界面删连接时曾经是「前端 for 循环逐条删」：40 条转发就是 40 次请求、
    /// 40 次文件写、40 次 hosts 同步。这里一次做完，只写一次文件、同步一次。
    pub async fn remove_forwards_of_server(&self, server_id: &str) -> Result<usize> {
        let ids: Vec<String> = self
            .tunnels
            .list()?
            .items
            .into_iter()
            .filter(|view| view.definition.server_id == server_id)
            .map(|view| view.definition.id)
            .collect();
        for id in &ids {
            let _ = self.runner.stop(id).await;
        }
        let removed = self.tunnels.remove_for_server(server_id)?;
        if !removed.is_empty() {
            self.sync_hosts_quietly();
        }
        Ok(removed.len())
    }

    pub async fn start_tunnel(self: &Arc<Self>, id: &str) -> Result<()> {
        self.runner.start(id).await
    }

    pub async fn stop_tunnel(&self, id: &str) -> Result<()> {
        self.runner.stop(id).await
    }

    /// 导入预判：只读，不改任何配置。
    pub fn profile_plan(
        &self,
        req: tl_protocol::ProfilePlanRequest,
    ) -> Result<tl_protocol::ProfilePlan> {
        profile::plan(&self.servers, &self.tunnels, req)
    }

    /// 按用户确认的结果逐条落地。
    ///
    /// 含域名的条目需要重新写 hosts：这里尽力同步一次，失败只留待提权标记，
    /// 不阻断导入（否则用户会以为是导入失败）。
    pub fn import_profile(
        &self,
        req: tl_protocol::ProfileImportRequest,
    ) -> Result<tl_protocol::ProfileImportResult> {
        let result = profile::import(&self.servers, &self.tunnels, req)?;
        self.notify_changed();
        self.sync_hosts_quietly();
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn credential_choice_never_rebuilds_without_a_new_secret() {
        // 有新材料才重建：否则改端口/备注都会重建凭据，私钥口令会被静默丢掉
        assert_eq!(credential_choice(true, "cred_1"), CredentialChoice::Rebuild);
        assert_eq!(credential_choice(false, "cred_1"), CredentialChoice::Reuse);
        // 待填凭据没有材料可沿用：只更新字段
        assert_eq!(credential_choice(false, ""), CredentialChoice::Skip);
        // 给了新密钥就该重建，哪怕原来没有材料
        assert_eq!(credential_choice(true, ""), CredentialChoice::Rebuild);
    }
}
