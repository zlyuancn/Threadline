# 安卓端 实施计划

> **给执行者**：REQUIRED SUB-SKILL — `superpowers:executing-plans`，按任务顺序逐条执行。
> 设计与交互已定稿：`docs/plans/2026-11-19-android-client-design.md`；本计划只回答"先写哪个文件、写到什么程度"。

**目标**：Android 上跑一条到家里 sshd 的 `-L` 转发，用内置 WebView 打开家里面板，本地端口同时供第三方 App 使用。

**架构**：Kotlin + Compose 做壳与 UI，Rust 共享核心（`tl-protocol` + `tl-core` 的 models / business / engine）编成 `.so`，
经 UniFFI 暴露给 Kotlin；不跑 HTTP 控制面，状态由核心独占，UI 只是投影。**同仓**：`android/` + `crates/tl-mobile`。

**技术栈**：Rust 1.99 / russh 0.64（`ring` 后端）/ tokio / UniFFI；Kotlin 2.x + Compose + AGP 8.x；`cargo-ndk` + NDK r27 LTS。
**版本策略**：AGP / Kotlin / UniFFI 落地时取各自最新稳定版并**钉死**在版本目录里；本文写的版本号只作参照。

---

## 前置约束

1. **只在本地 commit，不 push**（沿用 `docs/TODO.md` 决策：私有历史不推）。
2. **门禁**：每阶段末 `powershell -File scripts/verify.ps1` 全绿；A5 起它还包含 NDK 交叉编译与 Gradle 单测两关。
3. **分层**：`tl-mobile` 是纯门面，零业务规则；规则一律留在 `models`。msocheck R1–R7 拦截。
4. **安卓构建不编这些模块**：`interface/`（HTTP 适配）、`foundation/hosts.rs`、`foundation/registry.rs`、
   `business/hosts_sync.rs`、`business/gateway.rs`。用 `#[cfg(not(target_os = "android"))]` 表达，不写跨平台桩函数。
5. **B 档语义**：转发监听 `127.0.0.x` 对整机所有 App 可见，这是设计选择，不要"顺手收紧"成 App 私有。
6. **提交信息**：conventional commits，英文，例：`feat(android): run a -L forward from the app`。

---

## 阶段 E：环境准备（一次性，约 1 小时，含 3–4 GB 下载）

### Task E1：装 Android 工具链

**Files:** 无（只改机器环境）

**Step 1：确认当前缺失项**

```powershell
powershell -NoProfile -Command "& \"$env:USERPROFILE\.cargo\bin\rustup.exe\" target list --installed; java -version"
```

Expected: 只有 `x86_64-pc-windows-msvc`；`java` 找不到（本机现状即如此）。

**Step 2：装 JDK 17 与 Android SDK/NDK**

任选其一：

```powershell
# 路线 A：winget 装 JDK，再命令行装 SDK
winget install --id Microsoft.OpenJDK.17 -e
winget install --id Google.AndroidStudio -e     # 自带 SDK Manager
# 装完在 Android Studio 的 SDK Manager 里勾 NDK (Side by side) → r27 LTS
```

```powershell
# 路线 B：已有 Android Studio，只补 NDK
& "$env:LOCALAPPDATA\Android\Sdk\cmdline-tools\latest\bin\sdkmanager.bat" --list_installed
& "$env:LOCALAPPDATA\Android\Sdk\cmdline-tools\latest\bin\sdkmanager.bat" "ndk;27.2.12479018" "platforms;android-35" "build-tools;35.0.0"
```

**Step 3：写环境变量并验证**

```powershell
[Environment]::SetEnvironmentVariable('ANDROID_HOME', "$env:LOCALAPPDATA\Android\Sdk", 'User')
[Environment]::SetEnvironmentVariable('ANDROID_NDK_HOME', "$env:LOCALAPPDATA\Android\Sdk\ndk\27.2.12479018", 'User')
# 新开一个终端后
java -version   # Expected: openjdk version "17.x"
```

> 注意：`verify.ps1` 里刻意没依赖 PATH 里的 cargo（自己拼 `~/.cargo/bin`），新加的安卓关卡也照这个规矩写，
> 不要假设 `cargo` / `cargo-ndk` 在 PATH 上。

### Task E2：装 Rust 交叉目标与 cargo-ndk

**Files:** 无

**Step 1：装 target 与工具**

