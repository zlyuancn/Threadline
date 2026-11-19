//! 门面的运行部分：持有一条 tokio 运行时 + 一条 `-L` 转发的生命周期。
//!
//! **零业务规则**：能不能连、指纹要不要认、退避几次放弃，全部由 `tl-core` 的 models 判定；
//! 这里只做「调进去、把结果搬出来」。

use std::sync::{Arc, Mutex, OnceLock};

use tl_core::models::tunnel::engine::{self, EngineOutcome, Login, ServeConfig, Upstream};
use tl_core::models::tunnel::traffic::Traffic;
use tl_protocol::TunnelState;
use tokio::sync::watch;

use crate::api::{ForwardRequest, MobileError, TrafficCount};

/// 整个进程一个运行时（安卓端是单进程，没有第二个消费者）。
static RUNTIME: OnceLock<tokio::runtime::Runtime> = OnceLock::new();

fn runtime() -> &'static tokio::runtime::Runtime {
    RUNTIME.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .thread_name("tl-mobile")
            .build()
            .expect("tokio runtime")
    })
}

/// 状态词与桌面同一份：直接在 `TunnelState` 上取 serde 名字，不手写字面量。
fn state_word(state: TunnelState) -> String {
    serde_json::to_value(state)
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_else(|| "error".to_string())
}

/// 一条在跑的转发。Kotlin 侧持有它，用 `state()` 轮询、用 `stop()` 收工。
#[derive(uniffi::Object)]
pub struct Forward {
    listen_addr: String,
    // 状态与错误由后台任务写入、Kotlin 侧读取，所以是共享句柄而不是裸 `Mutex`。
    state: Arc<Mutex<String>>,
    last_error: Arc<Mutex<Option<String>>>,
    stop: Mutex<Option<watch::Sender<bool>>>,
    traffic: Arc<Traffic>,
}

#[uniffi::export]
impl Forward {
    /// 本地入口地址（`127.0.0.x:port`）——WebView 直接拿它当 URL 的 host:port。
    pub fn listen_addr(&self) -> String {
        self.listen_addr.clone()
    }

    /// `stopped` / `connecting` / `running` / `backoff` / `error`（与桌面同词）。
    pub fn state(&self) -> String {
        self.state.lock().expect("state lock").clone()
    }

    /// 最近一次可读的失败原因；`None` 表示没出过错。
    pub fn last_error(&self) -> Option<String> {
        self.last_error.lock().expect("last_error lock").clone()
    }

    pub fn traffic(&self) -> TrafficCount {
        let (up, down) = self.traffic.snapshot();
        TrafficCount { up, down }
    }

    /// 停止：释放监听、断开会话。重复调用无副作用。
    pub fn stop(&self) {
        if let Some(tx) = self.stop.lock().expect("stop lock").as_ref() {
            let _ = tx.send(true);
        }
        *self.state.lock().expect("state lock") = state_word(TunnelState::Stopped);
    }
}

/// 起一条 `-L`：`listen_host:listen_port` → 经 SSH → `remote_host:remote_port`。
///
/// 首次调用应传 `trusted_fingerprint = None`：会拿到 `NeedsTrust` 与观测到的指纹，
/// 用户核对后再用该指纹调一次。**指纹不一致时没有"仍然继续"这条路。**
#[uniffi::export]
pub fn start_forward(req: ForwardRequest) -> Result<Arc<Forward>, MobileError> {
    let keepalive_secs = if req.keepalive_secs == 0 {
        30
    } else {
        u64::from(req.keepalive_secs)
    };
    let listen_addr = format!("{}:{}", req.listen_host, req.listen_port);
    let config = ServeConfig {
        server_host: req.server_host.clone(),
        server_port: req.server_port,
        remote_host: req.remote_host.clone(),
        remote_port: req.remote_port,
        trusted_fingerprint: req.trusted_fingerprint.clone(),
        keepalive_secs,
    };
    let login = Login::Password {
        user: req.username.clone(),
        password: req.password.clone(),
    };

    let (stop_tx, stop_rx) = watch::channel(false);
    let traffic = Arc::new(Traffic::default());

    // 绑定与会话都必须在同一个运行时上完成，Kotlin 调用线程不参与。
    let (listener, session) = runtime().block_on(async {
        let listener = engine::bind_local(&req.listen_host, req.listen_port)
            .await
            .map_err(|e| MobileError::BindFailed {
                reason: format!("监听 {listen_addr} 失败：{e}"),
            })?;
        let session = engine::open_session(&config, login, stop_rx)
            .await
            .map_err(map_outcome)?;
        Ok::<_, MobileError>((listener, session))
    })?;

    let state = Arc::new(Mutex::new(state_word(TunnelState::Running)));
    let last_error: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));

    let task_state = Arc::clone(&state);
    let task_traffic = Arc::clone(&traffic);
    let task_stop_rx = stop_tx.subscribe();
    let remote_host = req.remote_host.clone();
    let remote_port = req.remote_port;
    let on_error = {
        let last_error = Arc::clone(&last_error);
        Box::new(move |message: String| {
            *last_error.lock().expect("last_error lock") = Some(message);
        })
    };

    runtime().spawn(async move {
        // 入口 TLS / 域名在安卓端首发不做（见设计「HTTPS 三格」），两个 `None` 是这一决定的出口。
        let (outcome, _listener) = engine::serve_listener(
            listener,
            Upstream::Ssh(session),
            remote_host,
            remote_port,
            keepalive_secs,
            task_stop_rx,
            on_error,
            task_traffic,
            None,
            None,
        )
        .await;
        let next = match outcome {
            EngineOutcome::Stopped => TunnelState::Stopped,
            _ => TunnelState::Error,
        };
        *task_state.lock().expect("state lock") = state_word(next);
    });

    Ok(Arc::new(Forward {
        listen_addr,
        state,
        last_error,
        stop: Mutex::new(Some(stop_tx)),
        traffic,
    }))
}

/// 引擎结局 → 门面错误。只是搬运，不做判定。
fn map_outcome(outcome: EngineOutcome) -> MobileError {
    match outcome {
        EngineOutcome::NeedsTrust { fingerprint } => MobileError::NeedsTrust { fingerprint },
        EngineOutcome::FingerprintMismatch { trusted, observed } => {
            MobileError::FingerprintMismatch { trusted, observed }
        }
        other => MobileError::Failed {
            reason: engine::describe(&other),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_words_match_desktop() {
        // Kotlin 侧按这些词分支，改 serde 规则时这条会先红。
        assert_eq!(state_word(TunnelState::Stopped), "stopped");
        assert_eq!(state_word(TunnelState::Connecting), "connecting");
        assert_eq!(state_word(TunnelState::NeedsTrust), "needs_trust");
        assert_eq!(state_word(TunnelState::Running), "running");
        assert_eq!(state_word(TunnelState::Backoff), "backoff");
        assert_eq!(state_word(TunnelState::Error), "error");
    }
}
