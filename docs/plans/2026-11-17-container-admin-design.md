# 容器管理 · 设计（已拍板）

> 状态：**已拍板**（2026-11-17）。引擎范围暂定**只认 docker**，podman / nerdctl 留到以后。
> 上游：`docs/plans/2026-11-16-server-admin-design.md`（概览已上线，容器模块现在只占位）
> 实施计划：`docs/plans/2026-11-17-container-admin-plan.md`
> 原型：`tmp/misc/container-admin-prototype.html`（交互与文案以它为准）

## 目标

概览已经能回答「这台机器上有几个容器、跑了几个」。容器管理要回答接下来的问题，并且能在页面里把事做完：

1. 现在跑着哪些容器，谁占了 CPU / 内存，端口映射到哪；
2. 某个容器为什么起不来（状态、退出码、日志最后几行）；
3. 不用登服务器就能把容器停掉 / 重启 / 启起来；
4. 出问题时能当场看日志，而不是先转发一个端口。

不做的事写清楚：**不编排**（不替用户写 compose、不改 compose 文件）、**不构建镜像**、**不管理卷与镜像**（只读展示）。

## 三种做法与取舍

| 方案 | 内容 | 代价 | 结论 |
|---|---|---|---|
| A 只读观察台 | 列表 + 详情 + 日志 | 不满足「管理」二字，用户还是要 SSH 敲命令 | 不够 |
| **B 观察 + 单容器生命周期**（推荐） | 列表 / 详情 / 日志 + 启动 / 停止 / 重启 / 删除 | 需要写操作契约、二次确认、危险动作分级 | **本期做这个** |
| C 全功能 | 再加 compose 项目视图、exec 进容器、镜像与卷管理 | exec 与终端模块重叠，镜像卷管理是另一个大模块 | 不做（compose 视图留 P3b 观察项） |

方案 B 的边界一句话：**能改「这个容器的运行状态」，不能改「这台机器上的东西是什么」**。删除容器不删卷、不删镜像；`docker rm -v` 与 `docker rmi` 一律不提供。

## 接口契约

沿用既有前缀，写操作单独成端点，不与「采集」混在一条里：

```
GET  /api/v1/servers/{id}/admin/containers?all=1&stats=1
→ ContainerList {
    engine, version, collected_at, elapsed_ms,
    total, running, paused, stopped,
    items: ContainerItem[],
    degraded: string[]
  }

GET  /api/v1/servers/{id}/admin/containers/{cid}
→ ContainerDetail          // inspect 摘要：命令、端口、挂载、网络、重启策略、退出码

GET  /api/v1/servers/{id}/admin/containers/{cid}/logs?tail=200&since=<rfc3339>
→ ContainerLogs { lines: string[], truncated: bool, next_since: string }

POST /api/v1/servers/{id}/admin/containers/{cid}/action
body { "action": "start" | "stop" | "restart" }     // 不做 pause/unpause，传了报 400 invalid_action
→ ContainerActionResult { ok, message, state }

DELETE /api/v1/servers/{id}/admin/containers/{cid}?force=0|1&image=0|1
→ ContainerActionResult

GET  /api/v1/servers/{id}/admin/containers/{cid}/console?mode=exec|root|debug&cols=100&rows=30&token=…
→ WebSocket（令牌走 query：浏览器不能给 WS 设请求头；只有这一条路由认 `?token=`）
服务端 → ← {type:"hello", command:"docker exec …", mode:"exec"}
          ← 二进制帧（PTY 原始字节，不 base64）
          ← {type:"exit", code:0} / {type:"error", message:"…"}
客户端 → {type:"input", data:"ls\n"} / {type:"resize", cols:120, rows:40} / {type:"close"}
```

契约要点：

