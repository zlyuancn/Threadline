//! 模型层：数据与规则的所有者。
//!
//! 归属：**模型层**。拥有：凭据、服务器、隧道、运行态四份数据与各自的判定规则。
//! 提供：每个模型一个 `port`（对外方法）；模型之间只走对方的公开操作，禁止翻内部实现。
//! 允许调用：`foundation`（底座）与**其它模型的公开操作**。
//!
//! 纪律：数据谁拥有谁写（`servers.json` 归 `server`、`tunnels.json` 归 `tunnel`、
//! `credentials.dat` 归 `credential`、`control.json` 归 `runtime`）；业务层只编排、不藏规则。

pub mod credential;
pub mod runtime;
pub mod server;
pub mod tunnel;

/// 删除判定结果：**值类型，无规则**（规则在各模型内部决定何时返回 Blocked）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RemoveDecision {
    Allowed,
    Blocked { referenced_by: Vec<String> },
}
