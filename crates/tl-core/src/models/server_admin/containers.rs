//! 容器管理：采集脚本、输出解析、动作白名单与状态机。
//!
//! 归属：`server_admin` 模型内部文件组。拥有：容器相关的**全部规则**——
//! 脚本正文、端口/卷/网络解析口径、`cid` 白名单、当前状态允许哪些动作。
//! 提供：`list_script()` / `stats_script()` / `detail_script()` / `action_script()` 等。
//! 允许调用：`tl_protocol`（DTO）、标准库、兄弟模块 `parse::split_sections`。
//!
//! 纪律：
//! - **只读脚本与写操作脚本分开**：采集那几条不许出现任何写命令（单测断言）；
//! - **无单引号**：整段被 `sh -c '...'` 包裹，正文再出现单引号会把命令截断；
//! - **cid 双重校验**：正则白名单 + 调用方（业务层）对照本次采集清单；
//! - **动作是枚举不是字符串**：命令只由白名单拼出来，不接受前端传任意命令。

use std::time::Duration;

use tl_protocol::{
    ContainerAction, ContainerEnv, ContainerItem, ContainerList, ContainerLogs, ContainerMount,
    ContainerNetwork, ContainerPort, ContainerStats,
};

use super::parse::split_sections;
use super::shell::pick_shell;

/// 写操作的预算：`stop` 默认 10s 超时，留一倍余量。
pub const ACTION_BUDGET: Duration = Duration::from_secs(20);

/// `stats --no-stream` 自带采样窗口，慢机器给到 20s。
pub const STATS_BUDGET: Duration = Duration::from_secs(20);

// ─────────────────────── 脚本 ───────────────────────

const LIST_SECTION: &str = r#"echo @@c_list
if command -v docker >/dev/null 2>&1; then
docker ps -a --no-trunc --format "{{.ID}}|{{.Names}}|{{.Image}}|{{.State}}|{{.Status}}|{{.Ports}}|{{.CreatedAt}}|{{.Label \"com.docker.compose.project\"}}" 2>/dev/null
else
echo __no_engine__
fi
"#;

const STATS_SECTION: &str = r#"echo @@c_stats
if command -v docker >/dev/null 2>&1; then
docker stats --no-stream --format "{{.ID}}|{{.CPUPerc}}|{{.MemUsage}}|{{.NetIO}}" 2>/dev/null
fi
"#;

/// 批量拿网络与卷：`ps` 给不了这两样，逐条 inspect 又要 N 次往返。
///
/// `$k` / `$v` 前面必须有反斜杠——整段在双引号里，不转义会被 shell 当变量展开。
const BATCH_INSPECT_SECTION: &str = r#"echo @@c_inspect
if command -v docker >/dev/null 2>&1; then
ids=$(docker ps -aq 2>/dev/null)
if [ -n "$ids" ]; then
docker inspect --format "{{.Name}}|{{range \$k,\$v := .NetworkSettings.Networks}}{{\$k}}={{\$v.IPAddress}},{{end}}|{{range .Mounts}}{{.Source}}=>{{.Destination}}:{{.Mode}},{{end}}" $ids 2>/dev/null
fi
fi
"#;

/// 单容器详情的前半段：状态、退出码、重启次数、重启策略、命令、镜像。
const DETAIL_MAIN_PREFIX: &str = r#"echo @@c_detail
if command -v docker >/dev/null 2>&1; then
docker inspect --format "{{.State.Status}}|{{.State.ExitCode}}|{{.RestartCount}}|{{.HostConfig.RestartPolicy.Name}}|{{.Config.Cmd}}|{{.Config.Image}}" "#;

/// 环境变量：一行一条，比 pipe 分隔好解析（值里本来就可能带 `|`）。
///
/// 自带 `@@c_env` 标记：没有它，主数据行会被当成 env 的第一条（真机上踩到过）。
const DETAIL_ENV_PREFIX: &str = r#"echo @@c_env
docker inspect --format "{{range .Config.Env}}{{println .}}{{end}}" "#;

fn wrap(body: &str) -> String {
    format!("sh -c '{body}'")
}

/// 列表 + 网络与卷：一次往返拿全（`ps` 给不了 IP 与卷，只能补 inspect）。
pub fn list_script() -> String {
    wrap(&format!("{LIST_SECTION}{BATCH_INSPECT_SECTION}"))
}

/// 资源：`docker stats --no-stream`（只在抽屉打开或显式要资源时跑）。
pub fn stats_script() -> String {
    wrap(STATS_SECTION)
}

/// 单容器详情（抽屉打开时）。
pub fn detail_script(cid: &str) -> Result<String, String> {
    validate_cid(cid)?;
    let body = format!(
        "{DETAIL_MAIN_PREFIX}{cid} 2>/dev/null\n{DETAIL_ENV_PREFIX}{cid} 2>/dev/null\nfi\n"
    );
    Ok(wrap(&body))
}

/// 日志：`--tail` 有上限，`--since` 只接受时间戳字符。
pub fn logs_script(cid: &str, tail: usize, since: Option<&str>) -> Result<String, String> {
    validate_cid(cid)?;
    let tail = tail.clamp(1, 1000);
    let since = match since {
        Some(text) if !text.is_empty() => {
            if !is_timestamp(text) {
                return Err("时间戳格式无效".to_string());
            }
            format!(" --since {text}")
        }
        _ => String::new(),
    };
    Ok(wrap(&format!(
        "echo @@c_logs\ndocker logs --tail {tail} --timestamps{since} {cid} 2>&1\necho @@c_end\n"
    )))
}