```powershell
& "$env:USERPROFILE\.cargo\bin\rustup.exe" target add aarch64-linux-android armv7-linux-androideabi
& "$env:USERPROFILE\.cargo\bin\cargo.exe" install cargo-ndk
```

**Step 2：验证**

```powershell
& "$env:USERPROFILE\.cargo\bin\cargo-ndk.exe" --version
```

Expected: 打印版本号（≥ 3.x）。

**Step 3：Commit** — 无代码改动，跳过。

---

## 阶段 A0：去风险 spike（3 天，任一任务失败即翻案）

> 本阶段的产物允许粗糙（无 Compose、无前台服务），唯一目的是把三个未知项钉死。
> **三项全绿才进入 A1；任一失败，先回写设计文档「风险」节，再决定退 `aws-lc-rs` 还是退 sshj。**

### Task A0.1：把 `russh + ring + tokio-rustls` 编成 Android `.so`

**Files:**
- Create: `crates/tl-mobile/Cargo.toml`
- Create: `crates/tl-mobile/src/lib.rs`
- Modify: `Cargo.toml`（workspace members 加 `crates/tl-mobile`）

**Step 1：写最小 crates**

```toml
# crates/tl-mobile/Cargo.toml
[package]
name = "tl-mobile"
version.workspace = true
edition.workspace = true
publish = false
description = "安卓端门面：把核心能力经 UniFFI 暴露给 Kotlin（零业务规则）"

[lib]
name = "tl_mobile"
crate-type = ["cdylib", "staticlib"]

[dependencies]
russh.workspace = true
tokio = { workspace = true }
tracing.workspace = true
uniffi = { version = "0.32", features = ["cli"] }

[build-dependencies]
uniffi = { version = "0.32", features = ["build"] }
```

```rust
// crates/tl-mobile/src/lib.rs
//! 门面层：只做类型映射与运行时持有。**禁止**在这里写任何判定规则。
uniffi::setup_scaffolding!();

#[uniffi::export]
pub fn build_info() -> String {
    format!("tl-mobile {} ({})", env!("CARGO_PKG_VERSION"), std::env::consts::ARCH)
}
```

**Step 2：交叉编译**

```powershell
cd d:\my\zlyuancn\project\threadline
& "$env:USERPROFILE\.cargo\bin\cargo-ndk.exe" -t arm64-v8a -o tmp\data\jniLibs build -p tl-mobile --release
```

Expected: `tmp/data/jniLibs/arm64-v8a/libtl_mobile.so` 存在，且 `> 2 MB`（含 russh/ring/tokio）。

**失败退路（按顺序试）**：
1. `ring` 的 C 部分编不过 → 换 `aws-lc-rs`（需 CMake + NASM，NDK 里可用）后重跑。
2. `aws-lc-rs` 也不行 → 记录到设计文档「风险」，翻案走 `sshj`（阶段 A0 作废，A1 改为纯 Kotlin 数据面）。

**Step 3：Commit**

```powershell
git add Cargo.toml crates/tl-mobile
git commit -m "feat(android): cross-compile a minimal rust facade for android"
```

### Task A0.2：经 UniFFI 起一条真转发

**Files:**
- Create: `android/settings.gradle.kts`、`android/build.gradle.kts`、`android/app/build.gradle.kts`、`android/gradle.properties`
- Create: `android/app/src/main/AndroidManifest.xml`
- Create: `android/app/src/main/java/cn/zly/threadline/SpikeActivity.kt`
- Create: `crates/tl-mobile/src/api.rs`、`crates/tl-mobile/src/forward.rs`

**Step 1：导出最小 API（同步、阻塞在专用线程）**

```rust
// crates/tl-mobile/src/api.rs
#[derive(uniffi::Record)]
pub struct ForwardRequest {
    pub host: String,
    pub port: u16,
    pub username: String,
    pub secret: String,
    pub listen_host: String,
    pub listen_port: u16,
    pub remote_host: String,
    pub remote_port: u16,
}
```

`forward.rs` 里起一个 `tokio::runtime::Runtime`（`new_multi_thread().worker_threads(2)`）：
调用现成的 `models::tunnel::engine` 建会话、`serve_listener` 监听 `listen_host:listen_port`，
把 `TunnelState` 通过 `uniffi::export(callback_interface)` 或轮询接口 `forward_state(id)` 抛回 Kotlin。
**先不接 `Services`**——spike 只验证数据面，配置存储留到 A1。