- `cid` 只接受**本次会话采集到的清单里存在**的 ID 或名字，另加正则白名单 `^[A-Za-z0-9][A-Za-z0-9_.-]{0,79}$`。两重校验缺一不可：正则挡住 shell 注入，清单校验挡住「猜一个名字乱删」。
- `ContainerItem` 在现有 `ContainerRow` 上补：
  - `ports[]`：`{ host_ip, host_port, container_port, proto }`。**宿主 IP 是必填**——`0.0.0.0` 与 `127.0.0.1` 决定这个端口是否对外，不能只留端口号；
  - `mounts[]`：`{ source, destination, mode: "rw"|"ro" }`（`docker inspect` 的 `Mounts`，匿名卷 `source` 是卷名不是路径，前端据 `type` 标注）；
  - `networks[]`：`{ name, ip }`（网络名 + 容器在该网络里的 IP，多网络全给）；
  - `cpu_percent` / `mem_bytes` / `mem_limit_bytes`（`stats=1` 时才有）、`compose_project`（label 取）、`health`、`created_at`、`exit_code`、`restarts`、`net_rx_rate` / `net_tx_rate`。
- 写操作返回 `ok` 与操作后的 `state`：编排层不猜「应该成功了」，一律以 `docker inspect` 回的 `.State.Status` 为准。
- 失败不吞：`docker stop` 卡住（stop 默认 10s 超时）要给出「仍在运行」而不是 200。
- `console` 的 `mode` 由前端选，核心只做白名单映射（`exec` / `root` / `debug`），不接受任意命令串；`cols` / `rows` 在核心侧夹到 20–400 / 5–200。
- `image=1` 的顺序是**先删容器、再处理镜像**：
  1. `docker rm [-f]`，失败就到此为止（容器还在，谈不上删镜像）；
  2. 再查这个 tag 还有谁用——此刻它自己已不在 `docker ps -a` 里，不必再做「排除自己」那层手工逻辑；
  3. 还有人就**不删**，回执写「镜像 `<tag>` 仍被 N 个容器使用（`a`、`b`），未删除」；
  4. 没人了才 `docker rmi`。
- **不能靠 `docker rmi` 自己报错**：它删的是 tag，多个容器共用一个 tag 时会把 tag 删掉并返回成功，别人的「镜像」随即变成悬空引用（真机验收撞到）。
- 也不能用 `--filter ancestor` 判断占用：它按镜像 ID 匹配，会把「用同一镜像层的别的 tag」也算成占用——`redis:8.10.3` 与它的副本 tag 是同一个 image ID，于是副本 tag 永远删不掉。要判的是 `docker ps -a --format '名字|镜像'` 里 `.Image` 的**字面相等**。
- 镜像 tag 之前已被删过（`rmi` 回 `No such image`）不算失败：容器照样删掉了，回执写「镜像已不存在」。

## 采集脚本

| 动作 | 命令（一次往返，单引号包裹内不得出现单引号） |
|---|---|
| 引擎探测 | `command -v docker \|\| command -v podman \|\| command -v nerdctl`，`<engine> version --format ...` |
| 列表 | `<engine> ps -a --format '<id>\|<names>\|<image>\|<state>\|<status>\|<ports>\|<created>\|<label compose.project>'` |
| 资源 | `<engine> stats --no-stream --format '<id>\|<cpupct>\|<memusage>\|<netio>'`（单独一条接口，3s 一档） |

两处真机踩过的坑，写进契约免得再犯：

- **id 一律截到 12 位**。`ps -a --no-trunc` 给 64 位，`stats` 只给 12 位，两档数据要按 id 合并，长度不先对齐就只能靠前缀猜。`resolve_cid` 两个方向都认前缀（用户可能粘 8 位，也可能粘完整 64 位）。
- **env 必须自带分段标记**（`@@c_env`）。detail 脚本的第一行是 `状态|退出码|重启次数|策略|命令|镜像`，命令里常带 `=`（`--default-time-zone=Asia/Shanghai`），没有标记时它会被当成第一条环境变量混进列表。
| 详情 | `<engine> inspect --format '<json 模板>' <cid>`（只取需要字段，不整段回传） |
| 网络与卷（批量） | `<engine> inspect --format '<name>\|<net名=ip 列表>\|<Source=>Destination:mode 列表>' $(<engine> ps -aq)`，一次拿全部容器的网络 IP、卷映射与健康状态 |
| 日志 | `<engine> logs --tail 200 --timestamps <cid> 2>&1`；跟进用 `--since <next_since>` |

三条纪律：