/// 写操作：`docker start|stop|restart|pause|unpause <cid>`，随后回读一次状态。
pub fn action_script(action: ContainerAction, cid: &str) -> Result<String, String> {
    validate_cid(cid)?;
    Ok(wrap(&format!(
        "echo @@c_action\ndocker {} {cid} 2>&1\necho @@c_state\ndocker inspect --format \"{{{{.State.Status}}}}\" {cid} 2>/dev/null\necho @@c_end\n",
        action_verb(action),
    )))
}

/// 删容器：`docker rm [-f]`。
///
/// **删镜像不在这一条里**：勾了「同时删镜像」也先只删容器，删完再查这个 tag
/// 还有谁用。顺序反过来（先查再删）就得在结果里「排除自己」——那个容器此时
/// 还在 `docker ps -a` 里，靠名字排除，多一层容易出错的手工逻辑。
pub fn remove_script(cid: &str, force: bool) -> Result<String, String> {
    validate_cid(cid)?;
    Ok(wrap(&format!(
        "echo @@c_remove\ndocker rm {}{cid} 2>&1\necho @@c_end\n",
        if force { "-f " } else { "" }
    )))
}

/// 删镜像 tag。单独一条，只在「容器已删、且没人再用这个 tag」之后发。
pub fn rmi_script(image: &str) -> Result<String, String> {
    validate_image(image)?;
    Ok(wrap(&format!(
        "echo @@c_rmi\ndocker rmi {image} 2>&1\necho @@c_end\n"
    )))
}

/// 列出所有容器与它们的镜像引用：`docker ps -a --format '名字|镜像'`。
///
/// 不接收参数——镜像名**不拼进命令**，只在 Rust 侧做字面比对，
/// 所以没有注入面，也不需要白名单。
///
/// **不能用 `--filter ancestor`**：它按镜像 ID 匹配，会把「用同一镜像层的别的 tag」
/// 也算成占用。`redis:8.10.3` 与它的副本 tag 是同一个 image ID，于是副本 tag
/// 永远删不掉（真机验收撞到）。要判的是「谁引用了这个 tag 字面」。
pub fn image_users_script() -> String {
    wrap("echo @@c_users\ndocker ps -a --format \"{{.Names}}|{{.Image}}\" 2>/dev/null\necho @@c_end\n")
}

/// 解析 `c_users`：只保留 `.Image` 与目标**字面相同**的容器名。
pub fn parse_image_users(raw: &str, image: &str) -> Vec<String> {
    split_sections(raw)
        .get("c_users")
        .map(|lines| {
            lines
                .iter()
                .filter_map(|line| {
                    let (name, used) = line.trim().split_once('|')?;
                    (used.trim() == image).then(|| name.trim().to_string())
                })
                .filter(|name| !name.is_empty())
                .collect()
        })
        .unwrap_or_default()
}

// ─────────────────────── 控制台（exec） ───────────────────────

/// 进容器的三种方式。
///
/// **枚举而不是命令串**：前端只选档位，命令由这里拼。用户能在终端里敲任何东西，
/// 但「用什么命令进去」不能由前端决定——否则等于把任意命令执行开了个口子。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConsoleMode {
    /// 普通用户 + 镜像里的 shell（bash 优先，没有就 sh）。
    Exec,
    /// root + shell：装包、改文件、看 root 才能看的路径。
    Root,
    /// 调试图：镜像里连 sh 都没有（distroless / scratch）时，
    /// 借同一个 pid / net / 卷跑一个 busybox。
    Debug,
}

impl ConsoleMode {
    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "exec" | "" => Some(ConsoleMode::Exec),
            "root" => Some(ConsoleMode::Root),
            "debug" => Some(ConsoleMode::Debug),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            ConsoleMode::Exec => "exec",
            ConsoleMode::Root => "root",
            ConsoleMode::Debug => "debug",
        }
    }

    /// 界面上的一句说明，说明这档会做什么、代价是什么。
    pub fn note(self) -> &'static str {
        match self {
            ConsoleMode::Exec => "以容器默认用户进入，bash 优先、没有就用 sh",
            ConsoleMode::Root => "以 root 进入，可以装包与改文件；权限更高",
            ConsoleMode::Debug => "调试图：镜像里没有 shell 时用，首次会拉 busybox",
        }
    }
}

/// 控制台命令。`cid` 必须是已核对的容器名。
///
/// shell 的选择走兄弟模块 `shell::pick_shell(false)`（非登录 shell）：容器里没有
/// profile 可读，`-l` 只会多跑一遍登录脚本。规则本身只维护一份，见 `shell.rs`。
pub fn console_command(mode: ConsoleMode, cid: &str) -> Result<String, String> {
    validate_cid(cid)?;
    let shell = pick_shell(false);
    Ok(match mode {
        ConsoleMode::Exec => format!("docker exec -it {cid} {shell}"),
        ConsoleMode::Root => format!("docker exec -it -u root {cid} {shell}"),
        // `--rm` + 别的容器命名空间：退出即销毁，不留下一个「调试图容器」。
        ConsoleMode::Debug => format!(
            "docker run --rm -it --pid=container:{cid} --net=container:{cid} --volumes-from {cid} busybox sh"
        ),
    })
}

