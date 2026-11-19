//! 门面暴露给 Kotlin 的数据形状。
//!
//! 纪律：这里只有**值类型与映射**，没有判定规则（规则在 `tl-core` 的 models 里）。

/// 一条 `-L` 转发的启动参数（A0 spike 只支持密码登录）。
#[derive(Debug, Clone, uniffi::Record)]
pub struct ForwardRequest {
    /// 公网那台跳板机 / 家里 sshd 的地址。
    pub server_host: String,
    pub server_port: u16,
    pub username: String,
    /// 密码。spike 阶段只在内存里用一次，不落盘。
    pub password: String,
    /// 已信任的主机密钥指纹；`None` = 首次连接，必须先由用户核对指纹。
    pub trusted_fingerprint: Option<String>,
    /// 本地监听地址（`127.0.0.1` / `127.0.0.x`）。
    pub listen_host: String,
    pub listen_port: u16,
    /// 从服务器那一侧要连的目标（家里内网地址）。
    pub remote_host: String,
    pub remote_port: u16,
    /// keepalive 秒数；`0` 取默认 30。
    pub keepalive_secs: u32,
}

/// 累计流量快照（界面自己两次采样做差）。
#[derive(Debug, Clone, Copy, uniffi::Record)]
pub struct TrafficCount {
    pub up: u64,
    pub down: u64,
}

/// 启动失败的分类。Kotlin 侧按类型分支，**不靠解析文案**。
///
/// 字段名刻意用 `reason` 而不是 `message`：UniFFI 会让 Error 变体的字段成为 Kotlin 异常类的属性，
/// 叫 `message` 会与 `Throwable.message` 撞车，生成的代码在 `FfiConverter` 里编译不过（overload ambiguity）。
#[derive(Debug, uniffi::Error)]
pub enum MobileError {
    /// 本地端口绑不上（被占用 / 无权限）。
    BindFailed { reason: String },
    /// 首次连接：必须把指纹给用户核对，再用它调一次。
    NeedsTrust { fingerprint: String },
    /// 指纹与记录不一致：**不提供"仍然继续"**。
    FingerprintMismatch { trusted: String, observed: String },
    /// 其它失败；`reason` 已是人能读的原因（与桌面同口径）。
    Failed { reason: String },
}

impl std::fmt::Display for MobileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MobileError::BindFailed { reason } | MobileError::Failed { reason } => {
                f.write_str(reason)
            }
            MobileError::NeedsTrust { fingerprint } => {
                write!(f, "主机密钥待确认：{fingerprint}")
            }
            MobileError::FingerprintMismatch { trusted, observed } => {
                write!(f, "主机密钥与记录不一致（记录 {trusted}，实测 {observed}）")
            }
        }
    }
}

impl std::error::Error for MobileError {}
