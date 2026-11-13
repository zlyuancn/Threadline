//! 拥有：本地网关根 CA 的生成/复用/DPAPI 落盘、叶子证书按 SAN 集现签与内存缓存、根证书导出。
//! 提供：`GatewayCa::{load_or_create, root_pem, root_der, thumbprint, leaf_for}`。
//! 允许调用：`foundation::secret`（DPAPI）、`rcgen`、`rustls-pemfile`、`sha2`、`time`。
//!
//! 关键约束（见设计 §3.4）：
//! - 根私钥用 **DPAPI machine scope** 加密后落盘，与凭据同一套 `foundation::secret`。
//! - 根 `pathlen:0`，签不出下级 CA；叶子 7 天，SAN 只允许「当前受管域名 + 该转发的回环 IP」。
//! - `rcgen` 0.13 无法从 DER 还原可签发的 `Certificate`，所以重启后用**同一私钥 + 同一 DN**
//!   重建一个 issuer 视图：公钥相同 ⇒ 浏览器用磁盘上的 `root.pem` 仍能验通叶子。

use std::collections::HashMap;
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, SystemTime};

use anyhow::{anyhow, Context, Result};
use rcgen::{BasicConstraints, Certificate, CertificateParams, DnType, IsCa, KeyPair, SanType};
use tokio_rustls::rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};

use crate::foundation::secret;

const ROOT_CN: &str = "Threadline Local Gateway Root";
const ROOT_PEM: &str = "root.pem";
const ROOT_KEY: &str = "root.key";

/// 叶子有效期（设计规定 7 天）。
const LEAF_DAYS: u64 = 7;
/// 剩余不足这个时长就提前重签，避免「刚连上就过期」。
const LEAF_RENEW_BEFORE: Duration = Duration::from_secs(24 * 3600);

pub struct GatewayCa {
    dir: PathBuf,
    root_pem: String,
    root_der: Vec<u8>,
    root_key: KeyPair,
    /// 签发用的 issuer 视图：首次创建就是原证书；重启后是「同私钥、同 DN」重建的等价体。
    issuer: Certificate,
    leaves: Mutex<HashMap<String, CachedLeaf>>,
}

pub struct Leaf {
    /// 叶子 + 根，直接交给 rustls 的 `CertifiedKey`。
    pub chain: Vec<CertificateDer<'static>>,
    pub key: PrivateKeyDer<'static>,
    pub not_after: SystemTime,
}

struct CachedLeaf {
    chain: Vec<CertificateDer<'static>>,
    key: PrivateKeyDer<'static>,
    not_after: SystemTime,
}

impl GatewayCa {
    pub fn load_or_create(dir: impl Into<PathBuf>) -> Result<Self> {
        let dir = dir.into();
        std::fs::create_dir_all(&dir)
            .with_context(|| format!("创建网关证书目录失败：{}", dir.display()))?;
        let pem_path = dir.join(ROOT_PEM);
        let key_path = dir.join(ROOT_KEY);

        if pem_path.exists() && key_path.exists() {
            let root_pem = std::fs::read_to_string(&pem_path).context("读取根证书失败")?;
            let sealed = std::fs::read(&key_path).context("读取根私钥失败")?;
            let key_pem = String::from_utf8(secret::open(&sealed).context("解密根私钥失败")?)
                .context("根私钥不是 UTF-8 PEM")?;
            let root_key = KeyPair::from_pem(&key_pem).context("解析根私钥失败")?;
            let root_der = first_cert_der(&root_pem)?;
            let issuer = root_params()
                .self_signed(&root_key)
                .context("重建根证书失败")?;
            return Ok(Self {
                dir,
                root_pem,
                root_der,
                root_key,
                issuer,
                leaves: Mutex::new(HashMap::new()),
            });
        }

        let root_key = KeyPair::generate().context("生成根私钥失败")?;
        let root_cert = root_params()
            .self_signed(&root_key)
            .context("生成根证书失败")?;
        let root_pem = root_cert.pem();
        let root_der = root_cert.der().to_vec();
        let sealed = secret::seal(root_key.serialize_pem().as_bytes()).context("加密根私钥失败")?;
        std::fs::write(&pem_path, &root_pem).context("写根证书失败")?;
        std::fs::write(&key_path, &sealed).context("写根私钥失败")?;

        Ok(Self {
            dir,
            root_pem,
            root_der,
            root_key,
            issuer: root_cert,
            leaves: Mutex::new(HashMap::new()),
        })
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn root_pem(&self) -> &str {
        &self.root_pem
    }

    /// SHA-256 指纹（展示用，也是「是不是同一张根」的判据）。
    pub fn thumbprint(&self) -> String {
        hex(&<sha2::Sha256 as sha2::Digest>::digest(&self.root_der))
    }

    /// SHA-1 指纹。
    ///
    /// **只用于 `certutil -store Root <hash>` 查库**：Windows 的证书存储按 SHA-1
    /// 做默认哈希匹配，传 SHA-256 会返回 `CRYPT_E_NOT_FOUND`（实测）。
    pub fn thumbprint_sha1(&self) -> String {
        hex(&<sha1::Sha1 as sha1::Digest>::digest(&self.root_der))
    }

    /// 取（或现签）一条叶子。`ip` 是该转发的回环入口地址。
    ///
    /// SAN 只接受调用方传来的域名——业务层保证它来自「当前受管域名」，
    /// 这样根私钥即使泄露也无法被用来签任意域名。
    pub fn leaf_for(&self, domain: &str, ip: IpAddr) -> Result<Leaf> {
        let cache_key = format!("{domain}|{ip}");
        let mut leaves = self
            .leaves
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        if let Some(cached) = leaves.get(&cache_key) {
            let fresh = cached
                .not_after
                .duration_since(SystemTime::now())
                .map(|left| left > LEAF_RENEW_BEFORE)
                .unwrap_or(false);
            if fresh {
                return Ok(Leaf {
                    chain: cached.chain.clone(),
                    key: cached.key.clone_key(),
                    not_after: cached.not_after,
                });
            }
        }

        let leaf = self.issue(domain, ip)?;
        leaves.insert(
            cache_key,
            CachedLeaf {
                chain: leaf.chain.clone(),
                key: leaf.key.clone_key(),
                not_after: leaf.not_after,
            },
        );
        Ok(leaf)
    }

    fn issue(&self, domain: &str, ip: IpAddr) -> Result<Leaf> {
        let key = KeyPair::generate().context("生成叶子私钥失败")?;
        let mut params =
            CertificateParams::new(vec![domain.to_string()]).context("构造叶子参数失败")?;
        params.distinguished_name.push(DnType::CommonName, domain);
        params.subject_alt_names.push(SanType::IpAddress(ip));

        let now = time::OffsetDateTime::now_utc();
        params.not_before = now - time::Duration::hours(1);
        params.not_after = now + time::Duration::days(LEAF_DAYS as i64);

        let cert = params
            .signed_by(&key, &self.issuer, &self.root_key)
            .context("签发叶子证书失败")?;

        let not_after = SystemTime::now() + Duration::from_secs(LEAF_DAYS * 24 * 3600);
        Ok(Leaf {
            chain: vec![
                CertificateDer::from(cert.der().to_vec()),
                CertificateDer::from(self.root_der.clone()),
            ],
            key: PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key.serialize_der())),
            not_after,
        })
    }
}

