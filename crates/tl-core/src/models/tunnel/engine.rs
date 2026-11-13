//! SSH 转发引擎（执行器）。
//!
//! 归属：`tunnel` 模型内部文件组。拥有：一次 SSH 会话的建立、主机密钥观测、`-L` 本地转发循环。
//! 提供：`bind_local` / `serve`（返回**一个**结局，不含重试）。
//! 允许调用：`russh`、`tokio`。
//!
//! 纪律（为什么引擎不做重试）：重试次数、退避序列、状态相位都是 `tunnel` 模型的规则；
//! 引擎只回答"这一次跑成了什么结局"。上层据此喂 `fsm::TunnelEvent`，避免两处各写一套重试。

use std::net::IpAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use russh::client;
use russh::keys::PublicKeyOrCertificate;
use russh::keys::{HashAlg, PrivateKeyWithHashAlg};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::watch;
use tokio::time::MissedTickBehavior;
use tokio_rustls::rustls;
use tokio_rustls::TlsAcceptor;

use super::traffic::Traffic;
use crate::foundation::tls_ca::GatewayCa;

/// 单隧道并发上限（超出的本地连接立即关闭，不排队）。
const MAX_CONNECTIONS: usize = 64;

/// 打开一条转发通道的上限。超时即视为"目标不可达"，记录错误并断开本地连接。
const CHANNEL_OPEN_TIMEOUT: Duration = Duration::from_secs(10);

/// 登录材料（由 `credential` 模型在进程内解密后传入，绝不落盘/落日志）。
#[derive(Clone)]
pub enum Login {
    Password {
        user: String,
        password: String,
    },
    PrivateKey {
        user: String,
        /// OpenSSH 或 PEM 私钥文本。
        pem: String,
        passphrase: Option<String>,
    },
}

impl std::fmt::Debug for Login {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Login::Password { user, .. } => {
                write!(f, "Login::Password{{user:{user}, password:<redacted>}}")
            }
            Login::PrivateKey {
                user, passphrase, ..
            } => write!(
                f,
                "Login::PrivateKey{{user:{user}, pem:<redacted>, passphrase:{}}}",
                if passphrase.is_some() {
                    "<set>"
                } else {
                    "<none>"
                }
            ),
        }
    }
}

impl Login {
    fn user(&self) -> &str {
        match self {
            Login::Password { user, .. } | Login::PrivateKey { user, .. } => user,
        }
    }
}

/// 一次会话服务的结局（上层据此决定下一个状态）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EngineOutcome {
    /// 会话正常建立后因外部请求停止（用户 stop / 进程退出）。
    Stopped,
    /// 主机密钥未知，需要用户确认（会话已断开，未转发任何字节）。
    NeedsTrust { fingerprint: String },
    /// 主机密钥与本机记录不一致——**绝不自动接受**。
    FingerprintMismatch { trusted: String, observed: String },
    /// 认证被拒（密钥/密码/方式不允许）——不应自动重试。
    AuthFailed { reason: String },
    /// 这条连接还没有登录材料（导入的分享包）——同样不应重试。
    CredentialMissing { reason: String },
    /// 传输层失败（网络不可达、握手失败、keepalive 超时）——可退避重试。
    NetworkFailed {
        reason: String,
        /// 失败发生在建连阶段还是转发阶段（决定喂 `HandshakeFailed` 还是 `NetworkLost`）。
        during: FailurePhase,
    },
}

/// 传输层失败发生的阶段。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailurePhase {
    /// 建连/握手/认证阶段。
    Connecting,
    /// 已在转发阶段（会话中途断开）。
    Serving,
}

/// 把引擎结局映射成状态机事件（业务层据此调用 `tunnel.apply`）。
///
/// 映射是纯函数：`NeedsTrust` 的指纹要先由业务层记进 `server.observe_host_key`，
/// 再喂 `HandshakeNeedsTrust`，两件事的顺序不能反。
/// 一条可复用的 SSH 会话句柄。多条端口转发共用同一个。
pub type SessionHandle = Arc<client::Handle<ClientHandler>>;

/// 把引擎结局翻成一句人能读的话（写进 `last_error`，也作为启动失败的原因）。
pub fn describe(outcome: &EngineOutcome) -> String {
    match outcome {
        EngineOutcome::Stopped => "已停止".to_string(),
        EngineOutcome::NetworkFailed { reason, .. } => reason.clone(),
        EngineOutcome::NeedsTrust { fingerprint } => {
            format!("主机密钥待确认：{fingerprint}")
        }
        EngineOutcome::FingerprintMismatch { trusted, observed } => {
            format!("主机密钥与记录不一致（记录 {trusted}，实测 {observed}）")
        }
        EngineOutcome::AuthFailed { reason } => reason.clone(),
        EngineOutcome::CredentialMissing { reason } => reason.clone(),
    }
}

pub fn to_event(outcome: &EngineOutcome) -> super::fsm::TunnelEvent {
    use super::fsm::TunnelEvent as E;
    match outcome {
        EngineOutcome::Stopped => E::Stop,
        EngineOutcome::NeedsTrust { .. } => E::HandshakeNeedsTrust,
        EngineOutcome::FingerprintMismatch { trusted, observed } => E::FingerprintMismatch {
            trusted: trusted.clone(),
            observed: observed.clone(),
        },
        EngineOutcome::AuthFailed { reason } => E::AuthFailed {
            reason: reason.clone(),
        },
        EngineOutcome::CredentialMissing { reason } => E::CredentialMissing {
            reason: reason.clone(),
        },
        EngineOutcome::NetworkFailed { reason, during } => match during {
            FailurePhase::Connecting => E::HandshakeFailed {
                reason: reason.clone(),
            },
            FailurePhase::Serving => E::NetworkLost,
        },
    }
}

/// 引擎参数。
#[derive(Debug, Clone)]
pub struct ServeConfig {
    pub server_host: String,
    pub server_port: u16,
    pub remote_host: String,
    pub remote_port: u16,
    /// 已信任指纹；`None` = 首次连接，需要确认。
    pub trusted_fingerprint: Option<String>,
    pub keepalive_secs: u64,
}

/// 探测卡在哪一步。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProbeStage {
    /// TCP 都连不上（地址写错 / 端口没开 / 防火墙）。
    Tcp,
    /// TCP 通了但 SSH 握手失败（不是 SSH 服务 / 协议版本不匹配）。
    Ssh,
    /// 握手成功但主机密钥与记录不一致（可疑，拒绝继续）。
    Fingerprint,
    /// 握手成功但认证被拒（用户名/密钥/密码不对）。
    Auth,
    /// 全部通过：能连、能认、能登录。
    Ok,
}