// ─────────────────────── 白名单与状态机 ───────────────────────

/// 动作 → docker 子命令。枚举是封闭的，`verb` 只可能来自这张表。
pub fn action_verb(action: ContainerAction) -> &'static str {
    match action {
        ContainerAction::Start => "start",
        ContainerAction::Stop => "stop",
        ContainerAction::Restart => "restart",
    }
}

/// 运行中能做什么。
const RUNNING_ACTIONS: &[ContainerAction] = &[ContainerAction::Restart, ContainerAction::Stop];
/// 被外部（命令行 `docker pause`）冻住的容器只给「停止」。
///
/// 不提供 unpause：本产品不做暂停，也就不该出现「继续」这个入口。
/// 真遇到 paused 的容器，停掉再启动是唯一出路——比留一个语义模糊的按钮清楚。
const PAUSED_ACTIONS: &[ContainerAction] = &[ContainerAction::Stop];
const RESTARTING_ACTIONS: &[ContainerAction] = &[ContainerAction::Stop];
const STOPPED_ACTIONS: &[ContainerAction] = &[ContainerAction::Start];

/// 当前状态下允许的动作。
///
/// **菜单与后端校验共用这一份**：两边各写一张表，早晚会出现「菜单能点、后端 409」。
pub fn allowed_actions(state: &str) -> &'static [ContainerAction] {
    match state {
        "running" => RUNNING_ACTIONS,
        "paused" => PAUSED_ACTIONS,
        "restarting" => RESTARTING_ACTIONS,
        _ => STOPPED_ACTIONS,
    }
}

/// 容器 id 或名字的白名单。
///
/// 两层：正则挡 shell 注入，业务层的清单校验挡「猜一个名字乱删」。这里只做第一层。
pub fn validate_cid(cid: &str) -> Result<(), String> {
    if cid.is_empty() {
        return Err("容器 ID 不能为空".to_string());
    }
    if cid.len() > 80 {
        return Err("容器 ID 格式无效：过长".to_string());
    }
    let mut chars = cid.chars();
    let first = chars.next().expect("已判空");
    if !first.is_ascii_alphanumeric() {
        return Err("容器 ID 格式无效".to_string());
    }
    if !cid
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-'))
    {
        return Err("容器 ID 格式无效".to_string());
    }
    Ok(())
}

/// 镜像名：`repo/name:tag@sha256:…`，字符集比 cid 宽一点，但仍不接受空白与引号。
pub fn validate_image(image: &str) -> Result<(), String> {
    if image.is_empty() {
        return Err("镜像名不能为空".to_string());
    }
    if image.len() > 255 {
        return Err("镜像名格式无效：过长".to_string());
    }
    if !image
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-' | '/' | ':' | '@'))
    {
        return Err("镜像名格式无效".to_string());
    }
    Ok(())
}

/// rfc3339 只放行时间戳字符，`--since` 直接拼进命令行。
fn is_timestamp(text: &str) -> bool {
    !text.is_empty()
        && text.len() <= 40
        && text
            .chars()
            .all(|c| c.is_ascii_digit() || matches!(c, '-' | ':' | '+' | '.' | 'T' | 'Z'))
}

// ─────────────────────── 解析 ───────────────────────

/// 列表输出 → `ContainerList`。
pub fn parse_list(raw: &str) -> ContainerList {
    let sections = split_sections(raw);
    let mut list = ContainerList::default();

    let Some(lines) = sections.get("c_list") else {
        list.degraded.push("容器列表".to_string());
        return list;
    };
    if lines.iter().any(|line| line.trim() == "__no_engine__") {
        return list;
    }

    list.engine = Some("docker".to_string());
    for line in lines {
        if let Some(item) = parse_list_line(line) {
            list.items.push(item);
        }
    }

    list.total = list.items.len();
    list.running = list.items.iter().filter(|i| i.state == "running").count();
    list.exited = list.items.iter().filter(|i| i.state == "exited").count();
    list.paused = list.items.iter().filter(|i| i.state == "paused").count();
    list.restarting = list
        .items
        .iter()
        .filter(|i| i.state == "restarting")
        .count();
    list
}

/// 统一成 12 位短 id。
///
/// `ps -a --no-trunc` 给的是 64 位完整 id，`stats` 只给 12 位——两份数据要按 id
/// 合并，长度不先对齐就只能靠前缀猜（真机上第一次跑就踩到了）。
fn short_id(id: &str) -> String {
    let id = id.trim();
    match id.get(..12) {
        Some(head) if id.len() > 12 => head.to_string(),
        _ => id.to_string(),
    }
}

fn parse_list_line(line: &str) -> Option<ContainerItem> {
    let fields: Vec<&str> = line.split('|').collect();
    if fields.len() < 7 {
        return None;
    }
    let status = fields[4].trim().to_string();
    let state = fields[3].trim().to_string();

    Some(ContainerItem {
        id: short_id(fields[0]),
        // `{{.Names}}` 在容器有多个名字时是 `a,b`，列表只认第一个。
        name: fields[1].split(',').next().unwrap_or("").trim().to_string(),
        image: fields[2].trim().to_string(),
        state,
        health: parse_health(&status),
        compose_project: fields
            .get(7)
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .map(str::to_string),
        created_at: short_time(fields[6].trim()),
        exit_code: parse_exit_code(&status),
        status,
        restarts: 0,
        ports: parse_ports(fields[5]),
        mounts: Vec::new(),
        networks: Vec::new(),
        cpu_percent: 0.0,
        mem_bytes: 0,
        mem_limit_bytes: 0,
        net_rx_bytes: 0,
        net_tx_bytes: 0,
    })
}