1. **`stats` 不跟着列表每秒跑**。一次 `docker stats --no-stream` 要 1–2 秒（它内部有采样窗口），30 个容器时更慢。列表 5 秒一档且默认**不带** stats；用户点开「资源」列或详情抽屉时才按 3 秒采，抽屉关掉即停。
2. **`inspect` 只在需要时调**。列表已经给足了名字/镜像/状态/端口，`inspect` 留给详情抽屉。
3. **日志带行数上限**。`--tail` 默认 200、上限 1000；返回 `truncated` 让界面能说「只显示最后 200 行」。

## 前端交互

```
┌ 容器 ──────────────────────────────────────────────────────────────────────────┐
│ docker 27.3.1 · 15 个 · 10 运行 · 3 退出   [搜索] [全部|运行中|已停止] [主要|详细] │
├──────────────────────────────────────────────────────────────────────────────────┤
│              容器                镜像 · 网络                 端口 · 卷          资源            │
│ ● immich_server   immich-app/immich-server:v1.119.0   0.0.0.0:2283→2283/tcp  CPU   2.4%      │
│   Up 3 days      网络 immich-net 172.18.0.4          卷 /mnt/…/immich → /data MEM  412 MB · 5% │
│   a1f93c2e7b10   创建 2026-11-03 21:04               卷 /mnt/…/photos → /photos:ro NET ↑812 ↓34 kB/s │
│   · immich                                                                  重启   0 次       [⋯] │
└──────────────────────────────────────────────────────────────────────────────────┘
```

- **一行一个容器**，与概览的磁盘/网络同一套「一项一行」语言；行内不放环形表（容器多时一圈圈看不清），CPU/内存只给数字与细进度条。
- **视图密度开关**（工具条右侧「主要 / 详细」）：
  - **主要**（单行 7 列）：状态点 · 名称（副行写状态）· 短镜像名（去 registry、去 org，如 `immich-server:v1.119.0`）· 主入口（多映射折成 `+N`）· CPU · 内存 · 「⋯」。行高 8px。
  - **详细**（四块，块内纵向堆叠、块间左对齐）：
    - **身份**：名称 / 状态副行（`Up 3 days (healthy)`）/ `短 ID · compose`。三行都贴同一左边界。
    - **镜像 · 网络**：`org/repo:tag`（去 registry，repo 压暗、**tag 提亮**；宽度不够先省略 repo，tag 必须可见）+ `网络 名 IP` + `创建 时间`。
    - **端口 · 卷**：端口逐行 `宿主IP:端口 → 容器端口/协议`（IP 压暗、端口提亮）；卷逐行 `卷 宿主路径 → 容器路径`（只读带 `:ro`）；都没有时写灰字「没有端口映射」。
    - **资源**：`CPU` / `MEM 用量 · 占比` / `NET ↑累计收发` / `重启 N 次` 四行，label 定宽 26px，数值左对齐。详细视图**不放进度条**——四条长短不一的条子会把数字挤成一片；条只留在主要视图。NET 是**累计字节**不是速率（`docker stats` 只给累计值，核心无状态，不缓存上一拍）。
  - 为什么四块而不是「主行 + 元信息行 + 卷行」：三层各画一套栅格，主行的列边界和下面两层对不上，一屏里两套网格互相打架；四块共用同一套 6 列，块内自己纵向排，行高反而更矮（约 75px，一屏 6–7 个）。
  - 为什么一定要带 IP：`0.0.0.0:2283` 与 `127.0.0.1:6379` 是两种访问边界，只写「2283」看不出这个端口是否对局域网开放。
  - **去 registry**：十行里九行同一个源（`ghcr.io/`），留着只把 repo 挤成省略号；完整镜像名放在 `title` 里。
  - 表头按档切换（主要 7 列 / 详细 6 列），列含义变化时表头必须跟着变，否则「资源」标题下面只有 CPU 会让人找内存。
  - 偏好存 `localStorage` 的 `tl.containers.density`（与主题同层，核心不参与），默认「详细」。
  - 密度只影响列表，不改变抽屉与采集内容：两种视图读的是同一份数据。
