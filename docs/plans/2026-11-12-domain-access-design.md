# 域名访问（hosts + 回环入口）设计

- 日期：2026-11-12
- 状态：**设计已定稿（含 HTTPS 两模式、代理绕过），待拍板实施**
- 前置事实（已实测）：本机非管理员身份可绑定 `127.0.0.5:80/443`、`127.0.0.9:80/443`；Windows 无低端口特权限制，整个 `127.0.0.0/8` 可 bind。
- **B0 实测（2026-11-12）**：
  - **hosts 提权写入 PASS**：UAC 提权后追加托管区块成功，安全客户端未拦截；写入后按原始字节还原，SHA256 与备份一致（`0797E4C0…`）。
  - **根证书安装受桌面约束（已证实）**：`Import-Certificate` 写 `CurrentUser\Root` 在无交互桌面的会话里报「此操作中不允许使用 UI」。这证明 §3.4「无可见桌面就禁用安装、只给下载」是必需的，不是保守设计。
  - **待补测**：有桌面的会话里 `certutil -user -addstore Root` 点「是」后的完整链路（B6c 真机门禁时补），以及在提权进程里 `ipconfig /flushdns` 的退出码。
- 参考实现：`D:\my\joygatecn`（joyshare）——「hosts + 回环池 + 域名型入口」（`docs/plans/2026-08-31-joyshare-design.md`）、
  「访问矩阵 + 本地根两层」（`docs/plans/2026-09-03-domain-and-tls-design.md`）、
  「入口 TLS 实施」（`docs/plans/2026-09-05-entry-tls-implementation.md`）。

## 1. 背景与目标

现状：threadline 每条转发是纯 TCP `-L` 映射，访问方式是 `127.0.0.1:<local_port>`。远端服务若按域名做虚拟主机或证书绑定，用 `127.0.0.1` 访问会 404 / 证书不匹配。

目标：让用户在浏览器里直接输入 `https://home.zly.m` 就能经 SSH 隧道访问远端服务。

**关键洞察**：域名访问的骨架只有两条——

1. **hosts** 把域名指向本机回环地址；
2. **本机在标准端口监听**（80/443），再经 SSH `direct-tcpip` 到远端。

HTTPS 在此之上是「本地入口协议 × 远端回源协议」的三格矩阵（joyshare 的「入口 ↔ 回源解耦」，见 §3.1–§3.3）：① 两端都明文、③ 两端都 TLS 时纯字节直通；② 远端明文而本地要 https 时由本地做 TLS 终止。三格都不解析 HTTP。**远端是 https 时本地必须是 https**（远端 https + 本地 http 直接禁止）。

## 2. 非目标

- 不做 HTTP 正向代理（不需要浏览器/系统配置代理）。
- 不做 SNI 或 Host 解析分流（一域名一回环 IP 已足够，且对非 HTTP 协议同样有效）。
- 不做通配符域名（hosts 不支持）；需要多个子域就建多条转发。
- 不做 IDN（中文域名不转 punycode，直接拒绝）。
- 不改现有不带域名的转发行为。
- 不对 `tcp` 入口做 TLS 终止（无法判断应用层协议，只搬字节）。

## 3. 地址翻译链

```
Chrome ──DNS(hosts)──> 127.0.0.5:443
       ──TCP──────────> tl-core 本地监听（127.0.0.5:443）
       ──direct-tcpip──> SSH ──> remote_host:remote_port（远端解析）
```

- 域名只在**本机**存在，网络上传递的仍是 `remote_host:remote_port`。
- `remote_host` 与域名无关：远端服务监听 `127.0.0.1:443` 就填 `127.0.0.1`，在另一台内网机就填那台的地址。
- 域名必须与远端服务的 `server_name` / 证书域名一致，否则远端 404 或证书报错——这是用户的责任，界面给出提示。

### 3.1 HTTPS：三种组合（入口 ↔ 回源解耦）

参考 joyshare 的结论（`2026-09-03-domain-and-tls-design.md` §5 访问矩阵、§6.2 本地根两层）：**入口形态与回源形态解耦**。joyshare 有 server 侧做回源交付，threadline 只有本地一端，所以回源这一段落在本地。

矩阵只剩三格：

| 本地（浏览器）＼ 远端 | 远端 http | 远端 https |
|---|---|---|
| **本地 http** | ① 直通（现状） | ❌ **禁止**（远端 https ⇒ 本地必须 https） |
| **本地 https** | ② 本地做 TLS 终止 | ③ 直通（端到端 TLS） |

- ①③ 是**纯字节管道**：两端协议一致，谁都不需要理解内容。
- ② 是**协议不匹配**：远端明文、本地 TLS，由本地补上 TLS 服务端。
- `tcp` 入口是第四种形态（非 HTTP 协议），永远纯字节，不看远端是什么。

**为什么禁止「本地 http + 远端 https」**：浏览器会以为自己处在明文站点，而远端按 TLS 语义行事——远端的 `301 → https://` 会把浏览器送到入口 443（若没配 https 入口就断）、`Secure` Cookie 在本机明文页面不生效、HSTS 与 OAuth 回调地址全部错位。这一格还必须额外维护一条「本地 TLS 客户端」代码路径，却没有明确收益，所以 v1 直接拒绝保存（提示「远端是 https 时本地必须也是 https」）。

### 3.2 三格逐一分析

**① 本地 http → 远端 http（直通）**

```
浏览器 ──HTTP/1.1 明文──> 127.0.0.5:80 ──SSH──> 远端 80（明文）
```

- 本地行为：`accept → direct-tcpip → 双向拷贝`，与现状完全一致。
- 证书：无。ALPN：不涉及。
- 支持：HTTP/1.1、WebSocket、任意明文 TCP 应用。
- 约束：远端 Web 服务必须认得 `Host: <域名>`（`server_name` 或 default server）。

**② 本地 https → 远端 http（本地 TLS 终止）**

```
浏览器 ──TLS(本地 CA 叶子)──> 127.0.0.5:443 ──[终止]──> HTTP/1.1 明文 ──SSH──> 远端 80
```

- 为什么不能直通：远端明文服务收到 `ClientHello`，会按非法 HTTP 请求返回 400 或断开；浏览器报 `ERR_SSL_PROTOCOL_ERROR`。
- 本地行为：`TlsAcceptor` 包裹监听；握手完成得到明文流，再照旧 `direct-tcpip` 双向拷贝。
- 证书：本地 CA 现签叶子（SAN = 域名 + 回环 IP），浏览器需信任本地根（装一次）。
- **ALPN 只宣告 `http/1.1`**（关键）：若本地宣告 h2，浏览器会把 HTTP/2 帧发进来，而解密后直接灌给远端 h1 服务就是乱码。浏览器降级到 h1 后功能完全正常，代价是没有多路复用。
- Host 头不改写：浏览器按 URL 写的 `Host: <域名>` 原样到达远端，与 ① 一致。
- 适用：内网 HTTP 服务想用 https 访问（最常见场景）。

**③ 本地 https → 远端 https（透传）**

```
浏览器 ──TLS(远端真证书)──> 127.0.0.5:443 ──字节直通──SSH──> 远端 443（TLS）
```

