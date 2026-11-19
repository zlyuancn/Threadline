# TODO（进行中 / 待办）

> 本文件只放**未完成**的工作。收口时整条搬入 `DONE.md`。
> 完成清单：`docs/DONE.md`；决策记录：`docs/decisions/`；回归战报：`docs/archive/`。

## 路线图

| 批次 | 状态 | 详情 |
|---|---|---|
| B7 收口（环境验证 + 安装实装） | 🚧 进行中 | `docs/TESTPLAN.md` |
| 安卓端 A0–A5（`-L` + 内置浏览器） | 🚧 A0 进行中（构建侧已通，真机三项待验证） | `docs/plans/2026-11-19-android-client-design.md`、`docs/plans/2026-11-19-android-client-plan.md` |
| 服务器管理 P2 · 终端 | 🚧 代码侧完成（2026-11-19），真机待验 | `docs/plans/2026-11-19-server-terminal-design.md`、`docs/plans/2026-11-19-server-terminal-plan.md` |

代码侧 B0–B6 已全部完成（见 `DONE.md`）。剩下的都是**只能在真机上做**的验证。

## 待办

### 安卓端（2026-11-19 立项；A0 构建侧已通，真机项待验证）

设计与计划：`docs/plans/2026-11-19-android-client-design.md`、`docs/plans/2026-11-19-android-client-plan.md`。
定位 **B 档**：App 内 `-L` 转发 + 内置 WebView，并让第三方 App 共用本地端口；不做 hosts / 域名 / 装根 / 自启 / 服务器管理；
C 档（DNS 劫持）留后续。技术栈：Kotlin + Compose + Rust 共享核心（UniFFI），**同仓** `android/` + `crates/tl-mobile`。

- [x] **E1 工具链（2026-11-19 完成）**：JDK 17（`D:\ruanjian\jdk17\jdk-17.0.21.1+2`，解压免安装）+ Android SDK
  （`%LOCALAPPDATA%\Android\Sdk`：platform-tools 37.0.1、platforms;android-35、build-tools 35.0.0、NDK 27.2.12479018）
  + Gradle 8.10.2（`D:\ruanjian\gradle`，工程内已生成 wrapper）。用户环境变量 `JAVA_HOME` / `ANDROID_HOME` / `ANDROID_SDK_ROOT` / `ANDROID_NDK_HOME` 已写。
- [x] **E2 交叉目标（2026-11-19 完成）**：`aarch64-linux-android` + `armv7-linux-androideabi`；`cargo-ndk 4.1.2`。
- [x] **A0.1 交叉编译（2026-11-19 完成）**：`russh 0.64 + ring + tokio-rustls` 经 `cargo ndk` 编出 `libtl_mobile.so`
  （release 6.33 MB，APK 7.43 MB）——`ring` 的 C 部分在 NDK 下无阻碍，**不需要退 `aws-lc-rs`，也不需要退 sshj**。
- [~] **A0.2 UniFFI 起一条 `-L`（构建侧完成）**：`crates/tl-mobile` 门面（`start_forward` / `Forward.state|stop|traffic|lastError`）
  + Kotlin 绑定（`android/app/src/main/java/uniffi/tl_mobile/tl_mobile.kt`，生成物入库）+ spike APK 构建通过；
  **剩真机**：装 `app-debug.apk` → 首次拿到指纹并核对 → `running` → 手机浏览器开 `127.0.0.1:8123` → 拔网 30s 回 `running`。
  宿主机 `cargo test -p tl-mobile` 已绿（状态词与桌面一致）。
  - 阻塞（**挂起中**）：`adb devices` 为空；2026-11-19 用户确认**暂时无法提供手机**，A0.2 真机三项与 A0.3 一并挂起，
    恢复条件 = 用户给出可调试设备（开 USB 调试）+ 一台真实 SSH 服务器（家里 sshd 的公网端口）。
    期间可做的不依赖手机的事：Keystore 桥（A1.3）、`atomic.rs` unix 分支、Compose 骨架、msocheck R7（A5.1）。
- [ ] **A0.3 WebView 打回环别名**：未开始（依赖 A0.2 真机通过）。
- [ ] A1 骨架：`tl-core` 安卓分流已提前落地一部分（`interface` 在 `cfg(not(android))` 之外的模块已能交叉编译、
  `secret` 给了明确报错的安卓分支）；剩下 Keystore 桥、`atomic.rs` 的 rename 分支、Compose 工程与前台服务。
