# 0005 · 许可证改为 PolyForm Noncommercial 1.0.0

- 日期：2026-11-12
- 状态：**已生效**（修正 `0003-github-open-source.md` 的许可证一项）
- 关联：`0003-github-open-source.md`、`0004-single-user-threat-model.md`

## 决定

许可证由 **CC BY-NC 4.0** 改为 **PolyForm Noncommercial 1.0.0**。

| 项 | 0003 的决定 | 现在的决定 |
|---|---|---|
| 许可证 | CC BY-NC 4.0 | **PolyForm Noncommercial 1.0.0** |
| 正文位置 | 未明确 | 根 `LICENSE`（唯一来源） |
| 元数据 | `Cargo.toml` 仍写 MIT（未同步） | `license-file = "LICENSE"`（Rust）、`"SEE LICENSE IN LICENSE"`（npm），不再使用 SPDX 标识符 |
| 发布到包管理器 | 未提 | 四个 crate 全部 `publish = false` |

## 理由

1. **CC 官方不建议把 CC 许可用于软件**：CC BY-NC 4.0 没有专利授权条款——对含 SSH、TLS、CA 签发的实现是实质缺口；也不区分源码与二进制、不定义"分发编译产物"的边界。
2. **NC 的歧义**：CC 的 `NonCommercial` 在"公司电脑上自用"这类场景含义不明；PolyForm 把非商业用途、个人使用、非商业组织逐条写清，判断成本更低。
3. 需求没变，仍是三点：可自由使用与分发、修改须署名（PolyForm 的 `Required Notice` 条款）、商业使用需授权。

## 已知代价

- 仍**不是 OSI 认可的开源许可证**，GitHub 会标 `Other`；README 必须首屏写清「源码可见，非商业免费，商业需授权」，否则会被误认为 MIT/Apache。
- `Cargo.toml` 不能写 `license = "PolyForm-Noncommercial-1.0.0"`（SPDX 标识符在 crates.io 上属实验性），只能用 `license-file`；因此 crates.io 页面不会展示标准许可徽章。
- 许可证正文只有一个来源（根 `LICENSE`），其余三处只写引用，不允许各写一套。

## 落地清单（本次已完成）

- 新增根 `LICENSE`（PolyForm Noncommercial 1.0.0 全文 + `Required Notice` 行）。
- 根 `Cargo.toml`：`license = "MIT"` → `license-file = "LICENSE"`，补 `repository`。
- 三个 crate：`license.workspace` → `license-file.workspace`，各加 `publish = false`。
- `src-tauri/Cargo.toml`：补 `license-file = "../LICENSE"`、`publish = false`、`repository`。
- `package.json`：补 `"license": "SEE LICENSE IN LICENSE"`。
- 新增 `THIRD_PARTY-NOTICES.md`（MPL-2.0 / Unicode-3.0 / WebView2 / NSIS 等）。
