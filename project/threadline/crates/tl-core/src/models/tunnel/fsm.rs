//! 隧道状态机（纯函数，无 IO）。
//!
//! 归属：`tunnel` 模型内部文件组。拥有：状态迁移规则（允许/禁止的边）。
//! 提供：`TunnelEvent` 与 `next_state`。
//! 允许调用：仅 `tl_protocol::TunnelState`。
//!
//! 相位约束（终止保障）：状态单向推进，`Error` 只能由用户显式重试或 `Stop` 离开；
//! `NeedsTrust` 只能由「确认信任」或「拒绝/超时/停止」离开。**没有自环**，也不允许从
//! `Running` 直接跳回 `Stopped` 之外的状态——这条约束是模型环的终止保障声明。
//!
//! termination: phase

use tl_protocol::TunnelState;

/// 驱动状态机的事件（来自用户操作、注册层、SSH 引擎或计时器）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TunnelEvent {
    /// 用户/开机自启请求启动。
    Start,
    /// 本地端口绑定失败（端口被占用）。
    BindFailed { reason: String },
    /// SSH 握手完成且主机密钥通过校验。
    HandshakeOk,
    /// SSH 握手完成但主机密钥未知/待确认。
    HandshakeNeedsTrust,
    /// 传输层握手失败（网络不可达、协议错误等，可退避重试）。
    HandshakeFailed { reason: String },
    /// 认证失败（密钥被拒 / 密码错误 / 服务器不允许该方式）——不可自动重试。
    AuthFailed { reason: String },
    /// 这条连接还没有登录材料（导入的分享包）。
    ///
    /// 与认证失败分开：这里根本还没尝试认证，重试没有意义，
    /// 只会让界面挂着一串"正在重连"的假象。
    CredentialMissing { reason: String },
    /// 主机密钥与本机记录不一致——**绝不自动接受**，直接进错误态等人工处理。
    FingerprintMismatch { trusted: String, observed: String },
    /// 网络中断或 keepalive 超时。
    NetworkLost,
    /// 退避计时到点，可以重连。
    BackoffElapsed,
    /// 重连次数耗尽。
    RetryExhausted,
    /// 用户确认了待确认的主机密钥。
    TrustAccepted,
    /// 用户拒绝待确认的主机密钥。
    TrustRejected,
    /// 待确认超时。
    TrustTimeout,
    /// 用户/系统请求停止。
    Stop,
}

/// 非法迁移（在纯函数里返回，由模型层转成错误态或日志）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransitionError {
    pub from: TunnelState,
    pub event: TunnelEvent,
}

impl std::fmt::Display for TransitionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "非法状态迁移：{:?} + {:?}", self.from, self.event)
    }
}

impl std::error::Error for TransitionError {}

/// 计算下一个状态；非法迁移返回 `Err`（调用方必须拒绝该事件，不得静默忽略）。
pub fn next_state(from: TunnelState, event: &TunnelEvent) -> Result<TunnelState, TransitionError> {
    use TunnelEvent as E;
    use TunnelState as S;

    let next = match (from, event) {
        // 停止：任何状态都能收敛到 Stopped（持有资源的状态在模型层负责释放）。
        (_, E::Stop) => S::Stopped,

        // 从静止启动。
        (S::Stopped, E::Start) => S::Connecting,
        // 错误态只能由用户显式重试离开。
        (S::Error, E::Start) => S::Connecting,

        // 绑定与握手。
        (S::Connecting, E::BindFailed { .. }) => S::Error,
        (S::Connecting, E::HandshakeFailed { .. }) => S::Error,
        (S::Connecting, E::AuthFailed { .. }) => S::Error,
        (S::Connecting, E::CredentialMissing { .. }) => S::Error,
        (S::Connecting, E::FingerprintMismatch { .. }) => S::Error,
        (S::Connecting, E::HandshakeNeedsTrust) => S::NeedsTrust,
        (S::Connecting, E::HandshakeOk) => S::Running,

        // 信任确认：接受后重新握手（不假设能暂停半成品会话）。
        (S::NeedsTrust, E::TrustAccepted) => S::Connecting,
        (S::NeedsTrust, E::TrustRejected) => S::Stopped,
        (S::NeedsTrust, E::TrustTimeout) => S::Error,
        // 用户确认指纹后点「启动」：等价于重新握手。
        // 少了这条，信任完再点启动会报「非法状态迁移：NeedsTrust + Start」——
        // 也就是"点了启动没反应"。
        (S::NeedsTrust, E::Start) => S::Connecting,

        // 运行期断线。
        (S::Running, E::NetworkLost) => S::Backoff,

        // 退避收敛。
        (S::Backoff, E::BackoffElapsed) => S::Connecting,
        (S::Backoff, E::RetryExhausted) => S::Error,
        // 退避期间用户直接重试。
        (S::Backoff, E::Start) => S::Connecting,

        _ => {
            return Err(TransitionError {
                from,
                event: event.clone(),
            })
        }
    };

    Ok(next)
}

/// 该状态是否持有本地监听（供模型层决定是否释放端口）。
pub fn holds_listener(state: TunnelState) -> bool {
    matches!(
        state,
        TunnelState::Connecting
            | TunnelState::NeedsTrust
            | TunnelState::Running
            | TunnelState::Backoff
    )
}

