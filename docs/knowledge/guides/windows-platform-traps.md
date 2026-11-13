# Windows / WebView / 构建链上的坑

每条都对应一次真实的线上或联调故障，写下来是为了不再付第二次学费。

## 1. 控制台黑窗：`windows_subsystem` 不能只在 release 生效

- 现象：双击助手，旁边挂着一个命令行黑窗。
- 根因：`#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]` 只在 release 切 GUI 子系统，
  而日常双击的恰恰是 debug 构建。
- 规则：**任何"双击运行"的二进制都写成无条件 `windows_subsystem = "windows"`**。
  从终端调用时 stderr 仍然可用（GUI 子系统只表示"不主动创建控制台"，继承的句柄照常工作）。
- 校验方式：读 PE 头 `OptionalHeader.Subsystem`，2 = GUI，3 = Console。

## 2. 子进程会额外弹一个黑框：`CREATE_NO_WINDOW`

- 现象：面板开着时每隔几秒闪一次黑框。
- 根因：核心以 `DETACHED_PROCESS` 运行（自身没有控制台），而它调用的 `schtasks.exe` 是控制台程序，
  未指定 `CREATE_NO_WINDOW` 时 Windows 会给它新开一个控制台。
- 规则：**从无控制台进程拉起的每个控制台子程序都要加 `CREATE_NO_WINDOW`（`0x0800_0000`）**。
- 顺带教训：心跳不要轮询"要起子进程"的接口。身份信息（`/whoami`）只在启动/切开关时取，
  心跳用纯内存的接口。

## 3. 「我改了代码，但用户用的是旧二进制」

这一类坑出现过两次，形态不同、后果一样：

| 场景 | 成因 | 现在的防线 |
|---|---|---|
| sidecar 陈旧 | `src-tauri/binaries/*` 只在手动脚本里更新，打包时照抄旧文件 | `tauri.conf.json` 的 `bundle.beforeBuildCommand` 强制重新暂存 |
| 冒烟脚本留下的 agent 副本 | 为了让"三者同目录"，脚本把 agent 拷到 `target\debug\`，副本永不更新 | Wake 自己解析真实构建；门禁检查该副本一旦存在即失败 |

- 规则：**不要用"拷贝一份到某处"来解决路径问题**——拷贝出来的东西会腐烂。
  让程序自己去解析真实位置。
- 规则：构建产物如果不带时间戳校验，就一定会有人（包括自己）跑到旧的。暂存脚本要打印构建时间。

## 4. PowerShell 的两个陷阱

- `Start-Process -Wait` 等的是**整棵进程树**，包括被拉起的常驻进程。冒烟脚本要用 `WaitForExit(ms)` 只等自身。
- `.ps1` 必须保持纯 ASCII：Windows PowerShell 5.1 按 ANSI 读文件，中文注释会把解析搞崩，
  而且报错信息（"Try 语句缺少 Catch"）与真实原因毫无关系。
- **更阴险的形态**：中文注释有时不报错，而是**静默吃掉下一行**——注释末尾的多字节字符
  按 ANSI 解码后可能以一个续行符结尾，把紧随其后的语句并入注释。表现是"命令执行了但
  什么都没发生"（巡检脚本里的 `Start-Process` 整句消失，进程根本没起来，脚本继续往下跑）。
  排查时先怀疑编码，而不是逻辑。

## 5. WebView 跨源：没有 CORS 就是 `TypeError: Failed to fetch`

- 核心监听 `127.0.0.1:<port>`，页面源是 `http://tauri.localhost` —— **跨源**。
- 没有 `Access-Control-Allow-Origin` 时浏览器在 JS 层直接拒绝，连响应体都拿不到，
  表现与"核心没启动"一模一样。
- 规则：CORS 层必须挂在**鉴权外层**，否则不带 token 的预检会被拦成 401，浏览器照样报 CORS 失败。

## 6. 页面刷新会清掉 `eval` 注入

- 用 `window.eval("window.__X__ = ...")` 推数据给 WebView，**F5 之后就没了**。
- 规则：跨页面的引导数据要能**拉**（暴露一个命令），推送只作为状态变化的加速。

## 7. 安全客户端可能按「可执行文件路径」拦截网络

详见 [connection-failure-triage.md](connection-failure-triage.md) 第 3 节。要点：