- [ ] A2 转发管理：列表、增删改、启停、主机密钥确认、退避重连（复用现有模型）
- [ ] A3 浏览器入口：WebView 宿主、回环别名开关、cookie 隔离验证、明文白名单
- [ ] A4 前台服务：常驻通知、进程被杀恢复、Doze 引导、日志导出
- [ ] A5 门禁：msocheck 纳入 `crates/tl-mobile`（+R7 门面不得藏规则）、`verify.ps1` 增 NDK 与 Gradle 两关
- [ ] **门禁待补跑**：2026-11-19 改动 core + 新增 `tl-mobile` / `android` 后，`verify.ps1` 卡在
  「app is running: Threadline(14564), tl-core(3320)」——core 侧 fmt/clippy/test 251+11+前端 142+msocheck 已全绿，
  agent 侧四关与 NDK/Gradle 两关需关掉桌面 App 后补跑。

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

### 待开工（设计已定，按计划推进）

- [~] **容器管理**（引擎只认 docker）。设计 `docs/plans/2026-11-17-container-admin-design.md`，
  计划 `docs/plans/2026-11-17-container-admin-plan.md`，原型 `tmp/misc/container-admin-prototype.html`。
  - [x] P3a 只读：列表（两档密度）+ 详情抽屉 + 日志跟随 + 资源档
  - [x] P3b 生命周期 + 删除（行内「⋯」菜单、可选删镜像、回执）
  - [x] P3c 控制台：`engine::open_pty` + WebSocket 帧 + 三档 exec + xterm.js（异步 chunk，不进首屏）
  - [x] 真机冒烟：`devb` 上全过（只读 18 / 控制台 4 / 写操作 13 / debug 档 3 / 删镜像 8），
        写操作与 debug 都用自建 `tl-wt-*` 容器与副本镜像 tag，用户容器未动
  - [ ] 调试图依赖本机有 `busybox`：国内直连 docker.io 会超时，需要镜像加速或手动
        `docker pull …/busybox && docker tag … busybox:latest`；要不要在界面里给出这句提示待定
  - [ ] `scripts/verify.sh` 的 agent 侧需先关掉运行中的 Threadline
- [ ] **虚拟机管理**（规划，P4）。侧边栏位置在「容器管理」下面，只做 `virsh` 列表 / 生命周期 / 串口控制台；
  创建 / 删除虚拟机、磁盘扩容、快照、迁移不做。细节见容器管理设计文档末节，届时单独出设计。

### 本机访问（代码已完成，剩真机验证）

- [ ] **[环境]** 把本机一个真实程序接上域名跑通：`app.zly.m` → `127.0.0.1:3000`，含 hosts 写入与
      「https 入口 + 本地 CA 终止回源 http」两档；顺带确认核心重启后 `auto_start` 的本机转发自动恢复。
- [ ] 观察项：本机目标端口没开时，浏览器看到的是「连接被重置」——要不要额外给一句话错误页（现在只有
      `last_error` 记一句「无法连接 127.0.0.1:3000（本机）」）。
- [ ] 视需要更新 README 功能表与 `docs/images/` 截图（侧栏多了「本机」内置条目）。

### 体验与健壮性（可自行推进）

- [ ] 打开通道超时（现固定 10s）做成可配置：慢速网络下需要更长。
- [ ] 转发目标长期不回包时，状态里给出「疑似目标不可达」的提示（现在只有超时后的 last_error）。
- [ ] 凭据编辑目前不能改类型/登录用户（属身份），若确需变更应给出"新建并切换"的引导。

### 桌面壳层（2026-11-16 完成）

- [x] 设置 → 全局快捷键加开关「呼出时回到「应用」页」（默认开）。开：热键 / 托盘 / 再次启动都回到
      「应用」；关：停在上次所在页。冷启动无论开关都先显示「应用」（它就是这个应用的首屏）。
      偏好存 `localStorage`（与主题同层），核心与助手都不参与。
- [x] 进入服务器管理页时左侧栏自动收起（宽度过渡到 0 + `inert`），返回隧道页自动展开。
      管理页自带模块导航，再叠一条侧栏会把内容挤成两条细缝。

### 应用页布局（编辑模式，2026-11-16 完成）

