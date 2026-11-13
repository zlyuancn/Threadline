# 0004 · 威胁模型：只保证单用户机器

- 日期：2026-11-12
- 状态：**已生效**
- 关联：`0003-github-open-source.md`（公开发布）、`../../docs/plans/2026-11-12-threadline-github-开源发布.md`

## 决定

Threadline 的**支持范围是"一台机器上只有一个真实使用者"**。多用户共用机器、共享桌面会话、带外人的远程桌面主机，**不在保证范围内**，也不为此做加固。

因此明确不做：

| 项 | 决定 | 理由 |
|---|---|---|
| 数据目录迁 `%LOCALAPPDATA%` | **不迁**，保持 `%PUBLIC%\Threadline` | 迁 per-user 会让"登录前由 SYSTEM 拉起核心"读不到用户目录下的配置，等于推翻自启设计 |
| 敏感文件 ACL 收紧 | **不做** | 收紧后 SYSTEM 写入、用户读取需要双向授权，会重新引入 UAC；收益只对多用户场景成立，而该场景已声明不支持 |
| DPAPI 从 machine scope 改 user scope | **不改** | 核心可能以 SYSTEM 运行，user scope 解不开；改 scope 等于放弃跨身份共用 |
| `Degradation::CredentialAclRelaxed` 落地 | **不做** | 该降级标记的前提（要求 ACL 收紧）已被本决策取消，保留枚举仅为历史兼容 |

## 已知风险（必须写进 README / SECURITY，不得隐瞒）

1. `%PUBLIC%\Threadline\` 继承 `INTERACTIVE: Modify`，**同机其他账户可读可写**该目录下的配置。
2. `control.json` 中的控制面 Bearer token 为明文；能读该文件的本机主体可调用全部控制 API。
3. `credentials.dat` 与 `gateway-ca\root.key` 使用 **DPAPI machine scope**，语义是"本机任何主体都能解开"——共享目录 + machine scope 叠加后，加密只挡"文件被拷走"，不挡"同机其他账户"。
4. 根 CA 一旦被用户装入信任库，其私钥泄露即可对同网段实施 HTTPS 中间人。

## 替代保证（已实现，构成单用户场景下的实际防线）

- 控制面只绑 `127.0.0.1`，token 每进程启动轮换（32 字节 CSPRNG）。
- 提权入口（`hosts sync` / `--fix-acl`）的目录白名单钉死系统公共目录，不读调用方环境变量。
- Agent 注入 token 走 JSON 序列化 + 形状校验，控制文件被篡改不会变成任意脚本执行。
- 凭据与 CA 私钥不落明文；`Login`/`CredentialMaterial` 的 `Debug` 已脱敏。

## 重开条件

出现以下任一情况时重开本决策：

- 有用户在 issue 里报告"多人共用一台机器"的真实需求；
- 产品要进企业批量部署（此时必须做 ACL 收紧 + user scope DPAPI + 安装器写 SID）；
- 出现与本威胁模型不符的实际泄露事件。
