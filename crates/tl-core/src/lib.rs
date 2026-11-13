//! Threadline 核心库。
//!
//! 归属：**装配层**——把三层（interface / business / models）与底座（foundation）暴露给壳层与测试。
//! 这里只做 `pub mod` 声明，不含任何逻辑（依赖方向：壳 → 接口 → 业务 → 模型 → 底座）。
//!
//! `main.rs` 是唯一的壳：解析参数、初始化日志、组装并运行。

pub mod business;
pub mod foundation;
pub mod interface;
pub mod models;