- [x] 应用页「编辑」开关：拖动磁贴改顺序与归属、组头按钮（置顶/上移/下移/置底）调整分组顺序、
      点组名就地改名、删组（成员回落未分组）、新建空组、磁贴上的分组下拉可选已有或直接输入新名字建组。
      设计：`docs/plans/2026-11-16-app-page-edit-mode-design.md`。
      数据：`TunnelDto.app_order`（只影响应用页排列）+ `app_groups.json`（组顺序与空组），
      成员归属仍是转发自己的 `group`。接口：`GET|PUT /app-groups`、`POST /app-groups/rename|delete`、
      `PUT /tunnels/app-order`。**不碰端口转发**：实测 39 条转发在改布局前后全部 running。
      单测：核心 11 项、前端 14 项。
- [ ] 多选批量移动 / 批量改分组（一次拖一个够用时不做）。
- **坑位记录**：应用页拖拽**必须用指针事件**（`pointerdown/move/up` + `elementFromPoint`），
  不能用 HTML5 `draggable`。Windows 上 Tauri 接管了原生拖放（`dragDropEnabled` 默认开），
  `draggable=true` 的元素收不到 `dragstart`，表现是"怎么拖都不动"。
  同类实现：首页排序弹窗的 `.order-item`。

### 服务器管理（管理页）

- [x] **P1 概览（2026-11-16 完成）**：隧道卡片「管理」→ 独立管理页；`GET /servers/{id}/admin/overview`
      借隧道已有会话执行**一条只读** `sh` 脚本（`/proc` + `df -kP` + `ps -eo` + `docker ps`，内含 0.3s
      二次采样算网络/磁盘速率），核心解析成 CPU / 内存 / 负载 / 磁盘 / 网络 / 磁盘 IO / Top 进程 / 容器，
      单块失败只进 `degraded` 不影响整页；模块清单由**核心**给（`ready` / `planned` / `unavailable`），
      未实现模块只有导航项与"规划中"说明，不放半成品按钮。设计：`docs/plans/2026-11-16-server-admin-design.md`。
      单测：核心 15 项（脚本只读性 + 解析 + 清单）、前端 13 项（格式化口径）。
- [x] **概览分档刷新 + 环形仪表（2026-11-16 完成）**：无开关，进页面即开始。
      核心按 `?probe=live|disk-io|disks` 出三段脚本，前端各档独立递归 `setTimeout`：
      CPU/内存/流量 1s、磁盘 IO 5s、磁盘容量 10s（实测 284 / 264 / 61ms，完整 758ms）。
      每秒那条**不跑 `df`**——挂了网络盘时 `df` 会把整条命令拖住。
      **不看就不采**：判据 = 有 serverId + 窗口有焦点 + `document.hidden` 为假 + 停在概览模块，
      任一不满足就清三档定时器；焦点回来各补一拍。焦点靠 `getCurrentWindow().onFocusChanged`，
      不能只靠 `document.hidden`（Tauri 隐藏窗口不一定触发 visibilitychange）。
      展示：CPU/内存/磁盘全部用**环形表**（`RingGauge.vue`，≥70% 黄、≥90% 红），
      去掉负载、Top 进程、容器三块；每块默认 2 行，「全部 N 个」可点开（展开状态不落盘）。
