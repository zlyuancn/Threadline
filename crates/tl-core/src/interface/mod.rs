//! 接口层：HTTP 协议适配。
//!
//! 归属：**接口层**。只做：路由、鉴权、请求体反序列化、调用业务层、把结果/错误翻译成 HTTP。
//! **零业务规则**——「能不能删」「要不要提权」全部由模型/业务层回答。
//! 允许调用：`business`（用例）、`models`（只读查询）、`tl_protocol`（契约）。

pub mod auth;
pub mod error;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path as AxumPath, Query, State};
use axum::http::StatusCode;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::Response;
use axum::routing::{get, patch, post, put};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use tl_protocol::{
    AcceptHostKeyRequest, AppGroupList, AutostartStatus, ContainerAction, ContainerActionResult,
    ContainerDetail, ContainerList, ContainerLogs, ContainerStats, CreateCredentialRequest,
    CreateServerRequest, CreateTunnelRequest, CredentialList, CredentialSummary,
    DeleteAppGroupRequest, GatewayCaStatus, GatewayTrustResult, HealthResponse, HostKeyView,
    HostsStatus, HostsSyncResult, LogsResponse, PatchCredentialRequest, PatchServerRequest,
    PatchTunnelRequest, ProbeDraftRequest, ProbeServerReport, ProxySyncResult,
    RenameAppGroupRequest, ReorderAppRequest, ServerDto, ServerEnabledResult, ServerList,
    ServerOverview, SessionClosed, SessionState, SetAppGroupsRequest, SetAutostartRequest,
    SetServerEnabledRequest, ShutdownAccepted, TunnelList, TunnelView, VmAction, VmActionResult,
    VmDetail, VmList, WhoamiResponse, API_PREFIX,
};

