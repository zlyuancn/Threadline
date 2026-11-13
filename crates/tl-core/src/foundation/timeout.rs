//! 拥有：统一的超时口径与默认值。
//! 提供：`with_timeout` / `with_timeout_secs` / 若干默认时长。
//! 允许调用：`tokio::time`。
//!
//! 为什么需要这一层：engine 里散着多处裸 `tokio::time::timeout(Duration::from_secs(10), ...)`，
//! 数值与语义各写各的。入口 TLS 握手（②）再插一处就更容易走样，所以先收口。

use std::future::Future;
use std::time::Duration;

/// 入口 TLS 握手（② 终止模式）：半开连接不应长期占着任务。
pub const TLS_HANDSHAKE: Duration = Duration::from_secs(10);

/// 建立 SSH `direct-tcpip` 通道。
pub const CHANNEL_OPEN: Duration = Duration::from_secs(10);

/// 读取 `ProxyOverride` / hosts 这类小文件的兜底上限（防被杀软卡死）。
pub const LOCAL_IO: Duration = Duration::from_secs(5);

pub async fn with_timeout<T, F>(
    duration: Duration,
    future: F,
) -> Result<T, tokio::time::error::Elapsed>
where
    F: Future<Output = T>,
{
    tokio::time::timeout(duration, future).await
}

pub async fn with_timeout_secs<T, F>(secs: u64, future: F) -> Result<T, tokio::time::error::Elapsed>
where
    F: Future<Output = T>,
{
    with_timeout(Duration::from_secs(secs), future).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn returns_value_when_it_finishes_in_time() {
        let value = with_timeout(Duration::from_millis(500), async { 42 })
            .await
            .expect("should not time out");
        assert_eq!(value, 42);
    }

    #[tokio::test]
    async fn times_out_when_future_stalls() {
        let result = with_timeout(Duration::from_millis(20), async {
            tokio::time::sleep(Duration::from_millis(500)).await;
            1
        })
        .await;
        assert!(result.is_err());
    }
}