**Step 2：生成 Kotlin 绑定**

```powershell
cargo ndk -t arm64-v8a -o android/app/src/main/jniLibs build -p tl-mobile --release
cargo run -p tl-mobile --bin uniffi-bindgen -- generate --library `
  target/aarch64-linux-android/release/libtl_mobile.so --language kotlin --out-dir android/app/src/main/java
```

Expected: 生成 `android/app/src/main/java/uniffi/tl_mobile/tl_mobile.kt`。
若 `:app` 编译报 `UnsatisfiedLinkError: libjnidispatch.so`，依赖必须改成 `implementation("net.java.dev.jna:jna:5.x@aar")`。

**Step 3：一个按钮打通**

`SpikeActivity` 一个按钮 → 调 `startForward(ForwardRequest(公网主机, 端口, 用户, 密码, "127.0.0.1", 8123, "192.168.x.x", 8123))`；
`adb logcat -s Threadline` 打印状态。

**Step 4：真机验证**

```powershell
adb install -r android\app\build\outputs\apk\debug\app-debug.apk
```

1. 点按钮 → logcat 出现 `running`；
2. 手机浏览器打开 `http://127.0.0.1:8123` → 看到家里的页面；
3. 拔掉 WiFi 30 秒再开 → 状态 `backoff` → `running`（验证 keeper 复用可行）。

> 凭据只在真机上手输一次，**不得写进仓库、不得写进 `strings.xml`**。

**Step 5：Commit** — `feat(android): open a -L forward through uniffi`

### Task A0.3：WebView 打回环别名

**Files:**
- Create: `android/app/src/main/java/cn/zly/threadline/WebSpikeActivity.kt`
- Create: `android/app/src/main/res/xml/network_security_config.xml`
- Modify: `android/app/src/main/AndroidManifest.xml`

**Step 1：明文白名单只放回环**

```xml
<network-security-config>
    <domain-config cleartextTrafficPermitted="true">
        <domain includeSubdomains="false">127.0.0.1</domain>
        <domain includeSubdomains="false">127.0.0.5</domain>
    </domain-config>
</network-security-config>
```

**Step 2：两条转发分别监听 `.1` 与 `.5`，各开一个 WebView 登录同一面板的不同账号**

Expected: 两个 WebView 登录态互不影响（cookie 按 host 隔离）；`WebSocket` 面板（如 HA）实时数据正常刷新。

**失败判据**：若 `127.0.0.5` 绑不上（个别 ROM），退路是同一地址不同端口 + 用 `CookieManager` 按实例隔离不可行 → 记录为「首发限制」，退回 B 档的 `127.0.0.1` 单地址。

**Step 3：Commit** — `feat(android): back a webview with a loopback alias`

---

## 阶段 A1：骨架（核心进 APK）

### Task A1.1：`tl-core` 平台分流

**Files:**
- Modify: `crates/tl-core/Cargo.toml`（`uniffi` 不进这里；仅确认 `windows-sys` 已是 target 依赖）
- Modify: `crates/tl-core/src/lib.rs`、`foundation/mod.rs`、`business/mod.rs`
- Modify: `crates/tl-core/src/foundation/secret.rs`、`atomic.rs`

**Step 1：按清单加 `cfg`**

```rust
// lib.rs
#[cfg(not(target_os = "android"))]
pub mod interface;
pub mod business;
pub mod foundation;
pub mod models;

// foundation/mod.rs
#[cfg(not(target_os = "android"))]
pub mod hosts;
#[cfg(not(target_os = "android"))]
pub mod registry;

// business/mod.rs
#[cfg(not(target_os = "android"))]
pub mod gateway;
#[cfg(not(target_os = "android"))]
pub mod hosts_sync;
```

`Services` 里对 `hosts` / `gateway` 的字段同样 `cfg` 掉；`sync_hosts_quietly()`、`sync_hosts()`、`startup_check()` 在安卓构建里变成空实现**或整段 cfg 掉**（优先整段掉，不留空函数）。

**Step 2：编译通过是唯一验收**

```powershell
cargo check -p tl-core --target aarch64-linux-android
```

Expected: 0 error。首轮失败通常落在 `runtime` 与 `secret`，逐个按「能力 → 平台实现」表处理。

**Step 3：Commit** — `refactor(core): gate windows-only modules behind cfg`

### Task A1.2：`tl-mobile` 变成真门面