/// `Up 3 days (healthy)` → `healthy`；`(health: starting)` 也认。
fn parse_health(status: &str) -> Option<String> {
    let open = status.find('(')?;
    let close = status[open..].find(')')? + open;
    let inner = status[open + 1..close].trim();
    if let Some(rest) = inner.strip_prefix("health:") {
        return Some(rest.trim().to_string());
    }
    matches!(inner, "healthy" | "unhealthy").then(|| inner.to_string())
}

/// `Exited (1) 4 hours ago` → `Some(1)`。
fn parse_exit_code(status: &str) -> Option<i32> {
    let rest = status.strip_prefix("Exited (")?;
    let end = rest.find(')')?;
    rest[..end].parse().ok()
}

/// `2026-11-03 21:04:05 +0800 CST` → `2026-11-03 21:04`。
fn short_time(text: &str) -> String {
    let trimmed = text.trim();
    if trimmed.len() >= 16 && trimmed.is_char_boundary(16) {
        trimmed[..16].to_string()
    } else {
        trimmed.to_string()
    }
}

/// `0.0.0.0:2283->2283/tcp, :::2283->2283/tcp` → 结构化，并按映射去重。
///
/// docker 对同一条映射同时给 IPv4 与 IPv6 两项，界面上一行显示两遍没有意义；
/// 保留 `0.0.0.0` 那条（IPv6 全绑定与它等价），显式绑定的 `127.0.0.1` 优先保留。
pub fn parse_ports(text: &str) -> Vec<ContainerPort> {
    let mut out: Vec<ContainerPort> = Vec::new();

    for piece in text.split(',') {
        let piece = piece.trim();
        if piece.is_empty() {
            continue;
        }
        let Some((left, proto)) = piece.split_once('/') else {
            continue;
        };
        let Some((host, container)) = left.split_once("->") else {
            continue;
        };

        let Ok(container_port) = container.trim().parse::<u16>() else {
            continue;
        };

        // `host` 可能是 `0.0.0.0:2283`、`127.0.0.1:5432`、`:::2283`、`2283`。
        let (host_ip, host_port) = match host.rsplit_once(':') {
            Some((ip, port)) => (normalize_ip(ip), port.trim().parse::<u16>().ok()),
            None => ("0.0.0.0".to_string(), host.trim().parse::<u16>().ok()),
        };
        let Some(host_port) = host_port else { continue };

        let port = ContainerPort {
            host_ip,
            host_port,
            container_port,
            proto: proto.trim().to_string(),
        };

        let duplicated = out.iter().any(|p| {
            p.host_port == port.host_port
                && p.container_port == port.container_port
                && p.proto == port.proto
        });
        if !duplicated {
            out.push(port);
        }
    }

    out
}

/// `::` 与 `0.0.0.0` 都表示"绑所有地址"，统一成 `0.0.0.0`，界面只呈现一种写法。
fn normalize_ip(ip: &str) -> String {
    let ip = ip.trim();
    if ip.is_empty() || ip == "::" || ip == "*" {
        "0.0.0.0".to_string()
    } else {
        ip.to_string()
    }
}

/// 批量 inspect → 网络与卷，按容器名合并进列表。
pub fn merge_inspect(list: &mut ContainerList, raw: &str) {
    let sections = split_sections(raw);
    let Some(lines) = sections.get("c_inspect") else {
        list.degraded.push("网络与卷".to_string());
        return;
    };

    for line in lines {
        let fields: Vec<&str> = line.split('|').collect();
        if fields.len() < 3 {
            continue;
        }
        let name = fields[0].trim().trim_start_matches('/');

        let networks = fields[1]
            .split(',')
            .filter_map(|pair| {
                let (net, ip) = pair.split_once('=')?;
                let net = net.trim();
                if net.is_empty() {
                    return None;
                }
                Some(ContainerNetwork {
                    name: net.to_string(),
                    ip: ip.trim().to_string(),
                })
            })
            .collect::<Vec<_>>();

        let mounts = fields[2]
            .split(',')
            .filter_map(|pair| {
                let (source, tail) = pair.split_once("=>")?;
                let (destination, mode) = tail.rsplit_once(':')?;
                if source.trim().is_empty() {
                    return None;
                }
                Some(ContainerMount {
                    source: source.trim().to_string(),
                    destination: destination.trim().to_string(),
                    mode: mode.trim().to_string(),
                })
            })
            .collect::<Vec<_>>();

        if let Some(item) = list.items.iter_mut().find(|i| i.name == name) {
            item.networks = networks;
            item.mounts = mounts;
        }
    }
}