impl ProbeStage {
    pub fn as_str(self) -> &'static str {
        match self {
            ProbeStage::Tcp => "tcp",
            ProbeStage::Ssh => "ssh",
            ProbeStage::Fingerprint => "fingerprint",
            ProbeStage::Auth => "auth",
            ProbeStage::Ok => "ok",
        }
    }
}

/// 探测结果。用于创建/编辑隧道时回答"这条配置真的连得上吗"。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProbeReport {
    pub stage: ProbeStage,
    /// 握手观测到的指纹（走到 SSH 阶段才有）。
    pub fingerprint: Option<String>,
    pub message: String,
}

impl ProbeReport {
    fn new(stage: ProbeStage, fingerprint: Option<String>, message: String) -> Self {
        Self {
            stage,
            fingerprint,
            message,
        }
    }
}

/// 试连一次：TCP → SSH 握手 → 指纹比对 → 认证。**不绑定本地端口、不转发任何字节**。
pub async fn probe(config: &ServeConfig, login: Login, budget: Duration) -> ProbeReport {
    // 1. 先单独验 TCP：能把"地址/端口错"与"SSH 服务坏了"分开，排障时差别很大。
    let connect_tcp =
        tokio::net::TcpStream::connect((config.server_host.as_str(), config.server_port));
    match tokio::time::timeout(budget, connect_tcp).await {
        Err(_) => {
            return ProbeReport::new(
                ProbeStage::Tcp,
                None,
                format!(
                    "连接 {}:{} 超时（{}s）",
                    config.server_host,
                    config.server_port,
                    budget.as_secs()
                ),
            )
        }
        Ok(Err(e)) => {
            return ProbeReport::new(
                ProbeStage::Tcp,
                None,
                format!(
                    "连接 {}:{} 失败：{e}",
                    config.server_host, config.server_port
                ),
            )
        }
        Ok(Ok(stream)) => drop(stream),
    }

    // 2. 先嗅服务端横幅。SSH 服务端是**先说话**的一方，所以：
    //    - 说出了别的东西 → 肯定不是 SSH（HTTP/SMTP 之类），直接给结论；
    //    - 干脆不说话 → 也不像 SSH，先记下来，等握手失败时用它解释原因。
    let banner = sniff_banner(&config.server_host, config.server_port, budget).await;
    if let Some(text) = &banner {
        if !text.to_ascii_uppercase().starts_with("SSH-") {
            return ProbeReport::new(
                ProbeStage::Ssh,
                None,
                format!(
                    "端口 {} 上不是 SSH 服务（收到：{text}）。请确认主机与端口是否正确。",
                    config.server_port
                ),
            );
        }
    }
    let silent = banner.is_none();

    // 3. SSH 握手（顺带采集指纹）。
    let observed = Observed::default();
    let ssh_config = Arc::new(client::Config {
        keepalive_interval: None,
        ..Default::default()
    });
    let handler = ClientHandler {
        observed: observed.clone(),
    };
    let handshake = client::connect(
        ssh_config,
        (config.server_host.as_str(), config.server_port),
        handler,
    );
    let mut session = match tokio::time::timeout(budget, handshake).await {
        Err(_) => {
            let message = if silent {
                format!(
                    "端口 {} 没有回应 SSH 握手（对方不是 SSH 服务？）",
                    config.server_port
                )
            } else {
                "SSH 握手超时".to_string()
            };
            return ProbeReport::new(ProbeStage::Ssh, None, message);
        }
        Ok(Err(e)) => {
            // 静默（对方没主动发 banner）通常是"这端口压根不是 SSH"，
            // 直接抛 russh 的 Disconnected / invalid version string 帮不上用户。
            let message = if silent {
                format!(
                    "端口 {} 没有回应 SSH 握手（对方不是 SSH 服务？），详情：{e}",
                    config.server_port
                )
            } else {
                format!("SSH 握手失败：{e}")
            };
            return ProbeReport::new(ProbeStage::Ssh, None, message);
        }
        Ok(Ok(session)) => session,
    };

    let fingerprint = observed.take();

    // 4. 指纹比对：不一致就到此为止（探测也不允许悄悄连上去）。
    if let (Some(trusted), Some(seen)) = (config.trusted_fingerprint.clone(), fingerprint.clone()) {
        if trusted != seen {
            return ProbeReport::new(
                ProbeStage::Fingerprint,
                Some(seen.clone()),
                format!("主机密钥与记录不一致（记录 {trusted}，实测 {seen}）"),
            );
        }
    }

    // 4. 认证。
    match authenticate(&mut session, &login).await {
        Ok(()) => {
            let note = match &config.trusted_fingerprint {
                Some(_) => "连接、指纹、登录全部通过".to_string(),
                None => "连接与登录通过；主机密钥尚未信任，保存指纹后才能正式启用".to_string(),
            };
            ProbeReport::new(ProbeStage::Ok, fingerprint, note)
        }
        Err(EngineOutcome::AuthFailed { reason }) => {
            ProbeReport::new(ProbeStage::Auth, fingerprint, reason)
        }
        Err(other) => ProbeReport::new(
            ProbeStage::Auth,
            fingerprint,
            format!("认证过程失败：{other:?}"),
        ),
    }
}

/// 读一行服务端横幅（SSH 的 banner 或别的协议的响应首行）。
///
/// 单独开一条连接做，读完立刻关闭：既不影响随后的 russh 握手，也能在
/// "端口通但不是 SSH" 时给出人能读懂的原因。
async fn sniff_banner(host: &str, port: u16, budget: Duration) -> Option<String> {
    use tokio::io::AsyncReadExt;

    let connect = tokio::net::TcpStream::connect((host, port));
    let Ok(Ok(mut stream)) =
        tokio::time::timeout(budget.min(Duration::from_secs(2)), connect).await
    else {
        return None;
    };

    let mut buf = [0u8; 128];
    let read = tokio::time::timeout(Duration::from_millis(600), stream.read(&mut buf)).await;
    let Ok(Ok(n)) = read else {
        // 对面不说话（SSH 服务通常会先发 banner；沉默也不当作判定依据）。
        return None;
    };
    if n == 0 {
        return None;
    }

    let line = String::from_utf8_lossy(&buf[..n])
        .lines()
        .next()
        .unwrap_or_default()
        .trim()
        .to_string();
    if line.is_empty() {
        return None;
    }
    Some(line.chars().take(80).collect())
}

/// 抢占本地监听（同端口不允许第二个监听者）。
pub async fn bind_local(host: &str, port: u16) -> std::io::Result<TcpListener> {
    TcpListener::bind((host, port)).await
}