**Files:**
- Modify: `crates/tl-mobile/src/lib.rs`、`api.rs`、`forward.rs`
- Test: `crates/tl-mobile/src/forward.rs`（`#[cfg(test)]`，跑在宿主 Windows 上）

**Step 1：持有 `Services` 的全局实例**

```rust
static CORE: OnceLock<Arc<CoreHandle>> = OnceLock::new();
```

`CoreHandle` 内部：`tokio::runtime::Runtime` + `Arc<Services>` + `data_dir`。导出：
`init(data_dir: String)`、`list_servers() -> Vec<ServerDto>`、`list_tunnels() -> Vec<TunnelView>`、
`start_tunnel(id)`、`stop_tunnel(id)`、`create_server(req)`、`patch_tunnel(id, req)`、`subscribe(cb)`。

**Step 2：测试只测映射，不测规则**

```rust
#[test]
fn forward_state_maps_to_the_same_strings_as_desktop() {
    // 保证 Kotlin 侧拿到的状态词与桌面一致：stopped/connecting/running/backoff/error
}
```

**Step 3：Commit** — `feat(android): expose the core services through the facade`

### Task A1.3：凭据与原子存储的平台实现

**Files:**
- Modify: `crates/tl-core/src/foundation/secret.rs`
- Modify: `crates/tl-core/src/foundation/atomic.rs`

**Step 1：`secret.rs` 三分支**

| 平台 | 实现 |
|---|---|
| Windows | 现有 DPAPI（不动） |
| Android | `uniffi` 回调进 Kotlin：`Keystore` 里生成 AES-256-GCM 主密钥，Rust 只持密文与 nonce，加解密那一刻回调 Kotlin |
| 其它 | 编译期报错（`compile_error!`），不留明文陷阱 |

> 为什么回调而不是自己调 JNI：Keystore 的 `Cipher` 只要在 Kotlin 里跑，Rust 侧就少一套 JNI 样板；密钥永不离开 Keystore。

**Step 2：`atomic.rs` 加 unix 分支**

```rust
#[cfg(not(windows))]
fn replace(src: &Path, dst: &Path) -> std::io::Result<()> {
    std::fs::rename(src, dst) // 同目录 rename 已是原子替换
}
```

**Step 3：单测**

`#[cfg(test)]` 里断言「写一半失败不破坏原文件」（用只读目录制造失败）。

**Step 4：Commit** — `feat(core): keystore-backed secrets and rename-based atomic writes`

### Task A1.4：Compose 工程与前台服务空壳

**Files:**
- Create: `android/app/src/main/java/cn/zly/threadline/TlApp.kt`
- Create: `android/app/src/main/java/cn/zly/threadline/CoreService.kt`
- Create: `android/app/src/main/java/cn/zly/threadline/ui/AppScaffold.kt`

**Step 1**：`CoreService`（`startForeground` + 常驻通知「Threadline · N 条转发运行中」）里 `init(data_dir)`；
UI 用 `StateFlow` 投影核心状态。**UI 不读文件、不建 SSH 会话。**

**Step 2**：`adb shell am force-stop` 后从通知栏回来，状态与配置一致。

**Step 3：Commit** — `feat(android): foreground service owns the core`

---

## 阶段 A2：转发管理

### Task A2.1：列表与只读投影

**Files:** `android/app/src/main/java/cn/zly/threadline/ui/servers/*`、`ui/forwards/*`
**Step 1**：服务器列表（含指纹状态、`needs_credential`）、转发列表（状态徽标、入口地址、`last_error`）。
**Step 2**：与桌面同一份 `TunnelView` 字段，**不自造文案**（状态词由核心给）。
**Step 3：Commit** — `feat(android): list servers and forwards`

### Task A2.2：新建 / 编辑 + 主机密钥确认

**Files:** `ui/servers/EditServerScreen.kt`、`ui/servers/HostKeySheet.kt`
**Step 1**：表单字段与桌面一致（主机 / 端口 / 用户名 / 密码或私钥 / keepalive）。
**Step 2**：首次连接弹指纹，**必须与 `ssh` 命令行显示的一致才允许信任**；指纹不匹配时不提供"仍然继续"。
**Step 3**：测试连接复用 `probe_draft`。
**Step 4：Commit** — `feat(android): create servers with host key confirmation`

### Task A2.3：启停与状态订阅

