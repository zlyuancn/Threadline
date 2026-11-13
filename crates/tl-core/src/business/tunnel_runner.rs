//! 隧道的业务编排。
//!
//! 归属：**业务层**。只做编排：连会话 → 绑端口 → 交给引擎 → 把结局翻成事件 → 按退避重来。
//! **不含判定规则**：状态能不能迁、什么时候进 Error、重试几次放弃，全部由 `tunnel` 模型与
//! `fsm` 决定；这里只负责"按顺序问、按结果驱动"。
//!
//! 关键结构：**一条「隧道」（server）只有一条 SSH 会话**，挂在它下面的多条「端口转发」
//! 共用这个会话（各自绑定自己的本地端口）。这样：
//! - N 条转发只登录一次，不会触发服务器的连接数限制；
//! - 断开隧道 = 所有挂上去的端口一起关；
//! - 隧道连着的时候随时可以加端口（新端口只是多一个监听，不需要重新握手）。

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{anyhow, Result};
use tl_protocol::{CredentialKind, TunnelDto};
use tokio::net::TcpListener;
use tokio::sync::watch;
use tokio::task::JoinHandle;

use crate::foundation::tls_ca::GatewayCa;
use crate::foundation::{clock, retry::Backoff};

/// ② 的入口 TLS acceptor；其它入口模式返回 `Ok(None)`。
///
/// 构造失败必须上抛：静默退回明文会把 ② 变成 ①，用户以为加密了其实没有。
fn build_entry_tls(
    gateway: Option<&Arc<GatewayCa>>,
    def: &TunnelDto,
) -> Result<Option<tokio_rustls::TlsAcceptor>, String> {
    if !matches!(entry_mode(def), Ok(EntryMode::Terminate)) {
        return Ok(None);
    }
    let Some(ca) = gateway else {
        return Err("本地网关证书不可用（② 需要它来终止 TLS）".to_string());
    };
    let host = effective_addr(def).0;
    let ip = host
        .parse::<std::net::IpAddr>()
        .map_err(|_| format!("域名入口地址不合法：{host}"))?;
    engine::tls_acceptor_for(Arc::clone(ca), def.domain.clone(), ip)
        .map(Some)
        .map_err(|e| format!("构造入口 TLS 失败：{e}"))
}
use crate::models::credential::{CredentialMaterial, CredentialStore};
use crate::models::server::ServerStore;
use crate::models::tunnel::engine::{self, Login, ServeConfig, SessionHandle};
use crate::models::tunnel::{effective_addr, entry_mode, fsm::TunnelEvent, EntryMode, TunnelStore};

struct Running {
    cancel: watch::Sender<bool>,
    join: JoinHandle<()>,
}

/// 一条已建立的 SSH 会话，按 `server_id` 共享给所有挂在上面的转发。
struct Session {
    handle: SessionHandle,
    /// 保留着以便断开时通知（`open_session` 只在连接阶段看它）。
    cancel: watch::Sender<bool>,
}

/// 隧道运行器：持有会话表与每条转发的监听任务。
pub struct TunnelRunner {
    tunnels: Arc<TunnelStore>,
    servers: Arc<ServerStore>,
    credentials: Arc<CredentialStore>,
    tasks: Mutex<HashMap<String, Running>>,
    sessions: Mutex<HashMap<String, Session>>,
    /// 每条隧道的会话守护任务（`启用` 期间保持连接）。
    keepers: Mutex<HashMap<String, watch::Sender<bool>>>,
    /// 会话意外断开时被暂停的转发（server_id → 转发 id 列表）。
    ///
    /// 监听循环只会在"有人连进来"时才发现会话没了；没人访问时它就一直挂在 accept 上，
    /// 状态永远停在 Running——界面于是出现"隧道未连接，但转发运行中"。
    /// 这里记下"掉线前真正在跑的转发"，重连后按名单恢复，而不是按 auto_start 猜。
    paused: Mutex<HashMap<String, Vec<String>>>,
    /// 建会话过程的串行锁：`ensure_session` 里"查表 → 连接 → 落表"是一个整体，
    /// 两条转发同时启动时若不加锁会各建一条 SSH 连接（留下孤儿会话）。
    session_lock: tokio::sync::Mutex<()>,
    /// ②（本地 TLS 终止）用的网关 CA。初始化失败时为 `None`，该档转发会明确报错而不是静默明文。
    gateway: Option<Arc<GatewayCa>>,
}

