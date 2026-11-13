//! 凭据模型。
//!
//! **拥有**：`credentials.dat` 里的全部凭据条目（种类、用户名、密钥材料、口令）与其加解密。
//! **提供**：`list` / `add` / `update_secret` / `remove` / `can_remove` / `material`。
//! **允许调用**：`foundation::secret`（DPAPI）、`foundation::store`（原子落盘）、`foundation::token`（生成 id）。
//!
//! 纪律：
//! - 明文**只**存在于本模块内存与调用方的请求体内；`list` 只返回摘要（不含任何密钥字段）。
//! - 落盘一律先过 DPAPI（machine scope），保证 SYSTEM 与用户身份都能解。
//! - 「能否删除」的判定留在本模型（`can_remove`），业务层只负责收集引用方 id 列表并编排。
//!
//! termination: phase —— 本模型对外的引用询问是纯只读、无副作用，不推进任何状态。

use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};
use base64::Engine;
use tl_protocol::{
    CreateCredentialRequest, CredentialItem, CredentialKind, CredentialList, CredentialSummary,
    CredentialsFile, PatchCredentialRequest,
};

use crate::foundation::{secret, store, token};

const SCHEMA_VERSION: u32 = 1;
const B64: base64::engine::GeneralPurpose = base64::engine::general_purpose::STANDARD;

pub use crate::models::RemoveDecision;

/// 认证材料：**只在进程内使用，绝不落日志、绝不进响应体**。
#[derive(Clone, PartialEq, Eq)]
pub struct CredentialMaterial {
    pub kind: CredentialKind,
    pub username: String,
    pub secret: String,
    pub passphrase: Option<String>,
}

impl std::fmt::Debug for CredentialMaterial {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CredentialMaterial")
            .field("kind", &self.kind)
            .field("username", &self.username)
            .field("secret", &"<redacted>")
            .field(
                "passphrase",
                &if self.passphrase.is_some() {
                    "<set>"
                } else {
                    "<none>"
                },
            )
            .finish()
    }
}

/// 凭据仓库：独占 `credentials.dat`。
#[derive(Debug, Clone)]
pub struct CredentialStore {
    path: PathBuf,
}

impl CredentialStore {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// 读取并解密；文件不存在视为空集合（首跑）。
    fn load(&self) -> Result<CredentialsFile> {
        let sealed = match store::read_json::<SealedFile>(&self.path)? {
            Some(file) => file,
            None => {
                return Ok(CredentialsFile {
                    schema_version: SCHEMA_VERSION,
                    items: Vec::new(),
                })
            }
        };
        let plain = secret::open(&sealed.payload_b64_decode()?)
            .context("解密凭据文件失败（可能由另一个身份加密）")?;
        let file: CredentialsFile =
            serde_json::from_slice(&plain).context("凭据文件内容不是合法 JSON")?;
        Ok(file)
    }

    /// 加密并原子落盘。
    fn save(&self, file: &CredentialsFile) -> Result<()> {
        let plain = serde_json::to_vec(file).context("序列化凭据失败")?;
        let sealed = secret::seal(&plain).context("加密凭据失败")?;
        store::write_json_atomic(
            &self.path,
            &SealedFile {
                schema_version: SCHEMA_VERSION,
                payload: B64.encode(sealed),
            },
        )
    }

    pub fn list(&self) -> Result<CredentialList> {
        let file = self.load()?;
        Ok(CredentialList {
            items: file.items.iter().map(summarize).collect(),
        })
    }