- [x] **概览第二轮打磨（2026-11-16 完成）**：
      - 系统信息改成键值表：主机名 / 发行版本 / 内核 / 架构 / **主机地址**（`hostname -I` 取第一个
        非回环，失败退回 `ip -4 -o addr`）/ **启动时间**（前端用 `now - uptime` 算）/ 运行时间（精确到秒）。
      - CPU / 内存 / 磁盘 IO 卡片背景画**最近 1 分钟**分时图（`Sparkline.vue`）。历史只存在内存，
        离开页面即清；X 轴按固定槽位右对齐，不满一分钟时左边留空，不伪装成满格。
      - 磁盘**按盘而不是按挂载点**：新增 `lsblk -b -P` 采集（`BlockDisk`），取 TYPE=disk 的物理盘，
        排除 `zram`/`loop`/`rom`；分区用量按名字前缀归到所属盘。`df` 里没被物理盘认领的
        **根分区与 Unraid 阵列（shfs）**补成额外行——否则阵列占用在概览里完全看不见。
        排序按已用绝对量降序（21 GB 的 1% 阵列排在 28% 的 11 GB 系统盘前面），未挂载的沉底。
      - **GPU**：与 CPU / 内存并排成三列。采集优先 `nvidia-smi --query-gpu`（唯一能同时拿到
        显存与温度的通用途径），否则退回 `/sys/class/drm/card*/device/gpu_busy_percent` +
        `uevent` 的 `DRIVER=` 映射成「AMD 显卡 / Intel 核显」。两来源单位不同（MB vs 字节），
        行内带 `nvidia|` / `sysfs|` 前缀，解析侧据此折算成 kB。没有 GPU 时后端返回空数组，
        前端连卡片都不渲染（不留空位）。实测热态 `probe=live` 仍是 280–300ms，GPU 段可忽略。
      - **内存环分段**：已用 / 缓存 / Buffers 三段不同颜色（`RingGauge` 新增 `segments`，
        每段一个 `<circle>` 靠 `stroke-dashoffset` 错开）。坑：核心的 `used_kb` 是
        `total − MemAvailable`，与 cached/buffers **直接相加会超过 100%**，必须先扣已用再把
        缓存放进剩余空间，否则环会画重。实测 22.28 GB 机器：已用 17.85 + 缓存 3.33 + Buffers 0 = 95.1%。
      - **磁盘卡合并**：占用（`block_disks`）与读写（`disk_io`）按设备名 join 成一张卡，
        一块盘一行（环 + 名字 + 容量 + `R/W` + 自己的曲线），卡占整行；网络同样一项一行。
      - **ZFS 识别**：`lsblk` 加采 `FSTYPE`，并用 `zpool list -vHp -o name,size,alloc` 取池清单
        （不缩进 = 池，缩进 = 成员，设备名去 `/dev/` 前缀）。成员盘自己没有挂载点，光看 `df`
        会以为"未挂载"，现在显示「池 deva · 1.9 TB / 5.6 TB」。`zpool` 不可用时退到
        `FSTYPE=zfs_member` 标成「池 zfs」。实测：3 块 1.9 TB NVMe → 池 `deva` 5.6 TB / 已用 1.9 TB。
      - **内存分源**：跟着 10 秒那一档采（`@@memdetail`）——VM 取 `ps -eo rss,comm` 里
        `qemu|kvm` 的 RSS 合计，容器取 cgroup（兼容 v2 的 `system.slice/docker-*.slice/memory.current`、
        `cgroup/docker/*/memory.current` 与 v1 的 `memory/docker/*/memory.usage_in_bytes`），
        ZFS 取 `/proc/spl/kstat/zfs/arcstats` 的 `size`。实测 deva：VM 4.12 GB、容器 2.98 GB、ARC 4.01 GB。
        两个坑：① ARC 在 Linux 上**算在 used 里**（`MemAvailable` 不含它），必须从 used 扣，
        与 used 相加会把环画到 96% 而实际只有 84%；② `Cached` 里含 **tmpfs/shmem**，
        那部分删不掉也回收不了（实测 deva 的 Shmem 有 3.12 GB，几乎等于全部 Cached），
        所以「缓存」= `Cached + Buffers − Shmem`，否则"能腾多少"会虚报。
      - **刷新频率收敛为两档**：原本担心"刷新本身有负载"才拆成 1s / 5s / 10s，实测完全不是——
        每档 280–300ms 里 200ms 是固定采样窗口的 `sleep`，命令本身只花几毫秒。于是把磁盘 IO
        并进 1 秒档（`/proc/diskstats` 只是一次 cat），只剩两档。容量仍然留在 10 秒，理由换成
        了真正成立的那条：`df` 会逐个问挂载点，挂 NFS/SMB 时等于每秒朝远端查一次容量。
        实测 live 297–305ms、disks 125ms。