- 本地行为：纯字节管道，不终止、不握手。
- 证书：浏览器直接验证**远端真证书**，零 CA、零安装；要求远端证书匹配该域名。
- ALPN：浏览器与远端直接协商，**HTTP/2、gRPC、WebSocket、TLS 1.3 全部支持**——这是三格里能力最完整的一格。
- 远端用自签证书时浏览器会告警：这是远端证书问题，threadline 不做「双段 TLS」替它兜底（见 §3.3 末）。
- 适用：远端已有匹配域名的真证书（当前 `deva.zly.m → 192.168.28.98` 大概率属于这一格）。

### 3.3 三格的能力与限制对照

| 能力 | ① http/http | ② https/http | ③ https/https |
|---|---|---|---|
| HTTP/1.1 | ✅ | ✅ | ✅ |
| WebSocket | ✅ | ✅ | ✅ |
| HTTP/2 多路复用 | —（浏览器不走 h2c） | ❌（只 h1） | ✅ |
| gRPC | — | ❌ | ✅ |
| 需要装根证书 | 否 | **是** | 否 |
| 需要远端证书匹配域名 | 否 | 否 | **是** |
| 需要解析 HTTP | 否 | 否 | 否 |

**为什么 ② 不能支持 h2**：它是「一侧 h1 字节流、一侧 TLS」，本地没有 HTTP 层，无法把 h2 帧翻译成 h1 请求。要支持 h2 就得引入真正的反向代理（hyper），那是另一个量级的改动。joyshare 能在 server 侧支持，正是因为它写的是 HTTP 反代而不是字节管道。

**v1 不做「改名」**：三格都要求入口域名 = 远端服务名（Host/SNI 一致）。joyshare 在「入口改名 + 注入原域名」时走的是 HTTP 反代；threadline v1 用字节管道做不到，需要时在远端加 `server_name` 或走 default server。

**不做「双段 TLS」**：入口终止后再以 TLS 连远端（SNI 可用另一个名字、可跳过校验）能覆盖「远端自签或证书域名不匹配」的场景，但那需要本地同时做 TLS 服务端与客户端，且仍受 h1 限制。v1 的选择是：远端有真证书就 ③ 直通；远端是明文就 ② 终止；远端自签就先在远端换成真证书（或把证书装进系统信任），不改用 MITM。

### 3.4 本地 CA（② 终止模式用）

沿用 joyshare `2026-09-05-entry-tls-implementation.md` 的规格，只把「JoyShare」换成「Threadline」：

- **根**：ECDSA P-256；CN=`Threadline Local Gateway Root`；`BasicConstraints CA:TRUE, MaxPathLen:0`（签不出下级 CA）；有效期 10 年。
- **根私钥**：DPAPI（machine scope，与现有凭据同一套 `foundation::secret`）加密后落盘；根证书 PEM 明文可导出（安装用）。
- **叶子**：按需现签，SAN = **当前受管域名 + 该转发的回环 IP**（IP SAN）；有效期 **7 天**；按 SAN 集指纹内存缓存，过期自动重签。**签叶子时校验 SAN 必须是受管集合内的域名**（根私钥泄露也无法被用来签任意域名，越界直接拒绝并记日志）。
- **TLS 服务端**：`GetCertificate` 忽略 SNI，直接返回本监听对应的叶子（叶子已含全部 SAN）；只启用 TLS 1.2/1.3，**ALPN 只提供 `http/1.1`**（理由见 §3.2 ②）。
- **运行时轮换**：每次 `GetCertificate` 检查叶子剩余有效期（不足 1 天即重签并替换缓存），**不重启监听**；改域名后重建该监听（SAN 跟着变）。
- **信任判定由 Agent 执行，Core 不查**：`certutil -user` / `CERT_SYSTEM_STORE_CURRENT_USER` 在 SYSTEM 核心进程里读的是 `S-1-5-18` 的根库，开机自启档会**永远**显示「未信任」。Agent（登录用户）用 `certutil -user -store Root <thumbprint>` 查指纹在不在，经 `POST /gateway/ca/trust-result` 上报；Core 只存「上报值 + 探测时间」。v1 只查库、不做验链。
- **安装是一次可见的交互动作**：`certutil -user -addstore Root <root.pem>` 会弹**系统安全警告对话框**（`-f` 压不掉），必须先弹我们自己的确认再执行；用户取消系统警告 ⇒ 记为用户取消，不重试；无可见桌面会话（零登录会话/服务上下文）时禁用该按钮，只给「下载根证书」。
- **Firefox**：默认不读 Windows 根存储——需在 `about:config` 开 `security.enterprise_roots.enabled=true`，或手动导入 `root.pem`；界面在证书卡片里提示（与代理例外的 Firefox 提示并列）。
- **重置**：删除 CA 目录 + 按 **thumbprint** 从当前用户根存储删除（`certutil -user -delstore Root <sha1>`，不要按 CN——多轮 reset 后同 CN 可能有多张）；② 的监听需重建。
- **存储位置**：`%PUBLIC%\Threadline\gateway-ca\root.pem` / `root.key`（DPAPI 密文）。
- **日志**：所有 CA 操作（生成/重签/安装/重置/上报）写 `<data-dir>\logs\gateway-ca.log`。

**为什么本机自签是安全的**：hosts 已把域名劫持到本机回环，这个域名在本机的解析结果完全由 threadline 决定；根私钥 DPAPI 保护、只签 7 天叶子、`pathlen:0` 签不出下级 CA、SAN 限定受管域名。风险与 joyshare 同型（mkcert 先例）：终端被攻破时 TLS 信任本就是最后一道防线。

## 4. 数据模型

`TunnelDto` 新增三个字段（均 `#[serde(default)]`，老配置读取为默认值）：

| 字段 | 类型 | 含义 |
|---|---|---|
| `domain` | `String` | 访问域名；空 = 不使用域名访问（保持现状） |
| `domain_host` | `String` | 自动分配的入口回环地址（如 `127.0.0.5`）；`domain` 为空时恒为空 |
| `remote_scheme` | `Option<String>` | 远端协议。`None` = 由 `scheme` 推导（见下）；显式值只能是 `http` / `https` |

`CreateTunnelRequest` / `PatchTunnelRequest` 同步增加 `domain` 与 `remote_scheme`（`domain_host` 不接受外部输入，只由模型分配）。

### 4.0 两条硬规则（防止静默改写存量行为）

1. **三格只在 `domain` 非空时生效。** `domain` 为空时入口恒为纯字节管道，与今天逐字节等价，**完全不看** `scheme`/`remote_scheme`。现有界面新建表单默认 `scheme=http`、存量里还有 `scheme=https` 的纯透传转发——它们不能被新语义碰到。
2. **`remote_scheme` 缺省必须由 `scheme` 推导**（`https`→`https`、`http`→`http`、`tcp`→不参与），而不是固定 `http`。实现用 `Option<String>` + 模型层 `effective_remote_scheme(dto)`（`None` 时回落 `scheme`），不要用 `#[serde(default = "固定值")]`。

否则一条存量 `scheme=https`、无域名的转发会被判成 ②（本地终止），把明文灌给远端 TLS 端口，升级即全断，且报错看起来像远端故障。

