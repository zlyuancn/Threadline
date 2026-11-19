//! 门面层：只做类型映射与运行时持有。**禁止**在这里写任何判定规则。
//!
//! 桌面端走 HTTP 控制面（`crates/tl-core/src/interface/`）；安卓端没有第二个进程，
//! 由 UniFFI 直接调进 Rust，状态仍只由核心持有，UI 只是无状态的投影。

uniffi::setup_scaffolding!();

pub mod api;
pub mod forward;

#[uniffi::export]
pub fn build_info() -> String {
    // 只 `use` 不引用会被 LTO 剪掉，踩不到 `ring` 的 C 链接问题；这里强制真正链接一次。
    let _ = std::hint::black_box(russh::client::Config::default());
    let _ = std::hint::black_box(
        tokio::runtime::Builder::new_current_thread()
            .build()
            .is_ok(),
    );
    format!(
        "tl-mobile {} ({})",
        env!("CARGO_PKG_VERSION"),
        std::env::consts::ARCH
    )
}
