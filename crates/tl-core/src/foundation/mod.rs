//! 底座：无业务语义的基础设施机制。
//!
//! 归属：**底座（foundation）**。拥有：文件原子读写、随机凭据生成、退避策略、并发预算、单实例文件锁。
//! 提供：`store` / `token` / `retry` / `limiter` / `lock` 等纯机制模块。
//! 允许调用：仅标准库与纯机制 crate（serde / getrandom / fs2）。
//!
//! 纪律：底座**不许 import 任何上层**（models / business / interface / shell）；
//! 读它的代码应当听不到任何业务名词。终止保障类机制（退避/预算/超时）统一在这里实现，
//! 模型层禁止自建私有重试轮子（MSO 反模式）。

pub mod atomic;
pub mod clock;
pub mod hosts;
pub mod limiter;
pub mod lock;
pub mod registry;
pub mod retry;
pub mod secret;
pub mod store;
pub mod timeout;
pub mod tls_ca;
pub mod token;
