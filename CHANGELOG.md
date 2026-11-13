# Changelog

格式参考 [Keep a Changelog](https://keepachangelog.com/zh-CN/1.1.0/)，版本号遵循语义化版本。

## [Unreleased]

## [0.1.0] - 2026-11-12

首个公开版本。

### 新增

- **多隧道并行**：每条隧道一条独立 SSH 会话，各自启停，互不影响；断线按退避重连，恢复后自动回到原状态。
- **端口转发**：`-L` 本地转发，支持 HTTP / HTTPS / RDP / SSH / TCP 五种入口协议。
- **域名访问**：一条转发可额外分配 `127.0.0.2–254` 的回环入口域名，由 hosts 托管区块解析；HTTPS 按「本地协议 × 远端协议」分三格处理——明文直通、本地 CA 终止、端到端透传。
- **本地网关 CA**：ECDSA P-256 根、`pathlen:0`、叶子 7 天自动续签、按 SAN 集缓存；根证书可一键装入当前用户信任库，也可导出 PEM 手动导入。
- **代理例外**：域名入口自动并入 WinINET `ProxyOverride`，保留用户已有条目。
- **开机自启**：一个开关切换「SYSTEM 计划任务 + 登录启动项」，开与不停正在运行的服务互不牵连。
- **应用页**：转发按分组摆放，自定义图标取自站点 favicon，支持全局搜索。
- **全局热键** `Ctrl+Alt+T` 唤起面板，可在设置里改绑。
- **主题**：14 套配色。

### 安全

- 控制面只监听 `127.0.0.1`，每次启动轮换 32 字节 CSPRNG Bearer token。
- 凭据与本地 CA 私钥经 DPAPI（machine scope）加密后落盘。
- 提权入口（hosts 写入、ACL 修复、自启切换）的数据目录白名单为固定常量，不读调用方环境变量；写入 hosts 前对域名与地址做二次校验。

### 已知限制

- 仅支持 Windows，仅支持单用户机器（见 [SECURITY.md](SECURITY.md)）。
- 不做远程转发（`-R`）、不做 SOCKS5（`-D`）。
- 安装包未做代码签名，首次运行会有 SmartScreen 提示。
- 核心日志暂未落盘，日志页可能为空。

[Unreleased]: https://github.com/zlyuancn/Threadline/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/zlyuancn/Threadline/releases/tag/v0.1.0