impl TunnelRunner {
    pub fn new(
        tunnels: Arc<TunnelStore>,
        servers: Arc<ServerStore>,
        credentials: Arc<CredentialStore>,
        gateway: Option<Arc<GatewayCa>>,
    ) -> Self {
        Self {
            tunnels,
            servers,
            credentials,
            tasks: Mutex::new(HashMap::new()),
            sessions: Mutex::new(HashMap::new()),
            keepers: Mutex::new(HashMap::new()),
            paused: Mutex::new(HashMap::new()),
            session_lock: tokio::sync::Mutex::new(()),
            gateway,
        }
    }

    /// 主动建立某条隧道的会话（用户打开「启用」时调用）。
    pub async fn connect_session(&self, server_id: &str) -> Result<()> {
        self.ensure_session(server_id)
            .await
            .map(|_| ())
            .map_err(|outcome| anyhow!(engine::describe(&outcome)))
    }

    /// 这条转发当前是否有监听任务在跑（界面状态之外的事实来源）。
    pub fn is_running(&self, id: &str) -> bool {
        self.tasks.lock().expect("tasks lock").contains_key(id)
    }

    /// 该隧道是否已有可用的 SSH 会话（界面用来显示"已连接/未连接"）。
    pub fn session_connected(&self, server_id: &str) -> bool {
        self.sessions
            .lock()
            .expect("sessions lock")
            .get(server_id)
            .is_some_and(|session| !session.handle.is_closed())
    }

    /// 启动一条端口转发。已在跑时幂等返回。
    ///
    /// 会话按需建立：同一隧道的第二条转发直接复用已有会话，**不会再登录一次**。
    pub async fn start(self: &Arc<Self>, id: &str) -> Result<()> {
        if self.tasks.lock().expect("tasks lock").contains_key(id) {
            return Ok(());
        }

        let def = find_tunnel(&self.tunnels, id)?;
        let params = self.servers.connect_params(&def.server_id)?;

        // 端口挂在隧道上：隧道没启用就不该建会话，否则那个开关就是一句空话
        // （界面显示"未启用"，实际连接和转发都在跑——这正是用户发现的问题）。
        if !params.enabled {
            let reason = "隧道未启用：请先在「隧道」页启用这条隧道";
            self.tunnels.stop(id)?;
            self.tunnels.set_last_error(id, reason);
            return Err(anyhow!(reason));
        }

        // 模型侧先进入 Connecting 并登记端口；规则（哪些状态能 start）由它判断。
        self.tunnels.start(id)?;

        let session = match self.ensure_session(&def.server_id).await {
            Ok(session) => session,
            Err(outcome) => {
                // 会话建不起来：把结局翻成事件，让模型给出正确状态（待确认/错误/退避）。
                let event = engine::to_event(&outcome);
                let _ = self.tunnels.apply(id, event);
                return Err(anyhow!(engine::describe(&outcome)));
            }
        };

        let (bind_host, bind_port) = crate::models::tunnel::effective_addr(&def);
        let listener = match engine::bind_local(bind_host, bind_port).await {
            Ok(listener) => listener,
            Err(e) => {
                let reason = e.to_string();
                self.tunnels.apply(
                    id,
                    TunnelEvent::BindFailed {
                        reason: reason.clone(),
                    },
                )?;
                return Err(anyhow!("本地端口绑定失败：{reason}"));
            }
        };

        let (cancel, cancel_rx) = watch::channel(false);
        let runner = Arc::clone(self);
        let tunnel_id = id.to_string();
        let cleanup_id = tunnel_id.clone();
        let cleanup = Arc::clone(&runner);
        // 每次启动都从零开始计数：界面看到的是"这一段的速率"，不是历史累计。
        self.tunnels.reset_traffic(id);
        let traffic = self.tunnels.traffic_of(id);
        let join = tokio::spawn(async move {
            runner
                .listener_loop(
                    tunnel_id,
                    def,
                    params.keepalive_secs as u64,
                    session,
                    listener,
                    cancel_rx,
                    traffic,
                )
                .await;
            // **必须摘除登记**：任务可能自己结束（会话丢、重试耗尽），
            // 不清理的话后续 start 会被开头那句 "contains_key 就返回" 挡掉。
            cleanup
                .tasks
                .lock()
                .expect("tasks lock")
                .remove(&cleanup_id);
        });

        self.tasks
            .lock()
            .expect("tasks lock")
            .insert(id.to_string(), Running { cancel, join });
        Ok(())
    }