/// 退避期间保持监听、但对新连接**快速失败**（接受后立即关闭，不排队、不挂死）。
///
/// 由业务层在 `Backoff` 状态反复调用；`wait` 给一个很短的窗口，避免空转占 CPU。
pub async fn reject_one(listener: &TcpListener, wait: Duration) {
    if let Ok(Ok((stream, _peer))) = tokio::time::timeout(wait, listener.accept()).await {
        drop(stream);
    }
}

#[derive(Clone, Default)]
struct Observed {
    fingerprint: Arc<Mutex<Option<String>>>,
}

impl Observed {
    fn take(&self) -> Option<String> {
        self.fingerprint.lock().expect("observed lock").clone()
    }
}

pub struct ClientHandler {
    observed: Observed,
}

impl client::Handler for ClientHandler {
    type Error = russh::Error;

    async fn check_server_key(
        &mut self,
        server_public_key: &PublicKeyOrCertificate,
    ) -> Result<bool, Self::Error> {
        match server_public_key {
            PublicKeyOrCertificate::PublicKey { key, hash_alg } => {
                let alg = hash_alg.unwrap_or(HashAlg::Sha256);
                let fingerprint = format!("{}", key.fingerprint(alg));
                *self.observed.fingerprint.lock().expect("observed lock") = Some(fingerprint);
                // 握手一律放行：信任由 `serve` 拿到指纹后按策略判定；
                // 未通过判定则立即断开，绝不在未确认时转发字节。
                Ok(true)
            }
            PublicKeyOrCertificate::Certificate(_) => Ok(false),
        }
    }
}

/// 建立会话、认证、校验信任、进入转发循环。
///
/// **监听不被释放**：调用方保留 `TcpListener` 的所有权（这里返回它），这样断线退避期间
/// 端口仍在本进程手上——既不会被别的进程抢走，也便于在 `Backoff` 期间对新连接做快速失败。
/// 建立一条**可复用**的 SSH 会话：一次登录，多条端口转发共用。
///
/// 与 `probe` 的区别：这里真的进入会话（供 `serve_listener` 开通道），
/// 但目标串是否可达由每条转发各自决定——会话与转发是两层。
pub async fn open_session(
    config: &ServeConfig,
    login: Login,
    mut shutdown: watch::Receiver<bool>,
) -> Result<Arc<client::Handle<ClientHandler>>, EngineOutcome> {
    let observed = Observed::default();
    let ssh_config = Arc::new(client::Config {
        keepalive_interval: Some(Duration::from_secs(config.keepalive_secs.max(1))),
        keepalive_max: 3,
        ..Default::default()
    });

    let handler = ClientHandler {
        observed: observed.clone(),
    };
    let connect = client::connect(
        ssh_config,
        (config.server_host.as_str(), config.server_port),
        handler,
    );

    let mut session = tokio::select! {
        biased;
        _ = shutdown.changed() => return Err(EngineOutcome::Stopped),
        result = connect => match result {
            Ok(session) => session,
            Err(e) => {
                // 必须给出人能读的原因：russh 的 "invalid SSH version string"
                // 只说明"对面不是 SSH"，看不出是被安全策略拦了还是端口填错了。
                let reason = diagnose_connect_error(
                    &config.server_host,
                    config.server_port,
                    &e.to_string(),
                )
                .await;
                return Err(EngineOutcome::NetworkFailed {
                    reason,
                    during: FailurePhase::Connecting,
                });
            }
        },
    };

    // 信任判定：未知 → 要确认；已知但不一致 → 拒绝（绝不自动接受）。
    let observed_fp = observed.take().unwrap_or_default();
    match &config.trusted_fingerprint {
        None => {
            return Err(EngineOutcome::NeedsTrust {
                fingerprint: observed_fp,
            })
        }
        Some(trusted) if trusted != &observed_fp => {
            return Err(EngineOutcome::FingerprintMismatch {
                trusted: trusted.clone(),
                observed: observed_fp,
            })
        }
        Some(_) => {}
    }

    authenticate(&mut session, &login).await?;
    Ok(Arc::new(session))
}

/// 入口流：明文 TCP，或已终止 TLS 的流。
///
/// ②（本地 TLS 终止）握手完成后，后续管道与 ① 完全一致，只是流的类型不同。
pub trait EntryStream: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> EntryStream for T {}

/// 构造 ② 的入口 TLS acceptor。
///
/// 用 `ResolvesServerCert` 而不是 `with_single_cert`：叶子 7 天到期后，
/// 下一次握手会现签新叶子，**不需要重启监听**（设计 §3.4 的运行时轮换）。
pub fn tls_acceptor_for(
    ca: Arc<GatewayCa>,
    domain: String,
    ip: IpAddr,
) -> Result<TlsAcceptor, rustls::Error> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let mut config = rustls::ServerConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()?
        .with_no_client_auth()
        .with_cert_resolver(Arc::new(GatewayResolver { ca, domain, ip }));
    // 只宣告 http/1.1：宣告 h2 会把 HTTP/2 帧灌给远端的 h1 服务（设计 §3.2 ②）。
    config.alpn_protocols = vec![b"http/1.1".to_vec()];
    Ok(TlsAcceptor::from(Arc::new(config)))
}

struct GatewayResolver {
    ca: Arc<GatewayCa>,
    domain: String,
    ip: IpAddr,
}

impl std::fmt::Debug for GatewayResolver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GatewayResolver")
            .field("domain", &self.domain)
            .field("ip", &self.ip)
            .finish()
    }
}

impl rustls::server::ResolvesServerCert for GatewayResolver {
    fn resolve(
        &self,
        _hello: rustls::server::ClientHello<'_>,
    ) -> Option<Arc<rustls::sign::CertifiedKey>> {
        let leaf = self.ca.leaf_for(&self.domain, self.ip).ok()?;
        let key = rustls::crypto::ring::sign::any_supported_type(&leaf.key).ok()?;
        Some(Arc::new(rustls::sign::CertifiedKey::new(leaf.chain, key)))
    }
}

/// 在一个**已建立**的会话上跑某条转发的本地监听。
///
/// 会话由上层（`SessionHub`）持有：多条转发共用同一个 handle，
/// 这里只负责 accept →（可选 TLS 终止）→ `direct-tcpip`；会话断了就结束。
// 这层是"把运行时状态一路传下去"的管线，参数天然多；拆结构体反而更难读。
#[allow(clippy::too_many_arguments)]
pub async fn serve_listener(
    listener: TcpListener,
    session: Arc<client::Handle<ClientHandler>>,
    remote_host: String,
    remote_port: u16,
    keepalive_secs: u64,
    mut shutdown: watch::Receiver<bool>,
    on_error: Box<dyn Fn(String) + Send + Sync>,
    traffic: Arc<Traffic>,
    entry_tls: Option<TlsAcceptor>,
    public_domain: Option<String>,
) -> (EngineOutcome, TcpListener) {
    let outcome = serve_listener_inner(
        &listener,
        session,
        remote_host,
        remote_port,
        keepalive_secs,
        &mut shutdown,
        on_error,
        traffic,
        entry_tls.as_ref(),
        public_domain.as_deref(),
    )
    .await;

    (outcome, listener)
}

