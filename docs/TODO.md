# TODO（进行中 / 待办）

> 本文件只放**未完成**的工作。收口时整条搬入 `DONE.md`。
> 完成清单：`docs/DONE.md`；决策记录：`docs/decisions/`；回归战报：`docs/archive/`。

## 路线图

| 批次 | 状态 | 详情 |
|---|---|---|
| B7 收口（环境验证 + 安装实装） | 🚧 进行中 | `docs/TESTPLAN.md` |

代码侧 B0–B6 已全部完成（见 `DONE.md`）。剩下的都是**只能在真机上做**的验证。

## 待办

### 阻塞中（需要用户/环境侧处理）

- [ ] **把安装目录加入安全软件白名单**（iOA / SmartVPN 等）。
  现状：`tl-core.exe` 从 `%LOCALAPPDATA%\Threadline\` 或 `target\debug\` 启动时，出网连接被打断并返回
  HTTP 403 网关页；从 `%TEMP%` 启动则正常。未加白名单前面板无法连接任何外部 SSH。
  证据与矩阵见 `knowledge/guides/connection-failure-triage.md`。

### 需要真实 SSH 服务器

- [x] **[环境]** 真实 sshd 打通一条隧道（T-01）—— 已验证：经转发访问 `1.1.1.1:80` 收到
  `HTTP/1.1 301 Moved Permanently`（381 字节），`bind → accept → direct-tcpip → 双向拷贝` 全链路成立。
- [ ] **[环境]** 断线恢复：拔网/重启 sshd，观察 `Backoff` → `Running` 且监听不丢（T-06）
- [ ] **[环境]** keepalive 失效：静默服务器后 ≤90s 从 `Running` 转 `Backoff`（T-09）
- [ ] **[环境]** 首次连接的主机密钥确认（T-12）
- [ ] **[环境]** 指纹不匹配拒绝（T-13）

### 体验与健壮性（可自行推进）

- [ ] 打开通道超时（现固定 10s）做成可配置：慢速网络下需要更长。
- [ ] 转发目标长期不回包时，状态里给出「疑似目标不可达」的提示（现在只有超时后的 last_error）。
- [ ] 凭据编辑目前不能改类型/登录用户（属身份），若确需变更应给出"新建并切换"的引导。

### 域名访问（hosts + 回环入口）

- [ ] 设计已定稿（2026-11-12）：`docs/plans/2026-11-12-domain-access-design.md`。
      方向：一域名一个回环 IP + hosts 托管区块 + 保存即同步（失败留「待提权」，UAC 由 Agent 拉起
      `tl-core hosts sync`）；HTTPS 按「本地协议 × 远端协议」三格：① http/http 直通、② https/http 本地 CA 终止、
      ③ https/https 透传（参考 joyshare 的入口/回源解耦）；**「本地 http + 远端 https」直接禁止**；② 只支持 HTTP/1.1；
      域名自动并入 WinINET `ProxyOverride`（Agent 以登录用户身份写，免提权）。待拍板后按 B1–B7 出实施计划。
- [x] **B0 前置探针（2026-11-12 完成）**：UAC 提权写 hosts 成功且安全客户端未拦截（写入后按原始字节还原，SHA256 一致）；
      无交互桌面下 `Import-Certificate` 写 CurrentUser Root 报「此操作中不允许使用 UI」，证实「无桌面降级为下载」必需；
      有桌面的 certutil 装根链路留到 B6c 真机补测。
- [x] **B1 模型与契约（完成）**：`TunnelDto` 加 `domain` / `domain_host` / `remote_scheme`（`Option`，缺省由 `scheme` 推导）；
      `effective_addr` 统一口径、`entry_mode` 三格判定（拒绝「本地 http + 远端 https」）、域名校验与回环池分配；
      `ports` 键改 `(host, port)`；`patch` 用旧地址快照释放、`remove` 先取地址再删。单测 8 项。
- [x] **B2 底座（完成）**：`foundation/hosts.rs`（按字节渲染托管区块、冲突检测、损坏区块只摘自己的行、令牌提权判定、flushdns）、
      `foundation/atomic.rs`（`ReplaceFileW` 保 ACL + 占用重试 + 同路径串行）、`foundation/timeout.rs`；`store` 收敛到 atomic。单测 11 项。
- [x] **B3 业务与 API（完成）**：`business/hosts_sync.rs`（sync/status/clear/启动自检/代理管理集）、`Services` 级 `/events`、
      `business::remove_tunnel`、`GET /hosts`、`POST /hosts/sync|clear|proxy-result|proxy-clear`。集成测试覆盖 `/hosts` 与 proxy-result 落盘。
- [x] **B4 提权链路（完成）**：`tl-core hosts sync --data-dir` 子命令（前置分流、只认默认数据目录、重解析点校验、写 `hosts.result.json`）；
      Agent `sync_hosts` 命令（runas + 结果轮询 + UAC 取消提示）。
- [x] **B5 代理绕过（完成）**：`tl-protocol::proxy_override`（分号 reconcile、保留用户项与 `<local>`、幂等、超长拒绝，8 项单测）；
      Agent `wininet.rs`（读写 HKCU、保持 `REG_SZ`/`REG_EXPAND_SZ`、双广播、`ProxyEnable`/`AutoConfigURL` 检测）与 `sync_proxy_override` 命令；
      `tl-agent` 新增 `tl-protocol` 依赖（已在 `src-tauri/Cargo.toml` 注明理由）。
- [x] **B6a CA 底座（完成）**：`foundation/tls_ca.rs`（ECDSA P-256、`pathlen:0`、根 10 年、叶子 7 天、SAN 限定、
      DPAPI machine 落盘、按 SAN 集缓存、剩余 1 天提前重签）；3 项单测含「重启后根复用」。
- [x] **B6b ② 链路（完成）**：`tls_acceptor_for`（`ResolvesServerCert` 运行时轮换、ALPN 只 `http/1.1`）、
      `serve_listener` 增加 `entry_tls`、握手 10s 超时与 `inflight` 回滚、`build_entry_tls` 构造失败**不退回明文**；
      duplex 端到端测试验证「本地根验通 + 只协商 http/1.1」。
- [x] **B6c CA 接口与 Agent（完成）**：`GET /gateway/ca`、`root.pem`、`reset`、`trust-result`；`GatewayService` 落盘信任状态、
      指纹不匹配的上报直接丢弃；Agent `install_gateway_ca` / `gateway_ca_trust` / `export_gateway_ca`（`certutil -user`，无窗口）。
- [ ] **B6c 真机补测**：装根时点「是/否」两条路径、`certutil -verify` 复核叶子验链、无桌面会话下载兜底。
- [x] **B7 前端（完成）**：HomeView 两个下拉（本地/远端）+ 禁止组合 + 域名模式下本地地址只读 + 入口端口按格默认；
      ForwardCard 域名徽标与入口地址（URL 优先域名，80/443 省略端口）；`App.vue` 顶部横幅（冲突 / 待写入 / 代理未同步）
      且事件到达后重拉 hosts；SettingsView 新增「域名访问」「本地网关证书」两块（同步/清理/装根/查库/下载/重新生成）。
- [x] **本机端到端验证（2026-11-12 完成）**：
      - CA 跨重启验链：重启后用「同私钥 + 同 DN」重建的 issuer 签出的叶子，能被磁盘上的旧根验通（`rcgen 0.13` 的补偿假设成立）；
      - Core 真机 API：指纹 64 hex、`trusted=null`、`root.pem` 200（602 字节）、`trust-result` 204 且回读生效、`GET /hosts` 指向真实 hosts；
      - 有域名配置时 `GET /hosts` 正确给出 `domain → 127.0.0.2`、`pending/needs_elevation`，`POST /hosts/sync` 返回 **409**；
      - `tl-core hosts sync --data-dir <非默认>` 退出码 2 且 hosts 字节不变（安全边界）；
      - `ProxyOverride` 真实注册表写入 + 还原（`THREADLINE_TEST_HKCU=1` 守卫）。
- [x] **装根真机链路（2026-11-12 完成）**：`certutil -user -addstore Root` 装根 → `certutil -user -store Root <SHA-1>` 返回 0 →
      `POST /gateway/ca/trust-result` 204 → 回读 `trusted=true`；随后 `-delstore` 删除，根库无残留、hosts 仍 31 字节。
- [x] **真机缺陷修复（2026-11-12）**：`certutil -store Root` 按 **SHA-1** 匹配，Core 原先只给 SHA-256 → 界面永远「未信任」。
      新增 `GatewayCaStatus.thumbprint_sha1`（查库用，展示仍 SHA-256）；`install_gateway_ca` 改为「装完用 SHA-1 查库」判定成功，
      不再只看 certutil 退出码（退出码 0 但未进库的环境存在）。
- [x] **三格真实链路（2026-11-12 完成，devb `192.168.28.99:33304`）**：
      - ① `http://plain.zly.m:18080` → 明文直通，返回远端内容，`up=81/down=205`；
      - ② `https://tls.zly.m` → 本地终止：`--cacert root.pem` 验通、`ALPN: server accepted http/1.1`、解密后回源明文；
        不带根时 `SEC_E_UNTRUSTED_ROOT`（证明叶子确由本地根签发）；
      - ③ `https://pass.zly.m` → 透传：`ALPN: server did not agree`（由远端决定），证明本地未终止；
      - 禁止组合（本地 http + 远端 https）→ **400**；三条隧道同机 `running`、端口/回环分配互不冲突。