/// stats 输出 → CPU / 内存 / 网络，按 id（或 id 前缀）合并。
pub fn merge_stats(list: &mut ContainerList, raw: &str) {
    let sections = split_sections(raw);
    let Some(lines) = sections.get("c_stats") else {
        list.degraded.push("资源".to_string());
        return;
    };

    for line in lines {
        let fields: Vec<&str> = line.split('|').collect();
        if fields.len() < 4 {
            continue;
        }
        let id = fields[0].trim();
        if id.is_empty() {
            continue;
        }
        let cpu = parse_percent(fields[1]);
        let (used, limit) = parse_mem_usage(fields[2]);
        let (rx, tx) = parse_net_io(fields[3]);

        let Some(item) = list
            .items
            .iter_mut()
            .find(|i| i.id == id || i.id.starts_with(id))
        else {
            continue;
        };
        item.cpu_percent = cpu;
        item.mem_bytes = used;
        item.mem_limit_bytes = limit;
        item.net_rx_bytes = rx;
        item.net_tx_bytes = tx;
    }
}

fn parse_percent(text: &str) -> f64 {
    text.trim()
        .trim_end_matches('%')
        .trim()
        .parse()
        .unwrap_or(0.0)
}

/// `412.3MiB / 8GiB` → `(432_537_190, 8_589_934_592)`。
fn parse_mem_usage(text: &str) -> (u64, u64) {
    let mut parts = text.split('/');
    let used = parts.next().map(parse_size).unwrap_or(0);
    let limit = parts.next().map(parse_size).unwrap_or(0);
    (used, limit)
}

/// `1.2GB / 3.4MB` → `(1_200_000_000, 3_400_000)`（docker 的 NetIO 用十进制单位）。
fn parse_net_io(text: &str) -> (u64, u64) {
    let mut parts = text.split('/');
    let rx = parts.next().map(parse_size).unwrap_or(0);
    let tx = parts.next().map(parse_size).unwrap_or(0);
    (rx, tx)
}

/// `412.3MiB` / `1.2GB` / `0B` → 字节。
///
/// docker 混着用二进制（内存 `MiB`）与十进制（网络 `MB`）单位，按后缀区分：
/// `iB` 结尾按 1024 进制，`B` 结尾按 1000 进制。
fn parse_size(text: &str) -> u64 {
    let text = text.trim();
    if text.is_empty() {
        return 0;
    }
    let digits_end = text
        .find(|c: char| !(c.is_ascii_digit() || c == '.'))
        .unwrap_or(text.len());
    let (number, unit) = text.split_at(digits_end);
    let Ok(value) = number.parse::<f64>() else {
        return 0;
    };

    let factor = match unit.trim() {
        "B" => 1.0,
        "kB" | "KB" => 1e3,
        "MB" => 1e6,
        "GB" => 1e9,
        "TB" => 1e12,
        "KiB" => 1024.0,
        "MiB" => 1024.0 * 1024.0,
        "GiB" => 1024.0 * 1024.0 * 1024.0,
        "TiB" => 1024.0_f64.powi(4),
        _ => 1.0,
    };
    (value * factor) as u64
}

/// 详情输出里除 env 之外的字段。
pub struct DetailParts {
    pub exit_code: Option<i32>,
    pub restarts: u32,
    pub command: String,
    pub restart_policy: String,
    pub env: Vec<ContainerEnv>,
}

pub fn parse_detail(raw: &str) -> Option<DetailParts> {
    let sections = split_sections(raw);
    let main = sections.get("c_detail")?.first()?;
    let fields: Vec<&str> = main.split('|').collect();
    if fields.len() < 6 {
        return None;
    }

    let command = fields[4]
        .trim()
        .trim_start_matches('[')
        .trim_end_matches(']')
        .to_string();

    let env = sections
        .get("c_env")
        .map(|lines| lines.as_slice())
        .unwrap_or_default()
        .iter()
        .filter_map(|line| {
            let (key, value) = line.split_once('=')?;
            let key = key.trim();
            if key.is_empty() {
                return None;
            }
            Some(ContainerEnv {
                key: key.to_string(),
                value: mask_if_sensitive(key, value),
                masked: is_sensitive_key(key),
            })
        })
        .collect();

    Some(DetailParts {
        exit_code: fields[1].trim().parse().ok(),
        restarts: fields[2].trim().parse().unwrap_or(0),
        command,
        restart_policy: fields[3].trim().to_string(),
        env,
    })
}

fn is_sensitive_key(key: &str) -> bool {
    let upper = key.to_ascii_uppercase();
    [
        "PASSWORD",
        "PASSWD",
        "SECRET",
        "TOKEN",
        "PRIVATE_KEY",
        "API_KEY",
    ]
    .iter()
    .any(|needle| upper.contains(needle))
}

/// 敏感值不出核心：界面只看到 `••••••`。
fn mask_if_sensitive(key: &str, value: &str) -> String {
    if is_sensitive_key(key) && !value.is_empty() {
        "••••••••".to_string()
    } else {
        value.to_string()
    }
}

/// 日志输出 → 行数组。`truncated` 只在明显被 `--tail` 截断时置位。
pub fn parse_logs(raw: &str, tail: usize) -> ContainerLogs {
    let sections = split_sections(raw);
    let lines: Vec<String> = sections
        .get("c_logs")
        .map(|lines| lines.iter().map(|line| line.to_string()).collect())
        .unwrap_or_default();

    let truncated = lines.len() >= tail;
    let next_since = lines
        .last()
        .and_then(|line| line.split_whitespace().next())
        .unwrap_or("")
        .to_string();

    ContainerLogs {
        lines,
        truncated,
        next_since,
    }
}

