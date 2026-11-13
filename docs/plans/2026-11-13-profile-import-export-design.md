# 配置导入导出设计（.threadline 包）

> 状态：**已实施**（2026-11-13）；计划见 `docs/plans/2026-11-13-profile-import-export-plan.md`，
> 完成记录见 `docs/DONE.md`。实现与设计的两处偏差：打包用零依赖 ZIP（不用 fflate）；偏好由前端读写而非 Core。
> 场景：把一组「隧道（SSH 连接）+ 转发」分享给团队；密码/私钥绝不进文件。

## 1. 术语

| 词 | 指什么 | 数据归属 |
|---|---|---|
| 隧道 / 连接 | 一条 SSH 连接（主机、端口、登录用户、认证方式、保活、指纹、启用状态） | `servers.json` |
| 转发 | 一条端口映射（本机端点、远端端点、协议、域名、分组、图标…） | `tunnels.json` |
| 包 | 导出的 `.threadline` 文件（内部是一个 zip 压缩包） | 生成物 |

界面上标题用「隧道管理」，但导出面板必须写清"勾连接 = 连它下面的转发一起带走"，否则用户以为转发不在这里。

## 2. 已拍板

| 决策 | 结论 |
|---|---|
| 用途 | 分享给他人 / 团队共用，**只导结构不导密钥** |
| 勾选粒度 | 两级：连接与转发都能勾；勾连接=整条带，勾转发=自动补上所属连接并去重 |
| 冲突 | 导入前出预览，逐条显示「新建 / 跳过 / 覆盖」，重复项默认跳过 |
| 凭据 | 连接导过去没有密码：导入后标「待填密码」，编辑里补一次即可用 |
| 图标 | 作为 `icons/*.png` 独立文件放进包，不塞 base64 |
| 偏好 | 主题、呼出快捷键作为**可选**导出项（勾选才带） |
| 扩展到 | `.threadline`（内部就是 zip，任何解压工具都能打开） |
| 入口 | 设置 →「隧道顺序」改名「隧道管理」：调整顺序 / 导出配置 / 导入配置 |
| 默认停用 | 导入后默认 `enabled=false`（可取消勾选），避免无密码连接反复重连 |
| 主机指纹 | 默认带上（它就是服务端身份证明），预览里可勾「重新确认指纹」 |
| 数据边界 | 除泄密项外全部导出（见 §4） |

## 3. 包结构

```
mypc.threadline                # zip 压缩包（改扩展名而已）
├── profile.json               # 结构定义，UTF-8 纯文本
├── icons/
│   ├── 91c2.png               # 文件名用转发 id 的短哈希，避开中文/特殊字符
│   └── a7f1.png
└── README.txt                 # 一句话：不含密码，导入方式，生成版本
```

### profile.json

```json
{
  "kind": "threadline.profile",
  "version": 1,
  "exported_at": "2026-11-13T10:20:00Z",
  "app_version": "0.1.0",
  "servers": [
    {
      "id": "mypc-3f2a",
      "name": "mypc",
      "host": "192.168.28.213",
      "port": 22,
      "username": "root",
      "credential_kind": "password",
      "keepalive_secs": 30,
      "enabled": true,
      "host_key_fingerprint": "SHA256:xxxx",
      "forwards": [
        {
          "id": "mypc-rdp-91c2",
          "name": "mypc远程桌面",
          "local_host": "127.0.0.1",
          "local_port": 3389,
          "remote_host": "192.168.28.213",
          "remote_port": 3389,
          "scheme": "rdp",
          "domain": "",
          "remote_scheme": null,
          "open_path": "",
          "note": "",
          "group": "",
          "in_app": true,
          "auto_start": true,
          "icon": "icons/91c2.png"
        }
      ]
    }
  ],
  "preferences": {
    "theme": "jade",
    "hotkey": "Ctrl+Alt+T"
  }
}
```

约定：

