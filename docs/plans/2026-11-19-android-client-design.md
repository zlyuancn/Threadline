# 安卓端 · 设计

> 状态：**已拍板（2026-11-19），待执行**；实施计划：`docs/plans/2026-11-19-android-client-plan.md`
> 来源：用户提出「threadline 准备做安卓端」，场景是「人在外面用手机连回家里的内网服务」。
> 一句话：**安卓端不做「手机的桌面版」，做「手机在外面连回家」的那一段**——`-L` 转发 + 内置 WebView 入口，
> 复用现有 Rust 数据面，UI 用 Kotlin + Compose 重写，**不新增仓库**。
>
> 场景锚点（用户原话）：家里 sshd 的端口映射在公网机器上，人在外面用手机连回家里的内网服务。

## 目标

在 Android 手机上：连一条到家里 sshd 的 SSH 会话，把家里内网的服务（面板、数据库、RDP…）
映射到手机本地端口；面板类服务用内置浏览器直接打开，专业客户端（SSH 终端 / 数据库工具）走第三方 App。

## 已拍板的前提

| 项 | 决定 | 依据 |
|---|---|---|
| 定位 | **B 档**：App 内自用（`-L` + 内置浏览器）**并**让第三方 App 共用本地端口 | 2026-11-19 用户选 B |
| C 档（DNS 劫持） | **本次不做**，保留为后续独立阶段 | 同上，B 不占用 VPN 资源，C 可后加且不冲突 |
| 技术栈 | Kotlin + Jetpack Compose（UI）+ Rust 共享核心（`russh` 数据面）经 UniFFI 绑定 | 见「技术选型」 |
| 仓库形态 | **同仓**：`android/`（Gradle 工程）+ `crates/tl-mobile`（UniFFI 门面） | 用户明确「不新增仓库」 |
| 域名 / hosts | **不做**：Android 写 `system` 分区需 root，且内置浏览器不需要域名 | 见「不做什么」 |
| 本地 CA 装根 | **不做**：Android 7+ App 不信用户证书，改用 WebView 自带信任 | 见「HTTPS 三格」 |

## 技术选型（为什么不选另外三个）

| 候选 | 结论 | 理由 |
|---|---|---|
| **Kotlin + Rust 共享核心 + UniFFI** | **采用** | `windows-sys` 已收在 `cfg(windows)` 里（`crates/tl-core/Cargo.toml:36`），Win32 只集中在 `foundation` 的 4 个文件 + 壳；`russh` 用的是 `ring` 后端（根 `Cargo.toml:26`），无 aws-lc/NASM 包袱 |
| Kotlin + sshj/JSch 重写 | 备选（spike 失败才退） | 主机密钥确认、退避重连、keepalive 要在第二份实现里重写，缺陷要修两遍 |
| Tauri 2 Android | 否 | 官方无前台服务插件（插件表里 Android 只有 fs/http/dialog/notification 这类）；Vue 那套 1280 宽侧栏布局到手机上仍要重排，复用的只有逻辑层 |
| Compose + 手写 JNI | 否 | 与 UniFFI 同一收益，但手写 `extern "C"` 与内存生命周期全是自找的坑 |

UniFFI 的已知成本（必须接受）：Kotlin 绑定底层走 **JNA**，需要 `jna@aar`（否则 `libjnidispatch.so` 缺失直接 `UnsatisfiedLinkError`），
每 ABI 约 1MB，冷启动有一次 JNA 初始化；UniFFI 仍是 0.x，跨大版本升级要跟着改绑定。

## 架构

```
┌───────────────────────────── Android App（单进程） ─────────────────────────────┐
│ Compose UI（隧道列表 / 连接 / 内置浏览器 / 设置）                                 │
│        ↕ UniFFI 生成的 Kotlin 绑定（JNA 桥）                                     │
│ Rust 共享核心：tl-protocol + tl-core 的 models / business / engine               │
│        ├─ 隧道监听 127.0.0.x:port ── direct-tcpip ──▶ 家里内网服务               │
│        └─ 状态广播 ──▶ Kotlin（Flow）                                            │
│ ForegroundService 持有核心与常驻通知（进程存活保障）                              │
└──────────────────────────────────────────────────────────────────────────────────┘
```

### 对 CAW 的偏离（必须在文档里说清）

桌面是「Core 常驻进程 + Agent 壳 + HTTP 控制面 + token」。安卓端：

- **不跑 HTTP 控制面**：没有 axum、没有 token、没有 `control.json`。核心以库的形式编进 App，
  UniFFI 直接调 Rust API——手机上没有第二个进程要用它，多一层 HTTP 只是开销与攻击面。
- **不变量保留**：状态仍然只有核心持有，UI 仍是无状态的投影（订阅状态流、动作回调）；
  「核心零 UI 依赖」「降级必标记」不变。
