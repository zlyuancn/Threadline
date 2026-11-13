//! 隧道模型。
//!
//! **拥有**：`tunnels.json`（本地/远端端点、所依赖服务器、是否开机自启）与**运行态**
//! （状态机相位、活动连接数、退避预算、本地端口占用）。
//! **提供**：`list` / `create` / `patch` / `remove` / `start` / `stop` / `stop_all` / `status` /
//! `apply`（引擎事件入口）/ `is_port_taken` / `references_server`。
//! **允许调用**：`server`（校验引用、取连接参数）、`foundation::{retry, limiter, store}`。
//!
//! 纪律：
//! - 状态迁移只走 `fsm::next_state`，非法迁移直接拒绝（不允许静默忽略事件）。
//! - 监听占用由本模型登记与释放；键是 `effective_addr`（域名入口是 `(domain_host, port)`，
//!   否则是 `(local_host, port)`），**所有登记/释放/唯一性判断只走这一个函数**。
//! - 退避与并发预算来自底座，本模型不实现私有重试轮子。

pub mod engine;
pub mod fsm;

use std::collections::{HashMap, HashSet};

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use tokio::sync::broadcast;

use anyhow::{anyhow, Result};
use tl_protocol::{
    CreateTunnelRequest, PatchTunnelRequest, TunnelDto, TunnelList, TunnelState, TunnelStatus,
    TunnelView, TunnelsFile,
};

use crate::foundation::store;
use crate::models::server::ServerStore;
use crate::models::RemoveDecision;

pub use fsm::{accepts_connections, holds_listener, TunnelEvent};
pub use traffic::Traffic;

mod traffic;

use tl_protocol::SCHEMA_VERSION;

/// 域名入口回环地址池：`127.0.0.2` – `127.0.0.254`（`.1` 留给普通转发与系统）。
const DOMAIN_POOL_FIRST: u8 = 2;
const DOMAIN_POOL_LAST: u8 = 254;

/// 入口模式（只在 `domain` 非空时才有意义）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryMode {
    /// 纯字节管道（明文，或 `tcp`）。
    Plain,
    /// 本地 TLS 终止，回源明文（②）。
    Terminate,
    /// TLS 透传，回源 TLS（③）。
    Passthrough,
}

/// **唯一**的监听地址口径：有域名走 `domain_host`，否则走 `local_host`。
///
/// 登记、释放、唯一性判断、绑定都必须用它；`patch` 还必须先用它快照旧地址。
pub fn effective_addr(def: &TunnelDto) -> (&str, u16) {
    if def.domain.is_empty() || def.domain_host.is_empty() {
        (&def.local_host, def.local_port)
    } else {
        (&def.domain_host, def.local_port)
    }
}

/// 远端实际协议。`remote_scheme` 缺省时由 `scheme` 推导（`https`→`https`，其余→`http`）。
pub fn effective_remote_scheme(def: &TunnelDto) -> &str {
    match def.remote_scheme.as_deref() {
        Some("https") => "https",
        Some("http") => "http",
        _ if def.scheme == "https" => "https",
        _ => "http",
    }
}

/// 入口模式判定。`domain` 为空或 `tcp` 时恒为 `Plain`（与老版本逐字节等价）。
pub fn entry_mode(def: &TunnelDto) -> Result<EntryMode> {
    // rdp / ssh 本质是 TCP：只有默认端口不同，不参与 TLS 终止判定。
    if def.domain.is_empty() || matches!(def.scheme.as_str(), "tcp" | "rdp" | "ssh") {
        return Ok(EntryMode::Plain);
    }
    match (def.scheme.as_str(), effective_remote_scheme(def)) {
        ("http", "http") => Ok(EntryMode::Plain),
        ("http", "https") => Err(anyhow!(
            "远端是 https 时本地必须也是 https（当前本地是 http）"
        )),
        ("https", "http") => Ok(EntryMode::Terminate),
        ("https", "https") => Ok(EntryMode::Passthrough),
        (other, _) => Err(anyhow!(
            "不支持的本地协议：{other}（只能是 tcp / http / https / rdp / ssh）"
        )),
    }
}

/// 域名归一与校验。空串合法（表示不用域名访问）。
pub fn validate_domain(raw: &str) -> Result<String> {
    let d = raw.trim().trim_end_matches('.').to_ascii_lowercase();
    if d.is_empty() {
        return Ok(String::new());
    }
    if !d.is_ascii() {
        return Err(anyhow!("域名只支持 ASCII（不支持中文域名）"));
    }
    if d.len() > 253 {
        return Err(anyhow!("域名最长 253 个字符"));
    }
    if d.parse::<std::net::IpAddr>().is_ok() {
        return Err(anyhow!("域名不能是 IP 地址"));
    }
    if d == "localhost" || d.ends_with(".localhost") {
        return Err(anyhow!("不允许使用 localhost"));
    }
    if d.ends_with(".local") {
        return Err(anyhow!("不允许使用 .local 域名（mDNS 保留）"));
    }
    let parts: Vec<&str> = d.split('.').collect();
    if parts.len() < 2 {
        return Err(anyhow!("域名至少要包含一个点（如 home.example.com）"));
    }
    for part in &parts {
        if part.is_empty() || part.len() > 63 {
            return Err(anyhow!("域名每段必须是 1–63 个字符"));
        }
        if part.starts_with('-') || part.ends_with('-') {
            return Err(anyhow!("域名段不能以连字符开头或结尾"));
        }
        if !part.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
            return Err(anyhow!(
                "域名只能包含字母、数字和连字符（不含端口/空格/斜杠/冒号/下划线）"
            ));
        }
    }
    Ok(d)
}

/// 规范化 `remote_scheme`：空 / `auto` → `None`（回到由 `scheme` 推导）。
fn normalize_remote_scheme(raw: Option<String>) -> Result<Option<String>> {
    match raw.as_deref().map(str::trim) {
        None | Some("") | Some("auto") => Ok(None),
        Some("http") => Ok(Some("http".to_string())),
        Some("https") => Ok(Some("https".to_string())),
        Some(other) => Err(anyhow!(
            "不支持的远端协议：{other}（只能是 http / https / auto）"
        )),
    }
}

/// 从池里取一个未被占用的回环地址。
fn allocate_domain_host(items: &[TunnelDto], exclude: Option<&str>) -> Result<String> {
    let used: HashSet<&str> = items
        .iter()
        .filter(|t| Some(t.id.as_str()) != exclude)
        .flat_map(|t| [t.domain_host.as_str(), t.local_host.as_str()])
        .filter(|s| !s.is_empty())
        .collect();
    for n in DOMAIN_POOL_FIRST..=DOMAIN_POOL_LAST {
        let candidate = format!("127.0.0.{n}");
        if !used.contains(candidate.as_str()) {
            return Ok(candidate);
        }
    }
    Err(anyhow!("域名入口地址池已满（127.0.0.2–127.0.0.254）"))
}

/// 单条隧道的运行态（内存；不落盘）。
#[derive(Debug, Clone)]
struct Runtime {
    state: TunnelState,
    last_error: Option<String>,
    active_connections: u32,
    retry_in_secs: Option<u64>,
}

impl Runtime {
    fn new() -> Self {
        Self {
            state: TunnelState::Stopped,
            last_error: None,
            active_connections: 0,
            retry_in_secs: None,
        }
    }
}

