# 虚拟机管理（vms）设计

2026-11-20 · 已拍板实施（原型：`tmp/misc/vm-admin-prototype.html`）

## 1. 目标与边界

管理页新增「虚拟机」模块，数据源只有 `virsh`。

做：列表（两档密度）、详情抽屉（详情 / 磁盘 / 网络 / 串口控制台）、生命周期动作（开机 / 优雅关机 /
重启 / 暂停 / 继续 / 强制断电 / 清理崩溃）、开机自启开关。

不做：创建 / 删除 / 克隆虚拟机、磁盘扩容、快照、迁移、VNC 图形控制台（RFB 另立项）。

沿用容器管理已定的三条底线：只走 SSH exec、不装 agent、不做 sudo 提权。用户不在 `libvirt` 组时
直说「无权限」，由用户自己处理。

## 2. 数据来源

### 2.1 模块状态（概览探测）

`Probe::Full` 增加一段 `@@virsh`：`command -v virsh` 有则回版本号，没有回 `__no_virsh__`。
`modules()` 据此决定 `vms` 是 `Ready` 还是 `Unavailable`（note 写「未安装 libvirt」）。

`virsh --version` 不需要连 libvirt，所以「装了但没权限」在探测阶段看不出来——那是列表请求的错误，
由面板的横幅承担，不把模块标成不可用。

### 2.2 列表（一次 exec）

```
@@v_engine      virsh --version
@@v_list        virsh list --all
@@v_stats       virsh domstats --state --balloon --block --interface     # 所有域一次拿
@@v_info:<name> virsh dominfo <name>          # 每个域：vCPU / 内存 / 自启 / UUID / OS 类型
@@v_if:<name>   virsh domiflist <name>        # 每域：网卡 / 类型 / 桥接 / 型号 / MAC
@@v_ip:<name>   virsh domifaddr <name> --source lease   # 每域：DHCP 租约
```

为什么把 `dominfo` 们放在一次 exec 里：单次 SSH 往返约 100ms，6 台域若逐台开请求就是 2s 起。
一次 exec 里跑 3N+2 条 virsh，代价是 3N+2 次本地进程，约 20ms/条。

**不单独做 stats 端点**：`domstats` 给的 `cpu.time` / `net.N.rx.bytes` 都是累计值，前端拿到两次
采样自己算差值即可（容器管理也是这么算网络速率的）。这样列表 4 秒一档就能同时喂饱 CPU 环与
网速，少一条接口、少一次 `sleep`。第一拍拿不到速率，环里显示 `—`。

### 2.3 详情（抽屉打开时一次 exec）

```
@@v_xml:<name>   virsh dumpxml <name>        # 机型 / 固件 / 磁盘格式
@@v_info:<name>  virsh dominfo <name>
@@v_blk:<name>   virsh domblklist <name> --details
@@v_if:<name>    virsh domiflist <name>
@@v_ip:<name>    virsh domifaddr <name> --source lease
@@v_stats        virsh domstats --state --balloon --block --interface
```

`dumpxml` 只用两处：`machine='...'`（机型）和有没有 `<loader`/`pflash`（UEFI 还是 BIOS）。
为这两处引入 XML 依赖不值得，用两个定点字符串查找，取不到就留空，界面显示 `—`。

### 2.4 与原型的两处差异

- **运行时长（uptime）去掉**：libvirt 不暴露域启动时刻（`domstats --state` 只有状态与原因，
  `dominfo` 只有累计 CPU 时间）。用 `pgrep` + `/proc` 猜 qemu 进程启动时间在改了名字的机器上不可靠，
  宁可不显示也不显示错的。副行改为「状态 · 自启」。
- **CPU 百分比是「占分配的百分比」**：`cpu.time` 增量 / (采样间隔 × vCPU 数)，0–100，正好喂环。
  这与容器「所有核合计、可超 100%」的口径不同，所以环下的说明要写「N 个 vCPU 为 100%」。

## 3. 解析契约

