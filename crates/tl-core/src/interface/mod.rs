//! 接口层：HTTP 协议适配。
//!
//! 归属：**接口层**。只做：路由、鉴权、请求体反序列化、调用业务层、把结果/错误翻译成 HTTP。
//! **零业务规则**——「能不能删」「要不要提权」全部由模型/业务层回答。
//! 允许调用：`business`（用例）、`models`（只读查询）、`tl_protocol`（契约）。

pub mod auth;
pub mod error;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::extract::{Path as AxumPath, Query, State};
use axum::http::StatusCode;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::routing::{get, patch, post, put};
use axum::{Json, Router};
use serde::Deserialize;
use tl_protocol::{
    AcceptHostKeyRequest, AutostartStatus, CreateCredentialRequest, CreateServerRequest,
    CreateTunnelRequest, CredentialList, CredentialSummary, GatewayCaStatus, GatewayTrustResult,
    HealthResponse, HostKeyView, HostsStatus, HostsSyncResult, LogsResponse,
    PatchCredentialRequest, PatchServerRequest, PatchTunnelRequest, ProbeDraftRequest,
    ProbeServerReport, ProxySyncResult, ServerDto, ServerEnabledResult, ServerList, SessionClosed,
    SessionState, SetAutostartRequest, SetServerEnabledRequest, ShutdownAccepted, TunnelList,
    TunnelView, WhoamiResponse, API_PREFIX,
};

use crate::business::Services;
use crate::foundation::{registry, token as token_gen};
use crate::models::runtime::AutostartError;

use error::ApiFailure;

/// 接口层共享状态（由壳层装配）。
pub struct AppState {
    pub services: Arc<Services>,
    pub token: String,
    pub log_dir: PathBuf,
    pub exe_path: PathBuf,
}

impl AppState {
    pub fn new(
        services: Arc<Services>,
        token: impl Into<String>,
        log_dir: impl Into<PathBuf>,
        exe_path: impl Into<PathBuf>,
    ) -> Arc<Self> {
        Arc::new(Self {
            services,
            token: token.into(),
            log_dir: log_dir.into(),
            exe_path: exe_path.into(),
        })
    }
}

/// 允许的跨源来源：Tauri WebView（Windows 上是 `http://tauri.localhost`，
/// macOS/Linux 是 `tauri://localhost`）。
///
/// vite 的 5173 只在 debug 构建里放行：release 也认它，等于允许本机任意进程
/// 起一个服务、冒充"合法页面源"。
#[cfg(not(debug_assertions))]
const ALLOWED_ORIGINS: &[&str] = &[
    "http://tauri.localhost",
    "https://tauri.localhost",
    "tauri://localhost",
];

#[cfg(debug_assertions)]
const ALLOWED_ORIGINS: &[&str] = &[
    "http://tauri.localhost",
    "https://tauri.localhost",
    "tauri://localhost",
    "http://localhost:5173",
    "http://127.0.0.1:5173",
];

/// CORS：WebView 的页面源与核心的 `127.0.0.1:port` 不同源，没有这层浏览器会直接
/// 报 `TypeError: Failed to fetch`（连响应体都拿不到，看起来像"核心没启动"）。
///
/// 安全边界没有放宽：仍然只监听回环、仍然要求 Bearer token，这里只放开"浏览器允许读取响应"。
fn cors_layer() -> tower_http::cors::CorsLayer {
    use axum::http::{header, HeaderValue, Method};

    let origins: Vec<HeaderValue> = ALLOWED_ORIGINS
        .iter()
        .map(|origin| HeaderValue::from_static(origin))
        .collect();

    tower_http::cors::CorsLayer::new()
        .allow_origin(origins)
        .allow_methods([
            Method::GET,
            Method::POST,
            // PUT 会先发一次 OPTIONS 预检：列表里没有它，浏览器就直接判"跨源拦截"，
            // 表现是 TypeError: Failed to fetch，而用 curl/脚本打同一个接口却正常。
            Method::PUT,
            Method::PATCH,
            Method::DELETE,
            Method::OPTIONS,
        ])
        .allow_headers([header::AUTHORIZATION, header::CONTENT_TYPE])
        .max_age(std::time::Duration::from_secs(600))
}

