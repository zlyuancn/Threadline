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

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use anyhow::{anyhow, Result};
use tl_protocol::{
    AcceptHostKeyRequest, CreateServerRequest, CredentialKind, HostKeyView, PatchServerRequest,
    ServerDto, ServerList, ServersFile,
};

use crate::foundation::store;
use crate::models::credential::CredentialStore;
use crate::models::RemoveDecision;

use tl_protocol::SCHEMA_VERSION;

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
        }
        Ok(ServerList { items })
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
        self.require_credential(&req.credential_id)?;

        let mut file = self.load()?;
        if file.items.iter().any(|s| s.name == req.name) {
            return Err(anyhow!("服务器名已存在：{}", req.name));
        }
        let dto = ServerDto {
            id: new_id(&req.name),
            name: req.name,
            host: req.host,
            port: req.port,
            credential_id: req.credential_id,
            username: req.username,
            credential_kind: req.kind.unwrap_or(CredentialKind::Password),
            host_key_fingerprint: req.host_key_fingerprint,
            keepalive_secs: req.keepalive_secs,
            enabled: req.enabled,
            session_error: None,
        };
        file.items.push(dto.clone());
        self.save(&file)?;
        Ok(dto)
    }

    /// 只改「启用」意图。走单独方法是因为它必须伴随会话的建立/断开，
    /// 由业务层编排；`patch()` 只管纯配置字段。
    pub fn set_enabled(&self, id: &str, enabled: bool) -> Result<ServerDto> {
        let mut file = self.load()?;
        let server = file
            .items
            .iter_mut()
            .find(|s| s.id == id)
            .ok_or_else(|| anyhow!("服务器不存在：{id}"))?;
        server.enabled = enabled;
        let dto = server.clone();
        self.save(&file)?;
        Ok(dto)
    }

    pub fn patch(&self, id: &str, req: PatchServerRequest) -> Result<ServerDto> {
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
        let updated = server.clone();
        self.save(&file)?;
        Ok(updated)
    }

    pub fn remove(&self, id: &str) -> Result<()> {
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
        let file = self.load()?;
        let server = file
            .items
            .iter()
            .find(|s| s.id == id)
            .ok_or_else(|| anyhow!("服务器不存在：{id}"))?;
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
    if req.credential_id.trim().is_empty() {
        return Err(anyhow!("必须指定凭据"));
    }
    Ok(())
}

fn new_id(name: &str) -> String {
    let slug: String = name
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .take(12)
        .collect();
    format!("srv_{slug}_{}", std::process::id() % 10000)
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
        let shown = f.servers.list().expect("list").items[0]
            .session_error
            .clone()
            .expect("错误要出现在列表里");
        assert!(shown.contains("403"), "原因要有诊断价值：{shown}");

        f.servers.clear_session_error(&server.id);
        assert!(
            f.servers.list().expect("list").items[0]
                .session_error
                .is_none(),
            "连上之后不能再挂着旧错误"
        );
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
        assert!(
            !f.servers.list().expect("list").items[0].enabled,
            "列表要反映真实意图"
        );
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

        let names = |store: &ServerStore| -> Vec<String> {
            store
                .list()
                .expect("list")
                .items
                .into_iter()
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