- `preferences` 只在勾选「包含本机偏好」时出现，整块可缺省
- `icon` 是包内相对路径；空串 = 无自定义图标（导入端按协议/favicon 自动取）
- `id` 仅供"重复导入识别"，导入时**重新生成**；跨机器 id 无意义
- `domain_host`（回环 IP 分配）不写：那是本机派生状态，导入时重新分配
- `server_id` 不写：转发靠嵌套关系挂在连接下
- `kind` / `version` 不匹配直接拒绝，提示"不是 Threadline 配置或版本不支持"

## 4. 数据边界

| 数据 | 导出 | 理由 |
|---|---|---|
| 连接：名称/主机/端口/用户名/认证方式/保活/enabled/顺序 | ✅ | 结构 |
| 连接：已信任主机指纹 | ✅ 默认带 | 服务端身份证明；预览可改为重新确认 |
| 转发：全部字段（端点/协议/域名/远端协议/打开路径/备注/分组/应用页/`auto_start`/图标） | ✅ | 结构 |
| 自定义图标 | ✅ 独立文件 | 分享方配好的 logo 应当保留 |
| 主题 / 呼出快捷键 | ✅ 可选 | 用户明确要作为勾选项 |
| 密码 / 私钥 / 私钥口令（`credentials.dat`） | ❌ | 泄密 |
| 网关 CA 根证书与私钥（`root.pem` / `root.key`） | ❌ | 本机密钥；对方要自己的根 |
| 回环 IP 分配（`domain_host`）、hosts 托管状态 | ❌ | 派生状态，导入时重算 |
| `control.json`（含 token）/ `install.json` / 日志 | ❌ | 本机运行契约 |
| 运行态（连接数、速率、错误、重试） | ❌ | 非配置 |

## 5. 导出交互

入口：设置 →「隧道管理」→「导出配置」。

面板（`el-dialog`，宽 640）：

1. **两级勾选树**
   - 每个连接一行：名称 + `user@host:port` + 「N 条转发」；勾选后其下转发全部勾上
   - 展开显示转发：名称 + 本机端口 → 远端 + 协议徽标
   - 转发可单独勾；勾中的转发若所属连接未勾，自动把连接标成"半选"（只导这一条转发）
   - 顶部「全选 / 全不选」、搜索框
2. **三个开关**
   - 包含自定义图标（默认开；关闭后 `icon` 全部留空）
   - 包含已信任主机指纹（默认开；关闭后字段留空，对方首次连接需确认）
   - 包含本机偏好（默认关；勾上才写 `preferences`）
3. 底部：已选 N 条连接 / M 条转发，预计大小（含图标时给个量级提示），「导出」

保存：与「下载证书」同一链路——`Blob` + `<a download>`，默认文件名 `<日期>-threadline.threadline`（WebView 里的下载行为由 Tauri 接管，不额外引 fs 插件）。

空选择时按钮禁用并提示"至少选一条"。

## 6. 导入交互

入口：同一块「导入配置」。

流程四步，全在一个弹窗里：

1. **选文件**：Tauri 打开对话框，过滤 `.threadline`
2. **解析与校验**（Core `plan` 端点）
   - 压缩包完整性/版本校验、条目数上限、图标 MIME 与大小
   - 每条算出：`新建` / `跳过`（同 id 或同名已存在）/ `覆盖`（用户显式选）
   - 端口冲突：显示"本机 3389 已被 mypc远程桌面 占用"，可就地改端口（输入框）或跳过
   - 域名冲突：显示"域名 x 已被另一条转发使用"
   - 含域名的条目顶部提示"导入后需到设置里同步 hosts（需一次 UAC）"
3. **确认清单**
   - 连接行：勾选框 + 动作下拉（新建/跳过/覆盖）+ 状态说明
   - 转发行：同上，附加端口编辑
   - 底部开关：「导入后保持停用」（默认开）、「不覆盖本机偏好」（默认开）
   - 覆盖本机偏好时，下面列出差异：主题 A→B、快捷键 X→Y