- 由此 `crates/tl-core/src/interface/` 在安卓构建里整块不编（`cfg(not(target_os = "android"))`），
  HTTP 适配退化为**桌面专属**。这一条要写进 `AGENTS.md` 的两进程说明里，否则下次读文档的人会以为手机也在跑 HTTP。

## 平台差异（Win32 → Android）

| 能力 | 桌面实现 | 安卓实现 | 备注 |
|---|---|---|---|
| 凭据加密 | DPAPI（machine scope） | Android Keystore 持 AES-256-GCM 主密钥，密文落文件 | `foundation/secret.rs` 按 `cfg` 分流；密钥不可导出，卸载即失效（合理） |
| 原子存储 | `ReplaceFileW` + 占用重试 | 同目录 `rename`（原子替换语义一致） | `foundation/atomic.rs` 已有平台分流点 |
| 文件锁 | `fs2` | `fs2`（Android 有 `flock`） | 单实例互斥改为「前台服务单例」，文件锁仅防重入 |
| hosts | 托管区块 + UAC | **无** | 不做域名，入口直接是 `127.0.0.x:port` |
| 本地 CA / 装根 | 自带 CA + certutil | **无** | 见「HTTPS 三格」 |
| 开机自启 | 计划任务（SYSTEM） | **不做**（首发） | 前台服务 + 通知足够；真要自启得处理厂商后台限制 |
| 日志 | 文件 + `/logs` 接口 | logcat + `filesDir/logs/`，设置页可导出 | — |

## 入口与「回环别名」

桌面用 `domain_host`（`127.0.0.2–254`，见 `models/tunnel/mod.rs:45`）做域名入口。安卓端直接复用这套池子，
但语义换成**「给这条转发一个独立回环地址」**：

- 默认监听 `127.0.0.1:<port>`（与桌面一致，端口唯一性规则不变）。
- 打开「独立浏览器会话」开关 → 监听地址改为池里下一个空闲 `127.0.0.x`。
  理由：cookie 按 **host** 隔离、不分端口，全都挤在 `127.0.0.1` 会让两个面板互相顶掉登录态。
- **零协议改动**：复用现有 `local_host` 字段，不新增 `TunnelDto` 字段、不写迁移。

这一条是 B 档的关键：`127.0.0.x` 是整机共享的回环，第三方 App（Termius / Navicat / 系统 Chrome）都能连——
**这是有意为之**，不是漏洞。安全边界由「手机是谁在用」承担（本产品定位单人多设备，非多租户）。

## 内置浏览器

- 用系统 **WebView（Chromium 内核）**，不是 Chrome（Chrome 不可嵌入）。
- **直接加载真实 TCP 地址** `http://127.0.0.5:8123`，**禁止**用 `shouldInterceptRequest` 做请求代理：
  它只覆盖 http(s) 资源请求，**WebSocket 与流式下载拦不住**，而 HA / noVNC / Jupyter / Proxmox 都依赖 WebSocket。
- `network_security_config`：只对 `127.0.0.0/8` 放行 cleartext；预留一张打包在 apk 内的根，供后续 https 本地终止。
- 每个隧道一条浏览记录（URL 可带 `open_path`），返回键在页面内后退、到底再退出到列表。

### HTTPS 三格（与桌面「本地协议 × 远端协议」对齐，但省掉 hosts 那一维）

| 场景 | 安卓做法 |
|---|---|
| 远端 http、入口 http | 直通，`http://127.0.0.x:port` |
| 远端 https、自签/内网 CA | **本地 TLS 终止**：核心用自己的根签叶子，WebView 通过 `network_security_config` 信这张根 |
| 远端 https、公网可信证书 | 透传，WebView 当作普通 https 访问 `127.0.0.x`（证书 SAN 不含该 IP → 会报错，此格首发**不支持**，文档写明） |

## 分层归属

| 位置 | 改动 |
|---|---|
| `crates/tl-protocol` | 原样复用（DTO / `SCHEMA_VERSION` / profile 契约） |
| `crates/tl-core` `foundation` | `secret.rs` / `atomic.rs` / 加 `cfg(target_os = "android")` 分支；`hosts.rs` / `registry.rs` 整块 `cfg(windows)` 或空实现 |
| `crates/tl-core` `models` / `business` | 原样复用；`hosts_sync` 与 `gateway` 在安卓构建里关掉（不做 hosts/CA） |
| `crates/tl-core` `interface` | 安卓构建不编（HTTP 是桌面专属适配） |
| `crates/tl-mobile`（新） | **纯门面，零业务规则**：`#[uniffi::export]` 暴露 `start/stop/state_flow/create_*`，只做类型映射与 `tokio` 运行时持有 |
| `android/`（新） | Compose UI、ForegroundService、Keystore 注入、WebView 宿主、通知；不读文件、不碰 SSH |
| `tools/msocheck` | `SRC_DIRS` 增加 `crates/tl-mobile/src`（按 shell 层对待）+ 新规则 R7：`tl-mobile` 禁止 import `models::*::internal`、禁止出现 `anyhow::bail!` 之外的判定逻辑（门面不得藏规则） |
| `scripts/verify.ps1` | 增加两关：`cargo ndk -t arm64-v8a build -p tl-mobile --release`（交叉编译通过）+ `gradlew :app:testDebugUnitTest`（Kotlin 单测） |

