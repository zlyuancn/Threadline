//! 拥有：本地网关 CA 的对外状态、Agent 信任上报的落盘、根证书导出与重置。
//! 提供：`GatewayService::{status, root_pem, apply_trust_result, reset}`。
//! 允许调用：`foundation::tls_ca`、`foundation::store`、`path`。
//!
//! 为什么信任状态只存「Agent 上报值」：开机自启档 Core 是 SYSTEM，查的是 `S-1-5-18`
//! 的根证书库，自己判断会永远显示未信任；只有登录用户身份的 Agent 能看到真实结果。

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use tl_protocol::{GatewayCaStatus, GatewayTrustResult};

use crate::foundation::tls_ca::GatewayCa;
use crate::foundation::{clock, store};

const TRUST_FILE: &str = "gateway-trust.json";

#[derive(Debug, Default, Serialize, Deserialize)]
struct TrustState {
    #[serde(default)]
    thumbprint: Option<String>,
    #[serde(default)]
    trusted: Option<bool>,
    #[serde(default)]
    at: Option<String>,
    #[serde(default)]
    error: Option<String>,
    /// 根已换但进程还持有旧 CA（重启后才生效）。
    #[serde(default)]
    restart_required: bool,
}

pub struct GatewayService {
    ca: Option<Arc<GatewayCa>>,
    ca_dir: PathBuf,
    trust_path: PathBuf,
    state: Mutex<TrustState>,
}

impl GatewayService {
    pub fn new(ca: Option<Arc<GatewayCa>>, data_dir: impl AsRef<Path>) -> Self {
        let data_dir = data_dir.as_ref();
        let trust_path = data_dir.join(TRUST_FILE);
        let state = store::read_json::<TrustState>(&trust_path)
            .ok()
            .flatten()
            .unwrap_or_default();
        Self {
            ca,
            ca_dir: data_dir.join("gateway-ca"),
            trust_path,
            state: Mutex::new(state),
        }
    }

    pub fn status(&self) -> Result<GatewayCaStatus> {
        let state = self.state.lock().expect("trust lock");
        let ca = self
            .ca
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("本地网关证书不可用（初始化失败，见启动日志）"))?;
        // 指纹变了（用户手动换了 CA 目录）→ 上报值失效，不能继续显示「已信任」。
        let fresh = state
            .thumbprint
            .as_deref()
            .map(|stored| stored == ca.thumbprint())
            .unwrap_or(false);
        Ok(GatewayCaStatus {
            dir: ca.dir().display().to_string(),
            thumbprint: ca.thumbprint(),
            thumbprint_sha1: ca.thumbprint_sha1(),
            trusted: if fresh { state.trusted } else { None },
            last_probed_at: if fresh { state.at.clone() } else { None },
            last_error: state.error.clone(),
            restart_required: state.restart_required,
        })
    }

    pub fn root_pem(&self) -> Result<String> {
        self.ca
            .as_ref()
            .map(|ca| ca.root_pem().to_string())
            .ok_or_else(|| anyhow::anyhow!("本地网关证书不可用"))
    }

    /// 记录 Agent 的探测结果。指纹不匹配的上报直接丢弃——那是上一次 CA 的结论。
    pub fn apply_trust_result(&self, result: GatewayTrustResult) -> Result<()> {
        let Some(ca) = self.ca.as_ref() else {
            // 没有 CA 时也允许上报（否则 Agent 一直重试），只是不落盘。
            return Ok(());
        };
        if result.thumbprint != ca.thumbprint() {
            return Ok(());
        }
        let mut state = self.state.lock().expect("trust lock");
        state.thumbprint = Some(result.thumbprint);
        state.trusted = Some(result.trusted);
        state.at = Some(result.at);
        state.error = None;
        drop(state);
        self.save()
    }

    /// 删除盘上的 CA。**不重建**：运行中的监听还持有旧 CA，换根必须重启 Core。
    pub fn reset(&self) -> Result<()> {
        if self.ca_dir.exists() {
            std::fs::remove_dir_all(&self.ca_dir)
                .with_context(|| format!("删除 CA 目录失败：{}", self.ca_dir.display()))?;
        }
        let mut state = self.state.lock().expect("trust lock");
        *state = TrustState {
            restart_required: true,
            ..TrustState::default()
        };
        drop(state);
        self.save()
    }

    fn save(&self) -> Result<()> {
        let state = self.state.lock().expect("trust lock");
        store::write_json_atomic(&self.trust_path, &*state)
    }
}

/// 供接口层生成「探测时间」用，与其它 DTO 的时间格式保持一致。
pub fn now_iso8601() -> String {
    clock::now_iso8601()
}