**不升 `SCHEMA_VERSION`**：新字段全部 `#[serde(default)]`，老文件原样可读。`main.rs` 的 `retire_legacy_data` 遇到低版本文件会改名留档——顺手升版等于让用户的隧道「消失」。

### 4.1 三格判定（仅在 `domain` 非空时）

| scheme | effective_remote_scheme | 对应格 | 本地入口行为 |
|---|---|---|---|
| `tcp` | 不参与 | — | 纯字节（非 HTTP；域名只作主机名） |
| `http` | `http` | ① | 明文直通 |
| `http` | `https` | — | **拒绝保存**：远端 https 时本地必须 https |
| `https` | `http` | ② | 本地 TLS 终止（本地 CA 叶子）+ 明文回源 |
| `https` | `https` | ③ | TLS 透传（远端证书，零 CA） |

老配置兼容：`scheme` 缺省仍为 `tcp`；`scheme=tcp` 时 `remote_scheme` 不参与判定，不会因为缺字段突然启用 TLS。回归用例：老配置 `{"scheme":"https"}`（无 `remote_scheme`）必须判成 ③，不是 ②。

### 4.2 有效监听地址（唯一口径）

定义**一个**函数，所有登记/释放/唯一性判断/绑定都只走它：

```rust
fn effective_addr(dto: &TunnelDto) -> (String, u16) {
    if dto.domain.is_empty() {
        (dto.local_host.clone(), dto.local_port)
    } else {
        (dto.domain_host.clone(), dto.local_port)
    }
}
```

- `local_port` 默认值**按格给**：① → 80；② → 443；③ → `remote_port`（远端 443 → 443）。不能统一用 `remote_port`——② 的远端是 80，默认填 80 会让 `https://域名` 连到 80 直接失败，而 ② 恰恰是最常见的场景。远端非标准端口时保留 `remote_port` 并在表单提示「可用 443」。
- `domain` 非空时 `local_host` 不参与监听（表单置灰，原值保留，清空域名后恢复）。
- **`patch` 必须先快照旧的 `effective_addr`**，释放时用快照；否则改完域名后 `apply(Stop)` 会用新键释放，旧键永久泄漏，之后同一地址端口永远「被占用」。
- 唯一性判定从「端口唯一」改为 **`(host, port)` 唯一**（修掉现有缺陷：不同回环地址上的同端口会被误判冲突）。
- 受影响调用点全部在 B1 逐条改造：`ports: HashMap<u16, String>` → `HashMap<(String, u16), String>`、`is_port_taken`、`start` 登记、`apply` 释放、`patch` 的 `old_port`、`is_port_taken_for_other`、`ensure_unique`、`release_port`、`release_resources`，以及 `tunnel_runner::start` 里的 `bind_local`。
- **非域名转发不得占用池地址**：新建、或本次改动把 `local_host` 改到 `127.0.0.2–254` 时拒绝；**存量已是池地址的放行**（不改现有行为，否则用户下次保存就被拒），只在表单标黄提示「该地址属于域名入口池」。
- **创建顺序**：`TunnelStore::create` 现在是 `ensure_unique` → `load`；引入分配后必须改为 `load`（拿现有配置）→ 分配 `domain_host` → `ensure_unique`（用 `effective_addr`）→ 写入。否则首次分配看不到刚加载的配置，可能与别的转发撞地址。

### 4.3 地址池与分配

- 池范围：`127.0.0.2` – `127.0.0.254`（`127.0.0.1` 保留给普通转发与系统）。
- 分配规则：本条转发已有 `domain_host` 且未被他人占用 → 复用；否则从池中取「未被任何其它转发使用」的最小地址。
- 释放：转发删除或域名清空时释放。
- 分配只保证「不被自己人占用」，**不保证该地址上的端口可绑定**（本机可能有进程绑了 `0.0.0.0:443`）；绑定失败的错误必须带实际地址与端口（现有「本地端口绑定失败」文案要同步改）。

### 4.4 域名校验

- 小写归一，并去掉末尾的点（`home.zly.m.` 归一为 `home.zly.m`）。
- 总长 ≤ 253；每段 1–63；只允许 `a-z 0-9 -`，不允许首尾连字符、不允许连续 `--` 出现在第 3/4 位（punycode 前缀）。
- 必须至少两段（含点）。
- 拒绝：空、IP 字面量、`localhost` 及其子域、`*.local`、含端口/空格/斜杠/冒号/下划线、非 ASCII（IDN 不转换）。
- 跨转发重复 → 拒绝，并指出被哪条转发占用；错误文案保留「已被…占用」子串（接口层用 `message.contains("已被")` 判 409）。

## 5. hosts 托管区块

路径：`%SystemRoot%\System32\drivers\etc\hosts`。

```
# >>> threadline domain access (managed) >>>
127.0.0.5	home.zly.m
127.0.0.6	git.zly.m
# <<< threadline domain access (managed) <<<
```

规则：

- **只改自己这两行标记之间的内容**；标记不存在则追加到文件末尾（追加前确保前一行非空则补换行）。
- 块外内容一律原样保留（包括用户自己的 `192.168.28.98 deva.zly.m`）。
- **按字节处理，不做转码**：只识别 ASCII 标记行与 `127.x` 地址行；块外可能是 GBK 注释或含 BOM，读进来再按 UTF-8 写回会把它们弄坏。BOM 原样保留。
- 保留原文件的换行风格（首个换行符是 `\r\n` 就用 CRLF）。
- 域名在块外已存在（含注释掉的行不算）→ **该条记为冲突，不写入**，界面提示「hosts 里已有一行指向 X，请先手工删除」，不静默覆盖。
- **区块损坏**（有开始标记、没有结束标记）：只摘掉开始标记行与紧随其后的 `127.x` 地址行，**不吞**后续用户条目（joyshare 2026-09-06 的同类教训：开区间吞到尾会删掉用户数据）。
- 域名只支持 ASCII（IDN 不转换 punycode，中文域名直接拒绝）。
- 写入方式：同目录临时文件 + 原子替换（`ReplaceFileW`，保留 hosts 原有 ACL）。**复用 `foundation::store::rename_with_retry` 的重试口径**（20 次、退避至 150ms，只对 `ACCESS_DENIED`/`SHARING_VIOLATION`/`LOCK_VIOLATION` 重试）——hosts 正是杀软重点盯防的文件，一次失败就报「待同步」会让横幅抖动。把这段重试抽成底座通用的「原子替换（可重试）」能力，hosts 与 JSON 共用。
- 失败文案区分两种：**权限不足**（需要提权）与**文件只读属性**（去掉只读即可），不要都甩「待同步」。
- **禁止截断式探测**：判断「能不能写」不能用 `File::create`/`fs::write`（会当场把 hosts 清空）。用 `GetTokenInformation(TokenElevation)` 判身份，或直接走「临时文件 + 替换」并只把 `ERROR_ACCESS_DENIED` 当需要提权。测试断言探测前后 hosts 字节不变。
- 写入成功后调用 `ipconfig /flushdns`（`CREATE_NO_WINDOW`，失败只记日志不影响结果）。

**常驻策略**：域名条目在**保存配置时**同步，不随转发启停增删。