    pub fn add(&self, req: CreateCredentialRequest) -> Result<CredentialSummary> {
        if req.username.trim().is_empty() {
            return Err(anyhow!("用户名不能为空"));
        }
        if req.secret.is_empty() {
            return Err(anyhow!("密钥材料不能为空"));
        }
        let mut file = self.load()?;
        // 名字只用于辨识，留空时用用户名兜底——列表里出现空白项没有任何意义。
        let name = if req.name.trim().is_empty() {
            req.username.trim().to_string()
        } else {
            req.name.trim().to_string()
        };
        let item = CredentialItem {
            id: new_id(),
            name,
            kind: req.kind,
            username: req.username,
            secret_b64: B64.encode(req.secret.as_bytes()),
            passphrase_b64: req.passphrase.map(|p| B64.encode(p.as_bytes())),
        };
        let summary = summarize(&item);
        file.items.push(item);
        self.save(&file)?;
        Ok(summary)
    }

    /// 局部更新：可以只改名字，也可以换密钥（换密钥时口令一起替换）。
    pub fn update(&self, id: &str, req: PatchCredentialRequest) -> Result<CredentialSummary> {
        if req.name.is_none() && req.secret.is_none() {
            return Err(anyhow!("没有需要更新的字段"));
        }
        if let Some(name) = &req.name {
            if name.trim().is_empty() {
                return Err(anyhow!("凭据名不能为空"));
            }
        }
        if let Some(secret) = &req.secret {
            if secret.is_empty() {
                return Err(anyhow!("密钥材料不能为空"));
            }
        }

        let mut file = self.load()?;
        let item = file
            .items
            .iter_mut()
            .find(|it| it.id == id)
            .ok_or_else(|| anyhow!("凭据不存在：{id}"))?;

        if let Some(name) = req.name {
            item.name = name.trim().to_string();
        }
        if let Some(secret) = req.secret {
            item.secret_b64 = B64.encode(secret.as_bytes());
            item.passphrase_b64 = req.passphrase.map(|p| B64.encode(p.as_bytes()));
        }

        let summary = summarize(item);
        self.save(&file)?;
        Ok(summary)
    }

    pub fn remove(&self, id: &str) -> Result<()> {
        let mut file = self.load()?;
        let before = file.items.len();
        file.items.retain(|it| it.id != id);
        if file.items.len() == before {
            return Err(anyhow!("凭据不存在：{id}"));
        }
        self.save(&file)
    }

    /// 是否仍被服务器引用（判定规则归本模型，业务层只提供引用方列表）。
    pub fn can_remove(&self, id: &str, servers_using_it: &[String]) -> Result<RemoveDecision> {
        if servers_using_it.is_empty() {
            return Ok(RemoveDecision::Allowed);
        }
        let file = self.load()?;
        if !file.items.iter().any(|it| it.id == id) {
            return Ok(RemoveDecision::Allowed);
        }
        Ok(RemoveDecision::Blocked {
            referenced_by: servers_using_it.to_vec(),
        })
    }

    /// 取出认证材料（进程内明文）。凭据不存在或解密失败都是错误。
    pub fn material(&self, id: &str) -> Result<CredentialMaterial> {
        let file = self.load()?;
        let item = file
            .items
            .iter()
            .find(|it| it.id == id)
            .ok_or_else(|| anyhow!("凭据不存在：{id}"))?;

        let secret_text = decode(&item.secret_b64).context("解码密钥材料失败")?;
        let passphrase = match &item.passphrase_b64 {
            Some(p) => Some(decode(p).context("解码口令失败")?),
            None => None,
        };
        Ok(CredentialMaterial {
            kind: item.kind,
            username: item.username.clone(),
            secret: secret_text,
            passphrase,
        })
    }
}

/// 磁盘格式：DPAPI 密文的 Base64 外壳（禁止把明文写进任何字段名里）。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct SealedFile {
    schema_version: u32,
    payload: String,
}

impl SealedFile {
    fn payload_b64_decode(&self) -> Result<Vec<u8>> {
        B64.decode(self.payload.as_bytes())
            .context("凭据文件不是合法 Base64")
    }
}

fn summarize(item: &CredentialItem) -> CredentialSummary {
    CredentialSummary {
        id: item.id.clone(),
        name: item.name.clone(),
        kind: item.kind,
        username: item.username.clone(),
        has_passphrase: item.passphrase_b64.is_some(),
    }
}