/// 该状态下新到的本地连接应如何处理。
pub fn accepts_connections(state: TunnelState) -> bool {
    matches!(state, TunnelState::Running)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tl_protocol::TunnelState as S;

    fn ev_all() -> Vec<TunnelEvent> {
        use TunnelEvent as E;
        vec![
            E::Start,
            E::BindFailed { reason: "x".into() },
            E::HandshakeOk,
            E::HandshakeNeedsTrust,
            E::HandshakeFailed { reason: "x".into() },
            E::AuthFailed { reason: "x".into() },
            E::CredentialMissing { reason: "x".into() },
            E::NetworkLost,
            E::BackoffElapsed,
            E::RetryExhausted,
            E::TrustAccepted,
            E::TrustRejected,
            E::TrustTimeout,
            E::Stop,
        ]
    }

    #[test]
    fn happy_path_start_to_running() {
        assert_eq!(
            next_state(S::Stopped, &TunnelEvent::Start).unwrap(),
            S::Connecting
        );
        assert_eq!(
            next_state(S::Connecting, &TunnelEvent::HandshakeOk).unwrap(),
            S::Running
        );
    }

    #[test]
    fn stop_from_every_state_converges_to_stopped() {
        for state in [
            S::Stopped,
            S::Connecting,
            S::NeedsTrust,
            S::Running,
            S::Backoff,
            S::Error,
        ] {
            assert_eq!(
                next_state(state, &TunnelEvent::Stop).unwrap(),
                S::Stopped,
                "从 {state:?} 停止必须收敛"
            );
        }
    }

    #[test]
    fn trust_gate_reconnects_or_stops() {
        assert_eq!(
            next_state(S::NeedsTrust, &TunnelEvent::TrustAccepted).unwrap(),
            S::Connecting,
            "确认后必须重新握手"
        );
        assert_eq!(
            next_state(S::NeedsTrust, &TunnelEvent::TrustRejected).unwrap(),
            S::Stopped
        );
        assert_eq!(
            next_state(S::NeedsTrust, &TunnelEvent::TrustTimeout).unwrap(),
            S::Error
        );
    }

    #[test]
    fn start_after_trusting_the_key_is_legal() {
        // 用户流程：首连 → NeedsTrust → 点「信任并记录」→ 再点「启动」。
        // 最后这一步必须能重新握手，否则界面上就是"点了没反应"。
        assert_eq!(
            next_state(S::NeedsTrust, &TunnelEvent::Start).unwrap(),
            S::Connecting
        );
    }

    #[test]
    fn error_state_needs_explicit_retry() {
        for event in [
            TunnelEvent::BackoffElapsed,
            TunnelEvent::HandshakeOk,
            TunnelEvent::NetworkLost,
        ] {
            assert!(
                next_state(S::Error, &event).is_err(),
                "Error 态不得被 {event:?} 自动带离"
            );
        }
        assert_eq!(
            next_state(S::Error, &TunnelEvent::Start).unwrap(),
            S::Connecting
        );
    }

    #[test]
    fn backoff_is_bounded_and_can_exhaust() {
        assert_eq!(
            next_state(S::Running, &TunnelEvent::NetworkLost).unwrap(),
            S::Backoff
        );
        assert_eq!(
            next_state(S::Backoff, &TunnelEvent::BackoffElapsed).unwrap(),
            S::Connecting
        );
        assert_eq!(
            next_state(S::Backoff, &TunnelEvent::RetryExhausted).unwrap(),
            S::Error
        );
    }

    #[test]
    fn bind_failure_and_auth_failure_are_terminal_errors() {
        assert_eq!(
            next_state(
                S::Connecting,
                &TunnelEvent::BindFailed {
                    reason: "in use".into()
                }
            )
            .unwrap(),
            S::Error
        );
        assert_eq!(
            next_state(
                S::Connecting,
                &TunnelEvent::AuthFailed {
                    reason: "denied".into()
                }
            )
            .unwrap(),
            S::Error
        );
    }

    #[test]
    fn illegal_transitions_are_rejected_across_the_matrix() {
        let states = [
            S::Stopped,
            S::Connecting,
            S::NeedsTrust,
            S::Running,
            S::Backoff,
            S::Error,
        ];
        let mut legal = 0;
        let mut illegal = 0;
        for state in states {
            for event in ev_all() {
                match next_state(state, &event) {
                    Ok(_) => legal += 1,
                    Err(_) => illegal += 1,
                }
            }
        }
        assert!(legal > 0 && illegal > 0);
        // 关键不变量：Error 只能被 Start/Stop 带离。
        for event in ev_all() {
            if event == TunnelEvent::Start || event == TunnelEvent::Stop {
                continue;
            }
            assert!(next_state(S::Error, &event).is_err());
        }
    }

    #[test]
    fn listener_and_accept_semantics_follow_the_table() {
        assert!(holds_listener(S::Connecting));
        assert!(holds_listener(S::NeedsTrust));
        assert!(holds_listener(S::Running));
        assert!(holds_listener(S::Backoff));
        assert!(!holds_listener(S::Stopped));
        assert!(!holds_listener(S::Error));

        assert!(accepts_connections(S::Running));
        for state in [
            S::Stopped,
            S::Connecting,
            S::NeedsTrust,
            S::Backoff,
            S::Error,
        ] {
            assert!(!accepts_connections(state), "{state:?} 不得接受新连接");
        }
    }
}