- 理由：每次启停都可能弹 UAC，不可接受。
- 停止转发后域名仍指向本机、但无监听 → 浏览器得到「连接被拒绝」，语义清晰，不会误连到别处。
- 界面明确显示「域名已写入 hosts，转发未运行」。
- 空集（没有任何域名）时连同区块标记一起删除，不留空壳。

**启动自检**：进程/Agent 启动时比对「期望区块」与 hosts 实际内容。

- SYSTEM 档或可写时：静默修复（上次异常退出可能留下已删除域名的旧条目）。
- 用户档不可写时：只标记「待同步」并在横幅提示，**不自动弹 UAC**（与 §7 的「用户点才提权」一致）。
- 同时校验 `domain_host` 分配是否仍有效（被别的转发占用/手工改配置导致漂移），必要时重新分配并标记待同步。

## 6. 控制 API（tl-protocol）

| 端点 | 语义 |
|---|---|
| `GET /api/v1/hosts` | 返回 `HostsStatus`：hosts 路径、托管条目 `[{domain, address, tunnel_id}]`、块外冲突 `[{domain, existing}]`、是否需要提权、上次错误 |
| `POST /api/v1/hosts/sync` | 尝试同步一次；直接写入成功返回 200 + 新状态；无权限返回 **409** + `needs_elevation: true`（保存动作本身不算失败） |
| `GET /api/v1/gateway/ca` | 根证书状态：指纹、**Agent 上报的**信任状态（含探测时间）、存储路径、last_error |
| `GET /api/v1/gateway/ca/root.pem` | 导出根证书（下载兜底，Agent 也可直接读数据目录） |
| `POST /api/v1/gateway/ca/reset` | 重新生成根（需确认；已装旧根不自动删除） |
| `POST /api/v1/gateway/ca/trust-result` | Agent 上报信任探测结果（`{trusted, thumbprint, at}`）——SYSTEM 核心无法自己判断用户根库 |
| `POST /api/v1/hosts/proxy-result` | Agent 回报代理例外同步结果（`{ok, code, applied, message, at}`，与 `AutostartResult` 同形），Core 用 `applied` 覆盖管理集 |
| `POST /api/v1/hosts/clear` | 清空托管区块（删标记与块内内容，不碰其它行）；需提权，走与 `sync` 相同的 409 → Agent runas 路径 |
| `POST /api/v1/hosts/proxy-clear` | 清空代理例外的管理集条目（只删我们加过的） |

`HostsStatus` 为新增 DTO。**`needs_elevation` 的判定按 §5**：`GetTokenInformation(TokenElevation)` 或「替换失败且错误为 `ERROR_ACCESS_DENIED`」——**绝不能用「以写方式打开 hosts」探测**（`File::create` 会当场清空文件）。

`POST /hosts/sync` 只做「按当前配置重算并写入」，不接受任何域名/内容参数。`POST /hosts/clear` 是唯一能让「配置里仍有域名、但 hosts 区块为空」的入口，清理后下一次 sync 会重新写入（这与「清理」的语义一致：清理是恢复干净的临时手段，不是持久开关）。

## 7. 提权链路

1. 保存/删除带域名的转发 → 业务层编排：
   - 收集所有 `domain` 非空的条目（tunnel 模型提供）；
   - 渲染期望区块（foundation）；
   - 尝试直接写（SYSTEM 档成功）；
   - 失败 → 标记「待同步」，保存仍然成功。
2. 面板顶部横幅：「N 个域名待写入 hosts」+ 按钮。
3. 点击 → Agent 调 `POST /hosts/sync`；若 409 需要提权，则 `ShellExecuteW(..., "runas", ...)` 拉起同目录 `tl-core.exe hosts sync --data-dir <dir>`。
   - **`ShellExecuteW` 拿不到退出码**（只返回 HINSTANCE）。沿用仓库既有模式（`autostart.result.json` / `acl.result.json`）：先删旧 `hosts.result.json` → runas → 轮询结果文件（与既有助手一致的 120s 上限）→ 再拉 `GET /hosts` 复核；`nShow` 用 `SW_HIDE`，避免每次同步闪一个控制台窗口。
   - UAC 取消判据：`ShellExecuteW` 返回值 ≤ 32（5 = `SE_ERR_ACCESSDENIED`）。
4. `hosts sync` 子命令的安全约束：
   - **不接受任何内容参数**（不传域名、不传区块内容）；`--data-dir` **只接受 `default_data_dir()`（`%PUBLIC%\Threadline`）这一个值**，其它路径一律拒绝（退出码 2）。不用 `install.json` 做白名单——它就在候选目录里，调用方放一份自己写的就能通过，等于自证；
   - 打开前校验目标目录与 `logs` 子路径不含重解析点（`FILE_ATTRIBUTE_REPARSE_POINT`），否则拒绝——防 junction/符号链接把提权进程变成任意文件追加；
   - 子命令分支必须在 `create_dir_all` / `retire_legacy_data` / `migrate_legacy_data` **之前返回**（现有 `main()` 会先做这三件事，会改名用户文件）；
   - 沿用 `--autostart` 的约定：不抢锁、不绑端口、不写 `control.json`；
   - 只写托管区块 → flushdns → 结果写 `hosts.result.json`（`{ok, code, message, at}`）→ 退出码 0/非 0；日志写 `<data-dir>\logs\hosts-sync.log`。
5. Agent 拿到结果后重拉 `GET /hosts` 刷新界面。

**安全边界**：`hosts sync` 的写入目标恒为 `127.0.0.x` 回环地址，域名来自本机配置；即使被其它程序调用，也只能把域名指到本机，无法劫持到外部 IP。提权入口不接收内容、数据目录固定为默认目录、路径不含重解析点，避免变成任意 hosts / 任意文件写入后门。

**同 exe 其它提权路径的口径（本次只约束新入口，但要写明）**：`--fix-acl` 与 `--autostart` 同样接受 `--data-dir`，其中 `--fix-acl` 会对该目录 `icacls /grant /T`。这是既有的更大暴露面，**不在本次改动范围**，但要在 §12 记为已知风险，并在 B4 提交说明里标注「未收敛，后续单独处理」。

**非默认数据目录的降级**：`--data-dir` 启动的绿色部署无法走提权写 hosts（白名单只认默认目录）。这类部署下域名功能显示「需默认安装位置」，不提供提权按钮，不静默失败。

**变更通知**：现有 `/events`（SSE）只订阅 `TunnelStore` 的广播，hosts 写入结果、proxy 结果、UAC 取消都不会敲——横幅会一直挂着不消失。需要把变更通道提升到 Services 级（业务层在 hosts/proxy 状态变化后调用同一个 notify），`/events` 语义升为「任一可观察状态变了就敲」，不引入轮询。

**删除走业务层**：现有 `remove_tunnel` 在接口层直接 `stop_tunnel` + `tunnels.remove`。要支持「删除后域名从 hosts 移除、地址回收」，必须补 `business::remove_tunnel(id)`（stop → remove → sync_hosts），接口层退化为一次调用；否则就是接口层编排，踩 MSO 红线。`create_tunnel` / `patch_tunnel` 的 hosts 同步同理必须收在业务用例内。

