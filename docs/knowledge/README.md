# 项目知识库

> 只放**代码里搜不到**的东西：平台陷阱、踩过的坑、排查路径。
> 能直接从代码读出来的结构/流程不在这里重复（那是 `docs/plans/` 与代码注释的职责）。
> 已完成事项见 `../DONE.md`，未完成见 `../TODO.md`，决策见 `../decisions/`。

## 知识目录

| 文档 | 一句话 |
|---|---|
| [guides/windows-platform-traps.md](guides/windows-platform-traps.md) | Windows/WebView/构建链上踩过的坑与规避方式 |
| [guides/connection-failure-triage.md](guides/connection-failure-triage.md) | 「连不上」的分层定位法：从 TCP 到认证，以及如何区分是网络、配置还是本机安全策略 |
| [guides/visual-selfcheck.md](guides/visual-selfcheck.md) | 界面改动的自检回路：构建→启动→抓图→看图，以及看什么 |
| [guides/ui-failure-modes.md](guides/ui-failure-modes.md) | 界面的"静默失败"三种形态（禁用按钮 / 错误被刷新抹掉 / 无依据的成功提示）与排查清单 |
| [guides/derived-state-traps.md](guides/derived-state-traps.md) | 派生状态（速率/计数/汇总）的残留键陷阱：实体消失后旧值仍被累加，出现「0 条隧道却有流量」 |