- [ ] **P2 进程管理**：全量进程列表 + 结束进程（写操作，需二次确认与独立契约）。
- [ ] **P2 文件管理**：SFTP 列目录 / 下载 / 上传 / 删除；大文件必须流式，不能一次性读进内存。
- [x] **P2 终端 + 页头切换器（代码侧，2026-11-19 完成）**：设计 `docs/plans/2026-11-19-server-terminal-design.md`，
      计划 `docs/plans/2026-11-19-server-terminal-plan.md`。管理页「终端」模块转正：单会话登录 shell
      （`bash -l` 优先、无则 `sh`），复用 `engine::open_pty` 与容器控制台的帧协议；接口层抽
      `pty_bridge`（容器控制台与终端共用）、鉴权 query-token 扩到 `/admin/terminal`；前端纯件
      `models/ptySession.ts` + xterm 接线 `ptyTerminal.ts`。管理页内切模块不断开（`v-show` 常驻）、
      返回隧道页断开；概览三态不再连坐其它模块（点「刷新」不会拆掉终端）。同批：页头服务器切换器
      （剔除「本机」、>6 台出过滤框、切换后停在当前模块），`App.vue` 的 `adminServerId` 仍是唯一真相。
      `shell::pick_shell(login)` 顺带收敛了容器控制的 `PICK_SHELL`（字符串逐字不变 + 单测断言）。
      门禁：`cargo test -p tl-core` 257 项、前端 151 项、`vue-tsc` / `vite build` 全绿。
- [ ] **[环境] 终端与切换器真机验证**：按 `docs/TESTPLAN.md` 的 U-42 / U-43 / U-44 逐条跑；
      重点看全屏程序排版、切走切回不变 20 列、返回隧道页后远端 shell 无残留（`ps -p <PID>`）。
- [ ] **B6 容器控制台共用纯件**（可选，有回归风险）：`ContainerConsole.vue` 改用
      `PTY_THEME` / `parseServerFrame` / `createTerminal`（只换纯件，生命周期逻辑不动），
      改完需真机回归 exec / root 两档。本轮未做——没有真机环境时不动已验证路径。
- [ ] **P3 容器管理 / 网络管理 / 防火墙**：启停与日志；网卡路由与监听端口；ufw/firewalld/iptables 规则查看。
- [ ] **P4 服务与日志**：systemd 单元启停、journalctl 跟随。
- [ ] **[环境] 真机验证概览**：真实 Linux 服务器上跑一遍，确认无 docker、多挂载点、`ps` 不支持 `--sort`
      三种情况都只降级不报错。

### 域名访问（hosts + 回环入口）

- [x] 设计已定稿（2026-11-12）：`docs/plans/2026-11-12-domain-access-design.md`。
      方向：域名回环 IP 池 + hosts 托管区块 + 保存即同步（失败留「待提权」，UAC 由 Agent 拉起
      `tl-core hosts sync`）；HTTPS 按「本地协议 × 远端协议」三格：① http/http 直通、② https/http 本地 CA 终止、
      ③ https/https 透传（参考 joyshare 的入口/回源解耦）；**「本地 http + 远端 https」直接禁止**；② 只支持 HTTP/1.1；
      域名自动并入 WinINET `ProxyOverride`（Agent 以登录用户身份写，免提权）。B1–B7 已按此落地。
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
- [x] **域名共享（2026-11-17 完成）**：`git.zly.m` 既要网页端（443）又要 SSH（22）提交推送。
      域名从「独占键」降为「共享键」：同域名的多条转发共用同一个 `domain_host`，唯一性只看 `(host, port)`；
      `domain_host_for()` 统一 create / patch / 导入预判的分配口径，hosts 与代理例外按域名去重；
      `ConflictKind` 与「域名已被转发占用」的判定一并删除（`find_conflict` 只回地址冲突）。单测 3 项新增/改写。
- [ ] **观察项**：curl 的 Schannel 后端对本地根叶子报「吊销状态未知」（`--ssl-no-revoke` 后正常）。
      浏览器通常对显式安装的根不强制吊销检查，需在有桌面的 Edge/Chrome 上确认一次；若不通过，再评估给叶子加 CRL DP 的代价。
- [ ] **仍需外部环境**：卸载清理（安装器未构建）。

### 需要管理员权限 / 重启

- [x] **[环境] UAC（2026-11-17 完成）**：`tl-core --autostart on|off` 实测——提权执行后
      `autostart.result.json` 回传 `ok=true`，任务 `ThreadlineCore` 落盘（BootTrigger + SYSTEM + HighestAvailable +
      `"D:\ruanjian\threadline\tl-core.exe" --boot --log-dir "C:\Users\Public\Threadline\logs"`），**开机自启功能本身有效**
      （当日 08:58 核心即以 `run_mode=boot` / `identity=user:MYPC$` 起来）。