- [ ] **观察项**：curl 的 Schannel 后端对本地根叶子报「吊销状态未知」（`--ssl-no-revoke` 后正常）。
      浏览器通常对显式安装的根不强制吊销检查，需在有桌面的 Edge/Chrome 上确认一次；若不通过，再评估给叶子加 CRL DP 的代价。
- [ ] **仍需外部环境**：卸载清理（安装器未构建）。

### 需要管理员权限 / 重启

- [ ] **[环境] UAC** `tl-core --autostart on|off` 实测：任务计划增删、`autostart.result.json` 回传
- [ ] **[环境] 权限边角**：标准用户能否读到 SYSTEM 任务的定义（`schtasks /query /tn ThreadlineCore /xml`）；若读不到，核心的 `GET /autostart` 需要在 ACL 或缓存上补一手
- [ ] **[环境] 零登录会话**：注销后 SYSTEM 核心仍在服务（`healthz` 可达）
- [ ] **[环境] 顶班**：自启开启但 SYSTEM 核心没起来时，Wake 拉起 `--stand-in` 并打降级标记
- [ ] **[环境] 安装器**：跑 `Threadline_0.1.0_x64-setup.exe` 实装、验证三 exe 同目录、卸载无残留

### 跨身份实测

- [ ] H1 文件锁：SYSTEM 进程持锁时用户身份进程能判出「已有核心」而非「打不开」
- [ ] H2 DPAPI：SYSTEM 写→用户读、用户写→SYSTEM 读
- [ ] H8 `%ProgramData%\threadline` 目录 ACL 允许用户身份核心写 JSON

