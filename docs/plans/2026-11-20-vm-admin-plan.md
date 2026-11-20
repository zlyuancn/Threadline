# 虚拟机管理（vms）实施记录

2026-11-20 · 对应设计：`2026-11-20-vm-admin-design.md` · 原型：`tmp/misc/vm-admin-prototype.html`

## 交付切片

1. **契约**（`crates/tl-protocol/src/lib.rs`）：`VmSummary` / `VmDisk` / `VmNet` / `VmItem` / `VmList` /
   `VmDetail` / `VmAction` / `VmActionResult`；`ServerOverview` 加 `vms`。
2. **模型层**（`crates/tl-core/src/models/server_admin/vms.rs`）：列表 / 详情 / 动作 / 自启四段脚本、
   `domstats` 分段解析、`domblklist` 与 `dumpxml` 补格式、`allowed_actions` 状态机、域名白名单。
   单测覆盖：状态表、`domstats` 两域分段、`balloon.rss` 字节/KiB 判别、`domifaddr` 去前缀、
   动作脚本注入拒绝。
3. **概览探测**（`collect.rs`）：`Probe::Full` 增加 `@@virsh`，`modules()` 据此把 `vms` 标成
   `Ready` / `Unavailable`（note「未安装 libvirt」），侧栏顺序紧跟在容器管理后面。
4. **业务层**（`business/admin.rs`）：`vms` / `vm_detail` / `vm_action` / `vm_autostart` / `vm_console`。
   写操作先拉一次列表（顺带做引擎与存在性校验），再按 `allowed_actions` 判状态，最后才拼脚本；
   动作后回读 `domstate`，`shutdown` 只承诺「已发送」。
5. **接口**（`interface/mod.rs`）：五条路由，控制台复用 PTY 桥（`pty_bridge` + `ConsoleGuard`，
   占用键 `server:vm:<name>`）。动作字符串在接口层映射成枚举，不进脚本。
6. **前端**：
   - `models/vms.ts`：列表 / 详情 / 动作、4 秒轮询、两拍差分算 CPU% 与网速（窗口 < 1s 不算，
     避免动作后的补采把百分比放大）、密度 `tl.vms.density`、`allowedActions` 与核心同表。
   - `views/admin/VmsPanel.vue`：两档密度、行内菜单、四页签抽屉、两套确认（强制断电必须勾选）。
   - `views/admin/VmConsole.vue`：串口控制台，异步 chunk。
   - `components/RingGauge.vue`：加 `text` / `sub`（不传时概览行为不变）。
   - `ServerAdminView.vue`：`vms` 挂 `VmsPanel`；`api` 与 `types` 补齐。

## 验证

- `cargo test --workspace`：278 / 11 / 1 / 24 / 18 全过（含新增 vms 单测）。
- `cargo clippy --workspace --all-targets -- -D warnings`、`cargo fmt --all --check`。
- `vue-tsc --noEmit`、`vitest run`（新增 `src/models/vms.test.ts` 12 条）、`vite build`。
- `msocheck`（R1–R6）。
- 视觉：用 `tmp/vm-preview/`（Vite + 假 API，已删）逐屏截图核对列表两档、菜单、抽屉四页签、
  强制断电确认框与回执，截图留在 `tmp/shots/app-vm-*.png`。

## 真机冒烟（2026-11-20，deva：libvirt 11.2.0，3 台域）

在 `deva` 上跑通列表 / 详情 / 磁盘 / 网络 / 串口控制台（`virsh console proxy` 真连上），
并对着真实输出改掉三处只有真机才会暴露的问题：

1. **`virsh domstats` 不认 `--inactive`**（libvirt 11 报 `doesn't support option --inactive`）。
   脚本原来带着它，`2>/dev/null` 又把报错吞了，结果 CPU / 磁盘 / 网速全部归零，
   看起来像「采集到了空数据」。去掉之后 `cpu.time`、`net.N.*.bytes`、`block.N.*` 才有值。
2. **CPU 时间要 `--cpu-total`**：默认的 `domstats` 不输出 `cpu.time`，CPU 环会永远显示 `—`。
3. **磁盘不能只靠 `domstats`**：它只覆盖运行中的域。改为逐域 `domblklist --details`
   取设备清单、`domblkinfo` 取容量与实占（关机的域也给得出），`domstats` 只做补充。

另外两个口径调整：

- `balloon.rss` 是**宿主侧 qemu 进程的 RSS**，含 qemu 自身开销（4 GB 的域实测 4.2 GB），
  所以展示值按分配量封顶、并标注「宿主 RSS」；guest 侧真实用量要等 guest agent / balloon stats，
  这台机器上 `dommemstat` 只回 `actual` / `rss`，没有 `unused`。
- 串口只对 `running` 开放：`paused` 时 qemu 被冻住，连上去没有输出。

截图：`tmp/shots/real-vms*.png`、`real-drawer2.png`、`real-disks.png`、`real-nets.png`、
`real-console.png`、`real-shutdown.png`。

## 未完成

- 「不在 libvirt 组」的横幅文案只在代码里，没有可用的受限账号实测。
- guest 侧内存用量（`used`）要等 guest agent 或支持 `balloon.available` 的宿主。