    /// 停止一条端口转发（只关这个端口，隧道本身继续为其它转发服务）。
    /// 用户显式停止：从"断线待恢复"名单里摘掉，避免重连后又被拉起来。
    fn forget_paused(&self, id: &str) {
        let mut paused = self.paused.lock().expect("paused lock");
        for list in paused.values_mut() {
            list.retain(|item| item != id);
        }
    }

    pub async fn stop(&self, id: &str) -> Result<()> {
        self.forget_paused(id);
        let running = self.tasks.lock().expect("tasks lock").remove(id);
        if let Some(running) = running {
            let _ = running.cancel.send(true);
            let _ = running.join.await;
        }
        self.tunnels.stop(id)?;
        Ok(())
    }

    /// 断开某条隧道的 SSH 会话：挂在它下面的所有端口一起关。
    ///
    /// 返回被顺带停掉的转发数量——界面要能说清"断一条隧道影响了几个端口"。
    pub async fn disconnect_session(&self, server_id: &str) -> Result<usize> {
        // 显式断开会显式停转发：这不是"掉线"，重连后不该被自动拉起来。
        self.paused.lock().expect("paused lock").remove(server_id);
        let session = self
            .sessions
            .lock()
            .expect("sessions lock")
            .remove(server_id);

        let forwards: Vec<String> = self
            .tunnels
            .list()?
            .items
            .into_iter()
            .filter(|view| view.definition.server_id == server_id)
            .map(|view| view.definition.id)
            .collect();

        for id in &forwards {
            // 先把锁放掉再 await：MutexGuard 跨 await 会让整个 future 不是 Send。
            let running = self.tasks.lock().expect("tasks lock").remove(id);
            if let Some(running) = running {
                let _ = running.cancel.send(true);
                let _ = running.join.await;
            }
            let _ = self.tunnels.stop(id);
            self.tunnels
                .set_last_error(id, "隧道已断开：这条转发随之停止");
        }

        if let Some(session) = session {
            let _ = session.cancel.send(true);
            // 主动送 disconnect，别等最后一个 Arc 被丢弃时才断。
            let _ = session
                .handle
                .disconnect(russh::Disconnect::ByApplication, "user closed tunnel", "en")
                .await;
        }

        Ok(forwards.len())
    }

    /// 停止全部（`/shutdown` 与进程退出时调用）。
    pub async fn stop_all(&self) -> Result<()> {
        let servers: Vec<String> = self
            .tunnels
            .list()?
            .items
            .into_iter()
            .map(|view| view.definition.server_id)
            .collect();
        for server_id in servers {
            let _ = self.disconnect_session(&server_id).await;
        }

        let ids: Vec<String> = self
            .tasks
            .lock()
            .expect("tasks lock")
            .keys()
            .cloned()
            .collect();
        for id in ids {
            let _ = self.stop(&id).await;
        }
        self.tunnels.stop_all()?;
        Ok(())
    }

    /// 启动一条隧道的**会话守护**：只要它还处于「启用」，就一直保持连接。
    ///
    /// 为什么要它：启动时那一次连接可能因为网络抖动或本机安全策略失败，
    /// 之前就"试一次、失败就再也不动"，用户看到的是"启用着却一直未连接"。
    /// 既然开关表示"我希望它连着"，那就该一直重试。
    pub fn ensure_keeper(self: &Arc<Self>, server_id: &str) {
        let mut keepers = self.keepers.lock().expect("keepers lock");
        if keepers.contains_key(server_id) {
            return;
        }

        let (cancel, cancel_rx) = watch::channel(false);
        keepers.insert(server_id.to_string(), cancel);

        let runner = Arc::clone(self);
        let id = server_id.to_string();
        let cleanup_id = id.clone();
        let cleanup = Arc::clone(&runner);
        tokio::spawn(async move {
            runner.keeper_loop(id, cancel_rx).await;
            cleanup
                .keepers
                .lock()
                .expect("keepers lock")
                .remove(&cleanup_id);
        });
    }

    /// 停掉某条隧道的会话守护（关闭开关时调用，避免它把连接又拉回来）。
    pub fn stop_keeper(&self, server_id: &str) {
        let keeper = self.keepers.lock().expect("keepers lock").remove(server_id);
        if let Some(cancel) = keeper {
            let _ = cancel.send(true);
        }
    }