4. **执行与汇总**
   - 逐条创建（先连接后转发），失败项列出原因
   - 结果：成功 N 条 / 跳过 M 条 / 失败 K 条；失败项可「重试」
   - 导入完成后刷新隧道与转发列表；有域名条目时提示同步 hosts

## 7. 冲突与边界

| 情况 | 处理 |
|---|---|
| 同 id 已存在 | 默认「跳过」；可选「覆盖」：更新名称/主机/端口/用户名/认证方式/保活与同名转发，**不动密码凭据与主机指纹** |
| 同名但 id 不同 | 默认「跳过」；选「新建」时自动改名 `名称 (2)` |
| 本机端口已被占用（配置层） | 就地改端口或跳过；不静默覆盖 |
| 域名已被占用 | 跳过或清空该条目的域名后导入 |
| 回环 IP 池耗尽 | 该条目失败并提示"域名入口已满（127.0.0.2–254）" |
| 系统级端口占用（别的程序占着 3389） | 不预判；落地后首次启动由运行时返回错误（跨平台可靠探测代价过高） |
| 无凭据连接 | 导入后 `needs_credential=true`：列表标黄「待填密码」，不能启用；编辑里填一次密码/私钥即恢复 |
| 偏好覆盖 | 用户取消「不覆盖本机偏好」才写入；主题只接受已知 id，快捷键失败（被占用）时保留原值并提示 |

## 8. 架构与实现

### 8.1 职责划分

- **前端负责打包与解包**：用 `fflate`（约 8KB 的 JS 库）在页面里把 `profile.json` 与图标压成一个 zip 字节流，用 `Blob` + `<a download>` 存成 `.threadline`；导入时用 `<input type="file">` 读回并解压。这里说"打包"就是压缩归档，与 Docker 无关。预览面板、图标类型与大小校验也在前端——归档格式属于格式问题，不进业务模型。
- **Core 负责规则**：条目能否创建、端口/域名/回环地址归谁、无凭据连接的状态。新增 `plan` 端点让预览与落地用同一套判定，避免前端自己预判导致"预览说能建、落地却失败"。

### 8.2 Core 改动

1. **允许无凭据连接存在**
   - `validate_create` 中 `credential_id` 允许为空（仅导入路径），`require_credential` 只在非空时校验
   - `ServerDto` 增加 `needs_credential: bool`，标 `#[serde(skip)]`：**不落盘**（它由 `credential_id` 是否为空派生），在 `list` / `create` / `patch` / `set_enabled` 的返回点统一重算，否则填完密码后列表仍显示「待填」
   - `set_enabled(true)` 返回 `409 credential_missing`；`connect_params` 同样拒绝
   - runner 把「未填密码」翻成**非重试**结果，不进入退避循环（现有 `ensure_session` 会把任意错误当 `NetworkFailed`，keeper 会一直重试）
   - 编辑保存时若填了 `secret` → 建凭据并回填 `credential_id`；**没有新 secret 时只更新字段、不重建材料**（现有 `patch_server` 会去读原凭据，空引用直接报错）
2. **配置级冲突查询**
   - 现有 `is_port_taken(port)` 只查**运行期**监听表、不分 host，不能用于导入预判；配置级判定在私有 `ensure_unique_in`
   - 新增 `TunnelStore::config_conflict(host, port, domain, exclude_id)`，`create` / `patch` / `plan` 三处共用，避免各写一套
3. **连接 id 唯一性**
   - `ServerStore::new_id` 现在是 `srv_{slug}_{pid % 10000}`，同进程内同名会撞；导入多条时必须按现有集合去重（照抄 `tunnel::new_id` 的写法）
4. **`POST /api/v1/profile/plan`**（只读）
   - 入参：profile JSON（图标已由前端转成 data URI 或留空）
   - 出参：逐条判定 `{ action, reason, suggested_name?, occupied_by? }`
   - **不含偏好**：主题在 WebView `localStorage`、快捷键在 Agent 的 `agent.json`，Core 拿不到也不该管
