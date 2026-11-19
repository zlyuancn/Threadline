//! 服务器模型。
//!
//! **拥有**：`servers.json`（主机、端口、登录用户、凭据引用、已信任主机密钥指纹、keepalive），
//! 以及**待确认**的主机密钥（内存态，不落盘）。判定规则：凭据引用必须存在、待确认指纹的接受/拒绝。
//! **提供**：`list` / `create` / `patch` / `remove` / `can_remove` / `connect_params` /
//! `host_key_view` / `observe_host_key` / `accept_host_key`。
//! **允许调用**：`credential`（校验引用、取认证材料）、`foundation::store`。
//!
//! termination: phase —— 对外的只读询问（`list` / `connect_params` / `host_key_view`）不改状态，
//! 不推进任何相位，按 MSO 纪律 1 显式声明。

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use anyhow::{anyhow, Result};
use tl_protocol::{
    AcceptHostKeyRequest, CreateServerRequest, CredentialKind, HostKeyView, PatchServerRequest,
    ServerDto, ServerKind, ServerList, ServersFile, LOCAL_SERVER_ID,
};

use crate::foundation::store;
use crate::models::credential::CredentialStore;
use crate::models::RemoveDecision;

use tl_protocol::SCHEMA_VERSION;

/// 本机条目只在 Windows 上存在——本机管理不做跨平台。
///
/// 用编译期常量而不是运行时开关：非 Windows 构建里这个条目根本不该有，
/// 而不是"先造出来再隐藏"。
pub const LOCAL_SUPPORTED: bool = cfg!(windows);

/// 这条 id 是否指内置的「本机」条目。
pub fn is_local(id: &str) -> bool {
    LOCAL_SUPPORTED && id == LOCAL_SERVER_ID
}

/// 内置「本机」条目：相当于一条**已经连上**的隧道。
///
/// 不落盘、无凭据、无主机密钥、恒为启用；`host` 只是给人看的，
/// 它的转发直接连本机端口，不读这两个字段以外的任何东西。
fn local_entry() -> ServerDto {
    ServerDto {
        id: LOCAL_SERVER_ID.to_string(),
        name: "本机".to_string(),
        kind: ServerKind::Local,
        host: "127.0.0.1".to_string(),
        port: 0,
        credential_id: String::new(),
        username: String::new(),
        credential_kind: CredentialKind::Password,
        host_key_fingerprint: None,
        keepalive_secs: 0,
        enabled: true,
        session_error: None,
        needs_credential: false,
    }
}

/// 对内置条目的写操作统一拒绝：它没有可改的配置，也不该被删除。
///
/// 措辞用「不允许」而不是「不支持」：接口层把它翻成 409（用户动作与当前状态冲突），
/// 报成 500 会让界面以为核心出了故障。
fn reject_local(id: &str, action: &str) -> Result<()> {
    if is_local(id) {
        return Err(anyhow!("「本机」是内置条目，不允许{action}"));
    }
    Ok(())
}

/// 待确认的主机密钥：**内存态、不落盘**（重启后重新观测，避免把可疑指纹持久化）。
#[derive(Debug, Clone, PartialEq, Eq)]
struct PendingHostKey {
    fingerprint: String,
    observed_at: String,
}

/// 建立 SSH 会话所需的参数（不含密钥材料）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectParams {
    pub server_id: String,
    pub host: String,
    pub port: u16,
    pub credential_id: String,
    pub keepalive_secs: u32,
    /// 已信任的主机密钥指纹；`None` = 首次连接，需要信任确认。
    pub trusted_fingerprint: Option<String>,
    /// 用户意图：这条隧道是否应保持连接。端口转发的启动要服从它。
    pub enabled: bool,
}

/// 服务器仓库：独占 `servers.json` + 内存里的待确认指纹。
#[derive(Debug)]
pub struct ServerStore {
    path: PathBuf,
    credentials: Arc<CredentialStore>,
    pending: Mutex<HashMap<String, PendingHostKey>>,
    /// 最近一次建立会话的失败原因（内存，不落盘）。
    session_errors: Mutex<HashMap<String, String>>,
}