- **状态点语义**：运行绿 / 暂停黄 / 退出灰 / 反复重启黄闪 / `unhealthy` 红点加感叹号（健康检查失败比「在运行」更值得先看到）。
- **行内只有一个「⋯」**：所有动作（含删除）都在菜单里，菜单项随状态变：
  - 运行中：重启 / 停止 / ——— / 进入控制台 / 查看日志 / 详情 / ——— / 删除容器
  - 已停止：启动 / ——— / 查看日志 / 详情 / ——— / 删除容器
  - 暂停（被命令行 `docker pause` 冻结的）：只有停止 —— 本产品不做暂停，也就不给「继续」；停掉再启动是唯一出路
  - 删除永远是最下面一条分隔线之后的单项；不放在行上，也不和只读入口平排——行内按钮越少，误点的机会越少。
- **删除确认框**：写清 `docker rm` 还是 `docker rm -f`（运行中的先强制停止）；下方一个**默认不勾**的选项「同时删除镜像 `<tag>`（`docker rmi`）」。数据卷永远不删。勾选后成功后回执写明「容器与镜像一并删除」；镜像被其他容器占用时回执写「容器已删除，镜像 `<tag>` 仍被 N 个容器使用（`a`、`b`），未删除」——把名字列出来，用户才知道该先处理谁。
- **详情抽屉**（右侧 440px，不跳页）：Tab = 详情 / 日志 / 控制台 / 环境。详情里有运行信息（网络收发、重启次数、健康检查、挂载数）；日志页签带「跟随」开关，用 `--since` 轮询，不整段重拉。
- **写操作进行中按钮转 loading 并禁用整行**，避免连点 `restart`。

## 控制台（exec 进容器）

进入容器不止一条路，能力与风险差别很大：

| 方式 | 命令 | 适用 | 取舍 |
|---|---|---|---|
| exec + shell（默认） | `docker exec -it <c> bash` | 镜像里有 shell | 最轻，不碰主进程；探测顺序 bash → sh → ash |
| exec + 指定命令 | `docker exec -it <c> <cmd>` | 没有 shell（distroless / scratch） | 只能跑单条命令，无交互 |
| exec 切 root | `docker exec -it -u root <c> bash` | 装包、改文件、看 root 才能看的路径 | 权限提升，界面上是独立一档，不做默认 |
| 调试图（ephemeral） | `docker run --rm -it --pid=container:<c> --net=container:<c> --volumes-from <c> busybox sh` | 镜像里连 sh 都没有 | 会在宿主机拉一个 busybox；退出即销毁 |
| nsenter | `nsenter -t <pid> -m -u -i -n -p sh` | 上面都不行、且有宿主机 root | 需要 sudo，风险最高，本期不做 |
| attach | `docker attach <c>` | 只看主进程 stdout | **不提供**：会抢走主进程 stdin，`Ctrl+C` 可能直接把容器停掉 |
| 其他引擎 | `podman exec` / `nerdctl exec` / `crictl exec` | 非 docker | 与 exec 同形态，按探测到的引擎替换命令头 |

产品化决定：抽屉「控制台」页签给三档 `exec` / `root` / `调试图`，三种都不需要用户敲命令；`attach` 与 `nsenter` 不做（前者危险、后者要宿主机 root）。控制台只在容器 `running` 时可用，其他状态直接说明原因。

调试图有个前置条件：宿主机得能拉到 `busybox`。国内直连 docker.io 经常超时（真机验收时是 `dial tcp 108.160.167.174:443: i/o timeout`），表现为进控制台后立刻看到这段英文错误——PTY 把远端 stderr 如实送到终端，用户能看懂原因，但不知道怎么解决。要在国内用这一档，先给 docker 配镜像加速，或手动 `docker pull docker.m.daocloud.io/library/busybox:latest && docker tag … busybox:latest`。功能本身已验证：把 busybox 补进本机后，`cat /proc/1/comm` 打出的是**目标容器**的 `redis-server`，说明 `--pid` / `--net` / `--volumes-from` 三个命名空间都借对了。

实现要点：