- [x] **[环境] 权限边角（2026-11-17 完成）**：标准用户 `schtasks /query /tn ThreadlineCore /xml ONE` 报 `Access is denied`，
      `C:\Windows\System32\Tasks\ThreadlineCore` 也读不了。原 `registry::query` 把**任何非零退出**都当「任务不存在」，
      于是用户身份的核心恒报 `autostart.enabled=false`：提权开启成功、回读仍显示「关闭」，看起来就像自启坏了。
      改法：查询失败分「明确找不到」与「读不到」两类；读不到时 `AutostartStatus.task_readable=false`，
      `enabled` 用 `autostart.state.json`（上次成功设置的方向，核心与提权进程两处都写）兜底，界面注明「读不到 / 取自上次结果」。
      **连带影响**：助手启动时按 `autostart.enabled` 同步 `HKCU\...\Run` 登录项，读不到 → 恒 false → **登录项被删**，
      表现成「开机核心起来了、登录后没托盘」。现在 `task_readable=false` 且 `enabled=false` 时不再动登录项
      （显式开关操作仍走自己的 `sync_logon_entry` 路径）；`sync_logon_entry` 幂等，下次启动会自愈。
- [ ] **观察项**：`schtasks /create` 生成的默认设置有 `DisallowStartIfOnBatteries=true`，
      笔记本用电池开机会不拉起核心（本机台式机无影响）。要支持笔记本得改用 XML 创建并显式关掉电池限制。
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

- **仓库**：单仓 `C:\my\threadline`（工作区文档 + `threadline`）。唯一远程是 `github`
  （`git@github.com:zlyuancn/Threadline.git`），`main` 跟踪 `github/main`。
  `git.zly.m` 那个私有远端已于 2026-11-18 从 remote 中移除——它在本机配置成拉取型镜像，
  push 会被拒（`Mirror Repository zlyuancn/threadline is read-only`），留着只会让
  `git status` 长期显示 ahead 并诱导误推。
