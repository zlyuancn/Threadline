# Security Policy

## 支持范围

| 版本 | 安全修复 |
|---|---|
| 0.1.x | ✅ |

只维护最新发布版。旧版本请先升级再报问题。

## 报告漏洞

**不要在公开 Issue 里贴漏洞细节。** 请走 GitHub 的私密通道：

<https://github.com/zlyuancn/Threadline/security/advisories/new>

报告里请包含：版本号、复现步骤、影响面（能做到什么）、你的环境（Windows 版本 / 是否多用户机器）。

**预期响应**：72 小时内确认收到；确认后 14 天内给出修复计划或说明不修的理由。

## 已知限制（不是漏洞）

Threadline 的威胁模型是**一台机器只有一个真实使用者**。以下行为是设计取舍，按"已知限制"处理，不作为漏洞修复：

- 数据目录 `%PUBLIC%\Threadline` 继承 `INTERACTIVE: Modify`，**同机其他账户可读可写**。
- `control.json` 中的控制面 Bearer token 是明文；能读该文件的本机主体可调用全部控制 API。
- 凭据（`credentials.dat`）与本地 CA 私钥（`gateway-ca\root.key`）使用 **DPAPI machine scope**——语义是"本机任何主体都能解开"，只挡文件被拷走，不挡同机其他账户。
- 启用域名访问会把 Threadline 的本地根证书装入**当前用户的**受信任根证书库，并写入 hosts 托管区块、修改 `HKCU` 的代理例外。

**多人共用一台机器时请勿使用。** 需要这种场景请先提 Issue 讨论。

## 我们对以下问题会认真处理

- 能被**远程**（网络可达）利用的问题；
- 能绕过鉴权访问控制 API（`127.0.0.1:18921-19020`）的问题；
- 提权链路（`hosts sync` / `--fix-acl` / `--autostart`）被滥用的问题；
- 凭据或 CA 私钥**落明文**、或经非预期路径外泄的问题；
- 本地 CA 能签发超出受管域名范围的证书的问题。

## 发布物校验

Release 页附 `SHA256SUMS.txt`。安装包**未做代码签名**，首次运行会出现 SmartScreen「未知发布者」提示；请核对 SHA256 后再决定是否运行。