**机检约束**：壳层（`main.rs`）禁止 `apply*`/`spawn*` 形态，所以子命令与业务入口命名为 `hosts sync` / `sync_hosts`，不放 `apply`。

### 7.1 系统代理绕过（WinINET ProxyOverride）

**为什么必须有**：系统代理开启时，浏览器会把 `https://home.zly.m` 发给代理服务器，代理无法解析内网域名 → 表现为「curl 直连正常、浏览器打不开」。只写 hosts 不够，域名还必须进入代理例外名单。（joyshare 2026-09-06 H-3 真机结论。）

**写什么**：`HKCU\Software\Microsoft\Windows\CurrentVersion\Internet Settings\ProxyOverride`（`REG_SZ`，分号分隔）：

```
home.zly.m;git.zly.m
```

- 只追加/移除我们托管的那几条，**用户既有条目一个不动**。`<local>`（绕过所有非 FQDN 主机）若已存在就原样保留——它不属于管理集，永不删除。
- 通配符（如 `*.zly.m`）**刻意不用**：WinINET 本身支持通配，但会误伤同后缀的其它域名，v1 只写精确域名。
- 写完调用 `InternetSetOptionW(NULL, INTERNET_OPTION_SETTINGS_CHANGED, ...)` 与 `INTERNET_OPTION_REFRESH` 广播，浏览器无需重启即可生效。
- 值是 `REG_SZ` 或 `REG_EXPAND_SZ`：读取按实际类型处理，写回保持原类型。

**谁写（关键约束）**：`HKCU` 属于**进程所属用户**。开机自启档的 Core 是 SYSTEM，写进去的是 `HKU\S-1-5-18`，对登录用户完全无效。因此：

- **代理例外一律由 Agent（登录用户身份）执行**，Core 只负责算清单。
- 单用户手动档（Core 与 Agent 同一用户）也走 Agent，保持单一执行路径，避免两套代码。
- Core 在 `GET /hosts` 里返回 `proxy: { desired: [...], managed: [...], add: [...], remove: [...], applied: bool, error }`。
- Agent 执行顺序：读现有 `ProxyOverride` → 按 `add` / `remove` 做字符串级 reconcile（保留其它项）→ 写回 → 广播 → `POST /hosts/proxy-result` 回报。
- 管理集（上次由我们写入的域名）由 Core 持久化在 `%PUBLIC%\Threadline\proxy-overrides.json`，**不依赖注册表反推**——否则无法区分「用户自己写的域名例外」与「我们写的」。
- Agent 回报的 `applied` = 实际写入后的托管条目集合，Core **用它覆盖管理集**。否则用户手工删掉条目后 Core 仍以为在管，下次 reconcile 的 `remove` 会删空、清理按钮变空操作、卸载残留。

**触发时机**：Agent 启动、界面收到 Core 的 hosts 变更事件、用户点「同步」、域名增删改保存后。

**降级与边界**：

| 情况 | 表现 |
|---|---|
| 系统代理未启用 | 仍可维护条目（无害），界面显示「当前未启用系统代理」 |
| PAC 自动配置脚本（`AutoConfigURL` 非空） | `ProxyOverride` 不生效，检测后提示「系统使用自动配置脚本，需在 PAC 中为该域名加直连规则」 |
| Firefox 自建代理设置 | 不跟随系统代理，界面提示需自行加例外 |
| WinHTTP（`netsh winhttp`） | 系统服务用，与浏览器无关，不在范围 |
| 条目超长（>1024 字符） | 提示「域名过多，代理例外可能被截断」，不做自动归并 |
| Agent 未运行 | 域名解析仍正常（hosts 已写），但系统代理开启时浏览器可能打不开；设置页显示「代理例外未同步」 |

### 7.2 卸载与清理

卸载器（NSIS，管理员身份）按顺序做四件事：

0. **先停 Core**：否则 SYSTEM 核心仍持有监听，且下一次配置变更会把托管区块写回去。
1. **hosts**：按托管区块标记整块删除（不碰块外）；若区块损坏按 §5 的规则只摘自己的行。
2. **代理例外**：读 `proxy-overrides.json` 的管理集，从 `ProxyOverride` 里逐条移除（只删我们加过的；读不到管理集就不动，避免误删用户条目）。
3. **本地根证书**：按 **thumbprint** 从 CurrentUser 根存储删除（`certutil -user -delstore Root <sha1>`），thumbprint 记在 `gateway-ca` 元数据里——按 CN 删在多轮 reset 后目标不唯一。卸载用户与安装用户不一致（`install.json` 记录的 SID）时跳过，并在卸载完成页留一条手工命令；删除失败不阻塞卸载，只提示。

数据目录（含 CA 私钥密文、tunnels.json、日志）由卸载器按安装器的选择决定是否保留（用户数据默认保留）。

## 8. 前端交互

- 转发表单：
  - 新增「访问域名（可选）」输入框，placeholder `home.zly.m`；
  - 填写后显示只读的「入口地址」`127.0.0.5`（自动分配）；
  - 「本地端口」在该模式下标签改为「入口端口」，默认值按格给（①→80、②→443、③→远端端口，见 §4.2）；
  - 实时预览最终地址：`https://home.zly.m`（按 `scheme`；`tcp` 时显示 `home.zly.m:端口`）。
- 表单改为两个下拉（对应 §3.1 的两轴）：
  - **本地访问协议**：`http` / `https` / `tcp`；
  - **远端协议**（`tcp` 时不显示）：`http` / `https`；
  - 选完实时显示落在哪一格与一句话说明：
    - ①「明文直通」
    - ②「本地加密：将使用本地网关证书（需信任一次）」
    - ③「端到端加密：浏览器直接验证远端证书」
  - **本地 http + 远端 https 直接禁止**：下拉一侧变动时立即内联提示「远端是 https 时本地必须也是 https」，保存按钮置灰。
  - 表单固定提示：「域名需与远端服务的 server_name 一致（v1 不改写 Host）」。
- 转发卡片：显示域名（可点击），「打开」按钮优先用域名 URL，其次才是 `127.0.0.1:port`。
- 卡片的「已写入 hosts / 待提权 / 冲突」状态**从 `GET /hosts` 的托管条目 join `tunnel_id` 得到**，不要在 `TunnelDto` 上再加一份状态字段（避免两个事实源）。
- URL 拼接规则：`scheme://域名`；入口端口为 80/443 时省略端口，其它端口显示 `域名:端口`；`open_path` 追加在后面（空 = `/`）。`tcp` 入口不生成 URL，只显示 `域名:端口`。
- 域名模式下 `local_host` 不参与监听：表单把「本地地址」置灰并注明「域名入口自动分配」，原值保留不动（清空域名后自动恢复原监听地址）。
- 设置页新增「本地网关证书」卡片：根证书指纹、信任状态（已信任/未信任）、「安装到当前用户」「下载根证书」「重置」。
- 首次开启终止模式的转发时，若根未受信任，卡片与转发卡片都给出「安装本地网关证书」入口（不自动装）。
- **横幅的触发源统一在 §8 这一处声明**（其余章节只引用，不再各写一套）：`hosts` 待提权 / `hosts` 冲突 / `代理例外未同步且系统代理开启` / `② 的本地根未信任`。
  - 待同步：「N 个域名待写入 hosts」+「写入」按钮；
  - 冲突：「域名 X 与 hosts 中已有条目冲突」+「查看」；
  - 代理：「系统代理已开启，域名的代理例外未同步」+「同步」；
  - 证书：「本地网关证书未受信任」+「安装」。
  - 横幅消失依赖事件到位（§7）与前端重拉（`App.vue` 的 `onChange` 增加 `hosts.refresh()`）。