**Step 1**：`start_tunnel` / `stop_tunnel` / `set_server_enabled` 全走门面。
**Step 2**：订阅回调 → `Flow` → Compose 重组；断线由核心 `keeper` 自动重退避，UI 只显示状态。
**Step 3**：真机验证：飞行模式 30 秒 → `backoff` → `running`，恢复的是掉线前在跑的那批。
**Step 4：Commit** — `feat(android): toggle forwards and follow core state`

---

## 阶段 A3：内置浏览器

### Task A3.1：WebView 宿主

**Files:** `ui/browser/BrowserScreen.kt`、`ui/browser/BrowserHost.kt`
**Step 1**：一条转发一个 WebView 实例（切走时 `pauseTimers`，别在后台渲染）。
**Step 2**：禁用 `shouldInterceptRequest`；开启 `javaScriptEnabled`、`domStorageEnabled`、`mediaPlaybackRequiresUserGesture=false`（noVNC 需要）。
**Step 3**：返回键先 `goBack()`，到底再退出页面。
**Step 4：Commit** — `feat(android): open forwards in a built-in browser`

### Task A3.2：回环别名开关

**Step 1**：新建转发时「独立浏览器会话」开关 → `local_host = 127.0.0.x`（复用 `domain_host_for` 的池子；池空时提示，不静默回落到 `.1`）。
**Step 2**：删除转发时地址回收（与桌面同一套规则）。
**Step 3：Commit** — `feat(android): give each forward its own loopback address`

---

## 阶段 A4：生存能力

### Task A4.1：通知与进程恢复
**Step 1**：`START_STICKY` + 通知里的「停止全部」动作。
**Step 2**：`adb shell am kill` 后 10 秒内服务重建、已启用转发自动恢复。

### Task A4.2：Doze 与厂商后台
**Step 1**：检测「电池优化」未豁免时给一条可点提示，跳系统设置（**不静默申请**）。
**Step 2**：连续断线 3 次时在列表顶部给「可能是后台限制」的说明，带 `last_error`。

### Task A4.3：日志
**Step 1**：`filesDir/logs/mobile-YYYYMMDD.log`，设置页「导出」走 `FileProvider`（不发网络）。
**Step 2：Commit** — `feat(android): survive backgrounding and export logs`

---

## 阶段 A5：门禁与验收

### Task A5.1：msocheck 覆盖 `tl-mobile`

**Files:** `tools/msocheck/msocheck.mjs`
**Step 1**：`SRC_DIRS` 增加 `join(ROOT, 'crates', 'tl-mobile', 'src')`。
**Step 2**：新增 R7（门面不得藏规则）：`tl-mobile` 源码里禁止出现 `crate::models::*::internal`、禁止 `if` 里出现
`state ==` 之类的状态判定（用正则限定：`internal`、`bail!` 之外的 `match ... state` 形态）。
**Step 3**：`node tools/msocheck/msocheck.mjs --selftest` 与全量检查均绿。
**Step 4：Commit** — `chore(msocheck): cover the mobile facade with r7`

### Task A5.2：`verify.ps1` 增两关

```powershell
Step 'cargo ndk (arm64)' { & $cargoNdk -t arm64-v8a -o tmp\data\jniLibs build -p tl-mobile --release }
Push-Location (Join-Path $root 'android')
try { Step 'gradle unit tests' { cmd /c 'gradlew.bat :app:testDebugUnitTest' } } finally { Pop-Location }
```

Expected: 全绿；`verify.ps1` 仍是 ASCII-only（Windows PowerShell 5.1 读 ANSI）。

### Task A5.3：文档收口

- `docs/TESTPLAN.md`：按设计文档「验收」7 条逐条落成可判定项。
- `AGENTS.md`：两进程说明补一句「安卓端不跑 HTTP 控制面，核心以库形式进程内运行（CAW 偏离，见设计文档）」。
- `docs/TODO.md`：A0–A5 完成项整条搬进 `DONE.md`。
- **Commit** — `docs(android): record the client's acceptance and caw deviation`

---

## 验收（与设计文档一致，不另立标准）