/// 构建路由：`/healthz` 免鉴权，其余全部要求 Bearer。
pub fn router(state: Arc<AppState>) -> Router {
    let public = Router::new().route(&format!("{API_PREFIX}/healthz"), get(healthz));

    let protected = Router::new()
        .route(&format!("{API_PREFIX}/whoami"), get(whoami))
        .route(
            &format!("{API_PREFIX}/servers"),
            get(list_servers).post(create_server),
        )
        .route(&format!("{API_PREFIX}/servers/order"), put(reorder_servers))
        .route(
            &format!("{API_PREFIX}/servers/{{id}}"),
            patch(patch_server).delete(remove_server),
        )
        .route(
            &format!("{API_PREFIX}/servers/{{id}}/enabled"),
            post(set_server_enabled),
        )
        .route(
            &format!("{API_PREFIX}/servers/{{id}}/session"),
            get(server_session),
        )
        .route(
            &format!("{API_PREFIX}/servers/{{id}}/session/disconnect"),
            post(disconnect_session),
        )
        .route(
            &format!("{API_PREFIX}/servers/{{id}}/probe"),
            post(probe_server),
        )
        // 放在顶层而不是 /servers 下：`/servers/{id}` 是动态段，同层级再加一个
        // 静态段会被它抢走（表现为 405 Method Not Allowed）。
        .route(&format!("{API_PREFIX}/probe-draft"), post(probe_draft))
        .route(
            &format!("{API_PREFIX}/servers/{{id}}/hostkey"),
            get(host_key),
        )
        .route(
            &format!("{API_PREFIX}/servers/{{id}}/hostkey/accept"),
            post(accept_host_key),
        )
        .route(&format!("{API_PREFIX}/events"), get(stream_events))
        .route(
            &format!("{API_PREFIX}/tunnels"),
            get(list_tunnels).post(create_tunnel),
        )
        .route(
            &format!("{API_PREFIX}/tunnels/{{id}}"),
            patch(patch_tunnel).delete(remove_tunnel),
        )
        .route(
            &format!("{API_PREFIX}/tunnels/{{id}}/start"),
            post(start_tunnel),
        )
        .route(
            &format!("{API_PREFIX}/tunnels/{{id}}/stop"),
            post(stop_tunnel),
        )
        .route(
            &format!("{API_PREFIX}/credentials"),
            get(list_credentials).post(create_credential),
        )
        .route(
            &format!("{API_PREFIX}/credentials/{{id}}"),
            patch(patch_credential).delete(remove_credential),
        )
        .route(
            &format!("{API_PREFIX}/autostart"),
            get(get_autostart).post(set_autostart),
        )
        .route(&format!("{API_PREFIX}/hosts"), get(get_hosts))
        .route(&format!("{API_PREFIX}/hosts/sync"), post(sync_hosts))
        .route(&format!("{API_PREFIX}/hosts/clear"), post(clear_hosts))
        .route(
            &format!("{API_PREFIX}/hosts/proxy-result"),
            post(proxy_result),
        )
        .route(
            &format!("{API_PREFIX}/hosts/proxy-clear"),
            post(clear_proxy),
        )
        .route(&format!("{API_PREFIX}/gateway/ca"), get(get_gateway_ca))
        .route(
            &format!("{API_PREFIX}/gateway/ca/root.pem"),
            get(gateway_root_pem),
        )
        .route(
            &format!("{API_PREFIX}/gateway/ca/reset"),
            post(reset_gateway_ca),
        )
        .route(
            &format!("{API_PREFIX}/gateway/ca/trust-result"),
            post(gateway_trust_result),
        )
        .route(&format!("{API_PREFIX}/profile/plan"), post(profile_plan))
        .route(
            &format!("{API_PREFIX}/profile/import"),
            post(profile_import),
        )
        .route(&format!("{API_PREFIX}/shutdown"), post(shutdown))
        .route(&format!("{API_PREFIX}/logs"), get(logs))
        .layer(axum::middleware::from_fn_with_state(
            Arc::clone(&state),
            auth::require_token,
        ));

    // CORS 必须是最外层：预检请求（OPTIONS）不带 Authorization，
    // 若放在鉴权里面会被拦成 401，浏览器随后报 CORS 失败。
    public
        .merge(protected)
        .with_state(state)
        .layer(cors_layer())
}

// ─────────────────────────── 基础 ───────────────────────────

async fn healthz(State(state): State<Arc<AppState>>) -> Json<HealthResponse> {
    Json(HealthResponse::ok(
        env!("CARGO_PKG_VERSION"),
        state.services.runtime.process().instance_id.clone(),
    ))
}

async fn whoami(State(state): State<Arc<AppState>>) -> Result<Json<WhoamiResponse>, ApiFailure> {
    state.services.whoami().map(Json).map_err(registry_failure)
}

fn registry_failure(err: registry::RegistryError) -> ApiFailure {
    match err.field {
        Some(field) => {
            ApiFailure::field(StatusCode::CONFLICT, "registry_error", err.message, field)
        }
        None => ApiFailure::conflict("registry_error", err.message),
    }
}

// ─────────────────────────── 服务器 ───────────────────────────

