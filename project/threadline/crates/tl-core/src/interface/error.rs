//! 接口层的错误适配：把内部错误翻成统一错误体 + HTTP 状态码。
//!
//! 归属：**接口层**（协议适配）。这里只做"错误 → 状态码 + 错误体"的翻译，不含业务判定。
//! 允许调用：`tl_protocol::{ErrorBody}`、`axum`。

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use tl_protocol::ErrorBody;

/// 带状态码的错误响应。
#[derive(Debug)]
pub struct ApiFailure {
    pub status: StatusCode,
    pub body: ErrorBody,
}

impl ApiFailure {
    pub fn new(status: StatusCode, code: &str, message: impl Into<String>) -> Self {
        Self {
            status,
            body: ErrorBody::new(code, message, None),
        }
    }

    pub fn field(status: StatusCode, code: &str, message: impl Into<String>, field: &str) -> Self {
        Self {
            status,
            body: ErrorBody::new(code, message, Some(field.to_string())),
        }
    }

    pub fn unauthorized() -> Self {
        Self::new(
            StatusCode::UNAUTHORIZED,
            "unauthorized",
            "缺少或错误的访问令牌",
        )
    }

    pub fn not_found(what: &str) -> Self {
        Self::new(StatusCode::NOT_FOUND, "not_found", format!("{what}不存在"))
    }

    pub fn conflict(code: &str, message: impl Into<String>) -> Self {
        Self::new(StatusCode::CONFLICT, code, message)
    }

    pub fn bad_request(code: &str, message: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, code, message)
    }

    pub fn internal(message: impl Into<String>) -> Self {
        Self::new(StatusCode::INTERNAL_SERVER_ERROR, "internal_error", message)
    }
}

impl IntoResponse for ApiFailure {
    fn into_response(self) -> Response {
        (self.status, Json(self.body)).into_response()
    }
}

/// 内部错误 → HTTP。**过渡实现**：按消息关键词分档，后续改成类型化错误。
impl From<anyhow::Error> for ApiFailure {
    fn from(err: anyhow::Error) -> Self {
        let message = err.to_string();
        // 顺序要紧：「不存在」是 404；字段校验（空值/非法值）是 **400**；
        // 引用冲突（重名、被引用、端口占用）才是 409。
        // 之前把「不能为空」「必须」归到 409，等于把用户填错参数报成资源冲突。
        let status = if message.contains("不存在") {
            StatusCode::NOT_FOUND
        } else if message.contains("不能为空")
            || message.contains("必须")
            || message.contains("无效")
            || message.contains("格式")
        {
            StatusCode::BAD_REQUEST
        } else if message.contains("待确认")
            || message.contains("拒绝了该认证方式")
            || message.contains("未启用")
            || message.contains("已存在")
            || message.contains("已被")
            || message.contains("占用")
            || message.contains("无法")
        {
            StatusCode::CONFLICT
        } else {
            StatusCode::INTERNAL_SERVER_ERROR
        };
        ApiFailure {
            status,
            body: ErrorBody::new(code_for(status), message, None),
        }
    }
}

fn code_for(status: StatusCode) -> &'static str {
    match status {
        StatusCode::NOT_FOUND => "not_found",
        StatusCode::CONFLICT => "conflict",
        StatusCode::BAD_REQUEST => "bad_request",
        _ => "internal_error",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn not_found_message_maps_to_404() {
        let f: ApiFailure = anyhow::anyhow!("隧道不存在：tun_1").into();
        assert_eq!(f.status, StatusCode::NOT_FOUND);
        assert_eq!(f.body.error.code, "not_found");
    }

    #[test]
    fn conflict_message_maps_to_409() {
        let f: ApiFailure = anyhow::anyhow!("本地端口已被其它隧道占用：18080").into();
        assert_eq!(f.status, StatusCode::CONFLICT);
    }

    #[test]
    fn validation_message_maps_to_400_not_409() {
        // 用户填错字段是请求错误，不是资源冲突。
        for message in ["主机不能为空", "端口必须在 1..=65535", "凭据类型无效"] {
            let f: ApiFailure = anyhow::anyhow!(message).into();
            assert_eq!(
                f.status,
                StatusCode::BAD_REQUEST,
                "「{message}」应映射为 400"
            );
            assert_eq!(f.body.error.code, "bad_request");
        }
    }

    #[test]
    fn unknown_message_falls_back_to_500() {
        let f: ApiFailure = anyhow::anyhow!("磁盘忽然消失").into();
        assert_eq!(f.status, StatusCode::INTERNAL_SERVER_ERROR);
    }

    #[test]
    fn field_error_keeps_field_name() {
        let f = ApiFailure::field(
            StatusCode::BAD_REQUEST,
            "invalid_port",
            "端口越界",
            "local_port",
        );
        assert_eq!(f.body.error.field.as_deref(), Some("local_port"));
    }
}