5. **`POST /api/v1/profile/import`**
   - 入参：用户确认后的条目（含动作、改名、改端口、图标 data URI、`keep_disabled`）
   - `keep_disabled` 用 `#[serde(default = "default_true")]`——缺省必须是「停用」，否则漏传就变成导入即启用
   - 出参：`{ created_servers, created_forwards, skipped, failed: [{name, reason}] }`
   - 逐条落地：先连接后转发；失败不回滚已成功的，结果中明确标出
   - **覆盖语义**：只更新名称/主机/端口/用户名/认证方式/保活，**不动 `credential_id` 与 `host_key_fingerprint`**（前者会清掉对方已填的密码，后者是安全状态，不能由文件单方面改写）

### 8.3 前端改动

| 位置 | 内容 |
|---|---|
| `src/models/profile.ts`（新） | 纯逻辑：勾选收集、去重、包结构序列化/反序列化、图标校验、产物大小估算；偏好读写（主题走 `theme` store，快捷键走现有 `invoke`）也在前端完成，不经 Core |
| `src/models/preferences.ts`（新） | 偏好快照与差异：读取主题/快捷键，导入时列出「将覆盖」项并按用户选择写回 |
| `src/components/ProfileExportDialog.vue`（新） | 勾选树 + 三个开关 |
| `src/components/ProfileImportDialog.vue`（新） | 预览清单 + 逐条动作 + 就地改端口 + 结果汇总 |
| `src/views/SettingsView.vue` | 「隧道顺序」→「隧道管理」，新增两行 |
| `src/api/profile.ts`（新） | 两个端点的调用封装（import 用 60s 超时，见下） |

前端单测覆盖：包序列化/反序列化往返、图标 MIME 与大小、`kind/version` 拒绝、偏好差异。

细节约束：

- 图标上限 **128KB 原始字节**：Core 的 `normalize_icon` 只接受 200KB 字符，256KB 图 base64 后约 34 万字符会被整条拒绝
- 解包用 `unzipSync(bytes, { filter })`：在 filter 里就按条目名与声明大小过滤，避免先全量解压再校验（那样炸弹防护无效）
- `profile/import` 走专用长超时（60s）：`client.ts` 默认 15s 对 200 条批量落地不够

### 8.4 依赖

- `fflate`（前端，zip 读写）
- 前端已能读写文件（现有 `exportGatewayCa` 下载链路），复用同一套 Tauri 能力

## 9. 安全

- 包里没有任何密钥材料；不提供"包含密码"的隐藏开关
- 解包防护：文件 ≤10MB、条目 ≤200、解压总量 ≤20MB；只接受 `profile.json` 与 `icons/<扁平文件名>`，拒绝 `..`、绝对路径、子目录
- 图标：白名单 `png/jpeg/webp/svg`，单张 ≤128KB（base64 后约 17 万字符，落在 Core `normalize_icon` 的 200KB 之内），超出则丢图标保条目
- 导入条数上限 200 条连接 / 1000 条转发
- 不自动写 hosts、不自动装证书、不自动启用连接

## 10. 测试计划

新增 TESTPLAN 条目：

- **U-16 导出**：勾连接带全部转发；单勾转发自动带连接；关图标后 `icon` 为空；关指纹后字段为空；勾偏好才写 `preferences`；产物可被标准解压工具打开
- **U-17 导入**：预览逐条动作；同名默认跳过；端口冲突可就地改；含域名条目提示同步 hosts；导入后连接标「待填密码」且不可启用；填密码后可连；覆盖偏好时先显示差异

单测（前端）：包往返、图标校验、冲突预判、版本拒绝。
单测（Core）：无凭据连接创建/启用拒绝/连接报错、plan 判定、import 逐条结果与部分失败。

## 11. 不做 / 后续

- 不做文件关联：双击 `.threadline` 不触发导入，也不在安装器里注册
- 不做"导入到已有连接的合并"：v1 只有跳过/覆盖/新建三种动作
- 不做自动同步 hosts：导入后提示用户去设置里点一次
- 不做加密包：分享场景不需要，加了反而要设计口令分发