- 设置页新增「域名访问」小节：hosts 状态、托管条目列表、「重新同步」（`POST /hosts/sync`）「清理托管区块」（`POST /hosts/clear`）。
- 设置页同一小节显示**代理例外**状态：已同步 / 未同步 / PAC 生效中 / Agent 未运行，附「重新同步代理例外」（`POST /hosts/sync` 的 proxy 部分）「清理代理例外」（`POST /hosts/proxy-clear`）。
- 清理托管区块只删除标记之间的内容与标记本身，不动其它行；清理代理例外只移除我们管理集里的条目。两者都需要提权时走与 sync 相同的 409 → runas 路径。

## 9. 错误处理

| 情况 | 表现 |
|---|---|
| 域名格式非法 | 表单内联报错，保存被拒 |
| 域名与其它转发重复 | 保存被拒，指出占用者 |
| hosts 块外已有同域名 | 保存成功但该条进「冲突」，横幅提示手工清理 |
| 入口端口绑定失败（如 `0.0.0.0:443` 被占） | 转发进 `Error`，原因写明「127.0.0.5:443 绑定失败，可能有程序占用 443」 |
| 无权限写 hosts | 保存成功，状态「待提权」，横幅给按钮 |
| hosts 写入失败（杀软/文件锁） | 保留待同步，显示具体错误 |
| UAC 被取消 | 提示「已取消」，状态不变 |
| 代理例外未同步且系统代理已开 | 横幅提示「系统代理已开启，域名可能被代理接管」+「同步代理例外」 |
| PAC 自动配置生效 | 提示需在 PAC 里加直连规则，`ProxyOverride` 无效 |
| 终止模式但本地根未装 | 转发照常监听（浏览器会告警）；卡片与设置页给「安装本地网关证书」，不自动装 |
| 根安装失败（certutil 报错） | 显示命令与错误码，提供「下载根证书」手工安装 |
| ③ 透传但远端不是 TLS | 浏览器报 `ERR_SSL_PROTOCOL_ERROR`；错误提示「远端疑似明文，改用 ②」 |
| ② 终止但远端其实是 TLS | 远端 400/断开；提示「远端疑似 HTTPS，改用 ③」 |
| 本地 http + 远端 https（且 `domain` 非空） | 保存被拒，提示「远端是 https 时本地必须也是 https」；`domain` 为空时**不做此校验**（保持纯字节管道） |
| TLS 握手卡住（半开连接） | 握手阶段 10s 超时，超时即断开并记日志（复用 `foundation` 的超时组件） |

## 10. 架构落点（MSO）

| 层 | 落点 | 职责 |
|---|---|---|
| 契约 | `crates/tl-protocol/src/proxy_override.rs`（新增） | `ProxyOverride` **纯函数** reconcile（分号项增删、保留用户项、幂等、长度校验），零 Win32 / 零 IO，Core 与 Agent 共用、可单测 |
| 契约 | `crates/tl-protocol/src/lib.rs`（扩展） | `TunnelDto` 新字段、`HostsStatus`（含 `proxy` 子结构）、`GatewayCaStatus`、`HostsSyncResult`、`ProxyResult`、`GatewayTrustResult` DTO |
| 底座 | `crates/tl-core/src/foundation/hosts.rs`（新增） | hosts 路径解析、按字节读取、区块渲染、冲突检测、原子写入、只读/权限判定、flushdns。无业务规则 |
| 底座 | `crates/tl-core/src/foundation/atomic.rs`（新增或从 `store` 抽出） | 「同目录临时文件 + 原子替换 + 可重试」，hosts 与 JSON 共用；`ReplaceFileW` 保 ACL |
| 底座 | `crates/tl-core/src/foundation/timeout.rs`（新增） | 统一 `with_timeout` / `Deadline`；**替换 engine 现有 6 处裸 `tokio::time::timeout`**（`engine.rs` 208/266/336/342/372/665） |
| 底座 | `crates/tl-core/src/foundation/tls_ca.rs`（新增） | 本地根 CA 生成/复用/DPAPI 落盘、叶子现签与缓存（SAN 限定受管域名）、根证书导出。**不做信任探测**（SYSTEM 判不出用户根库，见 §3.4） |
| 模型 | `crates/tl-core/src/models/tunnel/`（扩展） | `effective_addr` 唯一口径、`domain` 校验、`domain_host` 分配与回收、`(host, port)` 唯一登记、入口模式判定（①/②/③，拒绝「本地 http + 远端 https」）、`domains()` 查询 |
| 模型 | `crates/tl-core/src/models/tunnel/engine.rs`（扩展） | **新增 `EntryConfig`**（入口模式 + 目标 + keepalive），不动 engine 里既有的 `ServeConfig`（那是 SSH 连接参数，`probe`/`open_session` 在用）；`serve_stream` 可注入 seam（见下）；② 用 `TlsAcceptor`（ALPN 只 h1） |
| 业务 | `crates/tl-core/src/business/hosts_sync.rs`（新增） | 编排：收集域名 → 渲染 → 尝试写 → 标记待同步；计算代理例外 add/remove 与管理集落盘；启动自检；`clear_hosts` / `clear_proxy` |
| 业务 | `crates/tl-core/src/business/mod.rs`（扩展） | 补 `remove_tunnel`（stop → remove → sync_hosts）；`create/patch_tunnel` 内触发同步 |
| 接口 | `crates/tl-core/src/interface/mod.rs`（扩展） | §6 端点按批落地：B3 为 hosts 读写，B5 为 proxy-result/clear，B6c 为 gateway/ca；零规则 |
| 壳 | `crates/tl-core/src/main.rs`（扩展） | `hosts sync` 子命令：前置返回、数据目录固定默认值、重解析点校验、结果文件 |
| Agent | `src-tauri/src/main.rs`（或新增 `wininet.rs`） | 注册表读写 + `InternetSetOption` 广播（复用 §7.1 纯函数）、`certutil -user` 装根/查库/删根、runas + 结果文件轮询 |
| 前端 | `src/api/*`、`src/models/*`、`src/views/HomeView.vue`、`src/components/ForwardCard.vue`、`src/views/SettingsView.vue`、`src/App.vue` | 表单（两个下拉 + 禁止组合）在 HomeView；卡片在 ForwardCard；`App.vue` 的 `onChange` 补 `hosts.refresh()`；设置页加证书卡片 |

**engine seam（② 能单测的前提）**：