#[allow(clippy::too_many_arguments)]
async fn serve_listener_inner(
    listener: &TcpListener,
    session: Arc<client::Handle<ClientHandler>>,
    remote_host: String,
    remote_port: u16,
    keepalive_secs: u64,
    shutdown: &mut watch::Receiver<bool>,
    on_error: Box<dyn Fn(String) + Send + Sync>,
    traffic: Arc<Traffic>,
    entry_tls: Option<&TlsAcceptor>,
    public_domain: Option<&str>,
) -> EngineOutcome {
    let on_error = Arc::new(on_error);
    let inflight = Arc::new(AtomicUsize::new(0));
    let mut ticker = tokio::time::interval(Duration::from_secs(keepalive_secs.max(1)));
    ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);

    loop {
        tokio::select! {
            biased;
            _ = shutdown.changed() => return EngineOutcome::Stopped,
            _ = ticker.tick() => {
                if session.is_closed() {
                    return EngineOutcome::NetworkFailed {
                        reason: "SSH 会话已断开（keepalive 超时或对端关闭）".to_string(),
                        during: FailurePhase::Serving,
                    };
                }
            }
            accepted = listener.accept() => {
                let (inbound, _peer) = match accepted {
                    Ok(pair) => pair,
                    Err(e) => {
                        return EngineOutcome::NetworkFailed {
                            reason: format!("本地监听失败：{e}"),
                            during: FailurePhase::Serving,
                        }
                    }
                };

                if inflight.load(Ordering::SeqCst) >= MAX_CONNECTIONS {
                    drop(inbound);
                    continue;
                }
                inflight.fetch_add(1, Ordering::SeqCst);

                let report = Arc::clone(&on_error);
                let counter = Arc::clone(&inflight);

                // ②：先完成本地 TLS 握手，再把解出来的明文流交给同一条管道。
                let inbound: Box<dyn EntryStream> = match entry_tls {
                    Some(acceptor) => {
                        let accepted = crate::foundation::timeout::with_timeout(
                            crate::foundation::timeout::TLS_HANDSHAKE,
                            acceptor.accept(inbound),
                        )
                        .await;
                        match accepted {
                            Ok(Ok(stream)) => Box::new(stream),
                            Ok(Err(e)) => {
                                report(format!("本地 TLS 握手失败：{e}"));
                                counter.fetch_sub(1, Ordering::SeqCst);
                                continue;
                            }
                            Err(_) => {
                                report("本地 TLS 握手超时（10 秒），已断开该连接".to_string());
                                counter.fetch_sub(1, Ordering::SeqCst);
                                continue;
                            }
                        }
                    }
                    None => Box::new(inbound),
                };

                let handle = Arc::clone(&session);
                let target = (remote_host.clone(), remote_port);
                let traffic = Arc::clone(&traffic);
                // 引用不能进 'static 任务，先落成拥有所有权的值
                let rewrite_http = entry_tls.is_some();
                let public_fallback = public_domain.map(str::to_string);
                tokio::spawn(async move {
                    // 连不上远端端口时错误必须上报，否则界面只表现为"连接被重置"。
                    if let Err(e) = forward_one(
                        handle,
                        target.clone(),
                        inbound,
                        traffic,
                        rewrite_http,
                        public_fallback,
                    )
                    .await
                    {
                        report(format!(
                            "无法连接 {}:{}（服务器侧）：{e}",
                            target.0, target.1
                        ));
                    }
                    counter.fetch_sub(1, Ordering::SeqCst);
                });
            }
        }
    }
}

/// 把握手失败翻译成可执行的话：先嗅一次横幅，区分三种情况。
///
/// 第三种（HTTP 响应）值得单独说：SSH 服务不会回 HTTP。收到 HTTP 状态行，
/// 通常是**本机安全软件拦截了本进程**（它把拒绝包装成 HTTP 响应），
/// 而不是目标端口填错。不点明的话，用户会一直去查主机和端口。
async fn diagnose_connect_error(host: &str, port: u16, error: &str) -> String {
    match sniff_banner(host, port, Duration::from_secs(2)).await {
        Some(banner) if !banner.to_ascii_uppercase().starts_with("SSH-") => {
            if banner.to_ascii_uppercase().starts_with("HTTP/") {
                let exe = std::env::current_exe()
                    .map(|p| p.display().to_string())
                    .unwrap_or_else(|_| "tl-core.exe".to_string());
                return format!(
                    "连接 {host}:{port} 时收到 HTTP 响应（{banner}），这不是 SSH 服务的响应。多半是本机安全软件拦截了这个程序，请把 {exe} 加入白名单后重试。"
                );
            }
            format!("端口 {port} 上不是 SSH 服务（收到：{banner}）。请确认主机与端口是否正确")
        }
        Some(_) => format!("SSH 握手失败：{error}"),
        None => format!("端口 {port} 没有回应 SSH 握手（对方不是 SSH 服务？），详情：{error}"),
    }
}

async fn authenticate(
    session: &mut client::Handle<ClientHandler>,
    login: &Login,
) -> Result<(), EngineOutcome> {
    let user = login.user().to_string();
    let result = match login {
        Login::Password { password, .. } => session
            .authenticate_password(user, password.clone())
            .await
            .map_err(|e| EngineOutcome::NetworkFailed {
                reason: format!("认证过程失败：{e}"),
                during: FailurePhase::Connecting,
            })?,
        Login::PrivateKey {
            pem, passphrase, ..
        } => {
            let key = russh::keys::PrivateKey::from_openssh(pem).map_err(|e| {
                EngineOutcome::AuthFailed {
                    reason: format!("私钥格式无法解析：{e}"),
                }
            })?;
            let key = match (key.is_encrypted(), passphrase) {
                (true, Some(pass)) => key.decrypt(pass).map_err(|e| EngineOutcome::AuthFailed {
                    reason: format!("私钥口令错误：{e}"),
                })?,
                (true, None) => {
                    return Err(EngineOutcome::AuthFailed {
                        reason: "私钥已加密，但未提供口令".to_string(),
                    })
                }
                (false, _) => key,
            };
            session
                .authenticate_publickey(
                    user,
                    PrivateKeyWithHashAlg::new(Arc::new(key), Some(HashAlg::Sha256)),
                )
                .await
                .map_err(|e| EngineOutcome::NetworkFailed {
                    reason: format!("认证过程失败：{e}"),
                    during: FailurePhase::Connecting,
                })?
        }
    };

    match result {
        russh::client::AuthResult::Success => Ok(()),
        russh::client::AuthResult::Failure { .. } => Err(EngineOutcome::AuthFailed {
            reason: "服务器拒绝了该认证方式（密钥被拒或密码错误）".to_string(),
        }),
    }
}

