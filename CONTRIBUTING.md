# 参与开发

## 环境

| 依赖 | 版本 | 说明 |
|---|---|---|
| Windows | 10 1809+ / 11 | 仅支持 Windows（用到 hosts、DPAPI、WinINET、UAC） |
| Rust | ≥ 1.80（stable） | 需要 `rustfmt` 与 `clippy` 组件 |
| Node.js | ≥ 22.12 | |
| pnpm | ≥ 9 | 前端包管理器，不要用 npm/yarn 混装 |
| WebView2 Runtime | 系统自带或安装 | 运行 Agent 需要 |

```powershell
rustup component add rustfmt clippy
pnpm install
```

## 常用命令

```powershell
pnpm dev                    # 只起前端（浏览器里开发界面，走 vite 代理）
pnpm test                   # 前端单元测试
pnpm build                  # 前端类型检查 + 构建到 dist/

cargo test --workspace      # 核心侧测试（crates/*）

.\scripts\verify.ps1        # 本机门禁：fmt + clippy + test + 架构检查 + 前端测试与构建
```

**提交前必须 `scripts/verify.ps1` 全绿。** CI 跑同一套检查，另外多一步 `pnpm tauri build --no-bundle` 验证打包路径。

单独跑 Agent 侧测试要满足两个前置条件，否则 `tauri-build` 会直接失败：

```powershell
pnpm build                          # tauri-build 在编译期内嵌 ../dist
cargo build --release -p tl-core     # 或 cargo build，然后
.\scripts\bundle-sidecars.ps1        # 生成 src-tauri/binaries/tl-core-<triple>.exe
cd src-tauri; cargo test
```

`src-tauri` 是独立 workspace，所以根目录的 `cargo fmt/clippy` **看不到它**——Agent 侧的格式与 lint 必须单独跑（`verify.ps1` 已经替你做了）。

改了前端之后要让桌面窗口看到效果，必须三步：`pnpm build` → `cd src-tauri; cargo build` → 重启 `tl-agent.exe`。Tauri 的 `frontendDist` 在编译期内嵌，光改 `.vue` 不会热更。

## 结构约束

`scripts/msocheck` 强制分层，改代码前先看这几条：

- `crates/tl-core` 分 `foundation`（系统能力）/ `models`（状态与规则）/ `business`（编排）/ `interface`（HTTP 适配）。
- **`models` 不许直接调用 Win32、不许读文件、不许发网络请求**——这些都在 `foundation`。
- `interface` 只做协议适配，不写业务规则；同一规则不能在两处各写一遍。
- `crates/tl-protocol` 是 Core / Agent / Wake 共用的契约，**零依赖 Win32、零业务**。
- `src-tauri` 是独立 workspace，刻意不与 `crates/*` 共享依赖解析（Tauri 的依赖图很重）。

## 提交

- 一个提交只做一件事；信息用 `type(scope): 摘要`，`type` 取 `feat` / `fix` / `docs` / `refactor` / `test` / `chore`。
- 不要提交构建产物：`target/`、`dist/`、`src-tauri/binaries/`、`node_modules/`、`out/` 都已在 `.gitignore`。
- 涉及脱敏的文件（示例域名、地址、机器名）请用 `example.com` / `192.0.2.x`（TEST-NET-1）/ `dev-a` 这类占位，不要写真实内网信息。
- 新增依赖请说明理由；`Cargo.lock` / `pnpm-lock.yaml` 一并提交。

## PR

- 描述里写清「改了什么 / 为什么 / 怎么验证的」，附门禁输出。
- 行为变更请同步 `README.md` 与 `CHANGELOG.md`。
- 安全相关改动请先读 [`SECURITY.md`](SECURITY.md) 的威胁模型，多用户场景不在支持范围。

## 许可证

提交即表示同意以本仓库的 [PolyForm Noncommercial 1.0.0](LICENSE) 授权你的贡献。商业使用需另行取得授权。