impl ServerStore {
    pub fn new(path: impl Into<PathBuf>, credentials: Arc<CredentialStore>) -> Self {
        Self {
            path: path.into(),
            credentials,
            pending: Mutex::new(HashMap::new()),
            session_errors: Mutex::new(HashMap::new()),
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    fn load(&self) -> Result<ServersFile> {
        let mut file = store::read_json::<ServersFile>(&self.path)?.unwrap_or(ServersFile {
            schema_version: SCHEMA_VERSION,
            items: Vec::new(),
        });

        // 老文件没有 `credential_kind`：按实际凭据回填，否则编辑时永远显示「密码」。
        let kinds: HashMap<String, CredentialKind> = self
            .credentials
            .list()
            .map(|list| list.items.into_iter().map(|c| (c.id, c.kind)).collect())
            .unwrap_or_default();
        for server in &mut file.items {
            if let Some(kind) = kinds.get(&server.credential_id) {
                server.credential_kind = *kind;
            }
            // 文件里只允许远端条目：手改出来的 `kind: local` 一律归一成 ssh，
            // 否则会出现第二个「本机」抢同一个 id。
            server.kind = ServerKind::Ssh;
        }
        Ok(file)
    }

    fn save(&self, file: &ServersFile) -> Result<()> {
        store::write_json_atomic(&self.path, file)
    }

    /// 按给定 id 顺序重排，返回实际生效的顺序。
    ///
    /// 请求里没提到的隧道**追加在末尾**并保持它们原来的相对顺序：
    /// 界面只提交一部分（比如搜索过滤后）时不至于把别的隧道弄丢。
    pub fn reorder(&self, ids: &[String]) -> Result<()> {
        let mut file = self.load()?;
        let mut ordered = Vec::with_capacity(file.items.len());
        let mut taken = vec![false; file.items.len()];

        for id in ids {
            if let Some(index) = file.items.iter().position(|item| &item.id == id) {
                if !taken[index] {
                    taken[index] = true;
                    ordered.push(file.items[index].clone());
                }
            }
        }
        for (index, item) in file.items.iter().enumerate() {
            if !taken[index] {
                ordered.push(item.clone());
            }
        }

        file.items = ordered;
        self.save(&file)
    }

    pub fn list(&self) -> Result<ServerList> {
        let mut items = self.load()?.items;
        let errors = self.session_errors.lock().expect("session errors lock");
        for item in &mut items {
            item.session_error = errors.get(&item.id).cloned();
            decorate(item);
        }
        drop(errors);
        // 本机永远排在最前：它是"已经连上的那条"，不该被用户条目的顺序挤下去。
        let mut all = Vec::with_capacity(items.len() + 1);
        if LOCAL_SUPPORTED {
            all.push(local_entry());
        }
        all.extend(items);
        Ok(ServerList { items: all })
    }

    /// 这条 id 的传输种类。找不到时按"不存在"报错，不猜。
    pub fn kind_of(&self, id: &str) -> Result<ServerKind> {
        if is_local(id) {
            return Ok(ServerKind::Local);
        }
        if self.load()?.items.iter().any(|s| s.id == id) {
            return Ok(ServerKind::Ssh);
        }
        Err(anyhow!("服务器不存在：{id}"))
    }

    /// 记下"这次为什么没连上"。成功建立会话时由 `clear_session_error` 清掉。
    pub fn set_session_error(&self, id: &str, message: impl Into<String>) {
        self.session_errors
            .lock()
            .expect("session errors lock")
            .insert(id.to_string(), message.into());
    }

    pub fn clear_session_error(&self, id: &str) {
        self.session_errors
            .lock()
            .expect("session errors lock")
            .remove(id);
    }

    pub fn create(&self, req: CreateServerRequest) -> Result<ServerDto> {
        validate_create(&req)?;
        // 空 `credential_id` = 导入的分享包还没填密码，是合法状态（`needs_credential`）。
        // 普通创建路径在业务层已经保证凭据先建好，这里只在有引用时校验。
        if !req.credential_id.trim().is_empty() {
            self.require_credential(&req.credential_id)?;
        }

        let mut file = self.load()?;
        if file.items.iter().any(|s| s.name == req.name) {
            return Err(anyhow!("服务器名已存在：{}", req.name));
        }
        let taken: HashSet<String> = file.items.iter().map(|s| s.id.clone()).collect();
        let mut dto = ServerDto {
            id: new_id(&req.name, &taken),
            name: req.name,
            // 用户只能创建远端条目；本机由内核注入。
            kind: ServerKind::Ssh,
            host: req.host,
            port: req.port,
            credential_id: req.credential_id,
            username: req.username,
            credential_kind: req.kind.unwrap_or(CredentialKind::Password),
            host_key_fingerprint: req.host_key_fingerprint,
            keepalive_secs: req.keepalive_secs,
            enabled: req.enabled,
            session_error: None,
            needs_credential: false,
        };
        decorate(&mut dto);
        file.items.push(dto.clone());
        self.save(&file)?;
        Ok(dto)
    }

    /// 只改「启用」意图。走单独方法是因为它必须伴随会话的建立/断开，
    /// 由业务层编排；`patch()` 只管纯配置字段。
    pub fn set_enabled(&self, id: &str, enabled: bool) -> Result<ServerDto> {
        reject_local(id, "启用或关闭")?;
        let mut file = self.load()?;
        // 待填凭据的连接不能启用：否则会话建不起来，keeper 会一直退避重试。
        let pending = file
            .items
            .iter()
            .find(|s| s.id == id)
            .map(|s| s.credential_id.trim().is_empty())
            .ok_or_else(|| anyhow!("服务器不存在：{id}"))?;
        if pending && enabled {
            return Err(anyhow!("这条隧道还没填密码，无法启用"));
        }

        let server = file
            .items
            .iter_mut()
            .find(|s| s.id == id)
            .ok_or_else(|| anyhow!("服务器不存在：{id}"))?;
        server.enabled = enabled;
        let mut dto = server.clone();
        decorate(&mut dto);
        self.save(&file)?;
        Ok(dto)
    }

    pub fn patch(&self, id: &str, req: PatchServerRequest) -> Result<ServerDto> {
        reject_local(id, "修改")?;
        if let Some(cred) = &req.credential_id {
            self.require_credential(cred)?;
        }
        let mut file = self.load()?;
        let server = file
            .items
            .iter_mut()
            .find(|s| s.id == id)
            .ok_or_else(|| anyhow!("服务器不存在：{id}"))?;

        if let Some(v) = req.name {
            server.name = v;
        }
        if let Some(v) = req.host {
            if v.trim().is_empty() {
                return Err(anyhow!("主机不能为空"));
            }
            server.host = v;
        }
        if let Some(v) = req.port {
            if v == 0 {
                return Err(anyhow!("端口必须在 1..=65535"));
            }
            server.port = v;
        }
        if let Some(v) = req.credential_id {
            server.credential_id = v;
        }
        // `enabled` 不在这里处理：改它必须连带建立/断开会话，走 `set_enabled` + 业务层编排。
        if let Some(v) = req.keepalive_secs {
            server.keepalive_secs = v.max(1);
        }
        if let Some(v) = req.username {
            if !v.trim().is_empty() {
                server.username = v;
            }
        }
        // 认证方式跟着改：只换标记时不动 secret（密码/私钥正文由业务层按需替换）。
        if let Some(v) = req.kind {
            server.credential_kind = v;
        }
        let mut updated = server.clone();
        self.save(&file)?;
        decorate(&mut updated);
        Ok(updated)
    }

    pub fn remove(&self, id: &str) -> Result<()> {
        reject_local(id, "删除")?;
        let mut file = self.load()?;
        let before = file.items.len();
        file.items.retain(|s| s.id != id);
        if file.items.len() == before {
            return Err(anyhow!("服务器不存在：{id}"));
        }
        self.save(&file)?;
        self.pending.lock().expect("pending lock").remove(id);
        Ok(())
    }

    /// 是否仍被隧道引用（规则归本模型，业务层提供引用方列表）。
    pub fn can_remove(&self, id: &str, tunnels_using_it: &[String]) -> Result<RemoveDecision> {
        reject_local(id, "删除")?;
        if tunnels_using_it.is_empty() {
            return Ok(RemoveDecision::Allowed);
        }
        let file = self.load()?;
        if !file.items.iter().any(|s| s.id == id) {
            return Ok(RemoveDecision::Allowed);
        }
        Ok(RemoveDecision::Blocked {
            referenced_by: tunnels_using_it.to_vec(),
        })
    }

    /// 取连接参数（不碰密钥材料）。
    pub fn connect_params(&self, id: &str) -> Result<ConnectParams> {
        if is_local(id) {
            return Err(anyhow!(
                "「本机」不允许建立 SSH 会话：它的转发直接连本机端口"
            ));
        }
        let file = self.load()?;
        let server = file
            .items
            .iter()
            .find(|s| s.id == id)
            .ok_or_else(|| anyhow!("服务器不存在：{id}"))?;
        // 双保险：`set_enabled` 已经拦了启用，这里再拦一次直连调用。
        if server.credential_id.trim().is_empty() {
            return Err(anyhow!("这条隧道还没填密码，无法连接"));
        }
        Ok(ConnectParams {
            server_id: server.id.clone(),
            host: server.host.clone(),
            port: server.port,
            credential_id: server.credential_id.clone(),
            keepalive_secs: server.keepalive_secs,
            trusted_fingerprint: server.host_key_fingerprint.clone(),
            enabled: server.enabled,
        })
    }

    /// 记录握手观测到的指纹，进入「待确认」。**不自动信任**（不做隐式 TOFU）。
    pub fn observe_host_key(&self, id: &str, fingerprint: &str, observed_at: &str) -> Result<()> {
        reject_local(id, "记录主机密钥")?;
        let file = self.load()?;
        let server = file
            .items
            .iter()
            .find(|s| s.id == id)
            .ok_or_else(|| anyhow!("服务器不存在：{id}"))?;
        // 已信任且匹配 → 无需待确认。
        if server.host_key_fingerprint.as_deref() == Some(fingerprint) {
            self.pending.lock().expect("pending lock").remove(id);
            return Ok(());
        }
        self.pending.lock().expect("pending lock").insert(
            id.to_string(),
            PendingHostKey {
                fingerprint: fingerprint.to_string(),
                observed_at: observed_at.to_string(),
            },
        );
        Ok(())
    }

    pub fn host_key_view(&self, id: &str) -> Result<HostKeyView> {
        // 本机没有主机密钥：给出空视图，界面照常渲染「无待确认」。
        if is_local(id) {
            return Ok(HostKeyView {
                server_id: id.to_string(),
                trusted_fingerprint: None,
                pending_fingerprint: None,
                pending_observed_at: None,
            });
        }
        let file = self.load()?;
        let server = file
            .items
            .iter()
            .find(|s| s.id == id)
            .ok_or_else(|| anyhow!("服务器不存在：{id}"))?;
        let pending = self.pending.lock().expect("pending lock").get(id).cloned();
        Ok(HostKeyView {
            server_id: id.to_string(),
            trusted_fingerprint: server.host_key_fingerprint.clone(),
            pending_fingerprint: pending.as_ref().map(|p| p.fingerprint.clone()),
            pending_observed_at: pending.as_ref().map(|p| p.observed_at.clone()),
        })
    }

    /// 接受/拒绝待确认指纹。接受 = 落盘为新信任；拒绝 = 清掉待确认。
    pub fn accept_host_key(&self, id: &str, req: AcceptHostKeyRequest) -> Result<ServerDto> {
        reject_local(id, "确认主机密钥")?;
        let pending = self
            .pending
            .lock()
            .expect("pending lock")
            .remove(id)
            .ok_or_else(|| anyhow!("没有待确认的主机密钥：{id}"))?;

        if req.reject {
            let file = self.load()?;
            return file
                .items
                .into_iter()
                .find(|s| s.id == id)
                .ok_or_else(|| anyhow!("服务器不存在：{id}"));
        }

        let mut file = self.load()?;
        let server = file
            .items
            .iter_mut()
            .find(|s| s.id == id)
            .ok_or_else(|| anyhow!("服务器不存在：{id}"))?;
        server.host_key_fingerprint = Some(pending.fingerprint);
        let updated = server.clone();
        self.save(&file)?;
        Ok(updated)
    }

    /// 供隧道模型使用的存在性判断（纯只读）。
    pub fn exists(&self, id: &str) -> Result<bool> {
        if is_local(id) {
            return Ok(true);
        }
        Ok(self.load()?.items.iter().any(|s| s.id == id))
    }

    fn require_credential(&self, credential_id: &str) -> Result<()> {
        let exists = self
            .credentials
            .list()?
            .items
            .iter()
            .any(|c| c.id == credential_id);
        if !exists {
            return Err(anyhow!("凭据不存在：{credential_id}"));
        }
        Ok(())
    }
}

fn validate_create(req: &CreateServerRequest) -> Result<()> {
    if req.name.trim().is_empty() {
        return Err(anyhow!("服务器名不能为空"));
    }
    if req.host.trim().is_empty() {
        return Err(anyhow!("主机不能为空"));
    }
    if req.port == 0 {
        return Err(anyhow!("端口必须在 1..=65535"));
    }
    Ok(())
}

/// 派生字段统一在这里算：任何返回 DTO 的路径都要过一遍，
/// 否则填完密码后列表仍会显示「待填」。
fn decorate(dto: &mut ServerDto) {
    // 本机没有凭据概念：它不该被标成「待填密码」。
    dto.needs_credential = dto.kind == ServerKind::Ssh && dto.credential_id.trim().is_empty();
}

/// 生成一个在当前集合里唯一的 id。
///
/// 老实现是 `srv_{slug}_{pid % 10000}`：同一进程里 pid 部分是常量，
/// 导入多条同名连接时会拿到相同 id，`find(|s| s.id == id)` 会改错条。
fn new_id(name: &str, taken: &HashSet<String>) -> String {
    let slug: String = name
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .take(12)
        .collect();
    let base = format!("srv_{slug}");
    if !taken.contains(&base) {
        return base;
    }
    for n in 2.. {
        let candidate = format!("{base}_{n}");
        if !taken.contains(&candidate) {
            return candidate;
        }
    }
    unreachable!("id 空间不会耗尽")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::credential::CredentialStore;
    use tl_protocol::{CreateCredentialRequest, CredentialKind};

    struct Fixture {
        dir: PathBuf,
        servers: ServerStore,
        credential_id: String,
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    fn fixture(tag: &str) -> Fixture {
        let dir = std::env::temp_dir().join(format!("threadline-srv-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create dir");

        let credentials = Arc::new(CredentialStore::new(dir.join("credentials.dat")));
        let summary = credentials
            .add(CreateCredentialRequest {
                name: String::new(),
                kind: CredentialKind::Password,
                username: "dev".into(),
                secret: "pw".into(),
                passphrase: None,
            })
            .expect("add credential");
        let servers = ServerStore::new(dir.join("servers.json"), Arc::clone(&credentials));
        Fixture {
            dir,
            servers,
            credential_id: summary.id,
        }
    }

    fn create_req(name: &str, credential_id: &str) -> CreateServerRequest {
        CreateServerRequest {
            name: name.into(),
            host: "192.0.2.90".into(),
            port: 22,
            credential_id: credential_id.into(),
            username: "root".into(),
            kind: None,
            secret: None,
            passphrase: None,
            host_key_fingerprint: None,
            keepalive_secs: 30,
            enabled: false,
        }
    }

    /// 取某条持久化条目的当前状态。
    ///
    /// 列表现在总把内置「本机」排在最前，**不能再用 `items[0]`** 取用户条目。
    fn stored(f: &Fixture, id: &str) -> ServerDto {
        f.servers
            .list()
            .expect("list")
            .items
            .into_iter()
            .find(|s| s.id == id)
            .unwrap_or_else(|| panic!("列表里没有 {id}"))
    }

    #[test]
    fn local_entry_comes_first_and_is_never_persisted() {
        let f = fixture("local-entry");
        f.servers
            .create(create_req("web-01", &f.credential_id))
            .expect("create");

        let items = f.servers.list().expect("list").items;
        assert_eq!(items[0].id, tl_protocol::LOCAL_SERVER_ID, "本机排在最前");
        assert_eq!(items[0].kind, ServerKind::Local);
        assert_eq!(items[0].name, "本机");
        assert!(items[0].enabled, "本机恒为启用");
        assert!(!items[0].needs_credential, "本机没有凭据概念");

        // 文件里只有用户建的那条：本机是注入的，不落盘。
        let raw = std::fs::read_to_string(f.servers.path()).expect("read servers.json");
        assert!(!raw.contains(LOCAL_SERVER_ID), "本机不该写进 servers.json");
    }

    #[test]
    fn local_entry_refuses_every_mutation() {
        let f = fixture("local-guard");
        let id = tl_protocol::LOCAL_SERVER_ID;

        assert!(f.servers.kind_of(id).expect("kind") == ServerKind::Local);
        assert!(f.servers.exists(id).expect("exists"));
        assert!(f.servers.set_enabled(id, false).is_err());
        assert!(f.servers.remove(id).is_err());
        assert!(f.servers.can_remove(id, &[]).is_err());
        assert!(f.servers.patch(id, PatchServerRequest::default()).is_err());
        assert!(f.servers.connect_params(id).is_err(), "本机不需要会话");
        assert!(f
            .servers
            .accept_host_key(id, AcceptHostKeyRequest { reject: true })
            .is_err());

        // 主机密钥视图给空值：界面据此渲染「无待确认」，不弹红条。
        let view = f.servers.host_key_view(id).expect("view");
        assert!(view.trusted_fingerprint.is_none());
        assert!(view.pending_fingerprint.is_none());
    }

    #[test]
    fn forged_local_entry_in_file_is_normalized_back_to_ssh() {
        // 手改 servers.json 塞一条 kind=local：不能让它抢走内置条目的身份。
        let f = fixture("forged-local");
        std::fs::write(
            f.servers.path(),
            format!(
                r#"{{"schema_version":{SCHEMA_VERSION},"items":[{{"id":"srv_x","name":"x","kind":"local","host":"192.0.2.9","port":22,"credential_id":"{}","host_key_fingerprint":null,"keepalive_secs":30}}]}}"#,
                f.credential_id
            ),
        )
        .expect("write");

        let items = f.servers.list().expect("list").items;
        let forged = items.iter().find(|s| s.id == "srv_x").expect("srv_x");
        assert_eq!(
            forged.kind,
            ServerKind::Ssh,
            "文件里的 local 一律归一成 ssh"
        );
        assert_eq!(
            items.iter().filter(|s| s.kind == ServerKind::Local).count(),
            1
        );
    }

    #[test]
    fn session_errors_show_up_in_the_list_until_cleared() {
        // 隧道行只有"未连接"三个字时，用户没法判断原因；错误必须随列表带出来，
        // 连上之后再清掉，否则会一直挂着一个过期的失败理由。
        let f = fixture("session-error");
        let server = f
            .servers
            .create(create_req("web-01", &f.credential_id))
            .expect("create");
        assert!(f.servers.list().expect("list").items[0]
            .session_error
            .is_none());

        f.servers
            .set_session_error(&server.id, "连接 192.0.2.90:22 时收到 HTTP 响应（403）");
        let shown = stored(&f, &server.id)
            .session_error
            .clone()
            .expect("错误要出现在列表里");
        assert!(shown.contains("403"), "原因要有诊断价值：{shown}");

        f.servers.clear_session_error(&server.id);
        assert!(
            stored(&f, &server.id).session_error.is_none(),
            "连上之后不能再挂着旧错误"
        );
    }

    #[test]
    fn server_without_credential_is_pending_not_broken() {
        // 导入分享包时没有密码：连接先落盘，但必须明确标记为「待填」
        let f = fixture("pending-credential");
        let dto = f
            .servers
            .create(create_req("imported", ""))
            .expect("create without credential");
        assert!(dto.credential_id.is_empty());
        assert!(dto.needs_credential);

        // 不能启用：否则会进入无意义的连接-失败循环
        let err = f
            .servers
            .set_enabled(&dto.id, true)
            .expect_err("must reject");
        assert!(err.to_string().contains("还没填密码"), "{err}");

        // 连接参数也拒绝，防止绕过 set_enabled 直接建会话
        let err = f.servers.connect_params(&dto.id).expect_err("must reject");
        assert!(err.to_string().contains("还没填密码"), "{err}");
    }

    #[test]
    fn needs_credential_is_recomputed_on_every_return_path() {
        // patch 返回的是 clone：只在 load 时回填会导致填完密码仍显示「待填」
        let f = fixture("derive-pending");
        let dto = f
            .servers
            .create(create_req("p", ""))
            .expect("create without credential");

        let patched = f
            .servers
            .patch(
                &dto.id,
                PatchServerRequest {
                    credential_id: Some(f.credential_id.clone()),
                    ..Default::default()
                },
            )
            .expect("patch");
        assert!(!patched.needs_credential);

        let listed = f.servers.list().expect("list").items;
        assert!(!listed[0].needs_credential);
    }

    #[test]
    fn pending_server_can_still_be_renamed() {
        // 导入后只改名字/登录用户：不能因为「没有材料可沿用」而失败
        let f = fixture("pending-rename");
        let dto = f
            .servers
            .create(create_req("p", ""))
            .expect("create without credential");
        let updated = f
            .servers
            .patch(
                &dto.id,
                PatchServerRequest {
                    name: Some("p2".into()),
                    username: Some("ubuntu".into()),
                    ..Default::default()
                },
            )
            .expect("patch pending");
        assert_eq!(updated.name, "p2");
        assert_eq!(updated.username, "ubuntu");
        assert!(updated.needs_credential);
    }

    #[test]
    fn server_ids_stay_unique_even_when_slugs_collide() {
        // 中文名会被整段压成 `_`：老实现下两个中文名的 id 完全相同，
        // 导入多条时 `find(|s| s.id == id)` 会改错条。
        let f = fixture("id-unique");
        let a = f
            .servers
            .create(create_req("远程", &f.credential_id))
            .expect("a");
        let b = f
            .servers
            .create(create_req("桌面", &f.credential_id))
            .expect("b");
        assert_ne!(a.id, b.id);
    }

    #[test]
    fn written_file_declares_the_current_schema_version() {
        // 磁盘上的版本号必须与共享常量一致。
        // 曾经这里写死 1、启动时按 2 判断，结果**每次重启都把用户配置当旧数据清掉**。
        let f = fixture("schema");
        f.servers
            .create(create_req("web-01", &f.credential_id))
            .expect("create");

        let text = std::fs::read_to_string(f.dir.join("servers.json")).expect("read file");
        let value: serde_json::Value = serde_json::from_str(&text).expect("json");
        let written = value
            .get("schema_version")
            .and_then(|n| n.as_u64())
            .expect("schema_version");

        assert_eq!(
            written,
            tl_protocol::SCHEMA_VERSION as u64,
            "写入的版本号必须与共享常量一致"
        );
        assert!(
            written >= 2,
            "必须达到当前结构版本，否则启动时会被当成旧数据留档丢弃"
        );
    }

    #[test]
    fn enabled_is_persisted_and_patchable() {
        let f = fixture("enabled");
        let mut req = create_req("web-01", &f.credential_id);
        req.enabled = true;
        let server = f.servers.create(req).expect("create");
        assert!(server.enabled);

        let patched = f.servers.set_enabled(&server.id, false).expect("disable");
        assert!(!patched.enabled, "关闭后必须落盘为 false");
        assert!(!stored(&f, &server.id).enabled, "列表要反映真实意图");
    }

    #[test]
    fn legacy_server_without_enabled_deserializes() {
        // 老版本文件没有 enabled 字段：读回来默认 false，不能让升级变成"全部自动连"。
        let json = r#"{"id":"srv_1","name":"web-01","host":"192.0.2.90","port":22,"credential_id":"c1","host_key_fingerprint":null,"keepalive_secs":30}"#;
        let dto: ServerDto = serde_json::from_str(json).expect("legacy dto");
        assert!(!dto.enabled);
    }

    #[test]
    fn reorder_puts_named_ids_first_and_keeps_the_rest() {
        // 界面可能只提交一部分 id（例如调整后漏了某个），剩下的必须留在末尾，
        // 而且保持它们原来的相对顺序——绝不能因为排序把隧道弄丢。
        let f = fixture("reorder");
        let a = f
            .servers
            .create(create_req("a", &f.credential_id))
            .expect("a");
        let b = f
            .servers
            .create(create_req("b", &f.credential_id))
            .expect("b");
        let c = f
            .servers
            .create(create_req("c", &f.credential_id))
            .expect("c");

        // 只看用户条目：内置「本机」永远排在最前，不参与这组断言。
        let names = |store: &ServerStore| -> Vec<String> {
            store
                .list()
                .expect("list")
                .items
                .into_iter()
                .filter(|s| s.kind == ServerKind::Ssh)
                .map(|s| s.name)
                .collect()
        };

        // 只提交 c：a、b 追加在后面
        f.servers
            .reorder(std::slice::from_ref(&c.id))
            .expect("reorder partial");
        assert_eq!(names(&f.servers), vec!["c", "a", "b"]);

        // 完整顺序
        f.servers
            .reorder(&[b.id.clone(), a.id.clone(), c.id.clone()])
            .expect("reorder full");
        assert_eq!(names(&f.servers), vec!["b", "a", "c"]);

        // 未知 id 直接忽略，不报错、不影响已有顺序
        f.servers
            .reorder(&["missing".into(), c.id.clone()])
            .expect("unknown id ignored");
        assert_eq!(names(&f.servers), vec!["c", "b", "a"]);
    }

    #[test]
    fn create_requires_existing_credential() {
        let f = fixture("nocred");
        assert!(f.servers.create(create_req("a", "cred_missing")).is_err());
        let ok = f
            .servers
            .create(create_req("a", &f.credential_id))
            .expect("create");
        assert_eq!(ok.name, "a");
    }

    #[test]
    fn duplicate_name_is_rejected() {
        let f = fixture("dup");
        f.servers
            .create(create_req("a", &f.credential_id))
            .expect("first");
        assert!(f.servers.create(create_req("a", &f.credential_id)).is_err());
    }

    #[test]
    fn connect_params_carry_trust_state() {
        let f = fixture("params");
        let srv = f
            .servers
            .create(create_req("a", &f.credential_id))
            .expect("create");
        let params = f.servers.connect_params(&srv.id).expect("params");
        assert_eq!(params.host, "192.0.2.90");
        assert_eq!(params.keepalive_secs, 30);
        assert!(params.trusted_fingerprint.is_none(), "首次连接没有信任");
    }

    #[test]
    fn host_key_needs_explicit_accept() {
        let f = fixture("trust");
        let srv = f
            .servers
            .create(create_req("a", &f.credential_id))
            .expect("create");

        f.servers
            .observe_host_key(&srv.id, "SHA256:abc", "2026-10-19T09:00:00+08:00")
            .expect("observe");

        let view = f.servers.host_key_view(&srv.id).expect("view");
        assert_eq!(view.trusted_fingerprint, None, "观测不等于信任");
        assert_eq!(view.pending_fingerprint.as_deref(), Some("SHA256:abc"));

        let accepted = f
            .servers
            .accept_host_key(&srv.id, AcceptHostKeyRequest { reject: false })
            .expect("accept");
        assert_eq!(accepted.host_key_fingerprint.as_deref(), Some("SHA256:abc"));
        assert!(f
            .servers
            .host_key_view(&srv.id)
            .expect("view")
            .pending_fingerprint
            .is_none());
    }

    #[test]
    fn rejecting_pending_keeps_trust_unchanged() {
        let f = fixture("reject");
        let srv = f
            .servers
            .create(create_req("a", &f.credential_id))
            .expect("create");
        f.servers
            .observe_host_key(&srv.id, "SHA256:evil", "t")
            .expect("observe");
        let after = f
            .servers
            .accept_host_key(&srv.id, AcceptHostKeyRequest { reject: true })
            .expect("reject");
        assert!(after.host_key_fingerprint.is_none());
        assert!(f
            .servers
            .host_key_view(&srv.id)
            .expect("view")
            .pending_fingerprint
            .is_none());
    }

    #[test]
    fn matching_trusted_fingerprint_needs_no_confirmation() {
        let f = fixture("match");
        let mut req = create_req("a", &f.credential_id);
        req.host_key_fingerprint = Some("SHA256:good".into());
        let srv = f.servers.create(req).expect("create");

        f.servers
            .observe_host_key(&srv.id, "SHA256:good", "t")
            .expect("observe");
        assert!(f
            .servers
            .host_key_view(&srv.id)
            .expect("view")
            .pending_fingerprint
            .is_none());
    }

    #[test]
    fn can_remove_reports_referencing_tunnels() {
        let f = fixture("canremove");
        let srv = f
            .servers
            .create(create_req("a", &f.credential_id))
            .expect("create");
        assert_eq!(
            f.servers.can_remove(&srv.id, &[]).expect("ok"),
            RemoveDecision::Allowed
        );
        assert_eq!(
            f.servers
                .can_remove(&srv.id, &["tun_1".to_string()])
                .expect("ok"),
            RemoveDecision::Blocked {
                referenced_by: vec!["tun_1".to_string()]
            }
        );
    }

    #[test]
    fn remove_clears_pending_state() {
        let f = fixture("remove");
        let srv = f
            .servers
            .create(create_req("a", &f.credential_id))
            .expect("create");
        f.servers
            .observe_host_key(&srv.id, "SHA256:x", "t")
            .expect("observe");
        f.servers.remove(&srv.id).expect("remove");
        assert!(f.servers.host_key_view(&srv.id).is_err());
    }
}