    async fn keeper_loop(self: Arc<Self>, server_id: String, mut cancel: watch::Receiver<bool>) {
        let mut backoff = Backoff::default();
        loop {
            if *cancel.borrow() {
                return;
            }
            // 意图关闭了就退出——重试不该跟用户的关闭动作对着干。
            let enabled = self
                .servers
                .list()
                .ok()
                .and_then(|list| list.items.into_iter().find(|s| s.id == server_id))
                .map(|s| s.enabled)
                .unwrap_or(false);
            if !enabled {
                return;
            }

            if !self.session_connected(&server_id) {
                // 先把它下面的转发停干净：监听不该继续绑在一个死会话上。
                self.pause_forwards_for(&server_id).await;
                match self.ensure_session(&server_id).await {
                    Ok(_) => {
                        backoff.reset();
                        // 断线前在跑的优先恢复（不看 auto_start），再把自启的补齐。
                        self.resume_paused_forwards(&server_id).await;
                        self.resume_auto_start_forwards(&server_id).await;
                    }
                    Err(outcome) if wants_retry(&outcome) => {
                        // 网络类问题：退避后继续试，别停在那儿假装在连。
                        let wait = backoff.next_delay().unwrap_or(30);
                        if wait_cancel(&mut cancel, wait).await {
                            return;
                        }
                        continue;
                    }
                    Err(_) => {
                        // 指纹未确认 / 认证被拒：重试一万次也不会变，
                        // 还会把服务器惹到封 IP。停下来等人处理。
                        return;
                    }
                }
            }

            if wait_cancel(&mut cancel, 5).await {
                return;
            }
        }
    }

    /// 会话断了：把它下面正在跑的转发停下来，并记下名单。
    ///
    /// 不停掉的话，监听仍绑在本地端口上、状态仍是 Running，但背后已经没有 SSH 会话——
    /// 用户看到的是"运行中"却连不通。宁可显示"暂停"，也不要骗人。
    async fn pause_forwards_for(&self, server_id: &str) {
        let ids: Vec<String> = match self.tunnels.list() {
            Ok(list) => list
                .items
                .into_iter()
                .filter(|view| view.definition.server_id == server_id)
                .map(|view| view.definition.id)
                .collect(),
            Err(_) => return,
        };

        let mut paused_now = Vec::new();
        for id in ids {
            let running = self.tasks.lock().expect("tasks lock").remove(&id);
            let Some(running) = running else {
                continue;
            };
            let _ = running.cancel.send(true);
            let _ = running.join.await;
            let _ = self.tunnels.stop(&id);
            self.tunnels
                .set_last_error(&id, "SSH 会话已断开：转发已暂停，重连后自动恢复");
            paused_now.push(id);
        }

        if !paused_now.is_empty() {
            let mut paused = self.paused.lock().expect("paused lock");
            let list = paused.entry(server_id.to_string()).or_default();
            list.extend(paused_now);
            list.sort();
            list.dedup();
        }
    }

    /// 会话回来了：把断线前真在跑的那批转发拉起来。
    ///
    /// 与 `resume_auto_start_forwards` 的区别：这里不看 auto_start——它们本来就在跑，
    /// 是掉线打断的，不该因为用户没勾"自启"就被悄悄放弃。
    async fn resume_paused_forwards(self: &Arc<Self>, server_id: &str) {
        let ids = self
            .paused
            .lock()
            .expect("paused lock")
            .remove(server_id)
            .unwrap_or_default();
        for id in ids {
            let _ = self.start(&id).await;
        }
    }

    /// 会话刚恢复：把这条隧道下"启用"的转发补起来。
    ///
    /// 会话掉线的这段时间里，转发会停在 Error / Stopped；会话回来不会自动带它们回来。
    async fn resume_auto_start_forwards(self: &Arc<Self>, server_id: &str) {
        let items = match self.tunnels.list() {
            Ok(list) => list.items,
            Err(_) => return,
        };
        for id in forwards_to_resume(&items, server_id) {
            let _ = self.start(&id).await;
        }
    }