fn decode(b64: &str) -> Result<String> {
    let bytes = B64.decode(b64.as_bytes()).context("Base64 解码失败")?;
    String::from_utf8(bytes).context("凭据内容不是合法 UTF-8")
}

fn new_id() -> String {
    // 6 字节随机 → 12 hex 字符；足够短且不撞。
    let raw = token::random_token(6).unwrap_or_else(|_| "fallback".into());
    format!("cred_{}", raw.replace(['-', '_'], "x").to_lowercase())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_store(tag: &str) -> CredentialStore {
        let dir =
            std::env::temp_dir().join(format!("threadline-cred-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create dir");
        CredentialStore::new(dir.join("credentials.dat"))
    }

    fn password_req(
        username: &str,
        secret: &str,
        passphrase: Option<&str>,
    ) -> CreateCredentialRequest {
        CreateCredentialRequest {
            name: String::new(),
            kind: CredentialKind::Password,
            username: username.into(),
            secret: secret.into(),
            passphrase: passphrase.map(Into::into),
        }
    }

    #[test]
    fn name_is_persisted_and_returned_in_summaries() {
        let store = temp_store("name");
        let mut req = password_req("root", "pw", None);
        req.name = "生产机 root".into();
        let summary = store.add(req).expect("add");
        assert_eq!(summary.name, "生产机 root");

        let listed = store.list().expect("list");
        assert_eq!(listed.items[0].name, "生产机 root");

        let _ = std::fs::remove_dir_all(store.path().parent().unwrap());
    }

    #[test]
    fn blank_name_falls_back_to_username() {
        let store = temp_store("noname");
        let summary = store.add(password_req("deploy", "pw", None)).expect("add");
        assert_eq!(
            summary.name, "deploy",
            "没起名字时用用户名兜底，列表里不能是空白"
        );

        let _ = std::fs::remove_dir_all(store.path().parent().unwrap());
    }

    #[test]
    fn legacy_item_without_name_deserializes() {
        // 老版本落盘的条目没有 name 字段，读回来必须还能用（否则升级即丢配置）。
        let json = r#"{"id":"cred_1","kind":"password","username":"root","secret_b64":"cHc=","passphrase_b64":null}"#;
        let item: CredentialItem = serde_json::from_str(json).expect("legacy item");
        assert_eq!(item.name, "");
        assert_eq!(item.username, "root");
    }

    #[test]
    fn add_then_material_roundtrips() {
        let store = temp_store("roundtrip");
        let summary = store
            .add(password_req("dev", "s3cret", Some("pp")))
            .expect("add");
        assert_eq!(summary.username, "dev");
        assert!(summary.has_passphrase);

        let material = store.material(&summary.id).expect("material");
        assert_eq!(material.secret, "s3cret");
        assert_eq!(material.passphrase.as_deref(), Some("pp"));

        let _ = std::fs::remove_dir_all(store.path().parent().unwrap());
    }

    #[test]
    fn on_disk_file_leaks_no_plaintext() {
        let store = temp_store("leak");
        store
            .add(password_req("dev", "PLAINTEXT-MARKER-42", None))
            .expect("add");

        let raw = std::fs::read_to_string(store.path()).expect("read file");
        assert!(!raw.contains("PLAINTEXT-MARKER-42"), "磁盘上不得出现明文");
        assert!(
            !raw.contains("secret"),
            "磁盘格式字段名不得暗示明文存储：{raw}"
        );

        let _ = std::fs::remove_dir_all(store.path().parent().unwrap());
    }

    #[test]
    fn list_never_returns_secret_fields() {
        let store = temp_store("summary");
        store.add(password_req("dev", "hidden", None)).expect("add");
        let list = store.list().expect("list");
        let json = serde_json::to_string(&list).expect("serialize");
        assert!(!json.contains("hidden"));
        assert!(!json.contains("\"secret\""));
        assert_eq!(list.items.len(), 1);

        let _ = std::fs::remove_dir_all(store.path().parent().unwrap());
    }

    #[test]
    fn update_secret_replaces_material() {
        let store = temp_store("update");
        let s = store.add(password_req("dev", "old", None)).expect("add");
        store
            .update(
                &s.id,
                PatchCredentialRequest {
                    name: None,
                    secret: Some("new".into()),
                    passphrase: Some("pp2".into()),
                },
            )
            .expect("update");
        let material = store.material(&s.id).expect("material");
        assert_eq!(material.secret, "new");
        assert_eq!(material.passphrase.as_deref(), Some("pp2"));

        let _ = std::fs::remove_dir_all(store.path().parent().unwrap());
    }

    #[test]
    fn rename_keeps_the_secret_material() {
        let store = temp_store("rename");
        let s = store.add(password_req("dev", "pw", None)).expect("add");

        let summary = store
            .update(
                &s.id,
                PatchCredentialRequest {
                    name: Some("生产 root".into()),
                    secret: None,
                    passphrase: None,
                },
            )
            .expect("rename");

        assert_eq!(summary.name, "生产 root");
        assert_eq!(
            store.material(&s.id).expect("material").secret,
            "pw",
            "只改名不能动密钥材料"
        );

        let _ = std::fs::remove_dir_all(store.path().parent().unwrap());
    }

    #[test]
    fn patch_rejects_empty_name_and_empty_payload() {
        let store = temp_store("patch-bad");
        let s = store.add(password_req("dev", "pw", None)).expect("add");

        assert!(store
            .update(
                &s.id,
                PatchCredentialRequest {
                    name: Some("   ".into()),
                    secret: None,
                    passphrase: None,
                },
            )
            .is_err());

        assert!(
            store
                .update(
                    &s.id,
                    PatchCredentialRequest {
                        name: None,
                        secret: None,
                        passphrase: None,
                    },
                )
                .is_err(),
            "空 patch 应报错而不是静默成功"
        );

        let _ = std::fs::remove_dir_all(store.path().parent().unwrap());
    }

    #[test]
    fn remove_works_and_missing_id_is_an_error() {
        let store = temp_store("remove");
        let s = store.add(password_req("dev", "x", None)).expect("add");
        store.remove(&s.id).expect("remove");
        assert!(store.list().expect("list").items.is_empty());
        assert!(store.remove(&s.id).is_err());
        assert!(store.material(&s.id).is_err());

        let _ = std::fs::remove_dir_all(store.path().parent().unwrap());
    }

    #[test]
    fn can_remove_reports_referencing_servers() {
        let store = temp_store("canremove");
        let s = store.add(password_req("dev", "x", None)).expect("add");

        assert_eq!(
            store.can_remove(&s.id, &[]).expect("no reference"),
            RemoveDecision::Allowed
        );
        assert_eq!(
            store
                .can_remove(&s.id, &["srv_1".to_string()])
                .expect("decision"),
            RemoveDecision::Blocked {
                referenced_by: vec!["srv_1".to_string()]
            }
        );

        let _ = std::fs::remove_dir_all(store.path().parent().unwrap());
    }

    #[test]
    fn rejects_empty_username_or_secret() {
        let store = temp_store("validate");
        assert!(store.add(password_req("  ", "x", None)).is_err());
        assert!(store.add(password_req("dev", "", None)).is_err());
        let _ = std::fs::remove_dir_all(store.path().parent().unwrap());
    }

    #[test]
    fn material_debug_redacts_secret() {
        let material = CredentialMaterial {
            kind: CredentialKind::PrivateKey,
            username: "dev".into(),
            secret: "TOP-SECRET".into(),
            passphrase: Some("PP".into()),
        };
        let debug = format!("{material:?}");
        assert!(!debug.contains("TOP-SECRET"));
        assert!(!debug.contains("PP"));
    }
}
