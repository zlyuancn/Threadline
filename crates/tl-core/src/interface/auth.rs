//! 接口层的鉴权：Bearer token 校验（常量时间比较）。
//!
//! 归属：**接口层**（协议适配）。只做"请求是否携带正确令牌"的判定，不涉及业务。
//! 允许调用：`axum`、`tl_protocol`。

use std::sync::Arc;

use axum::extract::{Request, State};
use axum::middleware::Next;
use axum::response::Response;

use crate::interface::error::ApiFailure;
use crate::interface::AppState;

/// 中间件：除 `/healthz` 外的所有端点都要求 `Authorization: Bearer <token>`。
pub async fn require_token(
    State(state): State<Arc<AppState>>,
    request: Request,
    next: Next,
) -> Result<Response, ApiFailure> {
    let header = request
        .headers()
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default();

    let provided = header.strip_prefix("Bearer ").unwrap_or_default();
    // 空令牌一律拒绝：不依赖"装配方一定给非空 token"这条外部不变量。
    if provided.is_empty() || !constant_time_eq(provided.as_bytes(), state.token.as_bytes()) {
        return Err(ApiFailure::unauthorized());
    }
    Ok(next.run(request).await)
}

/// 常量时间比较，避免按字节提前返回泄露前缀信息。
pub fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn equal_tokens_match() {
        assert!(constant_time_eq(b"abc123", b"abc123"));
    }

    #[test]
    fn different_tokens_do_not_match() {
        assert!(!constant_time_eq(b"abc123", b"abc124"));
        assert!(!constant_time_eq(b"abc", b"abcd"));
        assert!(!constant_time_eq(b"", b"x"));
    }

    #[test]
    fn empty_matches_empty_but_never_a_real_token() {
        assert!(constant_time_eq(b"", b""));
        assert!(!constant_time_eq(b"", b"token"));
    }
}