/// 状态变更事件流（SSE）。
///
/// 只推"变了"这一件事，不带载荷：界面收到就重拉一次 `/tunnels`。
/// 这样核心不用维护增量补丁，界面也不会因为漏事件而长期跑偏。
async fn stream_events(
    State(state): State<Arc<AppState>>,
) -> Sse<impl tokio_stream::Stream<Item = Result<Event, std::convert::Infallible>>> {
    use tokio_stream::StreamExt;

    let receiver = state.services.subscribe();
    let stream = tokio_stream::wrappers::BroadcastStream::new(receiver)
        // 落后丢事件（Lagged）也当成"变了"处理：重拉一次自然对齐。
        .map(|_| Ok(Event::default().event("changed").data("1")));

    Sse::new(stream).keep_alive(KeepAlive::default())
}

async fn list_servers(State(state): State<Arc<AppState>>) -> Result<Json<ServerList>, ApiFailure> {
    state.services.servers.list().map(Json).map_err(Into::into)
}

async fn reorder_servers(
    State(state): State<Arc<AppState>>,
    Json(req): Json<tl_protocol::ReorderServersRequest>,
) -> Result<StatusCode, ApiFailure> {
    state.services.reorder_servers(&req.ids)?;
    Ok(StatusCode::NO_CONTENT)
}

async fn create_server(
    State(state): State<Arc<AppState>>,
    Json(req): Json<CreateServerRequest>,
) -> Result<(StatusCode, Json<ServerDto>), ApiFailure> {
    // 登录材料随隧道一起创建（界面上没有「凭据」这一层）。
    let dto = state.services.create_server(req).map_err(|e| {
        let message = e.to_string();
        if message.contains("已存在") {
            ApiFailure::conflict("invalid_reference", message)
        } else {
            e.into()
        }
    })?;
    Ok((StatusCode::CREATED, Json(dto)))
}

/// 打开/关闭隧道：开关只改"意图"，会话与转发随之建立/停止。
async fn set_server_enabled(
    State(state): State<Arc<AppState>>,
    AxumPath(id): AxumPath<String>,
    Json(req): Json<SetServerEnabledRequest>,
) -> Result<Json<ServerEnabledResult>, ApiFailure> {
    let affected = state
        .services
        .set_server_enabled(&id, req.enabled)
        .await
        .map_err(|e| {
            // 专门给界面一个可识别的码：它要提示「先填密码」，而不是弹红色错误横幅。
            if e.to_string().contains("还没填密码") {
                ApiFailure::conflict("credential_missing", e.to_string())
            } else {
                ApiFailure::from(e)
            }
        })?;
    let connected = state.services.session_connected(&id);
    if !req.enabled {
        return Ok(Json(ServerEnabledResult {
            enabled: false,
            session_connected: connected,
            affected_forwards: affected,
        }));
    }
    Ok(Json(ServerEnabledResult {
        enabled: true,
        session_connected: connected,
        affected_forwards: affected,
    }))
}

/// 这条隧道的 SSH 会话状态（会话按隧道共享）。
async fn server_session(
    State(state): State<Arc<AppState>>,
    AxumPath(id): AxumPath<String>,
) -> Result<Json<SessionState>, ApiFailure> {
    // 先确认隧道存在，否则不存在的 id 会被当成"未连接"，把错误藏起来。
    if !state
        .services
        .servers
        .list()?
        .items
        .iter()
        .any(|server| server.id == id)
    {
        return Err(anyhow::anyhow!("隧道不存在：{id}").into());
    }
    let attached = state
        .services
        .tunnels
        .list()?
        .items
        .into_iter()
        .filter(|view| view.definition.server_id == id)
        .count();
    Ok(Json(SessionState {
        connected: state.services.session_connected(&id),
        attached_forwards: attached,
    }))
}

/// 断开隧道：挂在它下面的端口转发全部停止。
async fn disconnect_session(
    State(state): State<Arc<AppState>>,
    AxumPath(id): AxumPath<String>,
) -> Result<Json<SessionClosed>, ApiFailure> {
    let stopped = state.services.disconnect_session(&id).await?;
    Ok(Json(SessionClosed {
        stopped_forwards: stopped,
    }))
}

async fn probe_server(
    State(state): State<Arc<AppState>>,
    AxumPath(id): AxumPath<String>,
) -> Result<Json<ProbeServerReport>, ApiFailure> {
    let report = state.services.probe_server(&id).await?;
    Ok(Json(ProbeServerReport {
        stage: report.stage.as_str().to_string(),
        fingerprint: report.fingerprint,
        message: report.message,
    }))
}

/// 保存前试连：拿请求里的表单值探一次，不落库。
async fn probe_draft(
    State(state): State<Arc<AppState>>,
    Json(req): Json<ProbeDraftRequest>,
) -> Result<Json<ProbeServerReport>, ApiFailure> {
    let report = state.services.probe_draft(&req).await?;
    Ok(Json(ProbeServerReport {
        stage: report.stage.as_str().to_string(),
        fingerprint: report.fingerprint,
        message: report.message,
    }))
}