- 新增 `EntryConfig { mode: EntryMode, target: (String, u16), keepalive_secs }`，把 `serve_listener` 的 8 参收敛成 2–3 个结构体参数（现签名已挂 `allow(too_many_arguments)`，再堆一个只会更难改）。**不要复用 `ServeConfig`**：engine 里已有同名的 6 字段 SSH 连接结构（`server_host`/`server_port`/`trusted_fingerprint` 等），`probe`、`open_session`、`business::probe_server`、`tunnel_runner` 都在构造它。
- 抽出 `serve_stream(stream, mode, open_channel)`：**上行只暴露「给定一条已接受的流，返回一条到远端的流」**，单测用 `tokio::io::duplex` + 假远端 h1 echo 覆盖 ①②③ 与 ALPN，**不需要真实 SSH**（本机外部 SSH 仍被安全软件拦着，不能把三格验证全押真机）。
- TLS 握手在 accept 分支内 `tokio::spawn` + `foundation::timeout`（10s，可配置）；失败只关该连接，`inflight` 必须 `fetch_sub`，不计泄漏。
- `serve_listener` 仍返回 listener（退避期快速拒绝的既有语义不变）。

**依赖清单**（关键坑）：

- `tl-core`：`tokio-rustls` + `rcgen` + `rustls-pemfile`，**全走 ring 后端**（`default-features = false` + `ring`，并**显式开 `tls12`**——它是 default feature，关了就没有 TLS 1.2）；默认 aws-lc-rs 需要 NASM，本机没有。只需服务端能力，不引入出站 TLS 客户端。
- `tl-core` 的 `windows-sys` 补特性：`Win32_Security`（`GetTokenInformation`）、`Win32_Storage_FileSystem`（`ReplaceFileW`）、`Win32_System_Threading`。
- `tl-agent`：新增 `tl-protocol`（**仅契约，无业务、无 Win32**）。**这会推翻 `src-tauri/Cargo.toml` 里「刻意空 workspace、不与 `crates/*` 共享依赖解析」的既有声明**，B5 提交时必须同步改那段注释并写明理由：共享的只是零依赖的 DTO 与 reconcile 纯函数，Agent 仍不依赖 `tl-core` 业务层。
- `tl-agent` 的 `windows-sys` 增加 `Win32_NetworkManagement_WinInet`；不引入 `rustls`——信任判定用 `certutil -user -store Root <thumbprint>` 查库（v1 只查指纹在不在，不做验链）。

机检注意：

- R1：`foundation` 不 import 上层；`interface` 不出现规则。
- R4：写 hosts / 读注册表只用非 UI 的 windows 特性；`rustls`/`rcgen` 不算 GUI。
- R5：壳层不出现 `apply*`/`spawn*`。
- 注意 `tls_ca` 若被 `engine` 横向调用，属同层（models → foundation）合法。

## 11. 测试计划

单元测试（cargo test）：

1. 域名校验：合法/非法/大小写归一/末尾点/长度/保留名/非 ASCII。
2. IP 分配：首次分配、复用、删除后回收、不与他人 `local_host` 撞、池耗尽报错、非域名转发不得占用 `127.0.0.2–254`。
3. `effective_addr` 与 `(host, port)` 唯一：不同回环同端口允许；同 host 同 port 拒绝；**`patch` 改域名/端口后旧键被正确释放**（快照旧地址）。
4. **存量兼容回归**：`{"scheme":"https"}`（无 domain、无 remote_scheme）→ 纯字节管道，行为与 v1 逐字节一致；带 domain 但无 remote_scheme → 由 `scheme` 推导（`https`→③，不是②）。
5. hosts 区块渲染：空配置（连标记一起删）、多条、幂等（连续两次结果一致）。
6. hosts 块外内容保留：含用户自定义行、注释、CRLF、BOM、非 UTF-8 字节（按字节比对，写回不变）。
7. 损坏区块：开始标记在、结束标记丢 → 只摘自己的行，后续用户条目原样保留。
8. 冲突检测：块外已有同域名 → 报冲突且不写入；只读属性 → 单独文案。
9. 域名清空 → 该条从区块移除、地址释放。
10. **权限探测不写坏 hosts**：探测「能不能写」前后 hosts 字节完全一致（禁止截断式探测）。
11. 原子替换重试：模拟 `SHARING_VIOLATION` 后释放 → 重试内成功；持续失败 → 待同步且文件未被破坏。
12. 代理例外 reconcile：只加/删托管项、用户项与 `<local>` 原样保留、幂等、空集清理、超长拒绝、无 `ProxyOverride` 值时新建、管理集被 `applied` 覆盖。
13. `hosts sync` 参数面：`--data-dir` **只接受 `default_data_dir()`**，其它路径退出码 2（明确不用 `install.json` 做白名单，避免自证）；拒绝其它参数；含重解析点的目录直接拒绝；子命令分支先于 `retire_legacy_data` 返回。
14. 本地 CA：生成幂等复用、`pathlen:0`、叶子 SAN 含域名与回环 IP、7 天有效期、过期重签、按 SAN 集缓存命中、**SAN 越界拒绝**。
15. 三格模式判定：`tcp` 永远直通；`http/http` 纯字节；`https/http` 走 TLS 服务端；`https/https` 纯字节；`http/https` **拒绝保存**。
16. 三格链路各一条端到端测试（`serve_stream` + `tokio::io::duplex` + 假远端，不依赖真实 SSH）：
    - ① 回环 echo 字节一致；
    - ② TLS 客户端（`rcgen` 测试根）写入 → 明文服务端读到原文；
    - ③ 字节原样透传。
17. ALPN：② 只提供 `http/1.1`（断言协商结果不含 h2）。
18. CA 信任上报：Core 不自行判断；`POST /gateway/ca/trust-result` 后状态与时间更新；伪造 `trusted=true` 不影响任何写入行为（只影响展示）。
19. 握手超时：连上但不发 ClientHello → 10s 内断开，`inflight` 归零。
20. URL 拼接：80/443 省略端口、其它端口保留、`open_path` 追加、`tcp` 不生成 URL。
21. `business::remove_tunnel`：删除后该域名从 hosts 区块消失、地址回收、监听已停。
22. 事件通知：hosts 写入结果 / proxy 结果 / UAC 取消后 `/events` 有事件，且前端 `onChange` 触发 `hosts.refresh()`（横幅**真的消失**，不只验证事件发出）。
23. 启动自检：手工往托管区块外或块内塞一条已删除域名的旧行 → 启动后按策略修复（可写则静默修，不可写则标记待同步且不弹 UAC）；`domain_host` 漂移（被别的转发占用）→ 重新分配并标记。
24. `POST /hosts/clear`：区块清空、块外不动、配置仍保留域名；随后 sync 能重新写回。

msocheck：`node tools/msocheck/msocheck.mjs --selftest` 与正式检查。

真机验证：

**B0 前置探针（真机一次性）**：用探针程序写一次 hosts 托管区块、装一次 `certutil -user` 根，确认当前安全客户端（火绒/360 等）不拦截；若拦截，先取得放行规则再进入实施——否则 B2/B6 会在真机门禁上反复卡住。