/// 隧道仓库。
#[derive(Debug)]
pub struct TunnelStore {
    path: PathBuf,
    servers: Arc<ServerStore>,
    runtimes: Mutex<HashMap<String, Runtime>>,
    /// 监听地址 → 隧道 id（仅持有监听的隧道占用）。键是 `effective_addr`。
    ports: Mutex<HashMap<(String, u16), String>>,
    /// 每条转发的累计流量。重启转发时归零，其余时候只增不减。
    traffic: Mutex<HashMap<String, Arc<Traffic>>>,
    /// 状态变更广播。界面订阅它做实时刷新，不必靠高频轮询。
    changes: broadcast::Sender<()>,
}

impl TunnelStore {
    pub fn new(path: impl Into<PathBuf>, servers: Arc<ServerStore>) -> Self {
        Self {
            path: path.into(),
            servers,
            runtimes: Mutex::new(HashMap::new()),
            ports: Mutex::new(HashMap::new()),
            traffic: Mutex::new(HashMap::new()),
            changes: broadcast::channel(64).0,
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    fn load(&self) -> Result<TunnelsFile> {
        let mut file = store::read_json::<TunnelsFile>(&self.path)?.unwrap_or(TunnelsFile {
            schema_version: SCHEMA_VERSION,
            items: Vec::new(),
        });
        // 老版本可能留下重复 id（同名重建）。这里就地修掉并回写，
        // 否则界面上"点一个开关改两条"的问题会一直存在。
        if dedupe_ids(&mut file.items) {
            let _ = self.save(&file);
        }
        Ok(file)
    }

    fn save(&self, file: &TunnelsFile) -> Result<()> {
        store::write_json_atomic(&self.path, file)
    }

    pub fn list(&self) -> Result<TunnelList> {
        let file = self.load()?;
        let runtimes = self.runtimes.lock().expect("runtimes lock");
        let items = file
            .items
            .iter()
            .map(|def| TunnelView {
                definition: def.clone(),
                status: status_of(def, runtimes.get(&def.id), self.traffic_snapshot(&def.id)),
            })
            .collect();
        Ok(TunnelList { items })
    }

    pub fn create(&self, req: CreateTunnelRequest) -> Result<TunnelStatus> {
        validate_create(&req)?;
        if !self.servers.exists(&req.server_id)? {
            return Err(anyhow!("服务器不存在：{}", req.server_id));
        }
        // 先 load 再分配：分配要看现有配置，否则首次分配可能与已占用的回环地址撞车。
        let mut file = self.load()?;
        if file.items.iter().any(|t| t.name == req.name) {
            return Err(anyhow!("隧道名已存在：{}", req.name));
        }
        let domain = validate_domain(&req.domain)?;
        let domain_host = if domain.is_empty() {
            String::new()
        } else {
            allocate_domain_host(&file.items, None)?
        };
        let taken: HashSet<String> = file.items.iter().map(|t| t.id.clone()).collect();
        let dto = TunnelDto {
            id: new_id(&req.name, &taken),
            name: req.name,
            server_id: req.server_id,
            local_host: req.local_host,
            local_port: req.local_port,
            remote_host: req.remote_host,
            remote_port: req.remote_port,
            auto_start: req.auto_start,
            note: req.note,
            open_path: normalize_open_path(&req.open_path)?,
            scheme: req.scheme,
            domain,
            domain_host,
            remote_scheme: normalize_remote_scheme(req.remote_scheme)?,
            icon: normalize_icon(&req.icon)?,
            group: normalize_group(&req.group)?,
            in_app: req.in_app,
        };
        // 入口模式在保存时就校验：拒绝「本地 http + 远端 https」，不让坏配置落盘。
        entry_mode(&dto)?;
        ensure_unique_in(&file.items, &dto, None)?;
        file.items.push(dto.clone());
        self.save(&file)?;
        self.runtimes
            .lock()
            .expect("runtimes lock")
            .insert(dto.id.clone(), Runtime::new());
        self.notify_changed();
        Ok(status_of(&dto, None, self.traffic_snapshot(&dto.id)))
    }

    pub fn patch(&self, id: &str, req: PatchTunnelRequest) -> Result<TunnelStatus> {
        if let Some(server_id) = &req.server_id {
            if !self.servers.exists(server_id)? {
                return Err(anyhow!("服务器不存在：{server_id}"));
            }
        }
        let mut file = self.load()?;
        let index = file
            .items
            .iter()
            .position(|t| t.id == id)
            .ok_or_else(|| anyhow!("隧道不存在：{id}"))?;

        let was_running = self
            .runtimes
            .lock()
            .expect("runtimes lock")
            .get(id)
            .map(|r| holds_listener(r.state))
            .unwrap_or(false);
        // 旧监听地址必须先快照：改完再读配置拿到的是新地址，释放旧监听会漏。
        let (old_host, old_port) = {
            let (h, p) = effective_addr(&file.items[index]);
            (h.to_string(), p)
        };

        let mut candidate = file.items[index].clone();
        if let Some(port) = req.local_port {
            if port == 0 {
                return Err(anyhow!("本地端口必须在 1..=65535"));
            }
            candidate.local_port = port;
        }
        if let Some(v) = req.name {
            if v.trim().is_empty() {
                return Err(anyhow!("隧道名不能为空"));
            }
            candidate.name = v;
        }
        if let Some(v) = req.server_id {
            candidate.server_id = v;
        }
        if let Some(v) = req.local_host {
            candidate.local_host = v;
        }
        if let Some(v) = req.remote_host {
            candidate.remote_host = v;
        }
        if let Some(v) = req.remote_port {
            if v == 0 {
                return Err(anyhow!("远端端口必须在 1..=65535"));
            }
            candidate.remote_port = v;
        }
        if let Some(v) = req.auto_start {
            candidate.auto_start = v;
        }
        if let Some(v) = req.scheme {
            ensure_scheme(&v)?;
            candidate.scheme = v;
        }
        if let Some(v) = req.open_path {
            candidate.open_path = normalize_open_path(&v)?;
        }
        if let Some(v) = req.note {
            // 备注只用于展示与搜索，长度收一下口，避免有人把整段日志贴进来。
            if v.chars().count() > 120 {
                return Err(anyhow!("备注最多 120 个字符"));
            }
            candidate.note = v;
        }
        if let Some(v) = req.domain {
            let domain = validate_domain(&v)?;
            if domain.is_empty() {
                candidate.domain = String::new();
                candidate.domain_host = String::new();
            } else {
                // 已有地址且没被别的转发占用 → 复用；否则重新分配。
                let keep = !candidate.domain_host.is_empty()
                    && !file
                        .items
                        .iter()
                        .any(|t| t.id != id && t.domain_host == candidate.domain_host);
                if !keep {
                    candidate.domain_host = allocate_domain_host(&file.items, Some(id))?;
                }
                candidate.domain = domain;
            }
        }
        if let Some(v) = req.remote_scheme {
            candidate.remote_scheme = normalize_remote_scheme(Some(v))?;
        }
        if let Some(v) = req.icon {
            candidate.icon = normalize_icon(&v)?;
        }
        if let Some(v) = req.group {
            candidate.group = normalize_group(&v)?;
        }
        if let Some(v) = req.in_app {
            candidate.in_app = v;
        }

        entry_mode(&candidate)?;
        ensure_unique_in(&file.items, &candidate, Some(id))?;

        file.items[index] = candidate.clone();
        self.save(&file)?;

        // 运行中的隧道改配置 → 停干净后回到 Stopped，交由调用方重新 start。
        if was_running {
            self.release_addr(&old_host, old_port, id);
            if let Some(runtime) = self.runtimes.lock().expect("runtimes lock").get_mut(id) {
                runtime.state = TunnelState::Stopped;
                runtime.active_connections = 0;
                runtime.retry_in_secs = None;
            }
        }
        let status = status_of(
            &candidate,
            self.runtimes.lock().expect("runtimes lock").get(id),
            self.traffic_snapshot(id),
        );
        self.notify_changed();
        Ok(status)
    }

    pub fn remove(&self, id: &str) -> Result<()> {
        let mut file = self.load()?;
        let index = file
            .items
            .iter()
            .position(|t| t.id == id)
            .ok_or_else(|| anyhow!("隧道不存在：{id}"))?;
        // 先取地址再删：删掉后 load 不到这条，release_resources 会漏掉监听登记。
        let (host, port) = {
            let (h, p) = effective_addr(&file.items[index]);
            (h.to_string(), p)
        };
        file.items.remove(index);
        self.save(&file)?;
        self.release_addr(&host, port, id);
        self.notify_changed();
        self.runtimes.lock().expect("runtimes lock").remove(id);
        Ok(())
    }

    /// 一次删掉挂在某条连接下的全部转发，返回删掉的 id。
    ///
    /// 单条 `remove` 每次都 load + save，还要释放地址登记；删一条连接下 40 条
    /// 转发就是 40 次文件写、40 次变更广播、40 次 hosts 同步。批量删除只写一次。
    pub fn remove_for_server(&self, server_id: &str) -> Result<Vec<String>> {
        let mut file = self.load()?;
        let mut removed: Vec<String> = Vec::new();
        let mut addresses: Vec<(String, u16, String)> = Vec::new();
        file.items.retain(|dto| {
            if dto.server_id != server_id {
                return true;
            }
            let (host, port) = effective_addr(dto);
            addresses.push((host.to_string(), port, dto.id.clone()));
            removed.push(dto.id.clone());
            false
        });
        if removed.is_empty() {
            return Ok(removed);
        }
        self.save(&file)?;
        for (host, port, id) in addresses {
            self.release_addr(&host, port, &id);
        }
        {
            let mut runtimes = self.runtimes.lock().expect("runtimes lock");
            for id in &removed {
                runtimes.remove(id);
            }
        }
        self.notify_changed();
        Ok(removed)
    }

    /// 是否仍引用某服务器（规则归本模型，业务层提供待删的服务器 id）。
    pub fn references_server(&self, server_id: &str) -> Result<bool> {
        Ok(self.load()?.items.iter().any(|t| t.server_id == server_id))
    }

    /// 判定某服务器能否删除（被隧道引用则拒绝）。
    pub fn can_remove_server(&self, server_id: &str) -> Result<RemoveDecision> {
        let referencing: Vec<String> = self
            .load()?
            .items
            .iter()
            .filter(|t| t.server_id == server_id)
            .map(|t| t.id.clone())
            .collect();
        if referencing.is_empty() {
            return Ok(RemoveDecision::Allowed);
        }
        Ok(RemoveDecision::Blocked {
            referenced_by: referencing,
        })
    }

    /// 本地端口是否已被其它隧道占用。
    /// 订阅状态变更。接收端只用来"敲一下"，不携带数据——界面收到就重拉一次列表，
    /// 这样不用维护"增量补丁"那套容易出错的东西。
    pub fn subscribe(&self) -> broadcast::Receiver<()> {
        self.changes.subscribe()
    }

    /// 状态变了就敲一下；没人订阅时发送失败也无所谓。
    fn notify_changed(&self) {
        let _ = self.changes.send(());
    }

    /// 取这条转发的流量计数器（不存在就创建）。
    ///
    /// 引擎拿着 `Arc` 直接累加，读状态时再取快照——中间不需要锁。
    pub fn traffic_of(&self, id: &str) -> Arc<Traffic> {
        let mut map = self.traffic.lock().expect("traffic lock");
        Arc::clone(map.entry(id.to_string()).or_default())
    }

    /// 转发重新启动：重置计数，避免把上一段会话的流量算进新连接。
    pub fn reset_traffic(&self, id: &str) {
        self.traffic_of(id).reset();
    }

    fn traffic_snapshot(&self, id: &str) -> (u64, u64) {
        let map = self.traffic.lock().expect("traffic lock");
        map.get(id).map(|t| t.snapshot()).unwrap_or((0, 0))
    }

    pub fn is_port_taken(&self, port: u16) -> bool {
        self.ports
            .lock()
            .expect("ports lock")
            .keys()
            .any(|(_, p)| *p == port)
    }

    /// 配置级冲突：返回（冲突类型, 占用者名称）。
    ///
    /// 与 `is_port_taken` 的区别：那个查的是**运行期**监听表、不分 host，
    /// 只能回答"现在能不能 bind"，不能回答"配置里是否已经有一条"。
    /// 导入预判与 `create`/`patch` 共用同一套判定，避免预览与落地各说各话。
    pub fn config_conflict(
        &self,
        local_host: &str,
        local_port: u16,
        domain: &str,
        exclude_id: Option<&str>,
    ) -> Result<Option<(ConflictKind, String)>> {
        let domain = validate_domain(domain)?;
        let file = self.load()?;
        let domain_host = if domain.is_empty() {
            String::new()
        } else {
            // 与 create 相同的分配规则：模拟"新加一条"，域名入口会拿到一个新的回环地址。
            allocate_domain_host(&file.items, exclude_id)?
        };
        let candidate = TunnelDto {
            id: String::new(),
            name: String::new(),
            server_id: String::new(),
            local_host: local_host.to_string(),
            local_port,
            remote_host: String::new(),
            remote_port: 1,
            auto_start: false,
            note: String::new(),
            open_path: String::new(),
            scheme: "tcp".to_string(),
            domain,
            domain_host,
            remote_scheme: None,
            icon: String::new(),
            group: String::new(),
            in_app: true,
        };
        Ok(find_conflict(&file.items, &candidate, exclude_id))
    }

    /// 记一条**不改变状态**的错误（例如某次转发连接失败）。
    ///
    /// 这类错误不该把整条隧道打回 Stopped——会话还是好的，只是某个目标连不上；
    /// 但也不能不吭声，否则用户只看到"本地连接被重置"。
    pub fn set_last_error(&self, id: &str, message: impl Into<String>) {
        let mut runtimes = self.runtimes.lock().expect("runtimes lock");
        // 没启动过的条目也要能留错误：否则"隧道未启用"这类拒绝会毫无痕迹，
        // 用户点了启动什么也看不到。
        runtimes
            .entry(id.to_string())
            .or_insert_with(Runtime::new)
            .last_error = Some(message.into());
        self.notify_changed();
    }

    /// 单条隧道的「定义 + 运行态」。
    ///
    /// 与 `list()` 的每一项同形：创建/修改/启停都用它作响应，
    /// 避免出现「创建返回扁平状态、列表返回嵌套视图」这种两套形状。
    pub fn view(&self, id: &str) -> Result<TunnelView> {
        let file = self.load()?;
        let def = file
            .items
            .iter()
            .find(|t| t.id == id)
            .ok_or_else(|| anyhow!("隧道不存在：{id}"))?;
        let runtimes = self.runtimes.lock().expect("runtimes lock");
        Ok(TunnelView {
            definition: def.clone(),
            status: status_of(def, runtimes.get(id), self.traffic_snapshot(id)),
        })
    }

    pub fn status(&self, id: &str) -> Result<TunnelStatus> {
        let file = self.load()?;
        let def = file
            .items
            .iter()
            .find(|t| t.id == id)
            .ok_or_else(|| anyhow!("隧道不存在：{id}"))?;
        let runtimes = self.runtimes.lock().expect("runtimes lock");
        Ok(status_of(def, runtimes.get(id), self.traffic_snapshot(id)))
    }

    /// 用户/开机自启请求启动。
    pub fn start(&self, id: &str) -> Result<TunnelStatus> {
        let file = self.load()?;
        let def = file
            .items
            .iter()
            .find(|t| t.id == id)
            .ok_or_else(|| anyhow!("隧道不存在：{id}"))?;
        if !self.servers.exists(&def.server_id)? {
            return Err(anyhow!("隧道引用的服务器不存在：{}", def.server_id));
        }
        let (host, port) = {
            let (h, p) = effective_addr(def);
            (h.to_string(), p)
        };
        if !self.is_addr_taken_for_other(&host, port, id) {
            self.ports
                .lock()
                .expect("ports lock")
                .insert((host, port), id.to_string());
        }
        self.apply(id, TunnelEvent::Start)
    }

    pub fn stop(&self, id: &str) -> Result<TunnelStatus> {
        let status = self.apply(id, TunnelEvent::Stop)?;
        self.release_resources(id);
        Ok(status)
    }

    /// 停止全部隧道（`/shutdown` 的前置动作）。
    pub fn stop_all(&self) -> Result<()> {
        let ids: Vec<String> = self.load()?.items.iter().map(|t| t.id.clone()).collect();
        for id in ids {
            let _ = self.apply(&id, TunnelEvent::Stop);
            self.release_resources(&id);
        }
        Ok(())
    }

    /// 引擎事件入口：唯一的迁移入口（含用户操作与 SSH 引擎回调）。
    pub fn apply(&self, id: &str, event: TunnelEvent) -> Result<TunnelStatus> {
        let file = self.load()?;
        let def = file
            .items
            .iter()
            .find(|t| t.id == id)
            .ok_or_else(|| anyhow!("隧道不存在：{id}"))?;

        let snapshot = {
            let mut runtimes = self.runtimes.lock().expect("runtimes lock");
            let runtime = runtimes.entry(id.to_string()).or_insert_with(Runtime::new);
            let next =
                fsm::next_state(runtime.state, &event).map_err(|e| anyhow!(e.to_string()))?;
            let err = error_message(&event);
            runtime.state = next;
            if err.is_some() {
                runtime.last_error = err;
            } else if next == TunnelState::Running {
                runtime.last_error = None;
            }
            runtime.retry_in_secs = retry_hint(&event);
            if next == TunnelState::Stopped {
                runtime.active_connections = 0;
            }
            runtime.clone()
        };

        if !holds_listener(snapshot.state) {
            let (host, port) = {
                let (h, p) = effective_addr(def);
                (h.to_string(), p)
            };
            self.release_addr(&host, port, id);
        }
        self.notify_changed();
        Ok(status_of(def, Some(&snapshot), self.traffic_snapshot(id)))
    }

    fn is_addr_taken_for_other(&self, host: &str, port: u16, id: &str) -> bool {
        self.ports
            .lock()
            .expect("ports lock")
            .get(&(host.to_string(), port))
            .map(|owner| owner != id)
            .unwrap_or(false)
    }

    fn release_addr(&self, host: &str, port: u16, id: &str) {
        let mut ports = self.ports.lock().expect("ports lock");
        let key = (host.to_string(), port);
        if ports.get(&key).map(|owner| owner == id).unwrap_or(false) {
            ports.remove(&key);
        }
    }

    fn release_resources(&self, id: &str) {
        let addr = self.load().ok().and_then(|f| {
            f.items.iter().find(|t| t.id == id).map(|t| {
                let (h, p) = effective_addr(t);
                (h.to_string(), p)
            })
        });
        if let Some((host, port)) = addr {
            self.release_addr(&host, port, id);
        }
        if let Some(runtime) = self.runtimes.lock().expect("runtimes lock").get_mut(id) {
            runtime.state = TunnelState::Stopped;
            runtime.active_connections = 0;
            runtime.retry_in_secs = None;
        }
    }
}

fn status_of(def: &TunnelDto, runtime: Option<&Runtime>, bytes: (u64, u64)) -> TunnelStatus {
    let (bytes_up, bytes_down) = bytes;
    match runtime {
        Some(rt) => TunnelStatus {
            id: def.id.clone(),
            state: rt.state,
            active_connections: rt.active_connections,
            bytes_up,
            bytes_down,
            last_error: rt.last_error.clone(),
            retry_in_secs: rt.retry_in_secs,
        },
        None => TunnelStatus {
            id: def.id.clone(),
            state: TunnelState::Stopped,
            active_connections: 0,
            bytes_up,
            bytes_down,
            last_error: None,
            retry_in_secs: None,
        },
    }
}

fn error_message(event: &TunnelEvent) -> Option<String> {
    match event {
        TunnelEvent::BindFailed { reason } => Some(format!("本地端口绑定失败：{reason}")),
        TunnelEvent::AuthFailed { reason } => Some(format!("SSH 认证失败：{reason}")),
        TunnelEvent::HandshakeFailed { reason } => Some(format!("SSH 握手失败：{reason}")),
        TunnelEvent::RetryExhausted => Some("重连次数已耗尽，请手动重试".to_string()),
        TunnelEvent::TrustTimeout => Some("等待主机密钥确认超时".to_string()),
        _ => None,
    }
}

fn retry_hint(event: &TunnelEvent) -> Option<u64> {
    match event {
        TunnelEvent::NetworkLost | TunnelEvent::HandshakeFailed { .. } => Some(1),
        _ => None,
    }
}

/// 冲突类型：界面据此显示「端口被占」还是「域名被占」。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConflictKind {
    Address,
    Domain,
}

/// 唯一性判定的核心：返回第一个冲突的（类型, 占用者名称）。
fn find_conflict(
    items: &[TunnelDto],
    candidate: &TunnelDto,
    exclude: Option<&str>,
) -> Option<(ConflictKind, String)> {
    let (host, port) = effective_addr(candidate);
    for other in items {
        if Some(other.id.as_str()) == exclude {
            continue;
        }
        let (other_host, other_port) = effective_addr(other);
        if other_host == host && other_port == port {
            return Some((ConflictKind::Address, other.name.clone()));
        }
        if !candidate.domain.is_empty() && other.domain == candidate.domain {
            return Some((ConflictKind::Domain, other.name.clone()));
        }
    }
    None
}

/// 唯一性：监听地址与域名都不能撞。
///
/// 错误文案保留「已被…占用」——接口层用 `message.contains("已被")` 判 409。
fn ensure_unique_in(
    items: &[TunnelDto],
    candidate: &TunnelDto,
    exclude: Option<&str>,
) -> Result<()> {
    if let Some((kind, name)) = find_conflict(items, candidate, exclude) {
        let (host, port) = effective_addr(candidate);
        return Err(match kind {
            ConflictKind::Address => {
                anyhow!("地址 {host}:{port} 已被转发「{name}」占用，换一个端口或域名")
            }
            ConflictKind::Domain => {
                anyhow!("域名 {} 已被转发「{name}」占用", candidate.domain)
            }
        });
    }
    Ok(())
}

fn validate_create(req: &CreateTunnelRequest) -> Result<()> {
    if req.name.trim().is_empty() {
        return Err(anyhow!("隧道名不能为空"));
    }
    if req.server_id.trim().is_empty() {
        return Err(anyhow!("必须指定服务器"));
    }
    if req.local_port == 0 {
        return Err(anyhow!("本地端口必须在 1..=65535"));
    }
    if req.remote_port == 0 {
        return Err(anyhow!("远端端口必须在 1..=65535"));
    }
    ensure_scheme(&req.scheme)?;
    normalize_remote_scheme(req.remote_scheme.clone())?;
    validate_domain(&req.domain)?;
    normalize_open_path(&req.open_path)?;
    if req.note.chars().count() > 120 {
        return Err(anyhow!("备注最多 120 个字符"));
    }
    Ok(())
}

/// 自定义图标：http(s) 地址或内联图片。
///
/// 界面把远程图标下载后转成 `data:image/...` 存进配置（随数据文件落盘、离线可用）；
/// 这里放宽到 200KB，够放一张 128px 的 PNG，但不允许把任意大文件塞进 tunnels.json。
fn normalize_icon(raw: &str) -> Result<String> {
    let trimmed = raw.trim();
    if trimmed.chars().count() > 200_000 {
        return Err(anyhow!("图标过大（最多 200KB）"));
    }
    if trimmed.is_empty() {
        return Ok(String::new());
    }
    if !(trimmed.starts_with("http://")
        || trimmed.starts_with("https://")
        || trimmed.starts_with("data:image/"))
    {
        return Err(anyhow!("图标只支持 http(s) 地址或 data:image"));
    }
    Ok(trimmed.to_string())
}

/// 自定义分组：纯展示用，去掉首尾空白并限制长度。
fn normalize_group(raw: &str) -> Result<String> {
    let trimmed = raw.trim();
    if trimmed.chars().count() > 32 {
        return Err(anyhow!("分组名最多 32 个字符"));
    }
    Ok(trimmed.to_string())
}

/// 打开路径统一成"以 / 开头"的形状：用户填 `lovelace/0` 和 `/lovelace/0` 都存成后者。
///
/// 存的时候统一，读的时候就不用到处补斜杠——否则拼 URL 的地方迟早漏一处。
fn normalize_open_path(raw: &str) -> Result<String> {
    let trimmed = raw.trim();
    if trimmed.chars().count() > 200 {
        return Err(anyhow!("打开路径最多 200 个字符"));
    }
    if trimmed.is_empty() {
        return Ok(String::new());
    }
    if trimmed.starts_with('/') {
        return Ok(trimmed.to_string());
    }
    Ok(format!("/{trimmed}"))
}

/// 本地协议写进配置前要收口，避免存进一个谁也解释不了的值。
///
/// `rdp` / `ssh` 与 `tcp` 同属纯转发（见 [`entry_mode`]，只差默认端口），
/// 必须一并放行：界面下拉里能选，保存却 400 是纯粹的自相矛盾。
fn ensure_scheme(value: &str) -> Result<()> {
    match value {
        "tcp" | "http" | "https" | "rdp" | "ssh" => Ok(()),
        other => Err(anyhow!(
            "不支持的本地协议：{other}（只能是 tcp / http / https / rdp / ssh）"
        )),
    }
}

/// 生成一个**在当前集合里唯一**的 id。
///
/// 老实现是 `tun_{slug}_{pid % 10000}`：同一进程里 pid 部分是常量，
/// 中文名又会被整段替换成 `_`，于是"重命名后再建同名转发"会拿到完全相同的 id
/// ——两条转发共用一个 id，界面上点一个开关会同时改两条。
fn new_id(name: &str, taken: &HashSet<String>) -> String {
    let slug: String = name
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .take(12)
        .collect();
    let base = format!("tun_{slug}");
    if !taken.contains(&base) {
        return base;
    }
    for n in 2.. {
        let candidate = format!("{base}_{n}");
        if !taken.contains(&candidate) {
            return candidate;
        }
    }
    unreachable!("id 空间不会用尽")
}

/// 修复历史上撞车的 id：保留第一条，其余重新分配。
///
/// 只改 id，不动名字/端口等用户数据；返回是否有改动（有则需回写）。
fn dedupe_ids(items: &mut [TunnelDto]) -> bool {
    let mut seen: HashSet<String> = HashSet::new();
    let mut changed = false;
    for item in items.iter_mut() {
        if seen.insert(item.id.clone()) {
            continue;
        }
        let mut n = 2;
        loop {
            let candidate = format!("{}_{n}", item.id);
            if seen.insert(candidate.clone()) {
                item.id = candidate;
                break;
            }
            n += 1;
        }
        changed = true;
    }
    changed
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::credential::CredentialStore;
    use crate::models::server::ServerStore;
    use tl_protocol::{CreateCredentialRequest, CreateServerRequest, CredentialKind};

    struct Fixture {
        dir: PathBuf,
        tunnels: TunnelStore,
        servers: Arc<ServerStore>,
        server_id: String,
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    fn fixture(tag: &str) -> Fixture {
        let dir = std::env::temp_dir().join(format!("threadline-tun-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create dir");

        let credentials = Arc::new(CredentialStore::new(dir.join("credentials.dat")));
        let cred = credentials
            .add(CreateCredentialRequest {
                name: String::new(),
                kind: CredentialKind::Password,
                username: "dev".into(),
                secret: "pw".into(),
                passphrase: None,
            })
            .expect("credential");

        let servers = Arc::new(ServerStore::new(
            dir.join("servers.json"),
            Arc::clone(&credentials),
        ));
        let srv = servers
            .create(CreateServerRequest {
                name: "web-01".into(),
                host: "192.0.2.90".into(),
                port: 22,
                credential_id: cred.id,
                username: "root".into(),
                kind: None,
                secret: None,
                passphrase: None,
                host_key_fingerprint: Some("SHA256:known".into()),
                keepalive_secs: 30,
                enabled: false,
            })
            .expect("server");

        let tunnels = TunnelStore::new(dir.join("tunnels.json"), Arc::clone(&servers));
        Fixture {
            dir,
            tunnels,
            servers,
            server_id: srv.id,
        }
    }

    fn create_req(name: &str, server_id: &str, port: u16) -> CreateTunnelRequest {
        CreateTunnelRequest {
            name: name.into(),
            server_id: server_id.into(),
            local_host: "127.0.0.1".into(),
            local_port: port,
            note: String::new(),
            open_path: String::new(),
            scheme: "tcp".to_string(),
            remote_host: "127.0.0.1".into(),
            remote_port: 8080,
            auto_start: false,
            domain: String::new(),
            remote_scheme: None,
            icon: String::new(),
            group: String::new(),
            in_app: true,
        }
    }

    #[test]
    fn rdp_and_ssh_are_accepted_as_plain_forwards() {
        // 界面下拉里有 rdp / ssh，它们和 tcp 一样是纯转发。保存链路必须放行——
        // 曾经 `ensure_scheme` 只认 tcp/http/https，用户选了 rdp 保存就 400。
        for (scheme, port) in [("rdp", 13389u16), ("ssh", 12222u16)] {
            let f = fixture(&format!("scheme-{scheme}"));
            let mut req = create_req("pane", &f.server_id, port);
            req.scheme = scheme.into();
            let created = f
                .tunnels
                .create(req)
                .unwrap_or_else(|e| panic!("{scheme} 应当可以直接建：{e}"));
            let view = f.tunnels.view(&created.id).expect("view");
            assert_eq!(view.definition.scheme, scheme);
            assert!(
                matches!(
                    entry_mode(&view.definition).expect("entry mode"),
                    EntryMode::Plain
                ),
                "{scheme} 必须按纯转发处理"
            );
        }
    }

    #[test]
    fn open_path_is_normalized_so_url_building_never_misses_a_slash() {
        // 用户填 "lovelace/0" 或 "/lovelace/0" 都应该存成后者：
        // 拼 URL 的地方就不必各自补斜杠（那种补法迟早漏一处）。
        let f = fixture("openpath");
        let mut req = create_req("web", &f.server_id, 18094);
        req.scheme = "http".into();
        req.open_path = "lovelace/0".into();
        let created = f.tunnels.create(req).expect("create");

        let listed = f.tunnels.list().expect("list");
        let path = listed
            .items
            .iter()
            .find(|t| t.definition.id == created.id)
            .expect("tunnel")
            .definition
            .open_path
            .clone();
        assert_eq!(path, "/lovelace/0");

        // 空值就是"根路径"，不要变成 "/" 占位符
        f.tunnels
            .patch(
                &created.id,
                PatchTunnelRequest {
                    open_path: Some("  ".into()),
                    ..Default::default()
                },
            )
            .expect("patch");
        let cleared = f
            .tunnels
            .list()
            .expect("list")
            .items
            .into_iter()
            .find(|t| t.definition.id == created.id)
            .expect("tunnel")
            .definition
            .open_path;
        assert_eq!(cleared, "");

        // 太长的路径要拒绝，避免有人把整段 query 塞进来
        assert!(f
            .tunnels
            .patch(
                &created.id,
                PatchTunnelRequest {
                    open_path: Some(format!("/{}", "x".repeat(201))),
                    ..Default::default()
                },
            )
            .is_err());
    }

    #[test]
    fn note_is_optional_limited_and_persisted() {
        // 备注只用于展示与搜索，但不能无限长——有人会把整段日志贴进来。
        let f = fixture("note");
        let mut req = create_req("web", &f.server_id, 18096);
        req.note = "测试小主机".into();
        let created = f.tunnels.create(req).expect("create");
        let id = created.id.clone();

        let listed = f.tunnels.list().expect("list");
        let note = listed
            .items
            .iter()
            .find(|t| t.definition.id == id)
            .expect("tunnel")
            .definition
            .note
            .clone();
        assert_eq!(note, "测试小主机");

        let long = "x".repeat(121);
        assert!(
            f.tunnels
                .patch(
                    &id,
                    PatchTunnelRequest {
                        note: Some(long),
                        ..Default::default()
                    },
                )
                .is_err(),
            "超过 120 字的备注要拒绝"
        );

        // 老配置没有 note 字段：反序列化后必须是空串，而不是解析失败
        let dto: TunnelDto = serde_json::from_str(
            r#"{"id":"t2","name":"x","server_id":"s","local_host":"127.0.0.1","local_port":1,
                "remote_host":"127.0.0.1","remote_port":2,"auto_start":false}"#,
        )
        .expect("parse legacy");
        assert_eq!(dto.note, "");
    }

    #[test]
    fn old_config_without_scheme_still_reads_as_tcp() {
        // 老的 tunnels.json 里没有 scheme。反序列化必须落到 tcp，
        // 而不是空串——否则界面会拿空 scheme 拼出一个冒号开头的怪地址。
        let dto: TunnelDto = serde_json::from_str(
            r#"{"id":"t1","name":"web","server_id":"s1","local_host":"127.0.0.1",
                "local_port":8080,"remote_host":"127.0.0.1","remote_port":80,
                "auto_start":false}"#,
        )
        .expect("parse legacy tunnel");
        assert_eq!(dto.scheme, "tcp");
    }

    #[test]
    fn unsupported_scheme_is_rejected_on_create_and_patch() {
        // scheme 只是界面提示，但存进去之前必须收口：脏值会让"打开本地页面"拼出无效地址。
        let f = fixture("scheme");
        let mut req = create_req("web", &f.server_id, 18095);
        req.scheme = "ftp".into();
        assert!(f.tunnels.create(req).is_err(), "ftp 不该被接受");

        let ok = f
            .tunnels
            .create(create_req("web", &f.server_id, 18095))
            .expect("create");
        let id = ok.id.clone();
        f.tunnels
            .patch(
                &id,
                PatchTunnelRequest {
                    scheme: Some("https".into()),
                    ..Default::default()
                },
            )
            .expect("patch");
        let listed = f.tunnels.list().expect("list");
        let scheme = listed
            .items
            .iter()
            .find(|t| t.definition.id == id)
            .expect("tunnel")
            .definition
            .scheme
            .clone();
        assert_eq!(scheme, "https");

        let bad = f.tunnels.patch(
            &id,
            PatchTunnelRequest {
                scheme: Some("gopher".into()),
                ..Default::default()
            },
        );
        assert!(bad.is_err(), "非法 scheme 不该写进配置");
    }

    #[test]
    fn ids_stay_unique_when_slugs_collide() {
        // 中文名的 slug 全是下划线：老实现（slug + 进程 pid）会给同名重建的两条
        // 转发分配完全相同的 id，界面上点一个开关会同时改两条。
        let mut taken = HashSet::new();
        let first = new_id("测试", &taken);
        taken.insert(first.clone());
        let second = new_id("测试", &taken);
        assert_ne!(first, second, "同 slug 的第二个 id 必须重新分配");
        assert!(
            second.starts_with(&format!("{first}_")),
            "应保留可读前缀: {second}"
        );
    }

    #[test]
    fn duplicate_ids_in_file_are_repaired_on_load() {
        let f = fixture("dupid");
        let base = TunnelDto {
            id: "tun____9396".into(),
            name: "测试x".into(),
            server_id: f.server_id.clone(),
            local_host: "127.0.0.1".into(),
            local_port: 18090,
            remote_host: "127.0.0.1".into(),
            remote_port: 8090,
            auto_start: false,
            note: String::new(),
            open_path: String::new(),
            scheme: "tcp".into(),
            domain: String::new(),
            domain_host: String::new(),
            remote_scheme: None,
            icon: String::new(),
            group: String::new(),
            in_app: true,
        };
        let duplicate = TunnelDto {
            name: "测试".into(),
            local_port: 18091,
            ..base.clone()
        };
        store::write_json_atomic(
            f.tunnels.path(),
            &TunnelsFile {
                schema_version: SCHEMA_VERSION,
                items: vec![base, duplicate],
            },
        )
        .expect("write");

        let ids: Vec<String> = f
            .tunnels
            .list()
            .expect("list")
            .items
            .into_iter()
            .map(|view| view.definition.id)
            .collect();
        let unique: HashSet<&String> = ids.iter().collect();
        assert_eq!(unique.len(), ids.len(), "加载后 id 必须唯一: {ids:?}");
        // 原来的数据不能丢
        assert_eq!(
            f.tunnels.list().expect("list").items.len(),
            2,
            "修 id 不能把记录弄丢"
        );
    }

    #[test]
    fn create_requires_existing_server() {
        let f = fixture("noserver");
        assert!(f
            .tunnels
            .create(create_req("a", "srv_missing", 18080))
            .is_err());
        let status = f
            .tunnels
            .create(create_req("a", &f.server_id, 18080))
            .expect("create");
        assert_eq!(status.state, TunnelState::Stopped);
    }

    #[test]
    fn config_conflict_matches_what_create_would_reject() {
        let f = fixture("config-conflict");
        f.tunnels
            .create(create_req("a", &f.server_id, 18080))
            .expect("a");
        let mut with_domain = create_req("b", &f.server_id, 18081);
        with_domain.domain = "a.example.com".into();
        f.tunnels.create(with_domain).expect("b");

        // 端口占用：返回占用者名称，供界面显示「已被 a 占用」
        assert_eq!(
            f.tunnels
                .config_conflict("127.0.0.1", 18080, "", None)
                .unwrap(),
            Some((ConflictKind::Address, "a".to_string()))
        );

        // 域名占用
        assert_eq!(
            f.tunnels
                .config_conflict("127.0.0.1", 18099, "a.example.com", None)
                .unwrap(),
            Some((ConflictKind::Domain, "b".to_string()))
        );

        // 编辑自己不算冲突
        let id = f.tunnels.list().unwrap().items[0].definition.id.clone();
        assert_eq!(
            f.tunnels
                .config_conflict("127.0.0.1", 18080, "", Some(&id))
                .unwrap(),
            None
        );
    }

    #[test]
    fn duplicate_local_port_is_rejected() {
        let f = fixture("portdup");
        f.tunnels
            .create(create_req("a", &f.server_id, 18080))
            .expect("a");
        assert!(f
            .tunnels
            .create(create_req("b", &f.server_id, 18080))
            .is_err());
    }

    #[test]
    fn start_moves_to_connecting_and_reserves_port() {
        let f = fixture("start");
        let s = f
            .tunnels
            .create(create_req("a", &f.server_id, 18081))
            .expect("create");
        let running = f.tunnels.start(&s.id).expect("start");
        assert_eq!(running.state, TunnelState::Connecting);
        assert!(f.tunnels.is_port_taken(18081));
    }

    #[test]
    fn handshake_ok_reaches_running_and_stop_releases_port() {
        let f = fixture("run");
        let s = f
            .tunnels
            .create(create_req("a", &f.server_id, 18082))
            .expect("create");
        f.tunnels.start(&s.id).expect("start");
        let running = f
            .tunnels
            .apply(&s.id, TunnelEvent::HandshakeOk)
            .expect("handshake");
        assert_eq!(running.state, TunnelState::Running);

        let stopped = f.tunnels.stop(&s.id).expect("stop");
        assert_eq!(stopped.state, TunnelState::Stopped);
        assert!(!f.tunnels.is_port_taken(18082));
    }

    #[test]
    fn illegal_event_is_rejected_not_ignored() {
        let f = fixture("illegal");
        let s = f
            .tunnels
            .create(create_req("a", &f.server_id, 18083))
            .expect("create");
        // Stopped 状态下收到 HandshakeOk 是非法事件，必须报错
        assert!(f.tunnels.apply(&s.id, TunnelEvent::HandshakeOk).is_err());
    }

    #[test]
    fn bind_failure_is_visible_on_status() {
        let f = fixture("bindfail");
        let s = f
            .tunnels
            .create(create_req("a", &f.server_id, 18084))
            .expect("create");
        f.tunnels.start(&s.id).expect("start");
        let status = f
            .tunnels
            .apply(
                &s.id,
                TunnelEvent::BindFailed {
                    reason: "已被占用".into(),
                },
            )
            .expect("bind failed");
        assert_eq!(status.state, TunnelState::Error);
        assert!(status.last_error.unwrap().contains("本地端口绑定失败"));
    }

    #[test]
    fn patching_a_listening_tunnel_drops_back_to_stopped() {
        let f = fixture("patch");
        let s = f
            .tunnels
            .create(create_req("a", &f.server_id, 18085))
            .expect("create");
        f.tunnels.start(&s.id).expect("start");
        assert!(f.tunnels.is_port_taken(18085));

        let patched = f
            .tunnels
            .patch(
                &s.id,
                PatchTunnelRequest {
                    local_port: Some(18086),
                    ..Default::default()
                },
            )
            .expect("patch");
        assert_eq!(patched.state, TunnelState::Stopped);
        assert!(!f.tunnels.is_port_taken(18085), "旧端口必须释放");
    }

    #[test]
    fn server_removal_is_blocked_while_referenced() {
        let f = fixture("blocked");
        let s = f
            .tunnels
            .create(create_req("a", &f.server_id, 18087))
            .expect("create");
        assert!(f.tunnels.references_server(&f.server_id).expect("ref"));
        assert_eq!(
            f.tunnels.can_remove_server(&f.server_id).expect("decision"),
            RemoveDecision::Blocked {
                referenced_by: vec![s.id.clone()]
            }
        );

        f.tunnels.remove(&s.id).expect("remove");
        assert_eq!(
            f.tunnels.can_remove_server(&f.server_id).expect("decision"),
            RemoveDecision::Allowed
        );
    }

    /// 删连接时连带删转发：一次写完，不留下孤儿。
    #[test]
    fn remove_for_server_takes_every_forward_of_that_server() {
        let f = fixture("remove-for-server");
        let a = f
            .tunnels
            .create(create_req("a", &f.server_id, 18091))
            .expect("create a");
        let b = f
            .tunnels
            .create(create_req("b", &f.server_id, 18092))
            .expect("create b");

        let removed = f.tunnels.remove_for_server(&f.server_id).expect("remove");
        assert_eq!(removed, vec![a.id.clone(), b.id.clone()]);
        assert!(f.tunnels.list().expect("list").items.is_empty());
        assert_eq!(
            f.tunnels.can_remove_server(&f.server_id).expect("decision"),
            RemoveDecision::Allowed
        );

        // 幂等：再删一次不报错，也不返回任何 id。
        assert!(f
            .tunnels
            .remove_for_server(&f.server_id)
            .expect("remove again")
            .is_empty());
    }

    #[test]
    fn errors_on_a_never_started_tunnel_are_still_visible() {
        // "隧道未启用" 这类拒绝发生在启动之前：此时还没有运行态条目，
        // 但错误必须留下痕迹，否则用户点了启动什么也看不到。
        let f = fixture("gate-error");
        let s = f
            .tunnels
            .create(create_req("a", &f.server_id, 18140))
            .expect("create");

        f.tunnels
            .set_last_error(&s.id, "隧道未启用：请先启用这条隧道");

        let status = f.tunnels.status(&s.id).expect("status");
        assert_eq!(status.state, TunnelState::Stopped);
        assert!(
            status.last_error.unwrap().contains("隧道未启用"),
            "未启动的条目也要能看到拒绝原因"
        );
    }

    #[test]
    fn connection_errors_are_visible_without_changing_state() {
        let f = fixture("last-error");
        let s = f
            .tunnels
            .create(create_req("a", &f.server_id, 18120))
            .expect("create");
        f.tunnels.start(&s.id).expect("start");
        let running = f
            .tunnels
            .apply(&s.id, TunnelEvent::HandshakeOk)
            .expect("handshake");
        assert_eq!(running.state, TunnelState::Running);

        // 某次转发连接失败：只记错误，状态仍是 Running（会话没断，别的连接照常）。
        f.tunnels
            .set_last_error(&s.id, "无法连接 192.0.2.98:16100（服务器侧）：refused");
        let status = f.tunnels.status(&s.id).expect("status");
        assert_eq!(status.state, TunnelState::Running);
        assert!(
            status.last_error.unwrap().contains("192.0.2.98"),
            "错误必须能说明是哪个目标连不上"
        );
    }

    #[test]
    fn stop_all_stops_everything_and_frees_ports() {
        let f = fixture("stopall");
        let a = f
            .tunnels
            .create(create_req("a", &f.server_id, 18088))
            .expect("a");
        let b = f
            .tunnels
            .create(create_req("b", &f.server_id, 18089))
            .expect("b");
        f.tunnels.start(&a.id).expect("start a");
        f.tunnels.start(&b.id).expect("start b");

        f.tunnels.stop_all().expect("stop all");
        assert!(!f.tunnels.is_port_taken(18088));
        assert!(!f.tunnels.is_port_taken(18089));
        assert_eq!(
            f.tunnels.status(&a.id).expect("status").state,
            TunnelState::Stopped
        );
        assert_eq!(
            f.tunnels.status(&b.id).expect("status").state,
            TunnelState::Stopped
        );
    }

    #[test]
    fn list_merges_definition_and_status() {
        let f = fixture("list");
        let s = f
            .tunnels
            .create(create_req("a", &f.server_id, 18090))
            .expect("create");
        f.tunnels.start(&s.id).expect("start");
        let list = f.tunnels.list().expect("list");
        assert_eq!(list.items.len(), 1);
        assert_eq!(list.items[0].definition.local_port, 18090);
        assert_eq!(list.items[0].status.state, TunnelState::Connecting);
        // server 存在性由 server 模型负责，这里只确认 fixture 没走歪
        assert!(f.servers.exists(&f.server_id).expect("exists"));
    }

    #[test]
    fn domain_is_validated_and_normalized() {
        assert_eq!(
            validate_domain("HOME.Example.COM.").unwrap(),
            "home.example.com"
        );
        assert_eq!(
            validate_domain("  home.example.com  ").unwrap(),
            "home.example.com"
        );
        assert_eq!(validate_domain("").unwrap(), "");
        assert!(validate_domain("localhost").is_err());
        assert!(validate_domain("127.0.0.5").is_err());
        assert!(validate_domain("home").is_err());
        assert!(validate_domain("home.example.com:443").is_err());
        assert!(validate_domain("-bad.example.com").is_err());
        assert!(validate_domain("bad-.example.com").is_err());
        assert!(validate_domain("home.local").is_err());
        assert!(validate_domain("中文.example.com").is_err());
        assert!(validate_domain("home_example.com").is_err());
    }

    #[test]
    fn domain_gets_pool_address_and_is_released() {
        let f = fixture("domainpool");
        let mut req = create_req("web", &f.server_id, 18095);
        req.scheme = "https".into();
        req.remote_scheme = Some("http".into());
        req.domain = "home.example.com".into();
        let a = f.tunnels.create(req).expect("create a");
        assert_eq!(
            f.tunnels.view(&a.id).unwrap().definition.domain_host,
            "127.0.0.2"
        );

        let mut req2 = create_req("web2", &f.server_id, 18096);
        req2.scheme = "https".into();
        req2.remote_scheme = Some("http".into());
        req2.domain = "git.example.com".into();
        let b = f.tunnels.create(req2).expect("create b");
        assert_eq!(
            f.tunnels.view(&b.id).unwrap().definition.domain_host,
            "127.0.0.3"
        );

        // 删除后地址回收：下一条重新拿到 127.0.0.2
        f.tunnels.remove(&a.id).expect("remove a");
        let mut req3 = create_req("web3", &f.server_id, 18097);
        req3.scheme = "https".into();
        req3.remote_scheme = Some("http".into());
        req3.domain = "nas.example.com".into();
        let c = f.tunnels.create(req3).expect("create c");
        assert_eq!(
            f.tunnels.view(&c.id).unwrap().definition.domain_host,
            "127.0.0.2"
        );
    }

    #[test]
    fn patch_clearing_domain_releases_old_address() {
        let f = fixture("domainpatch");
        let mut req = create_req("web", &f.server_id, 18098);
        req.scheme = "https".into();
        req.remote_scheme = Some("http".into());
        req.domain = "home.example.com".into();
        let created = f.tunnels.create(req).expect("create");
        f.tunnels.start(&created.id).expect("start");
        assert!(f.tunnels.is_port_taken(18098));

        let patched = f
            .tunnels
            .patch(
                &created.id,
                PatchTunnelRequest {
                    domain: Some(String::new()),
                    ..Default::default()
                },
            )
            .expect("patch");
        assert_eq!(patched.state, TunnelState::Stopped);
        assert!(!f.tunnels.is_port_taken(18098), "旧地址必须释放");
        assert_eq!(patched.id, created.id);
    }

    #[test]
    fn same_port_on_different_loopback_is_allowed() {
        let f = fixture("diffhost");
        let mut a = create_req("a", &f.server_id, 18099);
        a.scheme = "https".into();
        a.remote_scheme = Some("http".into());
        a.domain = "home.example.com".into();
        f.tunnels.create(a).expect("域名入口 127.0.0.2:18099");

        let mut b = create_req("b", &f.server_id, 18099);
        b.local_host = "127.0.0.9".into();
        f.tunnels.create(b).expect("不同回环地址上的同端口必须允许");
    }

    #[test]
    fn legacy_scheme_https_without_domain_stays_plain() {
        // 存量配置：界面默认 scheme=https，但没有域名 → 不能因为新语义变成 TLS 终止
        let dto: TunnelDto = serde_json::from_str(
            r#"{"id":"t","name":"x","server_id":"s","local_host":"127.0.0.1",
                "local_port":1,"remote_host":"127.0.0.1","remote_port":443,
                "auto_start":false,"scheme":"https"}"#,
        )
        .expect("parse legacy");
        assert_eq!(dto.domain, "");
        assert_eq!(dto.remote_scheme, None);
        assert_eq!(entry_mode(&dto).unwrap(), EntryMode::Plain);
    }