async fn forward_one<S>(
    session: Arc<client::Handle<ClientHandler>>,
    (remote_host, remote_port): (String, u16),
    inbound: S,
    traffic: Arc<Traffic>,
    rewrite_http: bool,
    public_fallback: Option<String>,
) -> Result<(), std::io::Error>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    // 目标不可达时 russh 可能一直等对端确认：没有超时的话，本地连接会永久挂起，
    // 用户看到的是"连上了但没反应"，也永远不会有错误记录。
    // 远端地址按转发规则里填的原样拼：它既是上行 `Host`，也是下行识别 Location 的依据。
    // 80/443 是默认端口，不写进 Host。
    let upstream_host = remote_host.clone();
    let authority = if remote_port == 80 || remote_port == 443 {
        upstream_host.clone()
    } else {
        format!("{upstream_host}:{remote_port}")
    };
    let channel = tokio::time::timeout(
        CHANNEL_OPEN_TIMEOUT,
        session.channel_open_direct_tcpip(remote_host, remote_port as u32, "127.0.0.1", 0),
    )
    .await
    .map_err(|_| {
        std::io::Error::new(
            std::io::ErrorKind::TimedOut,
            format!(
                "服务器在 {} 秒内没有确认通道（目标不可达或被防火墙丢弃）",
                CHANNEL_OPEN_TIMEOUT.as_secs()
            ),
        )
    })?
    .map_err(std::io::Error::other)?;

    let outbound = channel.into_stream();
    let (in_read, in_write) = tokio::io::split(inbound);
    let (out_read, out_write) = tokio::io::split(outbound);

    // copy_bidirectional 只在结束时给出总数，界面上就永远看不到"正在跑"的速率。
    // 拆成两条单方向搬运，边读边累加，计数器随时可读。
    //
    // TLS 终结的连接额外做一次 HTTP 头改写（见 `pipe_http`）：只有这里能看见明文。
    //
    // 域名是本机的约定：上行把 Host 换成远端地址，远端就不会把 `deva.zly.m`
    // 写进它生成的重定向；下行再把指向远端的 Location 换回浏览器访问的地址。
    let observed = Arc::new(std::sync::Mutex::new(None::<String>));
    let rewrite = rewrite_http.then(|| HttpRewrite {
        upstream_host,
        authority,
        fallback: public_fallback.clone(),
        observed: Arc::clone(&observed),
    });
    let up = Arc::clone(&traffic);
    let down = Arc::clone(&traffic);
    let up_ctx = rewrite.clone();
    let down_ctx = rewrite.clone();
    let (to_remote, to_local) = tokio::join!(
        pipe_dir(in_read, out_write, move |n| up.add_up(n), up_ctx, true),
        pipe_dir(
            out_read,
            in_write,
            move |n| down.add_down(n),
            down_ctx,
            false
        ),
    );
    to_remote?;
    to_local?;
    Ok(())
}

/// 一次 HTTP 头改写的上下文：两个方向共用一个 `observed`，上行写、下行读。
#[derive(Clone)]
struct HttpRewrite {
    /// 远端主机名（不含端口）
    upstream_host: String,
    /// 上行 `Host` 的值：80/443 是默认端口，写出来是噪音
    authority: String,
    /// 请求没带 Host 时的兜底（配置里的域名）
    fallback: Option<String>,
    /// 浏览器实际请求的 Host，上行解析出来后填进来
    observed: Arc<std::sync::Mutex<Option<String>>>,
}

impl HttpRewrite {
    /// 浏览器访问的主机名（去掉端口）。优先用请求里看到的，其次用配置兜底。
    fn public_host(&self) -> Option<String> {
        let observed = self.observed.lock().ok().and_then(|slot| slot.clone());
        observed.or_else(|| self.fallback.clone()).map(|host| {
            host.rsplit_once(':')
                .map(|(name, _)| name.to_string())
                .unwrap_or(host)
        })
    }
}

/// 按是否需要 HTTP 改写选择搬运方式。
async fn pipe_dir<R, W, F>(
    reader: R,
    writer: W,
    count: F,
    ctx: Option<HttpRewrite>,
    request: bool,
) -> Result<(), std::io::Error>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
    F: Fn(u64),
{
    match ctx {
        Some(ctx) => pipe_http(reader, writer, count, ctx, request).await,
        None => pipe(reader, writer, count).await,
    }
}

/// 头块上限：超过就不当 HTTP 处理，原样透传（防止把二进制流当文本缓冲）。
const MAX_HEAD: usize = 32 * 1024;

fn find_head_end(bytes: &[u8]) -> Option<usize> {
    bytes
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .map(|p| p + 4)
}