- 同一份二进制，从 `%TEMP%` 启动正常，从 `target\debug\` 或 `%LOCALAPPDATA%\Threadline\` 启动
  会被拦截，返回一个 HTTP 403 网关页面。
- 这类拦截**伪装成远端响应**，容易被误判成"目标不是 SSH"。
- 排查手段：用 `ssh.exe` / Python / Node 各连一次做对照；再换目录跑同一份二进制。

## 8. `tauri build` 不会重建前端

- `tauri.conf.json` 的 `build.frontendDist` 只是**指向** `dist/`；Tauri 自己**不跑** `vite build`。
  唯一的钩子是 `beforeBuildCommand`，而它原本只做 sidecar 暂存。
- 后果：改完 Vue 直接 `pnpm tauri build`，打出来的是**上一版界面**，构建日志一切正常。
  判断依据：看 `dist/assets/*.js` 的修改时间，比看日志可靠得多。
- 规则：`beforeBuildCommand` 必须包含前端构建。现在统一走 `scripts/prebuild.ps1`
  （sidecar 暂存 + `pnpm build`），忘记这回事不会再静默发旧界面。

## 9. 抓图自检：DPI 虚拟化与锁屏

- PowerShell 进程默认**不是 DPI-aware**：`GetWindowRect` 返回逻辑坐标，`CopyFromScreen`
  按物理像素取图。150% 缩放下两套坐标系差 1.5 倍，裁出来的窗口永远比真实窗口小、右侧被切，
  非常容易被误判成"布局横向溢出了"（本项目为此白改过一轮 CSS）。
- 规则：抓图脚本开头先 `SetProcessDPIAware()`。
- 桌面锁屏 / 会话切换时 `CopyFromScreen` 抛 `Win32Exception`，产出一张**纯白图**。
  改用 `PrintWindow(hwnd, hdc, 2 /* PW_RENDERFULLCONTENT */)` 只依赖窗口句柄，锁屏也能截。
- 判断依据：图是纯白且脚本报 `Win32Exception` → 先怀疑抓图，别怀疑界面。

## 10. 用 `</template>` 做文本切片会截断模板

- 批量改 `.vue` 时用 `index("<template>")` / `index("</template>")` 定界，
  第二个会命中弹窗里的 `<template #footer>`：后面的弹窗整段丢失，而**构建依然通过**
  （只是少渲染一块，运行时不报错）。
- 规则：模板切片用唯一注释定界（`<!-- 隧道：… -->`、`<!-- 端口转发 -->`），
  改完立刻 `grep -c "<el-dialog"` 数一数，别信"构建成功"。

## 11. 改 `tl-core` 后只跑 `pnpm tauri build`，界面拿到的是**老契约**

- `tl-core` 是 workspace 成员，`bundle-sidecars.ps1` 只是把 `target\debug\tl-core.exe`
  **复制**成 sidecar。它不会替你重新编译核心。
- 表现：字段加在协议里、前端也读了，但运行时永远取不到（例如新增的 `scheme` 一直是空），
  而且不报错——看起来像"前端没接上"。
- 规则：动了 `crates/**`，先 `cargo build -p tl-core`，再 `pnpm tauri build`。
  想确认版本，比较 `target\debug\tl-core.exe` 与 `src-tauri\target\debug\tl-core.exe` 的时间。

## 12. 内联 SVG 不写宽高会被撑满容器

- `<svg viewBox="...">` 没有 `width`/`height` 时，在 flex 容器里会按容器尺寸铺开。
- 表现：按钮里出现一个巨大的图形盖住整张卡片（文字被挤成两行、地址被挡）。
- 规则：图标一律在 CSS 里约束尺寸（`.btn svg { width: 14px; height: 14px }`），
  不要指望 `viewBox` 会限制渲染大小。

## 13. 全局热键：注册与注销必须在同一个线程

- `RegisterHotKey(hwnd = 0, ...)` 把热键挂在**调用线程**上，`UnregisterHotKey` 也必须由
  同一个线程调用。所以"改快捷键"不能写成"命令线程里注销 + 重新注册"，那是跨线程操作。
- 可行做法：起一个专职线程负责注册 + `GetMessageW` 循环；改绑时由其他线程
  `PostThreadMessageW(tid, WM_APP+x, ...)` 把消息**投给那个线程**，让它自己注销再注册。
  线程 id 用 `GetCurrentThreadId()` 存进 `AtomicU32`。