    /// 开机/启动时按**用户意图**恢复：先连启用的隧道，再拉起启用的转发。
    ///
    /// 隧道没启用时，挂在上面的转发不启动——但它的"启用"意图保留，
    /// 等隧道打开后由 `set_server_enabled` 统一恢复。
    pub async fn startup_autostart(self: &Arc<Self>) -> Vec<(String, String)> {
        let mut failures = Vec::new();

        let enabled_servers: Vec<String> = match self.servers.list() {
            Ok(list) => list
                .items
                .into_iter()
                .filter(|server| server.enabled)
                .map(|server| server.id)
                .collect(),
            Err(e) => return vec![("".to_string(), e.to_string())],
        };
        for server_id in &enabled_servers {
            // 先试一次（快路径），无论成败都挂上守护：失败会按退避继续重试。
            if let Err(e) = self.connect_session(server_id).await {
                failures.push((server_id.clone(), e.to_string()));
            }
            self.ensure_keeper(server_id);
        }

        let items = match self.tunnels.list() {
            Ok(list) => list.items,
            Err(e) => return vec![("".to_string(), e.to_string())],
        };
        for view in items {
            if !view.definition.auto_start {
                continue;
            }
            if !enabled_servers.contains(&view.definition.server_id) {
                self.tunnels
                    .set_last_error(&view.definition.id, "隧道未启用：这条转发不会自动启动");
                continue;
            }
            // 级联的第二段：端口跟着**已连上的**隧道走。
            // 隧道还没连上时不硬起（否则必然失败、留下一条 Error），交给守护连上后统一恢复。
            if !self.session_connected(&view.definition.server_id) {
                self.tunnels
                    .set_last_error(&view.definition.id, "等待隧道连接后自动启动");
                continue;
            }
            if let Err(e) = self.start(&view.definition.id).await {
                failures.push((view.definition.id.clone(), e.to_string()));
            }
        }
        failures
    }

    /// 拿到（必要时建立）某条隧道的会话。
    async fn ensure_session(
        &self,
        server_id: &str,
    ) -> Result<SessionHandle, engine::EngineOutcome> {
        let _guard = self.session_lock.lock().await;

        {
            let sessions = self.sessions.lock().expect("sessions lock");
            if let Some(session) = sessions.get(server_id) {
                if !session.handle.is_closed() {
                    return Ok(Arc::clone(&session.handle));
                }
            }
        }

        let params = self.servers.connect_params(server_id).map_err(|e| {
            let reason = e.to_string();
            // 「还没填密码」是配置没到位，不是网络问题：翻成不可重试的结局，
            // 否则 keeper 会把它当 NetworkFailed 一直退避重连。
            if reason.contains("还没填密码") {
                engine::EngineOutcome::CredentialMissing { reason }
            } else {
                engine::EngineOutcome::NetworkFailed {
                    reason,
                    during: engine::FailurePhase::Connecting,
                }
            }
        })?;
        let material = self
            .credentials
            .material(&params.credential_id)
            .map_err(|e| engine::EngineOutcome::NetworkFailed {
                reason: e.to_string(),
                during: engine::FailurePhase::Connecting,
            })?;

        let config = ServeConfig {
            server_host: params.host.clone(),
            server_port: params.port,
            // 会话层不关心转发目标，由每条监听各自决定。
            remote_host: "127.0.0.1".to_string(),
            remote_port: 0,
            trusted_fingerprint: params.trusted_fingerprint.clone(),
            keepalive_secs: params.keepalive_secs as u64,
        };

        let (cancel, cancel_rx) = watch::channel(false);
        let handle = match engine::open_session(&config, to_login(material), cancel_rx).await {
            Ok(handle) => handle,
            Err(outcome) => {
                // 隧道行只显示"未连接"时用户没法判断原因，把这次的失败理由一起记下。
                self.servers
                    .set_session_error(server_id, engine::describe(&outcome));
                // 未确认的指纹要记下来，界面才给得出"待确认"和三态结论。
                if let engine::EngineOutcome::NeedsTrust { fingerprint } = &outcome {
                    let _ = self.servers.observe_host_key(
                        server_id,
                        fingerprint,
                        &clock::now_iso8601(),
                    );
                }
                return Err(outcome);
            }
        };

        // 连上了：上一次的失败原因不再成立。
        self.servers.clear_session_error(server_id);

        self.sessions.lock().expect("sessions lock").insert(
            server_id.to_string(),
            Session {
                handle: Arc::clone(&handle),
                cancel,
            },
        );
        Ok(handle)
    }