/// 改写第一块 HTTP 头，协议之外的内容一律不动。
///
/// - 请求方向：`Host` 换成远端地址（本机域名不出本机），并补
///   `X-Forwarded-Proto: https`、`X-Forwarded-Host: <浏览器用的主机名>`。
///   同时把浏览器用的主机名记进 `observed`，供响应方向使用。
/// - 响应方向：`Location` 指向远端地址时，换回浏览器访问的那个地址（https）。
///   第三方地址一律不动。
///
/// 非 HTTP 流量（首行不含 `HTTP/`）原样返回。
fn rewrite_head(head: &[u8], ctx: &HttpRewrite, request: bool) -> Vec<u8> {
    let text = String::from_utf8_lossy(head);
    let mut lines: Vec<String> = text.split("\r\n").map(str::to_string).collect();
    let first = lines.first().cloned().unwrap_or_default();

    if request {
        if !first.contains(" HTTP/") {
            return head.to_vec();
        }
        let mut host_seen: Option<String> = None;
        let mut proto_seen = false;
        let mut forwarded_seen = false;
        for line in lines.iter_mut() {
            let lower = line.to_ascii_lowercase();
            if lower.starts_with("host:") {
                if let Some(colon) = line.find(':') {
                    host_seen = Some(line[colon + 1..].trim().to_string());
                }
                *line = format!("Host: {}", ctx.authority);
            } else if lower.starts_with("x-forwarded-proto:") {
                *line = "X-Forwarded-Proto: https".to_string();
                proto_seen = true;
            } else if lower.starts_with("x-forwarded-host:") {
                forwarded_seen = true;
            }
        }
        let mut extra = Vec::new();
        if !proto_seen {
            extra.push("X-Forwarded-Proto: https".to_string());
        }
        if let Some(host) = &host_seen {
            if !forwarded_seen {
                extra.push(format!("X-Forwarded-Host: {host}"));
            }
            if let Ok(mut slot) = ctx.observed.lock() {
                if slot.is_none() {
                    *slot = Some(host.clone());
                }
            }
        }
        if !extra.is_empty() {
            for (offset, line) in extra.into_iter().enumerate() {
                lines.insert(1 + offset, line);
            }
        }
    } else {
        if !first.starts_with("HTTP/") {
            return head.to_vec();
        }
        let Some(public) = ctx.public_host() else {
            return head.to_vec();
        };
        for line in lines.iter_mut() {
            let lower = line.to_ascii_lowercase();
            if !lower.starts_with("location:") {
                continue;
            }
            let Some(colon) = line.find(':') else {
                continue;
            };
            let value = line[colon + 1..].trim();
            let Some(rest) = value.strip_prefix("http://") else {
                continue;
            };
            let (authority, path) = match rest.find('/') {
                Some(idx) => (&rest[..idx], &rest[idx..]),
                None => (rest, ""),
            };
            let ours = authority.eq_ignore_ascii_case(&ctx.authority)
                || authority.eq_ignore_ascii_case(&ctx.upstream_host);
            if ours {
                *line = format!("Location: https://{public}{path}");
            }
        }
    }

    lines.join("\r\n").into_bytes()
}

/// 带 HTTP 头改写的单方向搬运：先把头凑齐改写，之后退回纯字节透传。
async fn pipe_http<R, W, F>(
    mut reader: R,
    mut writer: W,
    count: F,
    ctx: HttpRewrite,
    request: bool,
) -> Result<(), std::io::Error>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
    F: Fn(u64),
{
    let mut buffer = vec![0u8; 16 * 1024];
    let mut head: Vec<u8> = Vec::with_capacity(4096);
    let mut head_done = false;

    loop {
        let read = reader.read(&mut buffer).await?;
        if read == 0 {
            break;
        }
        if head_done {
            writer.write_all(&buffer[..read]).await?;
            count(read as u64);
            continue;
        }

        head.extend_from_slice(&buffer[..read]);
        if let Some(end) = find_head_end(&head) {
            let body = head.split_off(end);
            let rewritten = rewrite_head(&head, &ctx, request);
            writer.write_all(&rewritten).await?;
            count(rewritten.len() as u64);
            if !body.is_empty() {
                writer.write_all(&body).await?;
                count(body.len() as u64);
            }
            head_done = true;
        } else if head.len() > MAX_HEAD {
            // 不是 HTTP（或头异常长）：放弃改写，原样送出去
            writer.write_all(&head).await?;
            count(head.len() as u64);
            head.clear();
            head_done = true;
        }
    }

    if !head_done && !head.is_empty() {
        writer.write_all(&head).await?;
        count(head.len() as u64);
    }
    let _ = writer.shutdown().await;
    Ok(())
}

