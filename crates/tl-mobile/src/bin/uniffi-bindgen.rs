//! 绑定生成器入口：`cargo run -p tl-mobile --bin uniffi-bindgen -- generate --library ...`
//!
//! 与库本身用同一份 `uniffi` 版本，避免生成器/运行时版本漂移。

fn main() {
    uniffi::uniffi_bindgen_main()
}