- 换绑失败（被别的程序占用，`GetLastError() == 1409`）必须**把旧的注册回去**，
  否则用户改一次失败，快捷键就彻底没了。
- 热键线程不建窗口时，消息只会投递到线程队列；`GetMessageW(msg, 0, 0, 0)` 收得到，
  但 `PostThreadMessage` 之前队列必须已存在（先调一次 `GetMessage` 或 `PeekMessage`）。

## 14. 数据目录放哪：三种创建者 = 三种权限（实测）

背景：核心要以 SYSTEM 在**登录前**自启，界面是登录后的普通用户，两边共写一个目录。

| 目录 | 创建者 | 交互用户拿到 | 能否保存配置 |
|---|---|---|---|
| `%ProgramData%\threadline` | 管理员 | `ReadAndExecute` | ❌ |
| `%ProgramData%\threadline` | SYSTEM | `ReadAndExecute + Write`（**无 Delete**） | ❌（原子替换会失败） |
| `%ProgramData%\threadline` | 普通用户 | `FullControl`（经 CREATOR OWNER） | ✅ 仅限用户自己建的文件 |
| **`%PUBLIC%\Threadline`** | 任意 | `INTERACTIVE: Modify`（可继承） | ✅ 跨创建者都成立 |

关键点：

- **CREATOR OWNER 跟着创建者走**。用户建的目录，SYSTEM 后来写进去的文件，用户只剩
  `ReadAndExecute` —— 实测如此。所以"让用户建目录"只在"用户建的那一刻"正确。
- `Users: Write` 不等于能覆盖：配置保存是"临时文件 + rename"，目标文件上需要 `Delete`。
- `%PUBLIC%` 给的是**可继承的** `INTERACTIVE: Modify`，任何创建者生成的对象都带上它，
  所以 SYSTEM 写的文件用户也能原子替换（已用受限令牌 + 计划任务实测）。
- 代价：`INTERACTIVE` 是"任何交互登录的用户"，同机其他账户也能改配置、读
  `credentials.dat`（DPAPI machine scope，读到即可解密）。单用户机器可接受，多人共用不行。
- 若必须收紧：保留 `%ProgramData%` 并在目录上补一条显式
  `icacls <dir> /grant <用户>:(OI)(CI)M /T`（`OI/CI` 不可省，`/T` 修已存在文件）。
  这条需要 owner 或管理员身份——用户自己建的目录可以自授权，管理员建的就只能走一次 UAC。

## 15. `copy_bidirectional` 会把"正在跑多少"藏起来

- `tokio::io::copy_bidirectional` 只在**结束时**返回两个方向的字节数。想在界面上显示实时速率，
  用它就永远是 0，直到连接关闭。
- 做法：`tokio::io::split` 拆成读写两半，自己写单方向搬运循环（读一块 → 写一块 → 累加），
  两个方向各自持一个计数器。16KB 缓冲、64 并发下开销可忽略。
- 计数器归零要显式做：转发重启时不清零，界面会把上一段会话的流量算成"刚跑的"。
- **速率不要在核心里算**：核心只给单调递增的累计字节，界面按"这次采样 − 上次采样 ÷ 间隔"
  得到每秒速率。核心算速率就得维护定时器，还要把展示口径写进协议。
- 采样间隔别太短：两次刷新间隔 < 400ms 时抖动会被放大成几倍速率。

## 16. HTML5 拖拽：在 `dragover` 里换位会中断整次拖拽

- 想在拖动过程中实时换位，很容易写成 `@dragover` 里 `splice` 调换数组。
  但在 Chromium / WebView2 上，**被拖的那个节点一旦从 DOM 上被移除再插入**，
  浏览器认为拖拽源消失，`dragend` 立刻触发，拖拽中断——界面表现就是"拖不动"，且没有报错。
- 两条出路：
  - 只在 `drop` 里换位（`dragover` 仅 `preventDefault` + 记录落点），代价是没有实时预览；
  - 干脆用**指针事件**自己实现（`pointerdown/move/up` + 行中线判断落点），
    行为完全可控，还能顺手做出"源行半透明、目标行高亮"。
- 另：行内需要 `user-select: none`，否则按下时先进入文本选择，拖拽根本不会开始。
