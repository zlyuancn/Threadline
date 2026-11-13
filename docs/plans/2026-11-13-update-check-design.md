# 更新检查与主动升级 · 设计

> 状态：**已拍板**（2026-11-13，用户要求「检查更新并能主动升级」）

## 目标

用户点一次就能从旧版升到新版，不需要自己去 Releases 页找安装包。启动时静默检查一次，有新版才提示，不打扰。

## 方案

| 环节 | 做法 |
|---|---|
| 更新源 | `GET https://api.github.com/repos/zlyuancn/Threadline/releases/latest`（公开仓，无需 token） |
| 版本比较 | 按 `tag_name` 去掉 `v` 后逐段比数字；只在新版**严格大于**当前时提示 |
| 资产选择 | 从 `assets` 里取 `*_x64-setup.exe`（CI 产出的 NSIS 安装包） |
| 下载 | 下到 `%TEMP%\Threadline-<version>-setup.exe`，带进度回调 |
| 升级 | 启动安装器 `/S /D=<当前安装目录>`，随后应用自行退出；安装器的 `NSIS_HOOK_PREINSTALL` 负责结束残留 `tl-core.exe`，`POSTINSTALL` 在静默模式下重新拉起应用 |
| 静默检查 | 启动后延迟 8 秒查一次；失败不弹任何东西（内网/断网是常态） |
| 跳过版本 | 用户点「跳过此版本」后写进 localStorage，同一版本不再打扰 |

## 边界

- **只对已安装的实例提供自动升级**：当前进程路径不含 `Threadline` 目录时（开发态 `target\debug`），只显示「有新版本」和 Releases 链接，不自动装。
- **不做增量**：每次下载完整安装包（当前约 7 MB），不引入差分更新。
- **不引入 `tauri-plugin-updater`**：它要求签名密钥与 `latest.json` 托管，与现有 GitHub Release 资产重复；本方案直接复用 CI 已产出的安装包。
- **不改安装器的默认行为**：静默安装仍走 `/S`，`currentUser` 模式不需要提权。

## 分层

```
src-tauri/src/main.rs        check_update() / download_update() / install_update()
src/api/native.ts            三个 invoke 的薄封装 + 类型
src/models/update.ts         状态机：idle → checking → available → downloading → ready；跳过版本、本地缓存
src/views/SettingsView.vue   「版本更新」面板
src/App.vue                  启动后延迟触发一次静默检查
```

Rust 侧只用 `ureq`（同步、rustls、体积小），不引入 tokio HTTP 栈——Tauri 已有 async runtime，`spawn_blocking` 包一层即可。

## 验收

1. 装 0.2.0，发布 0.3.0 后启动应用 → 8 秒内出现「有新版本 v0.3.0」，不阻塞主界面。
2. 点「立即升级」→ 显示下载进度 → 自动退出 → 安装完成 → 自动打开新版 → 设置页版本号已是 0.3.0。
3. 断网启动 → 无任何提示、无报错日志（失败静默）。
4. 点「跳过此版本」→ 重启后不再提示该版本，但 0.3.1 发布后仍会提示。
5. 开发态（`target\debug`）点升级 → 只给 Releases 链接，不执行安装。