1. 经公网端口连上家里 sshd，`127.0.0.1:8123` 转发可用（浏览器可开）。
2. 断网 30 秒恢复后自动回到 `running`，恢复的是掉线前在跑的那批。
3. 首次连接核对指纹；指纹变更拒绝，提示口径与桌面一致。
4. 两条面板分别用 `127.0.0.2` / `127.0.0.3` 打开，登录态互不影响。
5. 第三方 App 与系统 Chrome 能连 `127.0.0.x:port`（B 档语义）。
6. 息屏 30 分钟隧道仍在；杀进程后从通知回来状态一致。
7. `scripts/verify.ps1` 全绿（含 R1–R7、NDK 交叉编译、Gradle 单测）。

## 风险与退路

| 风险 | 触发点 | 退路 |
|---|---|---|
| `ring` 编不过 NDK | A0.1 | 换 `aws-lc-rs`；再不行翻案 sshj（A1 改纯 Kotlin） |
| UniFFI/JNA 落地问题 | A0.2 | 用 `jna@aar`；绑定生成改 `--library` 模式；最差退手写 JNI |
| `127.0.0.5` 绑不上 | A0.3 | 记为首发限制，退回单一 `127.0.0.1`（cookie 隔离改为"面板各自退出登录"人工规避） |
| 厂商 ROM 杀后台 | A4 | 前台服务 + 电池优化引导 + 失败原因可见，不承诺"永不掉线" |
| `tl-core` cfg 分流撕出编译错误 | A1.1 | 一次只 gate 一个模块，`cargo check --target` 每步都跑 |

---

## 执行记录（2026-11-19 首次落地）

**已发生的事实，后续执行者直接采信，不要重新踩**：

1. **`tl-core` 在安卓下只有一处硬阻塞**：`foundation/secret.rs` 的 `compile_error!`。把 DPAPI 的实现整块留在
   `cfg(windows)`、给 `cfg(target_os="android")` 一个**明确返回 Err** 的分支（不写明文兜底）之后，
   `cargo ndk -t arm64-v8a check -p tl-core` 一次过，`interface`（axum）/ `hosts` / `registry` / `atomic` 都不需要额外 gate。
   所以 A1.1 的 cfg 面比设计预估的小得多；真正的缺口只剩 Keystore 桥与 `atomic.rs` 的 unix 分支。
2. **UniFFI 的 Error 变体字段不能叫 `message`**：会与 Kotlin `Throwable.message` 撞车，
   生成的 `FfiConverter` 里报 `Overload resolution ambiguity between candidates: val message: String`（编译期，不是运行期）。
   统一改用 `reason`。
3. **`cargo-ndk` 4.x 的 `cargo-ndk.exe` 不能直接执行**（会打印 "This binary may only be called via `cargo ndk`"），
   必须写成 `cargo ndk -t arm64-v8a -o <dir> build -p tl-mobile --release --lib`。
   带 `--lib` 是为了不把 `uniffi-bindgen` 这个 bin 也编进安卓目标。
4. **本机工具链走的是免安装路线**（不装 Android Studio、不触发 UAC）：JDK 17 用 zip 解压到 `D:\ruanjian\jdk17`，
   `cmdline-tools` 解压到 `%LOCALAPPDATA%\Android\Sdk\cmdline-tools\latest`，再 `sdkmanager` 装
   `platform-tools` / `platforms;android-35` / `build-tools;35.0.0` / `ndk;27.2.12479018`；Gradle 8.10.2 解压在 `D:\ruanjian\gradle`。
   `sdkmanager --licenses` 在 PowerShell 管道里喂 `y` 不生效（仍报 "8 of 8 licenses not accepted"），
   直接写 `%ANDROID_HOME%\licenses\android-sdk-license`（三行 SHA1）后 `sdkmanager` 才肯非交互安装。
5. **`.so` 不进源码树**：`cargo ndk -o tmp/data/jniLibs`，Gradle 用
   `sourceSets["main"].jniLibs.srcDirs(file("../../tmp/data/jniLibs"))` 指过去；`tmp/` 已被 gitignore。
6. **`libjnidispatch.so` 缺失的写法**：`implementation(libs.jna) { artifact { type = "aar" } }`（版本目录里 `jna = 5.14.0`）。
   构建产物 `app-debug.apk` 里能看到 `lib/arm64-v8a/libtl_mobile.so`（6.33 MB）与 `libjnidispatch.so`（0.16 MB）。
7. **UniFFI 生成的 Kotlin 绑定入库**（`android/app/src/main/java/uniffi/tl_mobile/tl_mobile.kt`），
   否则 clone 后 Gradle 直接编译失败；重生成命令见 Task A0.2 Step 2，参数照抄即可。