    /// 单条转发的监听循环：会话断了就把会话重建起来，再继续接受连接。
    // 与引擎同因：这是把上下文一路带下去的循环，参数多但每个都必要。
    #[allow(clippy::too_many_arguments)]
    async fn listener_loop(
        self: Arc<Self>,
        id: String,
        def: TunnelDto,
        keepalive_secs: u64,
        mut session: SessionHandle,
        listener: TcpListener,
        mut cancel: watch::Receiver<bool>,
        traffic: std::sync::Arc<crate::models::tunnel::Traffic>,
    ) {
        let mut backoff = Backoff::default();
        let mut current = listener;

        loop {
            if *cancel.borrow() {
                let _ = self.tunnels.apply(&id, TunnelEvent::Stop);
                return;
            }

            if *cancel.borrow() {
                let _ = self.tunnels.apply(&id, TunnelEvent::Stop);
                return;
            }

            // 会话已经建立并认证过：这条转发可以进入 Running 了。
            let _ = self.tunnels.apply(&id, TunnelEvent::HandshakeOk);

            let tunnels_for_error = Arc::clone(&self.tunnels);
            let error_id = id.clone();
            let on_error = Box::new(move |message: String| {
                tunnels_for_error.set_last_error(&error_id, message);
            });

            // ② 需要本地 TLS 终止；构造失败时**不允许**退回明文，直接停在错误上。
            let entry_tls = match build_entry_tls(self.gateway.as_ref(), &def) {
                Ok(acceptor) => acceptor,
                Err(message) => {
                    self.tunnels.set_last_error(&id, message);
                    let _ = self.tunnels.apply(&id, TunnelEvent::Stop);
                    return;
                }
            };

            let (outcome, returned) = engine::serve_listener(
                current,
                Arc::clone(&session),
                def.remote_host.clone(),
                def.remote_port,
                keepalive_secs,
                cancel.clone(),
                on_error,
                std::sync::Arc::clone(&traffic),
                entry_tls,
            )
            .await;
            current = returned;

            match &outcome {
                engine::EngineOutcome::Stopped => {
                    let _ = self.tunnels.apply(&id, TunnelEvent::Stop);
                    return;
                }
                engine::EngineOutcome::NetworkFailed { .. } => {
                    let _ = self.tunnels.apply(&id, engine::to_event(&outcome));

                    let Some(secs) = backoff.next_delay() else {
                        let _ = self.tunnels.apply(&id, TunnelEvent::RetryExhausted);
                        return;
                    };
                    if !wait_backoff(&current, secs, &mut cancel).await {
                        let _ = self.tunnels.apply(&id, TunnelEvent::Stop);
                        return;
                    }

                    // 重新拿会话：可能还是好的（只是本地监听抖动），也可能要重连。
                    match self.ensure_session(&def.server_id).await {
                        Ok(fresh) => {
                            if *cancel.borrow() {
                                let _ = self.tunnels.apply(&id, TunnelEvent::Stop);
                                return;
                            }
                            session = fresh;
                        }
                        Err(fatal) => {
                            let _ = self.tunnels.apply(&id, engine::to_event(&fatal));
                            self.tunnels.set_last_error(&id, engine::describe(&fatal));
                            return;
                        }
                    }
                    backoff.reset();
                    let _ = self.tunnels.apply(&id, TunnelEvent::BackoffElapsed);
                }
                engine::EngineOutcome::NeedsTrust { fingerprint } => {
                    let _ = self.servers.observe_host_key(
                        &def.server_id,
                        fingerprint,
                        &clock::now_iso8601(),
                    );
                    let _ = self.tunnels.apply(&id, TunnelEvent::HandshakeNeedsTrust);
                    return;
                }
                other => {
                    let _ = self.tunnels.apply(&id, engine::to_event(other));
                    return;
                }
            }
        }
    }
}

/// 这个失败值不值得重试。
///
/// 只有"网络/对端暂时不可用"才值得：指纹待确认、指纹不一致、认证被拒都属于
/// **配置或人没到位**，重试只会刷连接数（服务器侧常见做法是直接封 IP）。
fn wants_retry(outcome: &engine::EngineOutcome) -> bool {
    matches!(outcome, engine::EngineOutcome::NetworkFailed { .. })
}

/// 睡眠指定的秒数；收到取消信号则提前返回 `true`。
async fn wait_cancel(cancel: &mut watch::Receiver<bool>, secs: u64) -> bool {
    tokio::select! {
        biased;
        _ = cancel.changed() => true,
        _ = tokio::time::sleep(Duration::from_secs(secs)) => false,
    }
}