async fn patch_server(
    State(state): State<Arc<AppState>>,
    AxumPath(id): AxumPath<String>,
    Json(mut req): Json<PatchServerRequest>,
) -> Result<Json<ServerDto>, ApiFailure> {
    // `enabled` 不是纯配置：改它必须连带建立/断开会话，交给业务层。
    let enabled = req.enabled.take();
    let server = state.services.patch_server(&id, req)?;
    if let Some(enabled) = enabled {
        state.services.set_server_enabled(&id, enabled).await?;
        return Ok(Json(
            state
                .services
                .servers
                .list()?
                .items
                .into_iter()
                .find(|s| s.id == id)
                .unwrap_or(server),
        ));
    }
    Ok(Json(server))
}

#[derive(Deserialize, Default)]
struct RemoveServerQuery {
    /// 连同它下面的端口转发一起删。
    ///
    /// 界面原先自己 for 循环逐条删：一条连接挂 40 条转发就是 40 次请求、40 次
    /// 写盘、40 次 hosts 同步。交给服务端一次做完，只写一次。
    #[serde(default)]
    cascade: bool,
}

async fn remove_server(
    State(state): State<Arc<AppState>>,
    AxumPath(id): AxumPath<String>,
    Query(query): Query<RemoveServerQuery>,
) -> Result<StatusCode, ApiFailure> {
    if query.cascade {
        state.services.remove_forwards_of_server(&id).await?;
    } else {
        // 规则归 tunnel 模型：它回答"这个服务器还被哪些转发引用"。
        match state.services.tunnels.can_remove_server(&id)? {
            crate::models::RemoveDecision::Allowed => {}
            crate::models::RemoveDecision::Blocked { referenced_by } => {
                // 报错是给用户看的：说清"还剩几条、叫什么"，不要甩内部 id。
                let names: Vec<String> = state
                    .services
                    .tunnels
                    .list()?
                    .items
                    .into_iter()
                    .filter(|view| view.definition.server_id == id)
                    .map(|view| view.definition.name)
                    .collect();
                let labels = if names.is_empty() {
                    referenced_by.join("、")
                } else {
                    names.join("、")
                };
                let count = names.len().max(referenced_by.len());
                return Err(ApiFailure::conflict(
                    "server_in_use",
                    format!("这条隧道下还有 {count} 条端口转发（{labels}），请先删除它们"),
                ));
            }
        }
    }
    // 先收运行时再删配置：否则 SSH 会话会留在内存里，界面继续显示「已连接」，
    // 而这条连接其实已经没了（同名连接再导入时还会继承这个假连接）。
    // 这里是尽力而为：断会话失败也不该让用户删不掉配置。
    let _ = state.services.forget_server(&id).await;
    state.services.servers.remove(&id)?;
    Ok(StatusCode::NO_CONTENT)
}

async fn host_key(
    State(state): State<Arc<AppState>>,
    AxumPath(id): AxumPath<String>,
) -> Result<Json<HostKeyView>, ApiFailure> {
    state
        .services
        .servers
        .host_key_view(&id)
        .map(Json)
        .map_err(Into::into)
}

async fn accept_host_key(
    State(state): State<Arc<AppState>>,
    AxumPath(id): AxumPath<String>,
    Json(req): Json<AcceptHostKeyRequest>,
) -> Result<Json<ServerDto>, ApiFailure> {
    let reject = req.reject;
    let server = state
        .services
        .servers
        .accept_host_key(&id, req)
        .map_err(|e| {
            let message = e.to_string();
            if message.contains("没有待确认") {
                ApiFailure::conflict("no_pending_host_key", message)
            } else {
                e.into()
            }
        })?;

    // 确认之后把会话与"启用"的端口拉起来：否则用户点完信任，
    // 界面还是"未连接"——因为他刚处理完的那次失败已经把守护停掉了。
    if !reject {
        let _ = state.services.after_host_key_accepted(&id).await;
    }

    Ok(Json(server))
}

// ─────────────────────────── 隧道 ───────────────────────────

async fn list_tunnels(State(state): State<Arc<AppState>>) -> Result<Json<TunnelList>, ApiFailure> {
    state.services.tunnels.list().map(Json).map_err(Into::into)
}

async fn create_tunnel(
    State(state): State<Arc<AppState>>,
    Json(req): Json<CreateTunnelRequest>,
) -> Result<(StatusCode, Json<TunnelView>), ApiFailure> {
    let status = state.services.create_tunnel(req).await.map_err(|e| {
        let message = e.to_string();
        if message.contains("服务器不存在")
            || message.contains("已存在")
            || message.contains("已被")
        {
            ApiFailure::conflict("invalid_reference", message)
        } else {
            e.into()
        }
    })?;
    // 与 GET /tunnels 的每一项同形（definition + status），避免两套响应形状。
    let view = state.services.tunnels.view(&status.id)?;
    Ok((StatusCode::CREATED, Json(view)))
}