- 复用隧道已有的 SSH 会话，开一个 **PTY channel**：`request_pty(term: xterm-256color, cols, rows)` + `exec(docker exec -it …)`，stdin/stdout 双向透传。现有 `engine::exec_capture` 是「跑完收结果」，这里新增 `engine::open_pty` 返回 `PtyChannel`，两者并存。读与写塞进同一个 `select!`（`Channel::wait` 要 `&mut`，`data_bytes` / `window_change` 只要 `&`）。
- 传输用 **WebSocket**：`GET …/containers/{cid}/console?mode=exec|root|debug&cols=&rows=`。帧：服务端发 `{type:hello}` / 二进制原始字节 / `{type:exit}`；客户端发 `{type:input}` / `{type:resize}` / `{type:close}`。输出走二进制而不是 base64——终端流里多字节 UTF-8 会被切在包边界上，二进制帧配 `TextDecoder({stream:true})` 才不会乱码。
- **令牌走 query**：浏览器不能给 WebSocket 设请求头。核心只在 `/console` 这一条路上认 `?token=`（`interface/auth.rs::console_query_token`），其余端点仍必须 `Authorization`。
- 一次只允许一条控制台会话：`business/admin.rs` 用一张 `server_id:容器名` 的占用表挡住，`ConsoleGuard` 在连接断开时自动摘除。两个人同时 exec 进同一个容器，输入会串到对方终端上，看起来像「键盘失灵」。
- 会话不落盘、不记录输入内容；核心日志只记「谁在什么时候对哪个容器开了控制台」。

### 前端：xterm.js（按需加载）

`@xterm/xterm@5.5.0` + `@xterm/addon-fit@0.10.0`。实测体积：`ContainerConsole` 独立 chunk **296.53 kB / gzip 75.13 kB**，CSS 4.88 kB / 1.92 kB。

- **不在首屏**：`ContainersPanel` 用 `defineAsyncComponent` 挂它——只有真的点开「控制台」页签才下载。主 chunk 前后都是 401 kB（gzip），等于没变。
- 逐键发送：`term.onData` 直发 `input` 帧，vim / htop / Tab 补全才成立（行模式做不到这些）。
- 尺寸跟随：`FitAddon` + `ResizeObserver`，`onResize` 转成 `resize` 帧发给远端，否则全屏程序按旧列数排版。PTY 初始尺寸取 `fit()` 后的真实值，拿不到才退回 100×30。
- 输出：二进制帧直接 `term.write(Uint8Array)`，不经过字符串解码，多字节字符被切在包边界上也不会坏。
- 档位切换、`重开`、`会话已结束` 组件自管；离开抽屉即 `close` 帧 + 断连，核心侧主动 `eof` 远端 shell（光断 WS 不会让 `docker exec` 退出）。

> 依赖环境备注：这台机器的 `node_modules` 是早前在上一级目录用 pnpm 10 装的（`virtualStoreDir` 指向不存在的嵌套路径），PATH 里的 pnpm 是 9，`pnpm add` 会报 store/virtual-store 冲突。正确做法是 `$env:CI='true'; corepack pnpm@10.34.7 install --frozen-lockfile` 重建一次，之后 `corepack pnpm@10.34.7 add …`。**不要**为了装包去重装整个依赖树之外的任何东西。

## 刷新与「不看就不采」

沿用概览的判据（有 serverId + 窗口有焦点 + `document.hidden` 为假 + 停在容器模块）：

| 档 | 周期 | 内容 |
|---|---|---|
| 列表 | 5s | `ps -a` **+** 批量 `inspect`，一次往返（IP、卷、健康都在这段里拿） |
| 资源 | 3s | `stats --no-stream`，仅详情抽屉打开时 |
| 日志跟随 | 2s | `logs --since <last>`，仅日志页签 + 跟随开启时 |

`ps` 给不了容器 IP 与卷映射，所以列表那段脚本把 `ps` 与 `inspect $(ps -aq)` 拼进同一次 `exec`——两次往返在弱网下会让「打开容器模块」明显变慢，而它们本来就该一起到。

写操作成功后**立即补一拍**列表，不等下一个 5 秒。

**资源没采到时显示 `—`，不显示 `0%`**：资源是单独一档，抽屉没打开时列表里的 `cpu_percent` 是 0。直接渲染会写成「0.0%」，用户读成「这个容器没占 CPU」，而真相是「还没采」。前端用 `hasStats(id)` 区分这两种状态，并附带一句 hover 说明；抽屉里首次打开的 1–2 秒显示「正在采集资源」。

## 安全边界