- **①（http→http）**：`http://域名` 访问成功。
- **②（https→http）**：远端明文 + 本地 CA 装根后 `https://域名` 访问成功，浏览器无证书告警（curl `--cacert root.pem` 验链通过）；确认没有协商出 h2。
- **③（https→https）**：`https://域名`（远端真证书）访问成功，证书无告警，HTTP/2 正常。
- 不装根时 ② 给出明确引导，不静默失败；装根一次后所有 ② 域名共享。
- 装根时出现 **Windows 系统安全警告**，点「是」后成功；点「否」界面显示「已取消」且不反复弹。
- ③ 的远端自签场景：浏览器告警属预期，界面提示「远端证书不受信任，建议在远端换真证书」。
- 两个域名同时用 443，互不干扰。
- 转发停止后访问域名 → 连接被拒绝，不落到其它服务。
- UAC 提权一次写入，二次保存不再弹；点「否」提示「已取消」而不是失败。
- hosts 块外已有同域名 → 冲突提示且原行不被改动。
- 本机有程序占 `0.0.0.0:443` 时 → 错误里带实际地址与端口，不只说「端口被占用」。
- **系统代理开启时**，`https://域名` 仍可访问（代理例外生效，浏览器不重启）；关闭代理或删除域名后例外条目被移除，用户自有例外与 `<local>` 不受影响。
- 卸载后：hosts 区块消失、代理例外被清、用户根证书被删（或给出明确的手工命令）。

## 12. 风险与未决

| 风险 | 处置 |
|---|---|
| 本机已有进程绑 `0.0.0.0:443`，回环 443 整片不可用 | 报明确错误，建议换入口端口（域名带端口访问）或停掉占用者。与 joyshare 的处置一致 |
| 公网真实域名被劫持到本机，隧道未开时该域名在本机不可用 | 设计内行为；界面标明；提供「清理托管区块」 |
| 浏览器/系统 DNS 缓存导致改 hosts 后短时不生效 | 写入后 flushdns；文档提示必要时重启浏览器 |
| Chrome 的 HTTPS 优先/HSTS：域名若在 HSTS 列表内，http 访问会被升级 | 设计内行为，用户应用 https |
| 提权入口被滥用 | 只写回环地址、内容来自本机配置、不接参数（见 §7） |
| 同机多用户共写 hosts | 托管区块以「当前配置」全量重写；多用户同时用 threadline 会互相覆盖（与 joyshare 同限制），v1 不处理 |
| SYSTEM 核心写不到登录用户的 HKCU | 代理例外固定由 Agent 执行（见 §7.1）；Agent 未运行时降级为提示 |
| 系统用 PAC 脚本，`ProxyOverride` 不生效 | 检测 `AutoConfigURL` 并提示在 PAC 加直连规则；不尝试改写 PAC |
| ③ 下远端证书域名不匹配或自签 | 浏览器告警属预期；v1 不做双段兜底，建议远端换真证书或把证书装进系统信任 |
| 条目过多导致 `ProxyOverride` 超长 | 超过 1024 字符提示，不自动归并（避免误伤同后缀域名） |
| ② 不支持 HTTP/2 | 设计内（只宣告 h1）；要 h2/gRPC 只能走 ③，或 v2 引入真正的 HTTP 反代 |
| v1 不支持入口改名 | 入口域名必须等于远端 `server_name`；界面提示，必要时在远端加 `server_name` |
| 根私钥放在 `%PUBLIC%`，同机其他用户可能读到 | DPAPI machine scope 加密：**同机其他用户可解密**（machine key 机器级）、别机不行；多用户共机本就是 v1 记录在案的限制（见 TODO「多人共用机器」）。要彻底解决需收紧数据目录 ACL（会重新引入 UAC） |
| 卸载残留（hosts / 代理例外 / 根证书） | §7.2 明确四步清理；清理失败不阻塞卸载，但要留提示 |
| 同 exe 的 `--fix-acl` / `--autostart` 仍接受任意 `--data-dir` | 既有暴露面，本次不收敛；§7 已写明，B4 提交说明里标注，后续单独处理 |
| 域名带非标准端口时 URL 需带端口 | 设计内；卡片与「打开」按 §8 的拼接规则显示 `域名:端口` |
| 本地根一旦泄露可签任意域名 | `pathlen:0` + 7 天叶子 + DPAPI；与 mkcert / joyshare 同型风险论证（§3.4） |
| 用户不装根就开终止模式 | 浏览器证书告警是可见的；界面在开启前就提示并提供一键安装，不静默 |
| 叶子过期（7 天）时进程未运行 | 下次启动按 SAN 集重签；已建立的连接不受影响 |

## 13. 实施批次（建议）

- **B0 前置探针**：真机确认安全客户端放行 hosts 写入与 `certutil -user` 装根。不通过先解决环境，不写业务代码。
- **B1 模型与契约**：`TunnelDto` 新字段（`Option<String>` + `effective_remote_scheme`）、域名校验、IP 分配、`effective_addr` 唯一口径、`(host, port)` 全部调用点改造（`HashMap<u16,_>` → `HashMap<(String,u16),_>`，含 `start`/`apply`/`patch`/`release_*`/`ensure_unique`/`bind_local`/`remove`）、`create` 的 load→分配→校验顺序、单测。**这是「改存量」最重的一批**：`TunnelDto` 的 struct literal 在 tl-core 有 4 处（`models/tunnel` ×3、`tunnel_runner` ×1），另需同步改 `tl-protocol` 与 `src/api/types.ts` 里「scheme 只影响界面」的注释（编译器抓不到）。
- **B2 底座**：`foundation/hosts.rs` + `foundation/atomic.rs` + `foundation/timeout.rs`；区块渲染 / 按字节读写（保 BOM 与换行）/ 损坏区块 / 冲突 / 只读判定 / 替换重试 / flushdns + 单测。**B2 后跑一次真机门禁**（写一次托管区块并还原）。
- **B3 业务与 API**：`sync_hosts` 编排、启动自检、`business::remove_tunnel`、事件通道升级、`GET /hosts`、`POST /hosts/sync`、`POST /hosts/clear`。
- **B4 提权链路**：`hosts sync` 子命令（前置返回、数据目录固定默认值、重解析点校验、结果文件）、Agent runas + 结果轮询 + UAC 取消；提交说明标注 `--fix-acl`/`--autostart` 未收敛。
- **B5 代理绕过**：`tl-protocol::proxy_override` 纯函数 + 单测、Agent 注册表执行器、`POST /hosts/proxy-result`、`POST /hosts/proxy-clear`、PAC 检测；同步改 `src-tauri/Cargo.toml` 的 workspace 注释。
- **B6a CA 底座**：`foundation/tls_ca.rs`（生成/复用/签叶子/缓存/导出/SAN 限制）+ 单测。
- **B6b ② 链路**：engine 的 `EntryConfig` / `serve_stream` seam、`TlsAcceptor`、三格判定、duplex 测试。
- **B6c CA 接口与 Agent**：`GET /gateway/ca`、`root.pem`、`reset`、`trust-result`；Agent 安装/查库/导出；系统警告与无桌面降级。**B6c 后跑一次真机门禁**（装根 → ② 访问）。
- **B7 前端**：`HomeView.vue` 表单（两个下拉 + 禁止组合 + 域名模式下地址只读 + 端口按格默认）、`ForwardCard.vue` 卡片、横幅、`App.vue` 的 `onChange` 补 hosts 刷新、`SettingsView.vue` 证书卡片；TESTPLAN 与文档收口。

每批独立提交；最终 ①②③ 各一条链路必过。