use crate::business::admin::ConsoleGuard;
use crate::business::Services;
use crate::foundation::{registry, token as token_gen};
use crate::models::runtime::AutostartError;
use crate::models::server_admin::ConsoleMode;
use crate::models::tunnel::engine::{PtyChannel, PtyCommand, PtyEvent};

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
        // 管理页：只读采集。后续模块（进程/文件/终端…）沿用同一个 `admin/` 前缀。
        .route(
            &format!("{API_PREFIX}/servers/{{id}}/admin/overview"),
            get(server_admin_overview),
        )
        // 容器管理：列表 / 资源 / 详情 / 日志 / 动作 / 删除。
        // `/stats` 是静态段，注册在 `{cid}` 之前（axum 静态优先，不依赖顺序，但读起来明确）。
        .route(
            &format!("{API_PREFIX}/servers/{{id}}/admin/containers"),
            get(server_containers),
        )
        .route(
            &format!("{API_PREFIX}/servers/{{id}}/admin/containers/stats"),
            get(server_container_stats),
        )
        .route(
            &format!("{API_PREFIX}/servers/{{id}}/admin/containers/{{cid}}"),
            get(server_container_detail).delete(server_container_remove),
        )
        .route(
            &format!("{API_PREFIX}/servers/{{id}}/admin/containers/{{cid}}/logs"),
            get(server_container_logs),
        )
        .route(
            &format!("{API_PREFIX}/servers/{{id}}/admin/containers/{{cid}}/action"),
            post(server_container_action),
        )
        // 控制台：WebSocket。PTY 是双向的，请求-响应装不下。
        .route(
            &format!("{API_PREFIX}/servers/{{id}}/admin/containers/{{cid}}/console"),
            get(server_container_console),
        )
        // 虚拟机管理：列表 / 详情 / 动作 / 自启 / 串口控制台。
        .route(
            &format!("{API_PREFIX}/servers/{{id}}/admin/vms"),
            get(server_vms),
        )
        .route(
            &format!("{API_PREFIX}/servers/{{id}}/admin/vms/{{name}}"),
            get(server_vm_detail),
        )
        .route(
            &format!("{API_PREFIX}/servers/{{id}}/admin/vms/{{name}}/action"),
            post(server_vm_action),
        )
        .route(
            &format!("{API_PREFIX}/servers/{{id}}/admin/vms/{{name}}/autostart"),
            post(server_vm_autostart),
        )
        .route(
            &format!("{API_PREFIX}/servers/{{id}}/admin/vms/{{name}}/console"),
            get(server_vm_console),
        )
        // 服务器终端：同样是 WS（登录 shell），与容器控制台共用一条桥。
        .route(
            &format!("{API_PREFIX}/servers/{{id}}/admin/terminal"),
            get(server_terminal),
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
        // 应用页布局：分组清单与磁贴顺序。**只影响界面排列**，不动任何转发的运行态。
        .route(
            &format!("{API_PREFIX}/app-groups"),
            get(list_app_groups).put(set_app_groups),
        )
        .route(
            &format!("{API_PREFIX}/app-groups/rename"),
            post(rename_app_group),
        )
        .route(
            &format!("{API_PREFIX}/app-groups/delete"),
            post(delete_app_group),
        )
        .route(
            &format!("{API_PREFIX}/tunnels"),
            get(list_tunnels).post(create_tunnel),
        )
        // 静态段放在 `{id}` 之前：与 `/servers/order` 同一套写法。
        .route(&format!("{API_PREFIX}/tunnels/app-order"), put(reorder_app))
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

/// 管理页概览：借这条隧道的会话执行一次只读采集脚本。
///
/// `?probe=live|disk-io|disks` 只采对应那几块（每秒 / 每 5 秒 / 每 10 秒各一条），
/// 缺省是完整采集。拆开的理由是每秒跑 `df` 会在挂了网络盘时把整个脚本拖住。
async fn server_admin_overview(
    State(state): State<Arc<AppState>>,
    AxumPath(id): AxumPath<String>,
    Query(query): Query<OverviewQuery>,
) -> Result<Json<ServerOverview>, ApiFailure> {
    let probe = crate::models::server_admin::Probe::parse(query.probe.as_deref().unwrap_or("full"));
    let overview = state.services.server_overview(&id, probe).await?;
    Ok(Json(overview))
}

/// 概览接口的查询参数。缺省 = 完整采集。
#[derive(Debug, Default, Deserialize)]
struct OverviewQuery {
    #[serde(default)]
    probe: Option<String>,
}

// ── 容器管理 ──────────────────────────────────────────────

async fn server_containers(
    State(state): State<Arc<AppState>>,
    AxumPath(id): AxumPath<String>,
) -> Result<Json<ContainerList>, ApiFailure> {
    Ok(Json(state.services.containers(&id).await?))
}

async fn server_container_stats(
    State(state): State<Arc<AppState>>,
    AxumPath(id): AxumPath<String>,
) -> Result<Json<Vec<ContainerStats>>, ApiFailure> {
    Ok(Json(state.services.container_stats(&id).await?))
}

async fn server_container_detail(
    State(state): State<Arc<AppState>>,
    AxumPath((id, cid)): AxumPath<(String, String)>,
) -> Result<Json<ContainerDetail>, ApiFailure> {
    Ok(Json(state.services.container_detail(&id, &cid).await?))
}

/// 日志查询参数：`tail` 有上限（核心截断到 1000），`since` 只放行时间戳字符。
#[derive(Debug, Default, Deserialize)]
struct ContainerLogsQuery {
    #[serde(default)]
    tail: Option<usize>,
    #[serde(default)]
    since: Option<String>,
}

async fn server_container_logs(
    State(state): State<Arc<AppState>>,
    AxumPath((id, cid)): AxumPath<(String, String)>,
    Query(query): Query<ContainerLogsQuery>,
) -> Result<Json<ContainerLogs>, ApiFailure> {
    let tail = query.tail.unwrap_or(200);
    let logs = state
        .services
        .container_logs(&id, &cid, tail, query.since.as_deref())
        .await?;
    Ok(Json(logs))
}

#[derive(Debug, Deserialize)]
struct ContainerActionRequest {
    action: String,
}

/// 动作字符串 → 枚举。**不把前端传的字符串拼进命令**。
///
/// 只有启动 / 停止 / 重启。`pause` / `unpause` 会落到 `other` 分支被拒——
/// 本产品不做暂停（cgroup 冻结对在线服务是「挂起等超时」，比 stop 更难排查）。
fn parse_action(raw: &str) -> Result<ContainerAction, ApiFailure> {
    match raw {
        "start" => Ok(ContainerAction::Start),
        "stop" => Ok(ContainerAction::Stop),
        "restart" => Ok(ContainerAction::Restart),
        other => Err(ApiFailure::bad_request(
            "invalid_action",
            format!("不支持的操作：{other}"),
        )),
    }
}

async fn server_container_action(
    State(state): State<Arc<AppState>>,
    AxumPath((id, cid)): AxumPath<(String, String)>,
    Json(req): Json<ContainerActionRequest>,
) -> Result<Json<ContainerActionResult>, ApiFailure> {
    let action = parse_action(&req.action)?;
    Ok(Json(
        state.services.container_action(&id, &cid, action).await?,
    ))
}

/// 删除参数：`force=1` 处理运行中的容器，`image=1` 连镜像一起删。
#[derive(Debug, Default, Deserialize)]
struct ContainerRemoveQuery {
    #[serde(default)]
    force: Option<String>,
    #[serde(default)]
    image: Option<String>,
}

fn flag(raw: Option<&str>) -> bool {
    matches!(raw, Some("1") | Some("true"))
}

async fn server_container_remove(
    State(state): State<Arc<AppState>>,
    AxumPath((id, cid)): AxumPath<(String, String)>,
    Query(query): Query<ContainerRemoveQuery>,
) -> Result<Json<ContainerActionResult>, ApiFailure> {
    let result = state
        .services
        .container_remove(
            &id,
            &cid,
            flag(query.force.as_deref()),
            flag(query.image.as_deref()),
        )
        .await?;
    Ok(Json(result))
}

// ── 虚拟机管理 ────────────────────────────────────────────

async fn server_vms(
    State(state): State<Arc<AppState>>,
    AxumPath(id): AxumPath<String>,
) -> Result<Json<VmList>, ApiFailure> {
    Ok(Json(state.services.vms(&id).await?))
}

async fn server_vm_detail(
    State(state): State<Arc<AppState>>,
    AxumPath((id, name)): AxumPath<(String, String)>,
) -> Result<Json<VmDetail>, ApiFailure> {
    Ok(Json(state.services.vm_detail(&id, &name).await?))
}

#[derive(Debug, Deserialize)]
struct VmActionRequest {
    action: String,
}

/// 动作字符串 → 枚举。**不把前端传的字符串拼进命令**。
fn parse_vm_action(raw: &str) -> Result<VmAction, ApiFailure> {
    match raw {
        "start" => Ok(VmAction::Start),
        "shutdown" => Ok(VmAction::Shutdown),
        "reboot" => Ok(VmAction::Reboot),
        "suspend" => Ok(VmAction::Suspend),
        "resume" => Ok(VmAction::Resume),
        "destroy" => Ok(VmAction::Destroy),
        other => Err(ApiFailure::bad_request(
            "invalid_action",
            format!("不支持的操作：{other}"),
        )),
    }
}

async fn server_vm_action(
    State(state): State<Arc<AppState>>,
    AxumPath((id, name)): AxumPath<(String, String)>,
    Json(req): Json<VmActionRequest>,
) -> Result<Json<VmActionResult>, ApiFailure> {
    let action = parse_vm_action(&req.action)?;
    Ok(Json(state.services.vm_action(&id, &name, action).await?))
}

#[derive(Debug, Deserialize)]
struct VmAutostartRequest {
    enabled: bool,
}

async fn server_vm_autostart(
    State(state): State<Arc<AppState>>,
    AxumPath((id, name)): AxumPath<(String, String)>,
    Json(req): Json<VmAutostartRequest>,
) -> Result<Json<VmActionResult>, ApiFailure> {
    Ok(Json(
        state.services.vm_autostart(&id, &name, req.enabled).await?,
    ))
}

/// 串口控制台：`virsh console`，只能有一种模式，所以不接 `mode`。
async fn server_vm_console(
    ws: WebSocketUpgrade,
    State(state): State<Arc<AppState>>,
    AxumPath((id, name)): AxumPath<(String, String)>,
    Query(query): Query<ConsoleQuery>,
) -> Response {
    let cols = clamp_cols(query.cols.unwrap_or(100));
    let rows = clamp_rows(query.rows.unwrap_or(30));

    ws.on_upgrade(move |socket| async move {
        let opened = state
            .services
            .vm_console(&id, &name, cols, rows)
            .await
            .map(|(pty, command, guard)| (pty, command, Some(guard)))
            .map_err(|error| format!("{error:#}"));
        pty_bridge(socket, opened, "serial").await;
    })
}

// ── 控制台（WebSocket + PTY） ──────────────────────────────
//
// 帧协议（服务端 → 客户端）：
//   Text   `{"type":"hello","command":"docker exec …","mode":"exec"}`
//   Binary PTY 原始输出字节（不 base64：终端流里多字节字符会被切在包边界上）
//   Text   `{"type":"exit","code":0}` / `{"type":"error","message":"…"}`
// 客户端 → 服务端：
//   Text   `{"type":"input","data":"ls\n"}` / `{"type":"resize","cols":100,"rows":30}` / `{"type":"close"}`
//   Binary 也当作输入处理（个别前端更愿意直接发字节）

#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "lowercase")]
enum ServerFrame<'a> {
    Hello { command: &'a str, mode: &'a str },
    Exit { code: u32 },
    Error { message: &'a str },
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
enum ClientFrame {
    Input { data: String },
    Resize { cols: u32, rows: u32 },
    Close,
}

#[derive(Debug, Default, Deserialize)]
struct ConsoleQuery {
    #[serde(default)]
    mode: Option<String>,
    #[serde(default)]
    cols: Option<u32>,
    #[serde(default)]
    rows: Option<u32>,
}

/// 终端尺寸的允许区间：0 或者几万的列数会让远端 PTY 直接错乱。
fn clamp_cols(value: u32) -> u32 {
    value.clamp(20, 400)
}

fn clamp_rows(value: u32) -> u32 {
    value.clamp(5, 200)
}

async fn send_frame(socket: &mut WebSocket, frame: &ServerFrame<'_>) -> Result<(), ()> {
    let text = serde_json::to_string(frame).map_err(|_| ())?;
    socket
        .send(Message::Text(text.into()))
        .await
        .map_err(|_| ())
}

async fn server_container_console(
    ws: WebSocketUpgrade,
    State(state): State<Arc<AppState>>,
    AxumPath((id, cid)): AxumPath<(String, String)>,
    Query(query): Query<ConsoleQuery>,
) -> Result<Response, ApiFailure> {
    // 档位是枚举不是命令串：`mode` 解析不出来直接 400，不猜。
    let mode = ConsoleMode::parse(query.mode.as_deref().unwrap_or("exec")).ok_or_else(|| {
        ApiFailure::bad_request("invalid_mode", "控制台方式只支持 exec / root / debug")
    })?;
    let cols = clamp_cols(query.cols.unwrap_or(100));
    let rows = clamp_rows(query.rows.unwrap_or(30));

    Ok(ws.on_upgrade(move |socket| console_loop(socket, state, id, cid, mode, cols, rows)))
}

async fn console_loop(
    socket: WebSocket,
    state: Arc<AppState>,
    server_id: String,
    cid: String,
    mode: ConsoleMode,
    cols: u32,
    rows: u32,
) {
    // 容器不存在 / 没在运行 / 已有会话：这时已经升级成 WS 了，只能用错误帧说清楚。
    let opened = state
        .services
        .container_console(&server_id, &cid, mode, cols, rows)
        .await
        .map(|(pty, command, guard)| (pty, command, Some(guard)))
        .map_err(|error| format!("{error:#}"));

    pty_bridge(socket, opened, mode.as_str()).await;
}

/// PTY ↔ WebSocket 的双向桥。
///
/// 容器控制台与服务器终端只差"怎么打开"和 hello 帧的 mode；收发循环与退出清理
/// 必须只有一份——抄第二份迟早会在"断开后远端进程还在"上分叉。
///
/// `opened` 的第三项是容器控制台的占用凭据，在本函数里持有到结束：
/// 与改造前 `console_loop` 里的 `_guard` 普通绑定等价，互斥不会提前释放或泄漏。
async fn pty_bridge(
    mut socket: WebSocket,
    opened: Result<(PtyChannel, String, Option<ConsoleGuard>), String>,
    mode_label: &'static str,
) {
    let (pty, command, _guard) = match opened {
        Ok(parts) => parts,
        Err(message) => {
            let _ = send_frame(&mut socket, &ServerFrame::Error { message: &message }).await;
            let _ = socket.send(Message::Close(None)).await;
            return;
        }
    };

    let hello = ServerFrame::Hello {
        command: &command,
        mode: mode_label,
    };
    if send_frame(&mut socket, &hello).await.is_err() {
        return;
    }

    let (event_tx, mut event_rx) = tokio::sync::mpsc::unbounded_channel();
    let (command_tx, command_rx) = tokio::sync::mpsc::unbounded_channel();
    let pump = tokio::spawn(pty.run(command_rx, event_tx));

    loop {
        tokio::select! {
            event = event_rx.recv() => {
                match event {
                    Some(PtyEvent::Data(bytes)) => {
                        if socket.send(Message::Binary(bytes.into())).await.is_err() {
                            break;
                        }
                    }
                    Some(PtyEvent::Exit(code)) => {
                        let _ = send_frame(&mut socket, &ServerFrame::Exit { code }).await;
                        break;
                    }
                    // 对端关了：不再发 exit，前端从连接关闭就能看出会话结束
                    Some(PtyEvent::Closed) | None => break,
                }
            }
            incoming = socket.recv() => {
                match incoming {
                    Some(Ok(Message::Text(text))) => {
                        match serde_json::from_str::<ClientFrame>(&text) {
                            Ok(ClientFrame::Input { data }) => {
                                if command_tx.send(PtyCommand::Write(data.into_bytes())).is_err() {
                                    break;
                                }
                            }
                            Ok(ClientFrame::Resize { cols, rows }) => {
                                let _ = command_tx.send(PtyCommand::Resize {
                                    cols: clamp_cols(cols),
                                    rows: clamp_rows(rows),
                                });
                            }
                            Ok(ClientFrame::Close) => break,
                            // 不认识的控制帧忽略即可，没必要因此踢掉整条会话
                            Err(_) => {}
                        }
                    }
                    Some(Ok(Message::Binary(bytes))) => {
                        if command_tx.send(PtyCommand::Write(bytes.to_vec())).is_err() {
                            break;
                        }
                    }
                    Some(Ok(Message::Close(_))) | None => break,
                    Some(Ok(_)) => {}
                    Some(Err(_)) => break,
                }
            }
        }
    }

    // 丢掉指令端：PTY 任务收到 None 会自己 eof + close 远端 shell。
    // 光断 WS 不会让远端的 `docker exec` 退出——它会一直挂在那个 PTY 上。
    drop(command_tx);
    let _ = pump.await;
    let _ = socket.send(Message::Close(None)).await;
}

#[derive(Debug, Default, Deserialize)]
struct TerminalQuery {
    #[serde(default)]
    cols: Option<u32>,
    #[serde(default)]
    rows: Option<u32>,
}

/// 服务器终端：一条 WS 直连登录 shell。
///
/// 没有需要报 400 的参数（尺寸都夹在允许区间内），打开失败一律走 `error` 帧；
/// 命令由核心给（`server_admin::login_shell_command`），前端不传。
async fn server_terminal(
    ws: WebSocketUpgrade,
    State(state): State<Arc<AppState>>,
    AxumPath(id): AxumPath<String>,
    Query(query): Query<TerminalQuery>,
) -> Response {
    let cols = clamp_cols(query.cols.unwrap_or(100));
    let rows = clamp_rows(query.rows.unwrap_or(30));

    ws.on_upgrade(move |socket| async move {
        let opened = state
            .services
            .server_terminal(&id, cols, rows)
            .await
            .map(|(pty, command)| (pty, command, None))
            .map_err(|error| format!("{error:#}"));
        pty_bridge(socket, opened, "shell").await;
    })
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

// ─────────────────────────── 应用页布局 ───────────────────────────

/// 分组清单（含自动收录的组与空组）。
async fn list_app_groups(
    State(state): State<Arc<AppState>>,
) -> Result<Json<AppGroupList>, ApiFailure> {
    Ok(Json(state.services.app_groups()?))
}

/// 全量覆盖清单：新建、删除空组、调整组顺序。
async fn set_app_groups(
    State(state): State<Arc<AppState>>,
    Json(req): Json<SetAppGroupsRequest>,
) -> Result<Json<AppGroupList>, ApiFailure> {
    Ok(Json(state.services.set_app_groups(req)?))
}

/// 分组改名（成员一起改，核心一次写完）。
async fn rename_app_group(
    State(state): State<Arc<AppState>>,
    Json(req): Json<RenameAppGroupRequest>,
) -> Result<Json<AppGroupList>, ApiFailure> {
    Ok(Json(state.services.rename_app_group(req)?))
}

/// 删除分组（成员回落到「未分组」）。
async fn delete_app_group(
    State(state): State<Arc<AppState>>,
    Json(req): Json<DeleteAppGroupRequest>,
) -> Result<Json<AppGroupList>, ApiFailure> {
    Ok(Json(state.services.delete_app_group(req)?))
}

/// 磁贴顺序：只写 `app_order`，不触发任何转发启停。
async fn reorder_app(
    State(state): State<Arc<AppState>>,
    Json(req): Json<ReorderAppRequest>,
) -> Result<StatusCode, ApiFailure> {
    state.services.reorder_app(req)?;
    Ok(StatusCode::NO_CONTENT)
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

    /// 控制台帧的 JSON 形态：前端照这个实现，字段名/大小写变了就是破坏契约。
    #[test]
    fn console_frames_have_stable_shape() {
        let hello = ServerFrame::Hello {
            command: "docker exec -it web sh",
            mode: "exec",
        };
        assert_eq!(
            serde_json::to_string(&hello).expect("编码"),
            r#"{"type":"hello","command":"docker exec -it web sh","mode":"exec"}"#
        );

        let input: ClientFrame =
            serde_json::from_str(r#"{"type":"input","data":"ls\n"}"#).expect("解码 input");
        match input {
            ClientFrame::Input { data } => assert_eq!(data, "ls\n"),
            other => panic!("应为 input：{other:?}"),
        }

        let resize: ClientFrame =
            serde_json::from_str(r#"{"type":"resize","cols":120,"rows":40}"#).expect("解码 resize");
        match resize {
            ClientFrame::Resize { cols, rows } => {
                assert_eq!((cols, rows), (120, 40));
            }
            other => panic!("应为 resize：{other:?}"),
        }

        assert!(serde_json::from_str::<ClientFrame>(r#"{"type":"bogus"}"#).is_err());
    }

    /// 终端尺寸不能原样透给远端：0 或几万的列数会让 PTY 排版错乱。
    #[test]
    fn terminal_size_is_clamped() {
        assert_eq!(clamp_cols(0), 20);
        assert_eq!(clamp_cols(10_000), 400);
        assert_eq!(clamp_rows(1), 5);
        assert_eq!(clamp_rows(999), 200);
    }

    /// 本产品只做启动 / 停止 / 重启。`pause` 必须在解析阶段就被拒——
    /// 它是 cgroup 冻结，对在线服务表现为「调用方无限挂起」，比 stop 更难排查。
    #[test]
    fn parse_action_accepts_only_start_stop_restart() {
        assert!(parse_action("start").is_ok());
        assert!(parse_action("stop").is_ok());
        assert!(parse_action("restart").is_ok());

        for rejected in ["pause", "unpause", "kill", "exec", "rm", ""] {
            assert!(parse_action(rejected).is_err(), "{rejected} 不该被接受");
        }
    }

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

    /// 内置「本机」在接口上的契约：列表里有、恒已连接、写操作 409。
    #[tokio::test]
    async fn local_entry_is_listed_connected_and_immutable() {
        use tower::ServiceExt;

        let app = test_app();

        let response = app
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .uri("/api/v1/servers")
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
        let first = &body["items"][0];
        assert_eq!(first["id"], "local", "本机排在最前");
        assert_eq!(first["kind"], "local");
        assert_eq!(first["enabled"], true);
        assert_eq!(first["needs_credential"], false, "本机没有凭据概念");

        // 会话恒为已连接：界面据此显示「已连接」，不会去连 SSH。
        let response = app
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .uri("/api/v1/servers/local/session")
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
        assert_eq!(body["connected"], true);

        // 改配置 / 断路：都是「动作与状态冲突」= 409，不能报成 500。
        let response = app
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .method("PATCH")
                    .uri("/api/v1/servers/local")
                    .header("authorization", "Bearer token")
                    .header("content-type", "application/json")
                    .body(axum::body::Body::from("{}"))
                    .expect("request"),
            )
            .await
            .expect("call");
        assert_eq!(response.status(), StatusCode::CONFLICT);

        let response = app
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri("/api/v1/servers/local/session/disconnect")
                    .header("authorization", "Bearer token")
                    .body(axum::body::Body::empty())
                    .expect("request"),
            )
            .await
            .expect("call");
        assert_eq!(response.status(), StatusCode::CONFLICT);

        let response = app
            .oneshot(
                axum::http::Request::builder()
                    .method("DELETE")
                    .uri("/api/v1/servers/local")
                    .header("authorization", "Bearer token")
                    .body(axum::body::Body::empty())
                    .expect("request"),
            )
            .await
            .expect("call");
        assert_eq!(response.status(), StatusCode::CONFLICT, "本机不可删");
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

    /// 控制台的鉴权：没令牌 401；用 query 令牌能过鉴权这一关
    /// （之后的 400 是「不是 WS 升级」，说明已经进到处理器里了）。
    #[tokio::test]
    async fn console_route_accepts_query_token_only_with_token() {
        use tower::ServiceExt;

        let app = test_app();

        let response = app
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .uri("/api/v1/servers/s1/admin/containers/web/console?mode=exec")
                    .body(axum::body::Body::empty())
                    .expect("request"),
            )
            .await
            .expect("call");
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

        let response = app
            .oneshot(
                axum::http::Request::builder()
                    .uri("/api/v1/servers/s1/admin/containers/web/console?mode=exec&token=token")
                    .body(axum::body::Body::empty())
                    .expect("request"),
            )
            .await
            .expect("call");
        assert_eq!(
            response.status(),
            StatusCode::BAD_REQUEST,
            "过了鉴权后应当由 WS 提取器报「不是升级请求」"
        );
    }

    /// 服务器终端的鉴权：与容器控制台同一条规矩。
    ///
    /// 必须测路由而不是只测 `pty_query_token`：路由万一挂到 public 上，
    /// 函数级测试照样全绿，而这条 WS 就完全裸奔了。
    #[tokio::test]
    async fn terminal_route_accepts_query_token_only_with_token() {
        use tower::ServiceExt;

        let app = test_app();

        let response = app
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .uri("/api/v1/servers/s1/admin/terminal")
                    .body(axum::body::Body::empty())
                    .expect("request"),
            )
            .await
            .expect("call");
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

        let response = app
            .oneshot(
                axum::http::Request::builder()
                    .uri("/api/v1/servers/s1/admin/terminal?token=token")
                    .body(axum::body::Body::empty())
                    .expect("request"),
            )
            .await
            .expect("call");
        assert_eq!(
            response.status(),
            StatusCode::BAD_REQUEST,
            "过了鉴权后应当由 WS 提取器报「不是升级请求」"
        );
    }
}