| 风险 | 处置 |
|---|---|
| 命令注入 | cid 正则白名单 + 清单校验；action 枚举；exec 的 `mode` 是枚举不是命令串；脚本正文禁止单引号（沿用 `sh -c '...'` 约束，单测覆盖） |
| 误删 | 删除收在「⋯」菜单最后一项，不与只读入口并排；只删容器不带 `-v`；运行中必须显式 `force=1` 且二次确认 |
| 权限不足 | 用户不在 `docker` 组时 `ps` 报 permission denied：界面直说「当前 SSH 用户无 docker 权限」，不显示空列表（空列表会被误读成「没有容器」） |
| 长命令阻塞 | `stop` 默认 10s 超时；exec 预算复用 `EXEC_BUDGET`，写操作单独给 20s 并允许前端超时后重查状态 |
| 控制台失控 | PTY 通道随抽屉关闭 / 隧道断开一起关闭；同一容器同时只允许一条控制台会话；不提供 attach（会抢主进程 stdin） |
| 审计 | 每次写操作与每次打开控制台各写一条核心日志（动作 / 容器名 / 结果），不额外落盘 |

## 分层归属

| 位置 | 职责 |
|---|---|
| `models/server_admin/containers.rs`（新） | 脚本、解析、动作白名单、exec 方式映射、状态机允许的动作集合（**规则都在这里**，便于单测） |
| `models/server_admin/mod.rs` | `containers` 模块状态从 `Planned` 改 `Ready`（有引擎时） |
| `models/tunnel/engine.rs` | 新增 `exec_pty`：`request_pty` + 双向 channel（与 `exec_capture` 并存） |
| `business/admin.rs` | 编排：会话 → exec → 解析 → 组装；写操作后回读状态；PTY 桥接 |
| `interface/mod.rs` | 新路由 + WebSocket 升级 + 错误翻译（404 容器不存在 / 409 状态不允许该动作 / 504 超时） |
| `tl-protocol` | `ContainerItem` / `ContainerDetail` / `ContainerLogs` / `ContainerActionResult` / `TermFrame` |
| `src/models/containers.ts`（新） | 列表/详情/日志/控制台四份状态、轮询调度、乐观更新（写操作后就地改状态） |
| `src/views/admin/ContainersPanel.vue`（新） | 列表 + 抽屉 + 确认框；`ServerAdminView` 只做 `v-if` 挂载 |

`server_admin` 仍然无持久状态。**新增一个前端依赖**：`@xterm/xterm` + `@xterm/addon-fit`（控制台渲染层，按需 chunk，不进首屏）。

## 分期

| 期 | 内容 | 本设计 |
|---|---|---|
| P3a | 列表（两档密度）+ 详情抽屉 + 日志（只读）+ `stats` 资源列 | 本期主体 |
| P3b | 启动 / 停止 / 重启 + 删除（行内按钮 + 确认） | 本期主体 |
| P3c | 控制台（PTY + WebSocket + 三档进入方式 + xterm.js 按需加载） | 本期主体，依赖 `engine::open_pty` |
| P3d | compose 项目分组视图、镜像/卷只读页、事件（`docker events`） | 观察项，不在本期 |
| P4 | 虚拟机管理（`virsh` 列表 / 生命周期 / 串口控制台） | 后续独立设计，见下节 |
| — | nsenter / attach | 不做（前者要宿主机 root，后者会抢主进程 stdin） |

## 虚拟机管理（规划，后续开发）

侧边栏位置：**紧跟「容器管理」下面**，同为「这台机器上跑着什么」的一组。容器是跑应用，虚拟机是跑整机，分开放但挨着。

与容器模块的差别（决定了它不能简单复用）：

| | 容器 | 虚拟机（KVM / libvirt） |
|---|---|---|
| 命令 | `docker` / `podman` | `virsh`（`libvirt-clients`） |
| 一等对象 | 镜像、容器、卷、网络 | 域（domain）、磁盘（qcow2）、网络桥、快照 |
| 资源观 | 共享内核，CPU/内存在运行时可采 | 独占 vCPU 与内存，宿主机视角看是进程，客户机内要 guest agent |
| 控制台 | PTY + `docker exec`，轻 | 串口 `virsh console` 或 VNC，VNC 要走 RFB 协议，重得多 |
| 破坏性 | 删容器不动数据卷 | 删域可能连带删磁盘文件，风险高一个量级 |