## 不做什么（首发边界）

- **不做** `-R` 远程转发、**不做** `-D` SOCKS5（沿用 v1 铁律 8）。
- **不做** hosts 写入、**不做**域名、**不做**装根证书。
- **不做**开机自启、**不做**零登录会话（安卓没有这个概念）。
- **不做**服务器管理（概览 / 容器 / 控制台）：那些依赖 PTY 与采集脚本，功能上可复用，但手机上 320dp 宽装不下，二期再评估。
- **不做** C 档 DNS 劫持——它是独立的一层，接入点见下节。

## C 档（DNS 劫持）后续接入点

影响面已经预留：

- 核心不需要改：C 只多一个「域名 → `127.0.0.x`」的解析应答器，数据来自现有 `domain` / `domain_host` 字段。
- 新增 Kotlin 侧 `VpnService`（tun 只承载虚拟 DNS 地址的 UDP 53），未托管域名转发给手机原 DNS 或家里 DNS。
- 前置条件由用户处理：关「私人 DNS」与 Chrome「安全 DNS」；App 只做检测与提示，不代替修改。

## 阶段划分

| 阶段 | 内容 | 出口 |
|---|---|---|
| **A0 spike（3 天）** | ① `cargo ndk -t arm64-v8a` 编出 `russh + ring + tokio-rustls` 的 `.so`；② 最小 App 经 UniFFI 起一条 `-L` 打通家里公网端口；③ WebView 加载 `http://127.0.0.5:port` 打开家里面板 | 三项任一失败 → 翻案退 sshj，并回写本文档 |
| **A1 骨架** | Gradle 工程 + `crates/tl-mobile` + 平台分流（Keystore / atomic / 关闭 hosts·gateway·interface）+ 后台 Tokio 运行时 | 核心能在 APK 里启动并回答 `whoami` 等只读调用 |
| **A2 转发管理** | 隧道 / 服务器列表、新建编辑、启停、主机密钥确认流程、断线退避重连（全部复用现有模型） | 手机上增删改一条转发，状态与桌面语义一致 |
| **A3 浏览器入口** | WebView 宿主、回环别名开关、独立 cookie 验证、明文放行白名单 | 两个面板同时登录互不顶掉 |
| **A4 前台服务** | 常驻通知、进程被杀后恢复、Doze 白名单引导、日志导出 | 息屏 30 分钟后隧道仍在、转发可用 |
| **A5 门禁与真机验收** | msocheck R7、verify 增关、真机清单进 `docs/TESTPLAN.md` | 门禁全绿 + 真机全项通过 |

## 风险

| 风险 | 影响 | 应对 |
|---|---|---|
| `ring` 的 C 部分在 NDK 下编译失败 | A0 直接红 | 退 `aws-lc-rs`（需 CMake）或退 sshj；A0 就是为了尽早撞上 |
| UniFFI + JNA 落地问题（`libjnidispatch.so`、启动耗时） | 首屏慢或直接崩 | 用 `jna@aar`；`System.loadLibrary` 提前一次；A0 验证 |
| 厂商 ROM 杀后台 | 「连上了但一会儿就断」 | 前台服务 + 电池优化引导 + 每次重连写 `last_error`，让用户看得见原因 |
| 多 ABI 体积（Rust `.so` × 4 + JNA） | APK 偏大 | 首发只发 `arm64-v8a` + `armeabi-v7a`，AAB 按 ABI 拆包 |
| WebView 明文/证书策略 | 面板打不开且原因难查 | `network_security_config` 显式白名单 + 失败时给出可读原因（不是空白页） |

## 验收

1. 手机经公网端口连上家里 sshd，`-L` 转发 `127.0.0.1:8123` 可用（`curl` 或内置浏览器）。
2. 断网 30 秒恢复后自动回到 `running`，恢复的是掉线前在跑的那批（不看 `auto_start`）。
3. 首次连接必须核对主机密钥指纹；指纹变更拒绝连接，界面给出与桌面同口径的提示。
4. 两条面板分别用 `127.0.0.2` / `127.0.0.3` 打开，各自登录态互不影响。
5. 同一端口对外可见：系统 Chrome 与第三方 App 能连 `127.0.0.x:port`（B 档语义）。
6. 息屏 30 分钟后隧道仍在；杀掉 App 后从通知回来，状态与配置一致。
7. `scripts/verify.ps1` 全绿（含 msocheck R1–R7 与新增的 NDK 交叉编译、Gradle 单测两关）。
