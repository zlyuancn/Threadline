# 第三方组件声明

Threadline 自身以 [PolyForm Noncommercial 1.0.0](LICENSE) 发布。发布物（安装包与二进制）内含下列第三方组件，其许可条款同时适用。

## 需要随二进制保留声明的组件

| 组件 | 许可 | 形态 | 说明 |
|---|---|---|---|
| `option-ext` | MPL-2.0 | 编译进 `Threadline.exe` | 未修改源文件；许可见 [`licenses/mpl-2.0.txt`](licenses/mpl-2.0.txt)，源码 <https://crates.io/crates/option-ext> |
| `cssparser` | MPL-2.0 | 编译进 `Threadline.exe` | 未修改源文件；许可见 [`licenses/mpl-2.0-cssparser.txt`](licenses/mpl-2.0-cssparser.txt)，源码 <https://crates.io/crates/cssparser> |
| `selectors` | MPL-2.0 | 编译进 `Threadline.exe` | 未修改源文件；同 cssparser（同一份 MPL-2.0 正文），源码 <https://crates.io/crates/selectors> |
| `icu_collections`、`icu_locale_core`、`icu_normalizer`、`icu_properties`、`icu_provider` | Unicode-3.0 | 编译进 `Threadline.exe`（经 `url`/`idna`） | 许可见 [`licenses/unicode-3.0.txt`](licenses/unicode-3.0.txt) |
| `unicode-ident` | Unicode-DFS-2016 | 编译进 `Threadline.exe` 与 `tl-core.exe`（两个 Cargo.lock 都有） | 许可见 [`licenses/unicode-dfs-2016.txt`](licenses/unicode-dfs-2016.txt) |
| `ring` | ISC 与 OpenSSL 混合条款 | 编译进 `tl-core.exe` | 三份正文：本工程主体 [`licenses/ring.txt`](licenses/ring.txt)、其余条款 [`licenses/ring-other-bits.txt`](licenses/ring-other-bits.txt)、BoringSSL 部分 [`licenses/ring-boringssl.txt`](licenses/ring-boringssl.txt) |

> `licenses/` 下的副本直接用各 crate 发布包内的原始许可文件，未经改写；它们同时被打进安装包。

## 随安装包分发的运行时

| 组件 | 许可 | 说明 |
|---|---|---|
| Microsoft Edge WebView2 Runtime | Microsoft 软件许可条款 | 由安装器在目标机安装或下载；条款见 <https://developer.microsoft.com/microsoft-edge/webview2/> |
| NSIS（Nullsoft Scriptable Install System） | zlib/libpng 许可 | Tauri 打包器使用的安装器生成器；许可见 <https://nsis.sourceforge.io/License_page> |

## 直接依赖的许可证（宽松型）

Rust 侧：`anyhow`、`axum`、`tokio`、`serde`、`serde_json`、`sha2`、`sha1`、`base64`、`tower-http`、`tracing`、`tracing-subscriber`、`fs2`、`getrandom`、`time`（MIT 或 MIT OR Apache-2.0）、`rustls` / `tokio-rustls` / `rustls-pemfile`（Apache-2.0 OR ISC OR MIT）、`russh`（Apache-2.0）、`rcgen`（MIT OR Apache-2.0）、`windows-sys`（MIT OR Apache-2.0）、`tauri` / `tauri-plugin-single-instance`（MIT OR Apache-2.0）。

前端侧：`vue`、`pinia`、`element-plus`、`vite`、`vitest`、`vue-tsc`、`@vitejs/plugin-vue`、`@types/node`（MIT）、`typescript`（Apache-2.0）、`@tauri-apps/api` / `@tauri-apps/cli`（MIT OR Apache-2.0）。

完整依赖树与版本见 `Cargo.lock`、`src-tauri/Cargo.lock`、`pnpm-lock.yaml`。

## 平台说明

本项目只发布 Windows 构建。`src-tauri/Cargo.lock` 中出现的 `gtk` / `glib` / `webkit2gtk` / `soup3` 等是 Tauri 的 **Linux 条件依赖**，Windows 构建不编译、不分发；若要发布 Linux 版，需另行评估其 LGPL-2.1+ 义务。
