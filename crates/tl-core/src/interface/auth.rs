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

    let from_header = header.strip_prefix("Bearer ").unwrap_or_default();

    // 浏览器不能给 WebSocket 设请求头，所以两条 WS 路由只能用 query 带令牌。
    // **只对 `/console` 与 `/admin/terminal` 开这个口子**：其余端点仍必须走
    // Authorization，免得令牌以 query 的形式散落到各处日志里。
    let provided = if from_header.is_empty() {
        pty_query_token(&request).unwrap_or_default()
    } else {
        from_header.to_string()
    };

    // 空令牌一律拒绝：不依赖"装配方一定给非空 token"这条外部不变量。
    if provided.is_empty() || !constant_time_eq(provided.as_bytes(), state.token.as_bytes()) {
        return Err(ApiFailure::unauthorized());
    }
    Ok(next.run(request).await)
}

/// 只有这两条 WS 路由吃 `?token=`：容器控制台 `/admin/containers/{cid}/console`、
/// 服务器终端 `/admin/terminal`。
///
/// 用后缀比对而不是前缀：`/admin/terminals` 这类路径不在路由表里，
/// 也不该因为长度相似被放行。
fn is_pty_route(path: &str) -> bool {
    path.ends_with("/console") || path.ends_with("/admin/terminal")
}

/// 从 `?token=` 取令牌，**仅限上面两条 WS 路由**。
///
/// 不做百分号解码：核心签发的令牌是十六进制串，不含需要转义的字符；
/// 真出现别的字符，比较自然会失败，而不是被解码成另一个令牌。
fn pty_query_token(request: &Request) -> Option<String> {
    if !is_pty_route(request.uri().path()) {
        return None;
    }
    request
        .uri()
        .query()?
        .split('&')
        .find_map(|pair| pair.strip_prefix("token=").map(str::to_string))
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

    fn request(uri: &str) -> axum::extract::Request {
        axum::extract::Request::builder()
            .uri(uri)
            .body(axum::body::Body::empty())
            .expect("请求可构造")
    }

    #[test]
    fn query_token_is_accepted_for_pty_routes_only() {
        let console =
            request("/api/v1/servers/s1/admin/containers/web/console?mode=exec&token=abc");
        assert_eq!(pty_query_token(&console).as_deref(), Some("abc"));

        let terminal = request("/api/v1/servers/s1/admin/terminal?cols=100&token=abc");
        assert_eq!(pty_query_token(&terminal).as_deref(), Some("abc"));

        // 其余路由不吃 query 令牌：令牌不该顺着 URL 散到各处日志里
        let list = request("/api/v1/servers/s1/admin/containers?token=abc");
        assert_eq!(pty_query_token(&list), None);

        // 后缀比对的边界：`terminals` 不是终端路由
        let plural = request("/api/v1/servers/s1/admin/terminals?token=abc");
        assert_eq!(pty_query_token(&plural), None);
    }
}