| 来源 | 关键点 |
| --- | --- |
| `virsh list --all` | 三列 `Id Name State`，关机域 Id 是 `-`；状态字符串可能是 `shut off` / `pmsuspended` |
| `virsh dominfo` | `Key: Value` 行，取值去首尾空白；`Autostart: enable/disable` |
| `virsh domstats` | `Domain: 'name'` 起段，`key=value`；`balloon.current` 是 KiB，`balloon.rss` 是 **字节**，`block.N.capacity` 是字节 |
| `virsh domblklist --details` | 四列 `Type Device Target Source` |
| `virsh domiflist` | 五列 `Interface Type Source Model MAC` |
| `virsh domifaddr` | 表格，取 `ipv4`/`ipv6` 行的地址（去掉 `/24` 前缀长度） |

任何一段解析失败都不让整次列表失败：缺磁盘就是没磁盘，缺 IP 就是 `无租约`。

## 4. 状态机与动作白名单

| libvirt 状态 | 允许动作 |
| --- | --- |
| `running` | start✗ · shutdown · reboot · suspend · destroy · console |
| `paused` | resume · destroy（qemu 被冻住，串口连上也没输出，不给 console） |
| `shut off` | start · destroy✗ |
| `crashed` | destroy（文案叫「清理崩溃状态」） |
| `pmsuspended` | destroy |

白名单在核心（`allowed_actions`），前端只按它渲染菜单——不做「前端自己判断，核心信任」的事。
每个动作执行完再读一次 `virsh domstate` 回填真实状态，不假设成功。

`shutdown` 是异步的（ACPI 请求发出去，客户机可能不理），所以结果里 `state` 仍是 `running` 时
前端提示「已发送关机请求，等待客户机响应」，而不是假装关了。

开机自启是独立接口 `POST /vms/{name}/autostart {enabled}`，与生命周期动作分开（它不是状态迁移）。

## 5. 协议与接口

```
GET  /servers/{id}/admin/vms                        → VmList
GET  /servers/{id}/admin/vms/{name}                 → VmDetail
POST /servers/{id}/admin/vms/{name}/action          {action} → VmActionResult
POST /servers/{id}/admin/vms/{name}/autostart       {enabled} → VmActionResult
GET  /servers/{id}/admin/vms/{name}/console         WebSocket（串口，复用 PTY 桥）
```

写操作三件套照旧：白名单校验（域名 `[A-Za-z0-9_.-]{1,80}`）、动作必须在白名单内、域名必须出现在
刚采到的列表里。脚本里域名只以单引号包裹出现，`validate_name` 保证没有引号可逃。

控制台沿用 `ConsoleGuard`：同一 `server:vm:<name>` 只允许一个占用者，断开即释放；前端离开页签/关闭
抽屉要主动断开，不然会挡住下一次连接。

## 6. 前端

- `models/vms.ts`：列表 / 详情 / 动作、4 秒轮询（面板可见才跑）、密度 `tl.vms.density`
  （默认 `detail`）、由两次采样算 CPU% 与网速、`allowedActions` 与核心同表。
- `views/admin/VmsPanel.vue`：与 `ContainersPanel` 同一套结构与 class 前缀（`vm-`），顶栏 = 引擎版本 +
  搜索 + 状态筛选 + 密度切换 + 刷新；行内「⋯」菜单；抽屉四页签；关机两套确认（强制断电必须勾选）。
- 资源用环形：复用 `components/RingGauge.vue`，给它加 `text` / `sub` 两个可选属性（不传则保持原样，
  概览不受影响）。列表 40px，抽屉 84px。CPU 环内是百分比，内存环内是实占值 + 单位。
- 控制台 `views/admin/VmConsole.vue`：只有串口一种模式，复用 `models/ptySession` 的纯函数。

## 7. 验收

- 「虚拟机」在真实机器上能列出关机 / 运行 / 暂停域，两档密度切换正常，环形资源有值。
- 无 libvirt 的机器：模块显示「未安装」，面板不假装有空列表。
- 无权限：横幅直说 `libvirt` 组，不弹 sudo。
- 优雅关机与强制断电文案分开，强制断电必须勾选才能点。
- `verify.ps1` 全绿（fmt / clippy / test / vue-tsc / vite build / msocheck）。