/// 单方向搬运并计数。
///
/// 只要有一侧读到 EOF 就结束；错误原样上抛，由调用方决定是否上报。
async fn pipe<R, W, F>(mut reader: R, mut writer: W, count: F) -> Result<(), std::io::Error>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
    F: Fn(u64),
{
    let mut buffer = vec![0u8; 16 * 1024];
    loop {
        let read = reader.read(&mut buffer).await?;
        if read == 0 {
            break;
        }
        writer.write_all(&buffer[..read]).await?;
        count(read as u64);
    }
    // 半关闭：让对端知道这一侧说完了，而不是干等到超时。
    let _ = writer.shutdown().await;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::net::TcpStream;

    fn rewrite_ctx(observed: Option<&str>, fallback: Option<&str>) -> HttpRewrite {
        HttpRewrite {
            upstream_host: "192.168.28.98".to_string(),
            authority: "192.168.28.98:8080".to_string(),
            fallback: fallback.map(str::to_string),
            observed: Arc::new(std::sync::Mutex::new(observed.map(str::to_string))),
        }
    }

    #[test]
    fn request_host_is_replaced_by_forward_target() {
        // 本机域名不出本机：上行把 Host 换成转发规则里的远端地址
        let ctx = rewrite_ctx(None, None);
        let head = b"GET /Dashboard HTTP/1.1\r\nHost: deva.zly.m\r\nAccept: */*\r\n\r\n";
        let out = String::from_utf8(rewrite_head(head, &ctx, true)).expect("utf8");
        assert!(out.contains("Host: 192.168.28.98:8080"), "{out}");
        // 精确到整行：`X-Forwarded-Host:` 里仍应保留原域名
        assert!(
            !out.contains("\r\nHost: deva.zly.m\r\n"),
            "域名仍留在 Host 上：{out}"
        );
        assert!(out.contains("X-Forwarded-Proto: https"), "{out}");
        assert!(out.contains("X-Forwarded-Host: deva.zly.m"), "{out}");
        // 供下行改写使用
        assert_eq!(
            ctx.observed.lock().expect("lock").as_deref(),
            Some("deva.zly.m")
        );
    }

    #[test]
    fn response_location_is_rewritten_back_to_public_host() {
        // 远端按自己的地址生成 302，下行换回浏览器访问的那个地址
        let ctx = rewrite_ctx(Some("deva.zly.m"), None);
        let head =
            b"HTTP/1.1 302 Found\r\nLocation: http://192.168.28.98:8080/login\r\nContent-Length: 0\r\n\r\n";
        let out = String::from_utf8(rewrite_head(head, &ctx, false)).expect("utf8");
        assert!(out.contains("Location: https://deva.zly.m/login"), "{out}");
    }

    #[test]
    fn foreign_location_is_left_alone() {
        let ctx = rewrite_ctx(Some("deva.zly.m"), Some("deva.zly.m"));
        let head = b"HTTP/1.1 302 Found\r\nLocation: http://example.com/x\r\n\r\n";
        let out = String::from_utf8(rewrite_head(head, &ctx, false)).expect("utf8");
        assert!(out.contains("Location: http://example.com/x"), "{out}");
    }

    #[test]
    fn non_http_traffic_passes_through() {
        let ctx = rewrite_ctx(Some("deva.zly.m"), None);
        let raw = b"\x16\x03\x01\x00\x05binary";
        assert_eq!(rewrite_head(raw, &ctx, true), raw.to_vec());
    }

    #[test]
    fn login_debug_redacts_secrets() {
        let login = Login::Password {
            user: "dev".into(),
            password: "TOP-SECRET".into(),
        };
        let debug = format!("{login:?}");
        assert!(!debug.contains("TOP-SECRET"));
        assert!(debug.contains("dev"));

        let login = Login::PrivateKey {
            user: "dev".into(),
            pem: "-----BEGIN OPENSSH PRIVATE KEY-----SECRET".into(),
            passphrase: Some("PP".into()),
        };
        let debug = format!("{login:?}");
        assert!(!debug.contains("SECRET"));
        assert!(!debug.contains("PP"));
    }

    /// ② 的核心保证：叶子能被本地根验通，且 ALPN **只**协商出 `http/1.1`。
    ///
    /// 用 `tokio::io::duplex` 起一对流，不需要真实 SSH 会话。
    #[tokio::test]
    async fn entry_tls_is_trusted_by_local_root_and_only_offers_http11() {
        let dir =
            std::env::temp_dir().join(format!("threadline-engine-tls-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let ca = Arc::new(GatewayCa::load_or_create(&dir).expect("ca"));
        let acceptor = tls_acceptor_for(
            Arc::clone(&ca),
            "home.example.com".to_string(),
            "127.0.0.5".parse().expect("ip"),
        )
        .expect("acceptor");

        let mut roots = rustls::RootCertStore::empty();
        let mut cursor = std::io::Cursor::new(ca.root_pem().as_bytes().to_vec());
        for cert in rustls_pemfile::certs(&mut cursor) {
            roots.add(cert.expect("root pem")).expect("add root");
        }

        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let mut client_config = rustls::ClientConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .expect("versions")
            .with_root_certificates(roots)
            .with_no_client_auth();
        // 故意同时提 h2：服务端只能选 http/1.1（否则解密后灌给远端 h1 就是乱码）。
        client_config.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];

        let (server_io, client_io) = tokio::io::duplex(16 * 1024);
        let server = tokio::spawn(async move {
            let mut stream = acceptor.accept(server_io).await.expect("server handshake");
            stream.write_all(b"hello").await.expect("write");
            stream.shutdown().await.expect("shutdown");
        });

        let connector = tokio_rustls::TlsConnector::from(Arc::new(client_config));
        let name = rustls::pki_types::ServerName::try_from("home.example.com".to_string())
            .expect("server name");
        let mut client = connector
            .connect(name, client_io)
            .await
            .expect("client handshake");

        assert_eq!(
            client.get_ref().1.alpn_protocol(),
            Some(b"http/1.1".as_slice()),
            "② 不能协商出 h2"
        );
        let mut body = Vec::new();
        client.read_to_end(&mut body).await.expect("read");
        assert_eq!(body, b"hello");

        server.await.expect("server task");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 重启（重建 issuer 视图）之后签的叶子，必须能被**磁盘上的旧根**验通。
    ///
    /// 这是 `rcgen 0.13` 无法从 DER 还原 `Certificate` 的补偿方案的核心假设：
    /// 假设不成立就得换 rcgen 0.14 的 `Issuer`。
    #[tokio::test]
    async fn leaf_signed_after_reload_chains_to_persisted_root() {
        let dir = std::env::temp_dir().join(format!("threadline-rotate-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);

        // 第一次启动：生成根，记住它的 PEM，然后「退出」。
        let first = GatewayCa::load_or_create(&dir).expect("first");
        let root_pem = first.root_pem().to_string();
        drop(first);

        // 第二次启动：从盘上加载同一私钥，重建 issuer 视图。
        let second = Arc::new(GatewayCa::load_or_create(&dir).expect("reload"));
        let acceptor = tls_acceptor_for(
            Arc::clone(&second),
            "fresh.example.com".to_string(),
            "127.0.0.6".parse().expect("ip"),
        )
        .expect("acceptor");

        // 客户端只信任第一次生成的根。
        let mut roots = rustls::RootCertStore::empty();
        let mut cursor = std::io::Cursor::new(root_pem.as_bytes().to_vec());
        for cert in rustls_pemfile::certs(&mut cursor) {
            roots.add(cert.expect("root pem")).expect("add root");
        }
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let client_config = rustls::ClientConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .expect("versions")
            .with_root_certificates(roots)
            .with_no_client_auth();

        let (server_io, client_io) = tokio::io::duplex(16 * 1024);
        let server = tokio::spawn(async move {
            let mut stream = acceptor.accept(server_io).await.expect("server handshake");
            stream.write_all(b"ok").await.expect("write");
            stream.shutdown().await.expect("shutdown");
        });

        let connector = tokio_rustls::TlsConnector::from(Arc::new(client_config));
        let name =
            rustls::pki_types::ServerName::try_from("fresh.example.com".to_string()).expect("name");
        let mut client = connector
            .connect(name, client_io)
            .await
            .expect("重启后签的叶子必须能被旧根验通");
        let mut body = Vec::new();
        client.read_to_end(&mut body).await.expect("read");
        assert_eq!(body, b"ok");

        server.await.expect("server task");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn bind_local_reports_port_conflict() {
        let first = bind_local("127.0.0.1", 0).await.expect("first bind");
        let port = first.local_addr().expect("addr").port();
        let second = bind_local("127.0.0.1", port).await;
        assert!(second.is_err(), "同端口第二次绑定必须失败");
    }

    #[tokio::test]
    async fn bind_local_reports_invalid_host() {
        assert!(bind_local("256.256.256.256", 8080).await.is_err());
    }

    #[tokio::test]
    async fn reject_one_closes_incoming_connection_immediately() {
        use tokio::io::AsyncReadExt;

        let listener = bind_local("127.0.0.1", 0).await.expect("bind");
        let addr = listener.local_addr().expect("addr");

        let client = tokio::spawn(async move {
            let mut stream = TcpStream::connect(addr).await.expect("connect");
            let mut buf = [0u8; 8];
            // 服务端 accept 后立即 drop → 客户端读到 EOF（0 字节）
            stream.read(&mut buf).await.unwrap_or(0)
        });

        reject_one(&listener, Duration::from_millis(500)).await;
        let read = client.await.expect("client task");
        assert_eq!(read, 0, "被拒绝的连接应立即读到 EOF");
    }

    #[test]
    fn outcome_variants_are_distinguishable() {
        assert_ne!(
            EngineOutcome::NetworkFailed {
                reason: "x".into(),
                during: FailurePhase::Connecting,
            },
            EngineOutcome::AuthFailed { reason: "x".into() }
        );
        assert_ne!(
            EngineOutcome::NeedsTrust {
                fingerprint: "f".into()
            },
            EngineOutcome::FingerprintMismatch {
                trusted: "f".into(),
                observed: "g".into()
            }
        );
    }

    #[test]
    fn outcomes_map_to_state_machine_events() {
        use crate::models::tunnel::fsm::TunnelEvent as E;

        assert_eq!(to_event(&EngineOutcome::Stopped), E::Stop);
        assert_eq!(
            to_event(&EngineOutcome::NeedsTrust {
                fingerprint: "SHA256:x".into()
            }),
            E::HandshakeNeedsTrust
        );
        assert!(matches!(
            to_event(&EngineOutcome::FingerprintMismatch {
                trusted: "a".into(),
                observed: "b".into()
            }),
            E::FingerprintMismatch { .. }
        ));
        assert!(matches!(
            to_event(&EngineOutcome::AuthFailed { reason: "r".into() }),
            E::AuthFailed { .. }
        ));
        // 建连阶段失败 → 可退避的握手失败
        assert!(matches!(
            to_event(&EngineOutcome::NetworkFailed {
                reason: "r".into(),
                during: FailurePhase::Connecting
            }),
            E::HandshakeFailed { .. }
        ));
        // 转发阶段失败 → 断线（进入退避）
        assert_eq!(
            to_event(&EngineOutcome::NetworkFailed {
                reason: "r".into(),
                during: FailurePhase::Serving
            }),
            E::NetworkLost
        );
    }

    #[test]
    fn mapped_events_are_legal_from_connecting() {
        use crate::models::tunnel::fsm::{next_state, TunnelEvent as E};
        use tl_protocol::TunnelState;

        // 建连阶段的所有结局都必须是 Connecting 的合法出边。
        for outcome in [
            EngineOutcome::NeedsTrust {
                fingerprint: "f".into(),
            },
            EngineOutcome::FingerprintMismatch {
                trusted: "a".into(),
                observed: "b".into(),
            },
            EngineOutcome::AuthFailed { reason: "r".into() },
            EngineOutcome::NetworkFailed {
                reason: "r".into(),
                during: FailurePhase::Connecting,
            },
        ] {
            let event = to_event(&outcome);
            assert!(
                next_state(TunnelState::Connecting, &event).is_ok(),
                "事件 {event:?} 应是 Connecting 的合法出边"
            );
        }

        // 转发阶段断线必须是 Running 的合法出边。
        assert!(next_state(TunnelState::Running, &E::NetworkLost).is_ok());
    }

    #[tokio::test]
    async fn probe_reports_tcp_stage_when_nothing_listens() {
        // 取一个刚释放的端口，几乎不可能有人立刻占用。
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.expect("bind");
        let port = listener.local_addr().expect("addr").port();
        drop(listener);

        let config = ServeConfig {
            server_host: "127.0.0.1".into(),
            server_port: port,
            remote_host: "127.0.0.1".into(),
            remote_port: 0,
            trusted_fingerprint: None,
            keepalive_secs: 30,
        };
        let login = Login::Password {
            user: "root".into(),
            password: "pw".into(),
        };

        let report = probe(&config, login, Duration::from_millis(800)).await;
        assert_eq!(
            report.stage,
            ProbeStage::Tcp,
            "连不上时必须在 TCP 阶段就停下并说清楚，而不是含糊报错"
        );
        assert!(report.fingerprint.is_none());
        assert!(
            report.message.contains("失败") || report.message.contains("超时"),
            "消息要能说明原因：{}",
            report.message
        );
    }

    fn probe_config(port: u16) -> ServeConfig {
        ServeConfig {
            server_host: "127.0.0.1".into(),
            server_port: port,
            remote_host: "127.0.0.1".into(),
            remote_port: 0,
            trusted_fingerprint: None,
            keepalive_secs: 30,
        }
    }

    fn probe_login() -> Login {
        Login::Password {
            user: "root".into(),
            password: "pw".into(),
        }
    }

    #[tokio::test]
    async fn pipe_counts_bytes_while_copying() {
        // 用内存双工流验证：搬运多少就算多少，且两个方向独立计数。
        // 真实链路要一台可达的远端服务才能验证，这里把"计数逻辑"单独锁住。
        let (mut near, far) = tokio::io::duplex(1024);
        let (far_read, far_write) = tokio::io::split(far);
        let traffic = Arc::new(Traffic::default());

        let up = Arc::clone(&traffic);
        let server =
            tokio::spawn(async move { pipe(far_read, far_write, move |n| up.add_up(n)).await });

        tokio::io::AsyncWriteExt::write_all(&mut near, b"hello world")
            .await
            .expect("write");
        tokio::io::AsyncWriteExt::shutdown(&mut near)
            .await
            .expect("shutdown");

        server.await.expect("join").expect("pipe");
        assert_eq!(traffic.snapshot().0, 11, "写进去的 11 字节要被数到");
    }

    #[tokio::test]
    async fn probe_names_the_foreign_banner() {
        use tokio::io::AsyncWriteExt;

        // 会主动打招呼但不是 SSH 的服务（SMTP 就是这样）。
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.expect("bind");
        let port = listener.local_addr().expect("addr").port();
        tokio::spawn(async move {
            while let Ok((mut socket, _)) = listener.accept().await {
                let _ = socket.write_all(b"220 smtp.example ESMTP ready\r\n").await;
                tokio::time::sleep(Duration::from_millis(300)).await;
            }
        });

        let report = probe(&probe_config(port), probe_login(), Duration::from_secs(2)).await;
        assert_eq!(report.stage, ProbeStage::Ssh);
        assert!(
            report.message.contains("不是 SSH"),
            "要说清端口不是 SSH：{}",
            report.message
        );
        assert!(
            report.message.contains("220 smtp"),
            "把收到的横幅带上才有诊断价值：{}",
            report.message
        );
    }

    #[tokio::test]
    async fn probe_explains_a_silent_port() {
        // 接受连接但从不说话：HTTP 这类"客户端先开口"的协议就是这样。
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.expect("bind");
        let port = listener.local_addr().expect("addr").port();
        tokio::spawn(async move {
            let mut held = Vec::new();
            while let Ok((socket, _)) = listener.accept().await {
                held.push(socket);
            }
        });

        let report = probe(&probe_config(port), probe_login(), Duration::from_secs(2)).await;
        assert_eq!(report.stage, ProbeStage::Ssh);
        assert!(
            report.message.contains("没有回应 SSH 握手"),
            "静默也要给出人能懂的原因，而不是 Disconnected：{}",
            report.message
        );
    }

    #[test]
    fn probe_stage_strings_are_stable() {
        assert_eq!(ProbeStage::Tcp.as_str(), "tcp");
        assert_eq!(ProbeStage::Auth.as_str(), "auth");
        assert_eq!(ProbeStage::Ok.as_str(), "ok");
    }
}