## 暂缓 / 不做

- ~~远程转发 `-R`~~（❌ v1 不做，仅 `-L`）
- ~~SOCKS5 `-D`~~（❌ v1 不做）
- ~~引用 `~\.ssh` 私钥路径~~（❌ 凭据由使用者填写并机器级托管）
- ~~CAW R11 单进程合并~~（❌ 有桌面会话，分离可成立；合并即 AP1）
- ~~CAW R10 周期重拉 / 顶班后自动接棒~~（❌ 与「停止服务即停止」冲突；接棒推迟到下次开机）
- ~~运行期切换运行身份~~（❌ 只切「是否开机自启」这一个开关）
- ~~`install.json` 由安装器写入交互用户 SID~~（⏸ v1 未实现，凭据 ACL 退化为组授权并打 `credential_acl_relaxed` 标记）

## 续接点（新会话先读这里）

- **仓库**：单仓 `C:\my\threadline`（工作区文档 + `project/threadline`）；原私有远端 `git@git.zly.m:zlyuancn/threadline.git`
  **自 2026-11-13 起停止推送**（用户改从公共仓库同步），日常只做本地 commit。
- **测试环境状态**：本机门禁全绿（cargo test 152 项 + agent 6 + frontend 22 + msocheck 44 + clippy + fmt + 构建）；
  两条冒烟脚本（`smoke.ps1` 控制面、`smoke-agent.ps1` 托盘与单实例）——`smoke-wake.ps1` 已随两进程改造删除。
  NSIS 安装包已产出并**实际安装过一次**（`%LOCALAPPDATA%\Threadline\`，两 exe + uninstall）。
- **已真机验证**：真实 sshd 打通并跑通数据面（T-01）；主机密钥确认 → 信任 → 重新启动的完整用户流程。
- **未做**：断线恢复与 keepalive 失效（T-06/T-09）、UAC 自启实测、零登录会话实测、卸载残留检查。
- **下一步**：先解决"安全软件白名单"（否则面板无法连外部 SSH），其余按上面「待办」逐条验证。
- **拍板记录**：
  - 2026-10-19：产品名 **Threadline / 引线**；工作区 `C:\my\threadline`，项目 `project\threadline`；二进制 `tl` / `tl-core` / `tl-agent`。
  - 2026-10-19：开机自启 = 一个开关（运行期可切，切开关不停正在跑的服务）；停止服务 = 现在退出且不影响自启；Wake 在核心缺失时拉起用户身份 Core（自启已开却起不来 → `stand_in_core`）。
  - 2026-10-19：v1 仅 `-L`；凭据 = 使用者填写的私钥内容/密码，machine-scope DPAPI，不读 `~\.ssh`。
  - 2026-10-19：方法论版本戳 CAW `41f698a` / 工作区 `f78fd6e` / MSO `5c432f9`。
  - 2026-11-12：**域名访问**（用户提出、参考 joyshare）——定为「hosts + 一域名一回环 IP」，
    不做 HTTP 正向代理、不做 SNI 分流、不做 MITM；hosts 写入需 UAC 提权，保存成功即视为成功、
    提权失败留横幅。设计见 `docs/plans/2026-11-12-domain-access-design.md`。
  - 2026-11-13：**私有远端停止推送**——`git@git.zly.m:zlyuancn/threadline.git` 不再作为推送目标；
    用户改为从公共仓库同步代码。此后本仓只做本地 commit，AI 会话不得对该远端执行 push；
    何时与公共仓库对齐、是否重建历史由用户决定。

## 待确认（2026-10-21 收尾）

- [ ] **真实用户路径走一遍**：以普通权限（不提权）双击 `out\tl-agent.exe`，
      新建/编辑隧道、切换转发开关。此前所有验证都是在提权 shell 里做的，
      权限相关的问题（数据目录 ACL）正是因为这一点被掩盖了很久。
- [ ] **旧数据目录清理**：`C:\ProgramData\threadline` 已在首次启动时迁移到
      `C:\Users\Public\Threadline`（只补缺失、不覆盖），旧目录原样保留。
      确认新位置一切正常后可手工删除旧目录（含凭据，注意别误留）。
- [ ] **多人共用机器时收紧数据目录**：`%PUBLIC%` 给的是 `INTERACTIVE: Modify`，
      同机其他账户也能改配置、读 `credentials.dat`（DPAPI machine scope）。
      若要多用户环境，需 `/inheritance:r` 后只授 SYSTEM + 当前用户（会重新引入一次 UAC）。
- [x] **状态口径核对**（已定位并修，见 `DONE.md` 2026-10-21「会话掉线后的僵尸转发」）：
      不是 Backoff 的预期表现，是**监听循环只在"有人连进来"时才发现会话没了**，
      空闲时永远停在 `accept` 上，状态一直是 `Running`。
- [ ] **真机复现会话意外掉线**：拔网线 / 切飞行模式 30 秒再恢复，确认转发先变
      「已停止（SSH 会话已断开：转发已暂停）」、恢复后自动回到「运行中」，
      且恢复的是掉线前真在跑的那批（不看 auto_start）。当前只验证了显式断开路径。