async fn patch_tunnel(
    State(state): State<Arc<AppState>>,
    AxumPath(id): AxumPath<String>,
    Json(req): Json<PatchTunnelRequest>,
) -> Result<Json<TunnelView>, ApiFailure> {
    state.services.patch_tunnel(&id, req).await?;
    Ok(Json(state.services.tunnels.view(&id)?))
}

async fn remove_tunnel(
    State(state): State<Arc<AppState>>,
    AxumPath(id): AxumPath<String>,
) -> Result<StatusCode, ApiFailure> {
    state.services.remove_tunnel(&id).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn start_tunnel(
    State(state): State<Arc<AppState>>,
    AxumPath(id): AxumPath<String>,
) -> Result<Json<TunnelView>, ApiFailure> {
    match state.services.start_tunnel(&id).await {
        Ok(()) => Ok(Json(state.services.tunnels.view(&id)?)),
        Err(e) => {
            // 启动失败时模型已把状态推进到 Error，这里返回最新状态供 UI 展示原因。
            let message = e.to_string();
            let status = state.services.tunnels.status(&id).ok();
            Err(match status {
                Some(status) if status.state == tl_protocol::TunnelState::Error => {
                    ApiFailure::conflict("tunnel_start_failed", message)
                }
                _ => e.into(),
            })
        }
    }
}

async fn stop_tunnel(
    State(state): State<Arc<AppState>>,
    AxumPath(id): AxumPath<String>,
) -> Result<Json<TunnelView>, ApiFailure> {
    state.services.stop_tunnel(&id).await?;
    Ok(Json(state.services.tunnels.view(&id)?))
}

// ─────────────────────────── 凭据 ───────────────────────────

async fn list_credentials(
    State(state): State<Arc<AppState>>,
) -> Result<Json<CredentialList>, ApiFailure> {
    state
        .services
        .credentials
        .list()
        .map(Json)
        .map_err(Into::into)
}

async fn create_credential(
    State(state): State<Arc<AppState>>,
    Json(req): Json<CreateCredentialRequest>,
) -> Result<(StatusCode, Json<CredentialSummary>), ApiFailure> {
    let summary = state.services.credentials.add(req)?;
    Ok((StatusCode::CREATED, Json(summary)))
}

async fn patch_credential(
    State(state): State<Arc<AppState>>,
    AxumPath(id): AxumPath<String>,
    Json(req): Json<PatchCredentialRequest>,
) -> Result<Json<CredentialSummary>, ApiFailure> {
    state
        .services
        .credentials
        .update(&id, req)
        .map(Json)
        .map_err(Into::into)
}

async fn remove_credential(
    State(state): State<Arc<AppState>>,
    AxumPath(id): AxumPath<String>,
) -> Result<StatusCode, ApiFailure> {
    // 规则归 credential 模型：业务层先收集"哪些服务器在用"，模型判定能否删。
    let servers_using_it: Vec<String> = state
        .services
        .servers
        .list()?
        .items
        .into_iter()
        .filter(|s| s.credential_id == id)
        .map(|s| s.id)
        .collect();
    match state
        .services
        .credentials
        .can_remove(&id, &servers_using_it)?
    {
        crate::models::RemoveDecision::Allowed => {}
        crate::models::RemoveDecision::Blocked { referenced_by } => {
            return Err(ApiFailure::conflict(
                "credential_in_use",
                format!("凭据仍被服务器引用：{}", referenced_by.join(", ")),
            ))
        }
    }
    state.services.credentials.remove(&id)?;
    Ok(StatusCode::NO_CONTENT)
}

// ─────────────────────────── 自启开关 ───────────────────────────

async fn get_autostart(
    State(state): State<Arc<AppState>>,
) -> Result<Json<AutostartStatus>, ApiFailure> {
    state
        .services
        .autostart_status()
        .map(Json)
        .map_err(registry_failure)
}

async fn set_autostart(
    State(state): State<Arc<AppState>>,
    Json(req): Json<SetAutostartRequest>,
) -> Result<Json<AutostartStatus>, ApiFailure> {
    match state
        .services
        .set_autostart(req.enabled, &state.exe_path, &state.log_dir)
    {
        Ok(status) => Ok(Json(status)),
        Err(AutostartError::PrivilegeRequired) => Err(ApiFailure::conflict(
            "privilege_required",
            "修改开机自启需要管理员权限（将由界面引导一次提权）",
        )),
        Err(AutostartError::Registry(err)) => Err(registry_failure(err)),
    }
}

// ─────────────────────────── 域名访问 / hosts ───────────────────────────

async fn get_hosts(State(state): State<Arc<AppState>>) -> Result<Json<HostsStatus>, ApiFailure> {
    state.services.hosts.status().map(Json).map_err(Into::into)
}

/// 尝试写一次托管区块。写不进去（需要提权）返回 **409**，保存动作本身不算失败。
async fn sync_hosts(
    State(state): State<Arc<AppState>>,
) -> Result<(StatusCode, Json<HostsSyncResult>), ApiFailure> {
    let outcome = state.services.sync_hosts()?;
    let status = state.services.hosts.status()?;
    let code = if outcome.written {
        StatusCode::OK
    } else {
        StatusCode::CONFLICT
    };
    Ok((
        code,
        Json(HostsSyncResult {
            written: outcome.written,
            needs_elevation: outcome.needs_elevation,
            status,
        }),
    ))
}

async fn clear_hosts(
    State(state): State<Arc<AppState>>,
) -> Result<(StatusCode, Json<HostsSyncResult>), ApiFailure> {
    let outcome = state.services.clear_hosts()?;
    let status = state.services.hosts.status()?;
    let code = if outcome.written {
        StatusCode::OK
    } else {
        StatusCode::CONFLICT
    };
    Ok((
        code,
        Json(HostsSyncResult {
            written: outcome.written,
            needs_elevation: outcome.needs_elevation,
            status,
        }),
    ))
}

/// Agent 回报代理例外同步结果（Core 是 SYSTEM 时读不到登录用户的 HKCU）。
async fn proxy_result(
    State(state): State<Arc<AppState>>,
    Json(result): Json<ProxySyncResult>,
) -> Result<StatusCode, ApiFailure> {
    state.services.hosts.apply_proxy_result(result)?;
    state.services.notify_changed();
    Ok(StatusCode::NO_CONTENT)
}

async fn clear_proxy(State(state): State<Arc<AppState>>) -> Result<StatusCode, ApiFailure> {
    state.services.hosts.clear_proxy_managed()?;
    state.services.notify_changed();
    Ok(StatusCode::NO_CONTENT)
}

// ─────────────────────────── 本地网关证书 ───────────────────────────

async fn get_gateway_ca(
    State(state): State<Arc<AppState>>,
) -> Result<Json<GatewayCaStatus>, ApiFailure> {
    state
        .services
        .gateway
        .status()
        .map(Json)
        .map_err(Into::into)
}

/// 下载兜底：无可见桌面会话时装不了根，用户可以手动导入这个文件。
async fn gateway_root_pem(State(state): State<Arc<AppState>>) -> Result<String, ApiFailure> {
    state.services.gateway.root_pem().map_err(Into::into)
}

/// 删掉盘上的 CA。运行中的 ② 监听仍用旧根，必须重启 Core 才换新根。
async fn reset_gateway_ca(State(state): State<Arc<AppState>>) -> Result<StatusCode, ApiFailure> {
    state.services.gateway.reset()?;
    state.services.notify_changed();
    Ok(StatusCode::NO_CONTENT)
}

/// Agent（登录用户）上报信任探测结果；Core 只存，不自己判断。
async fn gateway_trust_result(
    State(state): State<Arc<AppState>>,
    Json(result): Json<GatewayTrustResult>,
) -> Result<StatusCode, ApiFailure> {
    state.services.gateway.apply_trust_result(result)?;
    state.services.notify_changed();
    Ok(StatusCode::NO_CONTENT)
}

// ─────────────────────────── 退出 / 日志 ───────────────────────────

async fn shutdown(State(state): State<Arc<AppState>>) -> (StatusCode, Json<ShutdownAccepted>) {
    let services = Arc::clone(&state.services);
    tokio::spawn(async move {
        // 先让出一次，确保 202 已经写回客户端，再停数据面并退出。
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        let _ = services.shutdown().await;
        std::process::exit(0);
    });
    (
        StatusCode::ACCEPTED,
        Json(ShutdownAccepted {
            accepted: true,
            retry_after_ms: 300,
        }),
    )
}

/// 导入预判：只读，不改配置。界面拿它渲染「新建/跳过/覆盖」。
async fn profile_plan(
    State(state): State<Arc<AppState>>,
    Json(req): Json<tl_protocol::ProfilePlanRequest>,
) -> Result<Json<tl_protocol::ProfilePlan>, ApiFailure> {
    state
        .services
        .profile_plan(req)
        .map(Json)
        .map_err(Into::into)
}

/// 按用户确认的结果逐条落地。部分失败不回滚已成功的条目。
async fn profile_import(
    State(state): State<Arc<AppState>>,
    Json(req): Json<tl_protocol::ProfileImportRequest>,
) -> Result<Json<tl_protocol::ProfileImportResult>, ApiFailure> {
    state
        .services
        .import_profile(req)
        .map(Json)
        .map_err(Into::into)
}

#[derive(Debug, Deserialize)]
struct LogsQuery {
    tail: Option<usize>,
}

const DEFAULT_TAIL: usize = 200;
const MAX_TAIL: usize = 2_000;

async fn logs(
    State(state): State<Arc<AppState>>,
    Query(query): Query<LogsQuery>,
) -> Result<Json<LogsResponse>, ApiFailure> {
    let requested = query.tail.unwrap_or(DEFAULT_TAIL);
    if requested == 0 {
        return Err(ApiFailure::field(
            StatusCode::BAD_REQUEST,
            "invalid_tail",
            "tail 必须大于 0",
            "tail",
        ));
    }
    let truncated = requested > MAX_TAIL;
    let limit = requested.min(MAX_TAIL);
    let (lines, file_truncated) = tail_lines(&state.log_dir, limit)?;
    Ok(Json(LogsResponse {
        lines,
        truncated: truncated || file_truncated,
    }))
}

/// 读日志目录下最新文件的最后 `limit` 行。目录或文件不存在时返回空（不是错误）。
fn tail_lines(dir: &Path, limit: usize) -> Result<(Vec<String>, bool), ApiFailure> {
    let mut newest: Option<(std::time::SystemTime, PathBuf)> = None;
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(_) => return Ok((Vec::new(), false)),
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let modified = entry
            .metadata()
            .and_then(|m| m.modified())
            .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
        if newest.as_ref().map(|(t, _)| modified > *t).unwrap_or(true) {
            newest = Some((modified, path));
        }
    }

    let Some((_, path)) = newest else {
        return Ok((Vec::new(), false));
    };
    let content = match std::fs::read_to_string(&path) {
        Ok(content) => content,
        Err(_) => return Ok((Vec::new(), false)),
    };
    let all: Vec<&str> = content.lines().collect();
    let truncated = all.len() > limit;
    let start = all.len().saturating_sub(limit);
    Ok((
        all[start..].iter().map(|s| s.to_string()).collect(),
        truncated,
    ))
}