/// 退避等待：期间保持监听并**快速拒绝**新连接（不排队、不挂死）。
async fn wait_backoff(
    listener: &TcpListener,
    secs: u64,
    cancel: &mut watch::Receiver<bool>,
) -> bool {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(secs);
    loop {
        if *cancel.borrow() {
            return false;
        }
        tokio::select! {
            biased;
            _ = cancel.changed() => return false,
            _ = tokio::time::sleep_until(deadline) => return true,
            _ = engine::reject_one(listener, Duration::from_millis(200)) => {}
        }
    }
}

fn find_tunnel(tunnels: &TunnelStore, id: &str) -> Result<TunnelDto> {
    let list = tunnels.list()?;
    list.items
        .into_iter()
        .map(|view| view.definition)
        .find(|def| def.id == id)
        .ok_or_else(|| anyhow!("隧道不存在：{id}"))
}

/// 登录用户的唯一来源是凭据，隧道本身不再存一份。
pub(crate) fn to_login(material: CredentialMaterial) -> Login {
    let user = material.username;
    match material.kind {
        CredentialKind::Password => Login::Password {
            user,
            password: material.secret,
        },
        CredentialKind::PrivateKey => Login::PrivateKey {
            user,
            pem: material.secret,
            passphrase: material.passphrase,
        },
    }
}

/// 会话恢复后要补启的转发：用户要求自动启动、且当前既没在跑也没在连。
///
/// 抽成纯函数是为了能直接测——真实场景要一台会掉线的 SSH 服务器才复现得了。
fn forwards_to_resume(items: &[tl_protocol::TunnelView], server_id: &str) -> Vec<String> {
    items
        .iter()
        .filter(|view| view.definition.server_id == server_id)
        .filter(|view| view.definition.auto_start)
        .filter(|view| {
            !matches!(
                view.status.state,
                tl_protocol::TunnelState::Running | tl_protocol::TunnelState::Connecting
            )
        })
        .map(|view| view.definition.id.clone())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::tunnel::engine::FailurePhase;

    #[test]
    fn only_auto_start_forwards_that_are_down_get_resumed() {
        use tl_protocol::{TunnelDto, TunnelState, TunnelStatus, TunnelView};

        let view = |id: &str, state: TunnelState, auto: bool| TunnelView {
            definition: TunnelDto {
                id: id.into(),
                name: id.into(),
                server_id: "srv".into(),
                local_host: "127.0.0.1".into(),
                local_port: 18080,
                remote_host: "127.0.0.1".into(),
                remote_port: 80,
                auto_start: auto,
                note: String::new(),
                open_path: String::new(),
                scheme: "tcp".into(),
                domain: String::new(),
                domain_host: String::new(),
                remote_scheme: None,
                icon: String::new(),
                group: String::new(),
                in_app: true,
            },
            status: TunnelStatus {
                id: id.into(),
                state,
                active_connections: 0,
                bytes_up: 0,
                bytes_down: 0,
                last_error: None,
                retry_in_secs: None,
            },
        };

        let items = vec![
            // 会话恢复后该被拉起来的两种"死"状态
            view("err", TunnelState::Error, true),
            view("stopped", TunnelState::Stopped, true),
            // 已经在跑/在连的不能重复启动
            view("running", TunnelState::Running, true),
            view("connecting", TunnelState::Connecting, true),
            // 用户没要求自动启动的，别替他做主
            view("manual", TunnelState::Stopped, false),
        ];

        let ids = forwards_to_resume(&items, "srv");
        assert_eq!(ids, vec!["err".to_string(), "stopped".to_string()]);
        assert!(
            forwards_to_resume(&items, "other").is_empty(),
            "别的隧道的转发不该被这条会话带起来"
        );
    }

    #[test]
    fn only_network_failures_are_worth_retrying() {
        let network = engine::EngineOutcome::NetworkFailed {
            reason: "timeout".into(),
            during: FailurePhase::Connecting,
        };
        assert!(wants_retry(&network));

        // 这三种重试一万次也不会变，还会把服务器惹到封 IP。
        assert!(!wants_retry(&engine::EngineOutcome::NeedsTrust {
            fingerprint: "SHA256:x".into(),
        }));
        assert!(!wants_retry(&engine::EngineOutcome::AuthFailed {
            reason: "拒绝".into(),
        }));
        assert!(!wants_retry(&engine::EngineOutcome::FingerprintMismatch {
            trusted: "a".into(),
            observed: "b".into(),
        }));
    }
}