规划边界（P4 一期）：

- **只读优先**：`virsh list --all` → 域名、ID、状态、vCPU、内存分配、自动启动；`virsh dominfo` 补详情；`virsh domblklist` → 磁盘与路径。
- **生命周期**：`start` / `shutdown`（发给客户机，礼貌）/ `destroy`（拔电，必须二段确认并区分文案）/ `reboot` / `suspend` / `resume`。
- **控制台**：先做**串口**（`virsh console`，PTY 即可，复用容器控制台那套通道）；VNC 单独一期（需要 noVNC 或自研 RFB，前端体积另算）。
- **不做**：创建 / 克隆 / 删除虚拟机、磁盘扩容、快照管理、迁移——都要磁盘与存储池操作，和「管理面板」不是一回事。
- **权限**：`virsh` 需要用户在 `libvirt` 组或用 `sudo`；拿不到权限时与容器一样直说，不给空列表。
- **探测**：`virsh --version` 失败 → 模块 `Unavailable`，侧边栏项显示灰点，与容器模块同一套 `AdminModuleState`。

设计文档单独出一份（`vms-design.md`），本设计只定位置与边界。

## 降级矩阵

| 情况 | 表现 |
|---|---|
| 无 docker / podman / nerdctl | 导航「容器管理」标不可用，点进去一句话说明 + 安装提示 |
| 有引擎但无权限 | 页面顶部红条「当前用户无 docker 权限」，附 `sudo usermod -aG docker $USER` 提示 |
| `stats` 超时 | 资源列显示 `—`，`degraded` 加「容器资源」 |
| `logs` 为空 | 日志页签显示「没有输出（容器可能未启动过）」 |
| 容器里没有 shell | 控制台提示「没找到 bash / sh」，并引导切到「调试图」 |
| 控制台一条能多次开关 | 每次进入新开 PTY；关闭抽屉 / 断开隧道时先发 `close` 再断 |
| 容器在列表刷新间隙被删 | 详情请求 404 → 抽屉关闭并提示「容器已不存在」，列表补一拍 |

## 验收

1. 有 docker 的机器：容器模块可用，5 秒内出列表，运行/退出数量与 `docker ps -a` 一致。
2. 点行开抽屉：详情字段与 `docker inspect` 一致；日志 tail 200 行可读、可跟随。
3. 停止一个运行中的容器：二次确认后状态变 exited，列表 1 秒内反映；`docker ps` 复核一致。
4. 启动一个已停止容器：菜单变「停止」，状态回 running。
5. 无 docker 的机器：模块不可用，不出现空列表。
6. 非 docker 用户：明确报权限，不静默空表。
7. 任意 `cid` 注入尝试（`$(...)`、`;`、空格、引号）→ 400，脚本未执行。
8. 门禁：`bash scripts/verify.sh` 全绿；新增单测覆盖「动作白名单 + 状态机 + 脚本只读性/无单引号」。
9. 视图密度：切「主要」只留单行 7 列；切「详细」四块俱全（身份 / 镜像·网络 / 端口·卷 / 资源），端口带宿主 IP、卷带宿主 → 容器路径、网络带容器 IP、资源四行数值对齐；镜像列宽度不足时 repo 省略、tag 仍可见；关掉面板重开保持上次选择。
10. 删除：`⋯` 菜单最后一项 → 确认 → 列表消失、统计减一；运行中的容器在确认框里写明先强制停止；默认不勾选镜像时 `docker images` 里镜像仍在；勾选后镜像一并消失；镜像被其他容器占用时回执写明「未删除」而容器确实已删。
11. 控制台：`exec` 进入后能执行 `ls` / `pwd` 并看到真实输出；切 `root` 提示符变为 `root@… #`；`exit` 后显示「会话已结束」，重开可用；非运行中的容器不给控制台入口。
12. 详情抽屉：端口表分「宿主 IP / 端口 / 容器」三列，`127.0.0.1` 与 `0.0.0.0` 必须区分显示；卷映射表有宿主路径、容器路径与 `rw`/`ro`；网络表有网络名与容器 IP。