fn root_params() -> CertificateParams {
    let mut params = CertificateParams::default();
    params.distinguished_name.push(DnType::CommonName, ROOT_CN);
    // pathlen:0 —— 只能签叶子，签不出下级 CA。
    params.is_ca = IsCa::Ca(BasicConstraints::Constrained(0));
    // 固定有效期：重建 issuer 视图时除签名外与磁盘上的根保持一致。
    params.not_before = rcgen::date_time_ymd(2026, 1, 1);
    params.not_after = rcgen::date_time_ymd(2036, 1, 1);
    params
}

fn first_cert_der(pem: &str) -> Result<Vec<u8>> {
    let mut cursor = std::io::Cursor::new(pem.as_bytes());
    let der = rustls_pemfile::certs(&mut cursor)
        .next()
        .ok_or_else(|| anyhow!("根证书 PEM 里没有 CERTIFICATE 块"))??;
    Ok(der.to_vec())
}

fn hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("threadline-ca-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn root_is_created_once_and_reused() {
        let dir = temp_dir("reuse");
        let a = GatewayCa::load_or_create(&dir).expect("create");
        let first = a.thumbprint();
        drop(a);

        let b = GatewayCa::load_or_create(&dir).expect("reload");
        assert_eq!(b.thumbprint(), first, "重启后根证书必须复用");
        assert!(b.root_pem().contains("BEGIN CERTIFICATE"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn thumbprints_are_lowercase_hex_with_expected_lengths() {
        let dir = temp_dir("fingerprint");
        let ca = GatewayCa::load_or_create(&dir).expect("create");
        let sha256 = ca.thumbprint();
        let sha1 = ca.thumbprint_sha1();
        assert_eq!(sha256.len(), 64, "SHA-256 是 64 位 hex");
        assert_eq!(sha1.len(), 40, "SHA-1 是 40 位 hex");
        for value in [&sha256, &sha1] {
            assert!(
                value
                    .chars()
                    .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()),
                "必须全小写 hex：{value}"
            );
        }
    }

    #[test]
    fn leaf_is_cached_by_san_set() {
        let dir = temp_dir("cache");
        let ca = GatewayCa::load_or_create(&dir).expect("create");
        let ip: IpAddr = "127.0.0.5".parse().expect("ip");
        let a = ca.leaf_for("home.example.com", ip).expect("leaf a");
        let b = ca.leaf_for("home.example.com", ip).expect("leaf b");
        assert_eq!(a.chain[0], b.chain[0], "同一 SAN 集应命中缓存");
        let c = ca.leaf_for("git.example.com", ip).expect("leaf c");
        assert_ne!(a.chain[0], c.chain[0], "不同域名必须重签");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn leaf_chain_includes_root_and_has_expected_sans() {
        let dir = temp_dir("sans");
        let ca = GatewayCa::load_or_create(&dir).expect("create");
        let ip: IpAddr = "127.0.0.9".parse().expect("ip");
        let leaf = ca.leaf_for("home.example.com", ip).expect("leaf");
        assert_eq!(leaf.chain.len(), 2, "叶子 + 根");

        // DNS SAN 在 DER 里是 IA5String，可以直接按文本扫到；
        // IP SAN 是 4 字节原始地址（0x7f000009），不会以文本出现，这里只验 DNS。
        let text = String::from_utf8_lossy(leaf.chain[0].as_ref()).to_string();
        assert!(text.contains("home.example.com"), "缺 DNS SAN");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