/// 资源档：3 秒一次，单独回，不混进列表。
pub fn parse_stats(raw: &str) -> Vec<ContainerStats> {
    let sections = split_sections(raw);
    let Some(lines) = sections.get("c_stats") else {
        return Vec::new();
    };

    lines
        .iter()
        .filter_map(|line| {
            let fields: Vec<&str> = line.split('|').collect();
            if fields.len() < 4 {
                return None;
            }
            let id = fields[0].trim();
            if id.is_empty() {
                return None;
            }
            let (used, limit) = parse_mem_usage(fields[2]);
            let (rx, tx) = parse_net_io(fields[3]);
            Some(ContainerStats {
                id: id.to_string(),
                cpu_percent: parse_percent(fields[1]),
                mem_bytes: used,
                mem_limit_bytes: limit,
                net_rx_bytes: rx,
                net_tx_bytes: tx,
            })
        })
        .collect()
}

/// 对照本次采集清单确认容器真实存在，并归一成容器名。
///
/// `validate_cid` 挡的是 shell 注入，这一步挡的是「猜一个不存在的名字」：
/// 两次校验缺一不可——前者保证命令安全，后者保证操作对象真实。
pub fn resolve_cid(list: &ContainerList, cid: &str) -> Result<String, String> {
    validate_cid(cid)?;
    if list.engine.is_none() {
        return Err("这台机器上没有容器引擎".to_string());
    }

    // 短 ID 前缀只有 6 位以上才认，避免 `a` 命中一堆容器。
    // 两个方向都认：列表里存 12 位，用户可能粘前 8 位，也可能粘完整 64 位。
    let prefix_ok = cid.len() >= 6;
    list.items
        .iter()
        .find(|item| {
            item.id == cid
                || item.name == cid
                || (prefix_ok && (item.id.starts_with(cid) || cid.starts_with(&item.id)))
        })
        .map(|item| item.name.clone())
        .ok_or_else(|| format!("容器不存在：{cid}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_LIST: &str = "@@c_list
a1f93c2e7b10|immich_server|ghcr.io/immich-app/immich-server:v1.119.0|running|Up 3 days (healthy)|0.0.0.0:2283->2283/tcp, :::2283->2283/tcp|2026-11-03 21:04:05 +0800 CST|immich
6de220b4c918|portainer|portainer/portainer-ce:2.20.3|exited|Exited (0) 2 hours ago||2026-10-28 09:12:31 +0800 CST|
@@end
";

    const SAMPLE_INSPECT: &str = "@@c_inspect
/immich_server|immich-net=172.18.0.4,|/mnt/user/appdata/immich=>/data:rw,/mnt/user/photos=>/photos:ro,
/portainer|bridge=172.17.0.2,|/var/run/docker.sock=>/var/run/docker.sock:rw,
@@end
";

    const SAMPLE_STATS: &str = "@@c_stats
a1f93c2e7b10|2.45%|412.3MiB / 8GiB|1.2GB / 34MB
6de220b4c918|0.00%|0B / 0B|0B / 0B
@@end
";

    #[test]
    fn list_is_parsed_with_counts() {
        let list = parse_list(SAMPLE_LIST);
        assert_eq!(list.engine.as_deref(), Some("docker"));
        assert_eq!(list.total, 2);
        assert_eq!(list.running, 1);
        assert_eq!(list.exited, 1);
        assert_eq!(list.items[0].name, "immich_server");
        assert_eq!(list.items[0].health.as_deref(), Some("healthy"));
        assert_eq!(list.items[0].compose_project.as_deref(), Some("immich"));
        assert_eq!(list.items[1].exit_code, Some(0));
        assert_eq!(list.items[0].created_at, "2026-11-03 21:04");
    }

    #[test]
    fn no_engine_is_not_a_degradation() {
        let list = parse_list("@@c_list\n__no_engine__\n@@end\n");
        assert!(list.engine.is_none());
        assert!(list.items.is_empty());
        assert!(list.degraded.is_empty());
    }

    #[test]
    fn ports_are_structured_and_ipv6_duplicates_dropped() {
        let ports =
            parse_ports("0.0.0.0:2283->2283/tcp, :::2283->2283/tcp, 127.0.0.1:6379->6379/tcp");
        assert_eq!(ports.len(), 2);
        assert_eq!(ports[0].host_ip, "0.0.0.0");
        assert_eq!(ports[0].host_port, 2283);
        assert_eq!(ports[0].container_port, 2283);
        assert_eq!(ports[0].proto, "tcp");
        assert_eq!(ports[1].host_ip, "127.0.0.1");
        assert_eq!(ports[1].host_port, 6379);
    }

    #[test]
    fn containers_without_ports_do_not_panic() {
        assert!(parse_ports("").is_empty());
        assert!(parse_ports("weird").is_empty());
    }

    #[test]
    fn inspect_merges_networks_and_mounts() {
        let mut list = parse_list(SAMPLE_LIST);
        merge_inspect(&mut list, SAMPLE_INSPECT);

        let immich = &list.items[0];
        assert_eq!(immich.networks.len(), 1);
        assert_eq!(immich.networks[0].name, "immich-net");
        assert_eq!(immich.networks[0].ip, "172.18.0.4");
        assert_eq!(immich.mounts.len(), 2);
        assert_eq!(immich.mounts[1].mode, "ro");
        assert_eq!(immich.mounts[1].destination, "/photos");
    }

    #[test]
    fn stats_merge_by_id() {
        let mut list = parse_list(SAMPLE_LIST);
        merge_stats(&mut list, SAMPLE_STATS);

        let immich = &list.items[0];
        assert!((immich.cpu_percent - 2.45).abs() < 0.01);
        assert_eq!(immich.mem_bytes, (412.3 * 1024.0 * 1024.0) as u64);
        assert_eq!(immich.mem_limit_bytes, 8 * 1024 * 1024 * 1024);
        assert_eq!(immich.net_rx_bytes, 1_200_000_000);
        assert_eq!(immich.net_tx_bytes, 34_000_000);
    }

    #[test]
    fn sizes_use_binary_and_decimal_units() {
        assert_eq!(parse_size("1B"), 1);
        assert_eq!(parse_size("1kB"), 1000);
        assert_eq!(parse_size("1KiB"), 1024);
        assert_eq!(parse_size("1MiB"), 1024 * 1024);
        assert_eq!(parse_size("1GiB"), 1024 * 1024 * 1024);
        assert_eq!(parse_size("0B"), 0);
        assert_eq!(parse_size(""), 0);
    }

    #[test]
    fn detail_parses_command_and_masks_secrets() {
        let raw = "@@c_detail
running|0|0|unless-stopped|[start.sh]|ghcr.io/immich-app/immich-server:v1.119.0
@@c_env
PATH=/usr/bin
DB_PASSWORD=hunter2
TZ=Asia/Shanghai
fi
";
        let parts = parse_detail(raw).expect("detail");
        assert_eq!(parts.command, "start.sh");
        assert_eq!(parts.restart_policy, "unless-stopped");
        assert_eq!(parts.env.len(), 3);
        assert!(parts.env[1].masked);
        assert_eq!(parts.env[1].value, "••••••••");
        assert!(!parts.env[2].masked);
    }

    /// 真机上踩到：主数据行里带 `=` 时，曾经被当成 env 的第一条混进来。
    #[test]
    fn detail_main_row_never_becomes_an_env_entry() {
        let raw = "@@c_detail
running|0|0|unless-stopped|[mariadbd --default-time-zone=+08:00]|mariadb:11
@@c_env
MARIADB_ROOT_PASSWORD=s3cret
PATH=/usr/bin
fi
";
        let parts = parse_detail(raw).expect("detail");
        assert_eq!(parts.env.len(), 2, "只有两条真正的环境变量");
        assert_eq!(parts.env[0].key, "MARIADB_ROOT_PASSWORD");
        assert!(parts.env[0].masked);
        assert_eq!(parts.env[0].value, "••••••••");
        assert!(
            !parts.env.iter().any(|row| row.key.contains('|')),
            "主数据行不该出现在 env 里"
        );
    }

    #[test]
    fn cid_whitelist_rejects_injection() {
        assert!(validate_cid("immich_server").is_ok());
        assert!(validate_cid("a1f93c2e7b10").is_ok());
        assert!(validate_cid("").is_err());
        assert!(validate_cid("; rm -rf /").is_err());
        assert!(validate_cid("$(id)").is_err());
        assert!(validate_cid("a b").is_err());
        assert!(validate_cid("a'b").is_err());
        assert!(validate_cid(&"a".repeat(81)).is_err());
    }

    #[test]
    fn allowed_actions_follow_state() {
        assert_eq!(
            allowed_actions("running"),
            &[ContainerAction::Restart, ContainerAction::Stop]
        );
        // 被外部冻住的容器只能停：本产品不做暂停，也就不给「继续」
        assert_eq!(allowed_actions("paused"), &[ContainerAction::Stop]);
        assert_eq!(allowed_actions("restarting"), &[ContainerAction::Stop]);
        assert_eq!(allowed_actions("exited"), &[ContainerAction::Start]);
        assert_eq!(allowed_actions("created"), &[ContainerAction::Start]);
    }

    #[test]
    fn scripts_are_single_quote_free() {
        let scripts = [
            list_script(),
            stats_script(),
            detail_script("immich_server").expect("detail"),
            logs_script("immich_server", 200, None).expect("logs"),
            logs_script("immich_server", 200, Some("2026-11-17T06:12:03Z")).expect("logs since"),
            action_script(ContainerAction::Stop, "immich_server").expect("action"),
            remove_script("immich_server", true).expect("remove"),
        ];

        for script in scripts {
            let body = script
                .strip_prefix("sh -c '")
                .and_then(|text| text.strip_suffix('\''))
                .expect("脚本应被 sh -c 单引号包裹");
            assert!(!body.contains('\''), "脚本正文不能含单引号：{body}");
        }
    }

    #[test]
    fn read_only_scripts_have_no_write_commands() {
        for script in [
            list_script(),
            stats_script(),
            logs_script("web", 50, None).expect("logs"),
        ] {
            for forbidden in [
                "docker rm",
                "docker stop",
                "docker start",
                "docker restart",
                "docker rmi",
            ] {
                assert!(
                    !script.contains(forbidden),
                    "采集脚本不能含写操作：{forbidden}"
                );
            }
        }
    }

    #[test]
    fn remove_script_never_carries_rmi() {
        // 删镜像是单独一条：顺序是「先删容器 → 再查占用 → 最后才 rmi」，
        // 混在一条脚本里就没法在中间判断「别人还在用」。
        let plain = remove_script("web", false).expect("rm");
        assert!(plain.contains("docker rm web"));
        assert!(!plain.contains("rmi"));

        let forced = remove_script("web", true).expect("rm -f");
        assert!(forced.contains("docker rm -f web"));
        assert!(!forced.contains("rmi"), "rmi 不该出现在删除容器的脚本里");

        let rmi = rmi_script("nginx:1.27").expect("rmi");
        assert!(rmi.contains("docker rmi nginx:1.27"));
        assert!(rmi_script("a; rm -rf /").is_err(), "镜像名仍走白名单");
    }

    #[test]
    fn logs_since_rejects_injection() {
        assert!(logs_script("web", 200, Some("2026-11-17T06:12:03Z")).is_ok());
        assert!(logs_script("web", 200, Some("1h; rm -rf /")).is_err());
        assert!(logs_script("web", 200, Some("$(id)")).is_err());
    }

    /// 删镜像前要靠它判断「还有谁在用」：按 tag 字面过滤，不能按 image ID。
    #[test]
    fn image_users_match_tag_literally() {
        // `redis-web` 用的是原 tag（同一个 image ID），删副本 tag 不该被它挡住
        let raw =
            "@@c_users\nredis-test|tl-redis-wt:tmp\nredis-web|redis:8.10.3\nblank|\n\n@@c_end\n";
        assert_eq!(
            parse_image_users(raw, "tl-redis-wt:tmp"),
            vec!["redis-test"],
            "只算引用这个 tag 的容器"
        );
        assert_eq!(parse_image_users(raw, "redis:8.10.3"), vec!["redis-web"]);
        assert!(parse_image_users(raw, "nobody:uses").is_empty());
        assert!(parse_image_users("@@c_users\n@@c_end\n", "x:y").is_empty());
        assert!(parse_image_users("", "x:y").is_empty());
    }

    #[test]
    fn console_modes_map_to_whitelisted_commands() {
        let exec = console_command(ConsoleMode::Exec, "immich_server").expect("exec");
        assert!(exec.starts_with("docker exec -it immich_server "));

        let root = console_command(ConsoleMode::Root, "immich_server").expect("root");
        assert!(root.contains(" -u root "));

        let debug = console_command(ConsoleMode::Debug, "immich_server").expect("debug");
        assert!(debug.contains("--pid=container:immich_server"));
        assert!(debug.contains("--net=container:immich_server"));
        assert!(debug.contains("--volumes-from immich_server"));
        assert!(debug.contains("--rm"));

        // 注入串仍被 cid 白名单挡住
        assert!(console_command(ConsoleMode::Exec, "a; rm -rf /").is_err());
    }

    /// 真机上第一次跑就踩到：`ps --no-trunc` 给 64 位，`stats` 只给 12 位。
    #[test]
    fn ids_are_shortened_so_the_two_passes_merge() {
        let raw = "@@c_list\nad52896aaf1e3a93a4719698b70d4a2d9236a97dd4889e576d4d98dae81cee70|web|nginx:1.27|running|Up 2 hours||2026-11-17 10:00:00 +0800 CST|\n@@end\n";
        let mut list = parse_list(raw);
        assert_eq!(list.items[0].id, "ad52896aaf1e");

        // 12 位的 stats 现在能精确对上，不必再靠前缀猜
        merge_stats(
            &mut list,
            "@@c_stats\nad52896aaf1e|1.00%|1MiB / 2MiB|0B / 0B\n@@end\n",
        );
        assert!((list.items[0].cpu_percent - 1.0).abs() < 0.01);
    }

    #[test]
    fn short_id_leaves_odd_input_alone() {
        assert_eq!(short_id("ad52896aaf1e"), "ad52896aaf1e");
        assert_eq!(short_id("  ad52896aaf1e  "), "ad52896aaf1e");
        assert_eq!(short_id("short"), "short");
        assert_eq!(short_id(""), "");
    }

    #[test]
    fn resolve_cid_accepts_partial_and_full_ids() {
        let list = parse_list(SAMPLE_LIST); // 里面是 12 位的 a1f93c2e7b10
        assert_eq!(
            resolve_cid(&list, "a1f93c2e7b10").expect("全 12 位"),
            "immich_server"
        );
        assert_eq!(
            resolve_cid(&list, "a1f93c2").expect("前 7 位"),
            "immich_server"
        );
        // 用户从 `docker ps --no-trunc` 里粘来的完整 id 也要认
        assert_eq!(
            resolve_cid(
                &list,
                "a1f93c2e7b10ffff0000000000000000000000000000000000000000000000ff"
            )
            .expect("完整 id"),
            "immich_server"
        );
        assert!(resolve_cid(&list, "a1f").is_err(), "3 位前缀太短，不该命中");
    }

    #[test]
    fn console_mode_parse_is_closed_set() {
        assert_eq!(ConsoleMode::parse("exec"), Some(ConsoleMode::Exec));
        assert_eq!(ConsoleMode::parse(""), Some(ConsoleMode::Exec));
        assert_eq!(ConsoleMode::parse("root"), Some(ConsoleMode::Root));
        assert_eq!(ConsoleMode::parse("debug"), Some(ConsoleMode::Debug));
        assert_eq!(ConsoleMode::parse("bash"), None);
        assert_eq!(ConsoleMode::parse("sh -i"), None);
    }
}