    #[test]
    fn rdp_and_ssh_are_plain_tcp() {
        let dto: TunnelDto = serde_json::from_str(
            r#"{"id":"t","name":"x","server_id":"s","local_host":"127.0.0.1",
                "local_port":3389,"remote_host":"127.0.0.1","remote_port":3389,
                "auto_start":false,"scheme":"rdp"}"#,
        )
        .expect("parse rdp");
        assert_eq!(entry_mode(&dto).unwrap(), EntryMode::Plain);

        let mut ssh = dto;
        ssh.scheme = "ssh".into();
        assert_eq!(entry_mode(&ssh).unwrap(), EntryMode::Plain);
    }

    #[test]
    fn domain_without_remote_scheme_derives_passthrough() {
        let dto: TunnelDto = serde_json::from_str(
            r#"{"id":"t","name":"x","server_id":"s","local_host":"127.0.0.1",
                "local_port":443,"remote_host":"127.0.0.1","remote_port":443,
                "auto_start":false,"scheme":"https","domain":"home.example.com",
                "domain_host":"127.0.0.5"}"#,
        )
        .expect("parse");
        assert_eq!(entry_mode(&dto).unwrap(), EntryMode::Passthrough);
    }

    #[test]
    fn local_http_with_remote_https_is_rejected() {
        let f = fixture("badmode");
        let mut req = create_req("web", &f.server_id, 18100);
        req.scheme = "http".into();
        req.remote_scheme = Some("https".into());
        req.domain = "home.example.com".into();
        let err = f.tunnels.create(req).unwrap_err().to_string();
        assert!(err.contains("必须也是 https"), "实际错误：{err}");
    }

    #[test]
    fn duplicate_domain_is_rejected() {
        let f = fixture("dupdomain");
        let mut a = create_req("a", &f.server_id, 18101);
        a.scheme = "https".into();
        a.remote_scheme = Some("http".into());
        a.domain = "home.example.com".into();
        f.tunnels.create(a).expect("create a");

        let mut b = create_req("b", &f.server_id, 18102);
        b.scheme = "https".into();
        b.remote_scheme = Some("http".into());
        b.domain = "HOME.example.com.".into();
        let err = f.tunnels.create(b).unwrap_err().to_string();
        assert!(err.contains("已被转发"), "实际错误：{err}");
    }
}