- **测试环境状态**：本机门禁全绿（cargo test 251 + agent 11 + frontend 142 + msocheck 通过 + clippy + fmt + 构建，
  2026-11-18 本机访问落地后重测）；
  两条冒烟脚本（`smoke.ps1` 控制面、`smoke-agent.ps1` 托盘与单实例）——`smoke-wake.ps1` 已随两进程改造删除。
  NSIS 安装包已产出并**实际安装过一次**（`%LOCALAPPDATA%\Threadline\`，两 exe + uninstall）。
- **已真机验证**：真实 sshd 打通并跑通数据面（T-01）；主机密钥确认 → 信任 → 重新启动的完整用户流程。
- **未做**：断线恢复与 keepalive 失效（T-06/T-09）、UAC 自启实测、零登录会话实测、卸载残留检查、
  本机程序接域名的真机链路。
- **下一步**：安卓端 **A0 真机三项**（工具链与交叉编译已通，见「待办 → 安卓端」）：插手机开 USB 调试 →
  `gradlew :app:assembleDebug` 产物 `android/app/build/outputs/apk/debug/app-debug.apk` → 装机核对指纹 → `running` →
  浏览器开 `127.0.0.1:8123`；随后 A0.3 回环别名（`127.0.0.5` + 两个 WebView 的 cookie 隔离）。
  桌面侧并行先解决"安全软件白名单"（否则面板无法连外部 SSH），其余按「待办」逐条验证。
- **拍板记录**：
  - 2026-11-19：**安卓端立项**——定位 B 档（App 内 `-L` + 内置 WebView，并让第三方 App 共用本地端口），
    不做 hosts / 域名 / 装根 / 自启；C 档（VpnService 只承载 DNS 的劫持）留作后续独立阶段。
    技术栈 = Kotlin + Compose + **复用 Rust 核心经 UniFFI**（理由：`windows-sys` 已收在 `cfg(windows)`、
    Win32 只在 4 个 foundation 文件、`russh` 用 `ring` 后端）；**不新增仓库**，同仓加 `android/` 与 `crates/tl-mobile`。
    安卓端不跑 HTTP 控制面（CAW 偏离，状态仍由核心独占）。先做 A0 三天 spike，失败退 sshj。
    设计见 `docs/plans/2026-11-19-android-client-design.md`，计划见 `docs/plans/2026-11-19-android-client-plan.md`
    （含环境准备 E1–E2：本机当前缺 JDK / SDK / NDK / android target / cargo-ndk）。
  - 2026-10-19：产品名 **Threadline / 引线**；工作区 `C:\my\threadline`，项目 `threadline`；二进制 `tl` / `tl-core` / `tl-agent`。
  - 2026-10-19：开机自启 = 一个开关（运行期可切，切开关不停正在跑的服务）；停止服务 = 现在退出且不影响自启；Wake 在核心缺失时拉起用户身份 Core（自启已开却起不来 → `stand_in_core`）。
  - 2026-10-19：v1 仅 `-L`；凭据 = 使用者填写的私钥内容/密码，machine-scope DPAPI，不读 `~\.ssh`。
  - 2026-10-19：方法论版本戳 CAW `41f698a` / 工作区 `f78fd6e` / MSO `5c432f9`。
  - 2026-11-12：**域名访问**（用户提出、参考 joyshare）——定为「hosts + 域名回环 IP 池」，
    不做 HTTP 正向代理、不做 SNI 分流、不做 MITM；hosts 写入需 UAC 提权，保存成功即视为成功、
    提权失败留横幅。设计见 `docs/plans/2026-11-12-domain-access-design.md`。
  - 2026-11-17：**域名共享**——同一个域名允许多条转发共用（`git.zly.m` 的 443 网页 + 22 SSH），
    唯一性只看 `(domain_host, local_port)`；hosts 一条域名一条映射，不再因域名重复而拒绝。
    设计文档新增 §4.5，原「一域名一个回环 IP」的独占语义作废。
  - 2026-11-17：**自启状态可读性**——「开机自启无效」的真因是**读不到**而不是没设置：用户身份的核心查询
    SYSTEM 任务被拒，代码把拒绝当成「不存在」。定为「查询失败要分找不到/读不到 + 读不到时用上次设置方向兜底」，
    不做「给任务加 SDDL 让普通用户可读」（要动 Win32 安全描述符，收益不如缓存方案）。
  - 2026-11-16：**服务器管理**（用户提出）——隧道卡片给一个「管理」入口，点进去是独立管理页；
    先做只读概览（系统/CPU/内存/负载/磁盘/流量/磁盘 IO/进程/容器），终端 / 文件 / 防火墙 / 网络 /
    服务等模块只留位置（核心给清单标 `planned`），写操作类模块逐个另立契约。
    不新建 SSH 连接（复用隧道会话），不落盘，不做 Windows 采集。
    设计见 `docs/plans/2026-11-16-server-admin-design.md`。
  - 2026-11-16：**应用页编辑模式**（用户提出）——四项全做：拖动磁贴改顺序/归属、磁贴改分组（含输新名建组）、
    组改名、组排序与空组。布局存核心（`TunnelDto.app_order` + `app_groups.json`），成员归属仍用转发自己的
    `group`；**转发逻辑一律不读布局**，改布局不动任何一条隧道。设计见
    `docs/plans/2026-11-16-app-page-edit-mode-design.md`。
  - 2026-11-13：**私有远端停止推送**——`git@git.zly.m:zlyuancn/threadline.git` 不再作为推送目标；
    用户改为从公共仓库同步代码。此后本仓只做本地 commit，AI 会话不得对该远端执行 push；
    何时与公共仓库对齐、是否重建历史由用户决定。
  - 2026-11-18：**本机访问**（用户提出）——把本机在跑的程序也接进域名访问，**不经过 SSH**。
    定为「本机 = 内置的、恒已连接的服务器条目」（`kind = local`，不落盘），它的转发走
    `Upstream::Direct`（直接连本机端口），入口的域名 / hosts / TLS 三格与远端完全共用；
    **入口侧不加任何特殊校验**（不限回环、不禁止同址），本机条目本身不可改、不可删、不可断开。
    「本机管理」里那套服务器管理（概览 / 容器）需要 SSH 会话，本机暂不提供。
    设计见 `docs/plans/2026-11-18-local-machine-access-design.md`。
  - 2026-11-18：**移除 `git.zly.m` 远程**——「不推送」还不够，它留在 remote 列表里会让
    `git status` 长期显示 `ahead N`，看起来像「有东西没推上去」，而真去推又会被拒
    （`Mirror Repository ... is read-only`）。现在 `git remote remove origin`，
    唯一远程是 `github`、`main` 跟踪 `github/main`，`status` 直接反映与真源的差异。
    镜像侧如何同步（Gitea 拉取周期等）由用户自行配置。

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
