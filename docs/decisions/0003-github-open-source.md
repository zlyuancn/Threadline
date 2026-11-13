# 0003 · 开源发布：范围、历史、许可证与包标识

- 日期：2026-11-12
- 状态：**已生效**（许可证一项已被 `0005-license-polyform-noncommercial.md` 修正）
- 关联：`../../docs/plans/2026-11-12-threadline-github-开源发布.md`

## 决定

| 项 | 决定 | 理由 |
|---|---|---|
| 公开范围 | 仅代码子树 `project/threadline/` | 工作区文档含真实主机、内网拓扑与决策过程，长期公开成本高 |
| 历史 | **orphan 干净首提交**，内部历史留在私有仓库 | 89 个提交混有真实域名/地址/调试记录，逐 blob 审查漏一处即永久泄露 |
| 许可证 | **CC BY-NC 4.0**（署名—非商业性使用） | 需求是「可自由使用与分发 + 修改须署名 + 商业需授权」，CC BY-NC 同时覆盖署名与非商业 |
| 包标识 | `cn.zly.threadline` → `io.github.zlyuancn.threadline` | 去掉个人缩写，使用 GitHub 命名空间的反向域名，无需自有域名 |
| 数据目录 | **保持** `C:\Users\Public\Threadline` | core 需登录前由任务计划拉起、多用户共用、提权改 hosts；改到 `%LOCALAPPDATA%` 会让登录前自启失效 |
| README | 中英双语全文（`README.md` + `README.en.md`） | 兼顾国内受众与 GitHub 英文读者 |
| 审查 | 子智能体**串行**审查 → 主会话修复 → 复审，连续 2 轮 0 条 P0/P1 收敛 | 审查与修复分离，避免自审自过 |

## 已知代价

- GitHub 上没有历史；想追溯请到私有仓库。
- CC BY-NC 4.0 面向内容而非软件：无专利授权条款，GitHub 会标 `Other`；README 必须写清「非商业」与「署名」，避免被误认为 MIT。
- 改 identifier 后，本机旧安装会留下旧卸载项，需先卸旧版再装新版；WebView2 缓存重建，core 中的数据（`C:\Users\Public\Threadline`）不受影响。
- 公开快照是**单次导出**：之后每次发布都需要重新从私有仓库生成快照并推送，避免两套代码漂移。