/// 生成一个 token 并打印到日志（壳层调用）。
///
/// CSPRNG 失败时**不降级**：时间戳派生一天只有 86400 种可能，等于把控制面敞开。
/// 宁可启动失败，也不带着弱 token 对外服务。
pub fn new_token() -> String {
    token_gen::random_token(32)
        .expect("CSPRNG unavailable: refusing to start with a weak control token")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 最小可用的 AppState（临时目录，不触碰真实数据）。
    fn test_app() -> Router {
        let dir = std::env::temp_dir().join(format!("threadline-cors-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let credentials = Arc::new(crate::models::credential::CredentialStore::new(
            dir.join("credentials.dat"),
        ));
        let servers = Arc::new(crate::models::server::ServerStore::new(
            dir.join("servers.json"),
            Arc::clone(&credentials),
        ));
        let tunnels = Arc::new(crate::models::tunnel::TunnelStore::new(
            dir.join("tunnels.json"),
            Arc::clone(&servers),
        ));
        let runtime = Arc::new(crate::models::runtime::RuntimeStore::new(
            &dir,
            crate::models::runtime::ProcessIdentity {
                instance_id: "test".into(),
                version: "0.1.0".into(),
                run_mode: tl_protocol::RunMode::Manual,
                identity: "user:test".into(),
            },
        ));
        router(AppState::new(
            Services::new(credentials, servers, tunnels, runtime, &dir),
            "token",
            dir.join("logs"),
            "tl-core.exe",
        ))
    }

    #[tokio::test]
    async fn gateway_ca_status_and_trust_result_round_trip() {
        use tower::ServiceExt;

        let app = test_app();
        let response = app
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .uri("/api/v1/gateway/ca")
                    .header("authorization", "Bearer token")
                    .body(axum::body::Body::empty())
                    .expect("request"),
            )
            .await
            .expect("call");
        assert_eq!(response.status(), StatusCode::OK);

        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body");
        let body: serde_json::Value = serde_json::from_slice(&bytes).expect("json");
        let thumbprint = body["thumbprint"].as_str().expect("thumbprint").to_string();
        assert_eq!(thumbprint.len(), 64, "展示用 SHA-256");
        let sha1 = body["thumbprint_sha1"]
            .as_str()
            .expect("thumbprint_sha1")
            .to_string();
        assert_eq!(sha1.len(), 40, "查库用 SHA-1");
        assert!(
            body["trusted"].is_null(),
            "还没上报过，必须是 null 而不是 false"
        );

        let response = app
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri("/api/v1/gateway/ca/trust-result")
                    .header("authorization", "Bearer token")
                    .header("content-type", "application/json")
                    .body(axum::body::Body::from(format!(
                        r#"{{"trusted":true,"thumbprint":"{thumbprint}","at":"2026-11-12T00:00:00Z"}}"#
                    )))
                    .expect("request"),
            )
            .await
            .expect("call");
        assert_eq!(response.status(), StatusCode::NO_CONTENT);

        let response = app
            .oneshot(
                axum::http::Request::builder()
                    .uri("/api/v1/gateway/ca")
                    .header("authorization", "Bearer token")
                    .body(axum::body::Body::empty())
                    .expect("request"),
            )
            .await
            .expect("call");
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body");
        let body: serde_json::Value = serde_json::from_slice(&bytes).expect("json");
        assert_eq!(body["trusted"], serde_json::Value::Bool(true));
        assert_eq!(body["last_probed_at"], "2026-11-12T00:00:00Z");
    }

    #[tokio::test]
    async fn hosts_status_shape_and_proxy_result_round_trip() {
        use tower::ServiceExt;

        let app = test_app();
        let response = app
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .uri("/api/v1/hosts")
                    .header("authorization", "Bearer token")
                    .body(axum::body::Body::empty())
                    .expect("request"),
            )
            .await
            .expect("call");
        assert_eq!(response.status(), StatusCode::OK);

        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body");
        let body: serde_json::Value = serde_json::from_slice(&bytes).expect("json");
        assert!(body["path"].as_str().unwrap_or_default().ends_with("hosts"));
        assert!(body["managed"].is_array());
        assert!(body["proxy"].is_object());
        assert!(body["needs_elevation"].is_boolean());

        let response = app
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri("/api/v1/hosts/proxy-result")
                    .header("authorization", "Bearer token")
                    .header("content-type", "application/json")
                    .body(axum::body::Body::from(
                        r#"{"ok":true,"applied":["home.example.com"],"at":"2026-11-12T00:00:00Z"}"#,
                    ))
                    .expect("request"),
            )
            .await
            .expect("call");
        assert_eq!(response.status(), StatusCode::NO_CONTENT);

        let dir = std::env::temp_dir().join(format!("threadline-cors-{}", std::process::id()));
        let raw =
            std::fs::read_to_string(dir.join("proxy-overrides.json")).expect("管理集应已落盘");
        let stored: Vec<String> = serde_json::from_str(&raw).expect("json");
        assert_eq!(stored, vec!["home.example.com".to_string()]);
    }

    #[tokio::test]
    async fn preflight_from_tauri_origin_is_allowed_without_token() {
        use tower::ServiceExt;

        let response = test_app()
            .oneshot(
                axum::http::Request::builder()
                    .method("OPTIONS")
                    .uri(format!("{API_PREFIX}/whoami"))
                    .header("Origin", "http://tauri.localhost")
                    .header("Access-Control-Request-Method", "GET")
                    .header("Access-Control-Request-Headers", "authorization")
                    .body(axum::body::Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");

        assert!(
            response.status().is_success(),
            "预检不能被鉴权拦成 {:?}",
            response.status()
        );
        assert_eq!(
            response
                .headers()
                .get("access-control-allow-origin")
                .map(|v| v.to_str().unwrap_or_default()),
            Some("http://tauri.localhost"),
            "WebView 的跨源请求必须拿到允许头，否则前端只会看到 Failed to fetch"
        );
    }

    #[tokio::test]
    async fn actual_response_carries_allow_origin_header() {
        use tower::ServiceExt;

        let response = test_app()
            .oneshot(
                axum::http::Request::builder()
                    .uri(format!("{API_PREFIX}/healthz"))
                    .header("Origin", "http://tauri.localhost")
                    .body(axum::body::Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");

        assert!(response.status().is_success());
        assert!(
            response
                .headers()
                .get("access-control-allow-origin")
                .is_some(),
            "普通响应也要带允许头"
        );
    }

    #[test]
    fn missing_log_dir_yields_empty_logs() {
        let (lines, truncated) =
            tail_lines(Path::new("C:\\definitely\\no\\such\\dir"), 10).expect("no error");
        assert!(lines.is_empty());
        assert!(!truncated);
    }

    #[test]
    fn tail_returns_last_lines() {
        let dir = std::env::temp_dir().join(format!("threadline-logs-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("mkdir");
        let file = dir.join("core-20261019.log");
        std::fs::write(&file, "1\n2\n3\n4\n5\n").expect("write");

        let (lines, truncated) = tail_lines(&dir, 2).expect("tail");
        assert_eq!(lines, vec!["4".to_string(), "5".to_string()]);
        assert!(truncated);

        let (all, not_truncated) = tail_lines(&dir, 10).expect("tail");
        assert_eq!(all.len(), 5);
        assert!(!not_truncated);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn token_is_non_empty() {
        assert!(!new_token().is_empty());
        assert_ne!(new_token(), new_token());
    }
}
