//! 拥有：随机 Bearer token 的生成（CSPRNG）。
//! 提供：`random_token`。
//! 允许调用：`getrandom`、`base64`。
//!
//! 每次 Core 启动轮换一次，写进 `control.json`；Agent 与 Wake 读同一个值。

use anyhow::{anyhow, Result};
use base64::Engine;

/// 生成 `bytes` 字节随机数的 Base64URL（无填充）表示。
pub fn random_token(bytes: usize) -> Result<String> {
    let mut buf = vec![0u8; bytes];
    getrandom::getrandom(&mut buf).map_err(|e| anyhow!("获取系统随机数失败：{e}"))?;
    Ok(base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(buf))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn token_has_expected_entropy_and_is_url_safe() {
        let token = random_token(32).expect("token");
        // 32 字节 → Base64URL 43 个字符（无填充）
        assert_eq!(token.len(), 43);
        assert!(token
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'));
    }

    #[test]
    fn tokens_do_not_repeat() {
        let mut seen = HashSet::new();
        for _ in 0..64 {
            assert!(seen.insert(random_token(32).expect("token")));
        }
    }

    #[test]
    fn length_tracks_requested_bytes() {
        assert_eq!(random_token(16).expect("16").len(), 22);
        assert_eq!(random_token(48).expect("48").len(), 64);
    }
}
