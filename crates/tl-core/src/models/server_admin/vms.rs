//! 虚拟机管理（libvirt）：采集脚本、输出解析、动作白名单与状态机。
//!
//! 归属：`server_admin` 模型内部文件组。拥有：`virsh` 脚本正文、`dominfo` / `domstats` /
//! `domiflist` / `domifaddr` / `dumpxml` 的解析口径、域名白名单、当前状态允许哪些动作。
//! 提供：`list_script()` / `detail_script()` / `action_script()` / `parse_list()` 等。
//! 允许调用：`tl_protocol`（DTO）、标准库、兄弟模块 `parse::split_sections`。
//!
//! 纪律与容器那套一致：
//! - **只读脚本与写操作脚本分开**：列表/详情里不许出现 `start` / `destroy` / `autostart`；
//! - **无单引号**：整段被 `sh -c '...'` 包裹，正文再出现单引号会把命令截断；
//! - **域名双重校验**：字符白名单 + 业务层对照本次采集清单；
//! - **动作是枚举不是字符串**：命令只由白名单拼出来。
//!
//! 两个口径上的决定：
//! - **不给运行时长**：libvirt 不暴露域启动时刻（`domstats --state` 只有状态与原因，
//!   `dominfo` 只有累计 CPU 时间）。猜 qemu 进程启动时间在改了进程名的机器上不可靠，
//!   宁可不显示；
//! - **速率不在核心算**：`cpu.time` / `net.N.rx.bytes` 都是累计值，模型层无状态、不留上一拍，
//!   所以照实传累计，前端两次采样自己算（容器管理也是这样）。

use std::collections::HashMap;
use std::time::Duration;

use tl_protocol::{VmAction, VmDetail, VmDisk, VmItem, VmList, VmNet};

use super::parse::split_sections;

/// 列表的预算：3N+2 条 virsh，慢机器也要够。
pub const LIST_BUDGET: Duration = Duration::from_secs(20);
/// 详情：多一条 `dumpxml`，同为 20s。
pub const DETAIL_BUDGET: Duration = Duration::from_secs(20);
/// 写操作：`shutdown` 只是发 ACPI 请求，秒回；`destroy` 可能等 qemu 收尾。
pub const ACTION_BUDGET: Duration = Duration::from_secs(30);

// ─────────────────────── 脚本 ───────────────────────

const LIST_SECTION: &str = r#"echo @@v_engine
if command -v virsh >/dev/null 2>&1; then virsh --version 2>/dev/null; else echo __no_virsh__; fi
echo @@v_list
virsh list --all 2>&1
echo @@v_stats
virsh domstats --state --cpu-total --balloon --block --interface 2>/dev/null
virsh list --all --name 2>/dev/null | while IFS= read -r d; do
[ -n "$d" ] || continue
echo "@@v_info:$d"
virsh dominfo "$d" 2>/dev/null
echo "@@v_blk:$d"
virsh domblklist "$d" --details 2>/dev/null
echo "@@v_if:$d"
virsh domiflist "$d" 2>/dev/null
echo "@@v_ip:$d"
virsh domifaddr "$d" --source lease 2>/dev/null
virsh domblklist "$d" --details 2>/dev/null | while read -r dt dd tg ds; do
case "$tg" in Target|"") continue ;; esac
echo "@@v_disk:$d:$tg"
virsh domblkinfo "$d" "$tg" 2>/dev/null
done
done
echo @@v_end
"#;

fn wrap(body: &str) -> String {
    format!("sh -c '{body}'")
}

/// 列表：版本 + 状态表 + 全部域的 `domstats` + 逐域的三份明细，一次往返。
///
/// 逐域那三条之所以塞进同一条 exec：单次 SSH 往返约 100ms，6 台域各开一次就是 600ms 起；
/// 一次 exec 里跑 3N 条 virsh，每条只是本地进程，20ms 上下。
pub fn list_script() -> String {
    wrap(LIST_SECTION)
}

/// 单台详情（抽屉打开时）。域名走白名单，直接拼、不加引号。
pub fn detail_script(name: &str) -> Result<String, String> {
    validate_name(name)?;
    let body = format!(
        "echo @@v_xml\nvirsh dumpxml {name} 2>/dev/null\n\
         echo @@v_info\nvirsh dominfo {name} 2>/dev/null\n\
         echo @@v_blk\nvirsh domblklist {name} --details 2>/dev/null\n\
         virsh domblklist {name} --details 2>/dev/null | while read -r dt dd tg ds; do\n\
         case \"$tg\" in Target|\"\") continue ;; esac\n\
         echo \"@@v_disk:{name}:$tg\"\nvirsh domblkinfo {name} \"$tg\" 2>/dev/null\n\
         done\n\
         echo @@v_if\nvirsh domiflist {name} 2>/dev/null\n\
         echo @@v_ip\nvirsh domifaddr {name} --source lease 2>/dev/null\n\
         echo @@v_stats\nvirsh domstats --state --cpu-total --balloon --block --interface {name} 2>/dev/null\n\
         echo @@v_display\nvirsh domdisplay {name} 2>/dev/null\n\
         echo @@v_end\n"
    );
    Ok(wrap(&body))
}

/// 生命周期动作：`virsh start|shutdown|reboot|suspend|resume|destroy`，随后回读一次状态。
pub fn action_script(action: VmAction, name: &str) -> Result<String, String> {
    validate_name(name)?;
    let verb = action_verb(action);
    Ok(wrap(&format!(
        "echo @@v_action\nvirsh {verb} {name} 2>&1\n\
         echo @@v_state\nvirsh domstate {name} 2>/dev/null\n\
         echo @@v_end\n"
    )))
}

/// 开机自启：`virsh autostart` / `virsh autostart --disable`。
///
/// 单独一条脚本而不是塞进 `action_script`：它不是状态迁移，执行完域还是原来那个状态，
/// 混在一起会让「动作后回读状态」这件事看起来没生效。
pub fn autostart_script(name: &str, enabled: bool) -> Result<String, String> {
    validate_name(name)?;
    let flag = if enabled { "" } else { " --disable" };
    Ok(wrap(&format!(
        "echo @@v_action\nvirsh autostart{flag} {name} 2>&1\n\
         echo @@v_info\nvirsh dominfo {name} 2>/dev/null\n\
         echo @@v_end\n"
    )))
}

/// 串口控制台：`virsh console <name>`。VNC 不在本期。
pub fn console_command(name: &str) -> Result<String, String> {
    validate_name(name)?;
    Ok(format!("virsh console {name}"))
}

// ─────────────────────── 白名单与状态机 ───────────────────────

/// 动作 → virsh 子命令。枚举封闭，`verb` 只可能来自这张表。
pub fn action_verb(action: VmAction) -> &'static str {
    match action {
        VmAction::Start => "start",
        VmAction::Shutdown => "shutdown",
        VmAction::Reboot => "reboot",
        VmAction::Suspend => "suspend",
        VmAction::Resume => "resume",
        VmAction::Destroy => "destroy",
    }
}

const RUNNING_ACTIONS: &[VmAction] = &[
    VmAction::Shutdown,
    VmAction::Reboot,
    VmAction::Suspend,
    VmAction::Destroy,
];
/// 暂停（`virsh suspend`）之后只给「继续」；强制断电也留着，卡死时的唯一出口。
const PAUSED_ACTIONS: &[VmAction] = &[VmAction::Resume, VmAction::Destroy];
const SHUTOFF_ACTIONS: &[VmAction] = &[VmAction::Start];
/// `crashed`：qemu 已经没了，只能清理；清理完才允许再开机。
const CRASHED_ACTIONS: &[VmAction] = &[VmAction::Destroy];

/// 当前状态下允许的动作。**菜单与后端校验共用这一份**。
pub fn allowed_actions(state: &str) -> &'static [VmAction] {
    match state {
        "running" => RUNNING_ACTIONS,
        "paused" => PAUSED_ACTIONS,
        "shut off" => SHUTOFF_ACTIONS,
        "crashed" => CRASHED_ACTIONS,
        // `pmsuspended` 要 `dompmwakeup` 才能唤醒，本产品不做电源管理，只留清理。
        "pmsuspended" => CRASHED_ACTIONS,
        _ => SHUTOFF_ACTIONS,
    }
}

/// 域名白名单：与容器 `cid` 同一套字符集。
///
/// 第一层挡 shell 注入；第二层（业务层）挡「猜一个名字乱操作」。
pub fn validate_name(name: &str) -> Result<(), String> {
    if name.is_empty() {
        return Err("虚拟机名不能为空".to_string());
    }
    if name.len() > 80 {
        return Err("虚拟机名格式无效：过长".to_string());
    }
    let mut chars = name.chars();
    let first = chars.next().expect("已判空");
    if !first.is_ascii_alphanumeric() {
        return Err("虚拟机名格式无效".to_string());
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-'))
    {
        return Err("虚拟机名格式无效".to_string());
    }
    Ok(())
}

/// 域名必须在本次采集到的清单里——校验的是"这个域真实存在"，不是格式。
pub fn resolve_name(name: &str, known: &[String]) -> Result<String, String> {
    if known.iter().any(|n| n == name) {
        Ok(name.to_string())
    } else {
        Err("没有这台虚拟机（可能已被删除或改名）".to_string())
    }
}

// ─────────────────────── 解析 ───────────────────────

#[derive(Debug, Default, Clone)]
struct DomStats {
    cpu_time_ns: u64,
    balloon_current_kib: u64,
    balloon_rss: u64,
    disks: Vec<VmDisk>,
    /// 网卡名 → 累计收发字节。
    nets: HashMap<String, (u64, u64)>,
}

#[derive(Debug, Default, Clone)]
struct Iface {
    target: String,
    kind: String,
    source: String,
    model: String,
    mac: String,
}

fn kv_map(lines: &[&str]) -> HashMap<String, String> {
    lines
        .iter()
        .filter_map(|line| line.split_once(':'))
        .map(|(k, v)| (k.trim().to_string(), v.trim().to_string()))
        .collect()
}

fn first_number(text: &str) -> u64 {
    text.split_whitespace()
        .next()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(0)
}

/// `virsh list --all` → `(name, id, state)`。
fn parse_dom_list(lines: &[&str]) -> Vec<(String, Option<i64>, String)> {
    let mut out = Vec::new();
    for line in lines {
        let text = line.trim();
        if text.is_empty() || text.starts_with("Id") || text.chars().all(|c| c == '-') {
            continue;
        }
        let fields: Vec<&str> = text.split_whitespace().collect();
        if fields.len() < 3 {
            continue;
        }
        out.push((
            fields[1].to_string(),
            fields[0].parse::<i64>().ok(),
            fields[2..].join(" "),
        ));
    }
    out
}

/// `virsh domstats` 的 `Domain: 'x'` 分段与 `key=value`。
fn parse_domstats(lines: &[&str]) -> HashMap<String, DomStats> {
    let mut out: HashMap<String, DomStats> = HashMap::new();
    let mut current: Option<String> = None;
    let mut blocks: HashMap<usize, VmDisk> = HashMap::new();
    let mut nets: HashMap<usize, (String, u64, u64)> = HashMap::new();

    let flush = |out: &mut HashMap<String, DomStats>,
                 name: &str,
                 blocks: &mut HashMap<usize, VmDisk>,
                 nets: &mut HashMap<usize, (String, u64, u64)>| {
        if let Some(stat) = out.get_mut(name) {
            let mut disks: Vec<VmDisk> = blocks.drain().map(|(_, d)| d).collect();
            disks.sort_by(|a, b| a.target.cmp(&b.target));
            stat.disks = disks;
            for (_, (nic, rx, tx)) in nets.drain() {
                stat.nets.insert(nic, (rx, tx));
            }
        }
    };

    for line in lines {
        let text = line.trim();
        if let Some(rest) = text.strip_prefix("Domain: '") {
            if let Some(name) = &current {
                flush(&mut out, name, &mut blocks, &mut nets);
            }
            let name = rest.trim_end_matches('\'').to_string();
            out.entry(name.clone()).or_default();
            current = Some(name);
            continue;
        }
        let Some((key, value)) = text.split_once('=') else {
            continue;
        };
        let Some(name) = current.as_ref() else {
            continue;
        };
        let Some(stat) = out.get_mut(name) else {
            continue;
        };
        match key {
            "cpu.time" => stat.cpu_time_ns = value.parse().unwrap_or(0),
            "balloon.current" => stat.balloon_current_kib = value.parse().unwrap_or(0),
            "balloon.rss" => stat.balloon_rss = value.parse().unwrap_or(0),
            _ => {
                if let Some(rest) = key.strip_prefix("block.") {
                    let mut parts = rest.splitn(2, '.');
                    let idx = parts.next().and_then(|v| v.parse::<usize>().ok());
                    let field = parts.next().unwrap_or_default();
                    if let Some(idx) = idx {
                        let disk = blocks.entry(idx).or_default();
                        match field {
                            "name" => disk.target = value.to_string(),
                            "path" => disk.source = value.to_string(),
                            "capacity" => disk.capacity_bytes = value.parse().unwrap_or(0),
                            "allocation" => disk.allocation_bytes = value.parse().unwrap_or(0),
                            _ => {}
                        }
                    }
                } else if let Some(rest) = key.strip_prefix("net.") {
                    let mut parts = rest.splitn(2, '.');
                    let idx = parts.next().and_then(|v| v.parse::<usize>().ok());
                    let field = parts.next().unwrap_or_default();
                    if let Some(idx) = idx {
                        let entry = nets.entry(idx).or_insert_with(|| (String::new(), 0, 0));
                        match field {
                            "name" => entry.0 = value.to_string(),
                            "rx.bytes" => entry.1 = value.parse().unwrap_or(0),
                            "tx.bytes" => entry.2 = value.parse().unwrap_or(0),
                            _ => {}
                        }
                    }
                }
            }
        }
    }
    if let Some(name) = &current {
        flush(&mut out, name, &mut blocks, &mut nets);
    }
    out
}

/// `virsh domiflist` 的表格行。
fn parse_ifaces(lines: &[&str]) -> Vec<Iface> {
    let mut out = Vec::new();
    for line in lines {
        let text = line.trim();
        if text.is_empty() || text.starts_with("Interface") || text.chars().all(|c| c == '-') {
            continue;
        }
        let f: Vec<&str> = text.split_whitespace().collect();
        if f.len() >= 5 {
            out.push(Iface {
                target: f[0].to_string(),
                kind: f[1].to_string(),
                source: f[2].to_string(),
                model: f[3].to_string(),
                mac: f[4].to_string(),
            });
        } else if f.len() == 4 {
            // `--type user` 这类没有 source 列，少一列时按「网卡 类型 型号 MAC」认。
            out.push(Iface {
                target: f[0].to_string(),
                kind: f[1].to_string(),
                source: String::new(),
                model: f[2].to_string(),
                mac: f[3].to_string(),
            });
        }
    }
    out
}

/// `domblklist --details` 的行 → `(target, device, source)`。
///
/// 列是 `Type Device Target Source`；空光驱只有前三列（没有源）。
fn parse_blklist(lines: &[&str]) -> Vec<(String, String, String)> {
    let mut out = Vec::new();
    for line in lines {
        let text = line.trim();
        if text.is_empty() || text.starts_with("Type") || text.chars().all(|c| c == '-') {
            continue;
        }
        let f: Vec<&str> = text.split_whitespace().collect();
        let (device, target) = match f.len() {
            4.. => (f[1], f[2]),
            3 => (f[1], f[2]),
            2 => (f[1], ""),
            _ => continue,
        };
        if target.is_empty() {
            continue;
        }
        // 空光驱的源就是 `-`，别把它当成路径显示。
        let source = f.get(3..).map(|rest| rest.join(" ")).unwrap_or_default();
        let source = if source == "-" { String::new() } else { source };
        out.push((target.to_string(), device.to_string(), source));
    }
    out
}

/// `domblkinfo` 的 `Capacity:` / `Allocation:`，单位是字节。
fn parse_blkinfo(lines: &[&str]) -> (u64, u64) {
    let mut capacity = 0;
    let mut allocation = 0;
    for line in lines {
        let text = line.trim();
        if let Some(v) = text.strip_prefix("Capacity:") {
            capacity = v.trim().parse().unwrap_or(0);
        } else if let Some(v) = text.strip_prefix("Allocation:") {
            allocation = v.trim().parse().unwrap_or(0);
        }
    }
    (capacity, allocation)
}

/// 磁盘清单以 `domblklist` 为准。
///
/// 不能只靠 `domstats --block`：它只覆盖**运行中**的域，而且 libvirt 11 的
/// `domstats` 根本没有 `--inactive`（带上会整条命令报错，CPU / 磁盘 / 网速一起归零）。
/// 容量优先取 `domblkinfo`（关机的域也给得出），取不到再退回 `domstats`。
fn build_disks(
    name: &str,
    sections: &HashMap<String, Vec<&str>>,
    stat: Option<&DomStats>,
) -> Vec<VmDisk> {
    let lines_of = |key: &str| sections.get(key).map(Vec::as_slice).unwrap_or(&[]);
    // 列表里每台一个 `@@v_blk:<name>`；详情只有一台，用的是不分段后缀的 `@@v_blk`。
    let mut entries = parse_blklist(lines_of(&format!("v_blk:{name}")));
    if entries.is_empty() {
        entries = parse_blklist(lines_of("v_blk"));
    }
    if entries.is_empty() {
        return stat.map(|s| s.disks.clone()).unwrap_or_default();
    }

    entries
        .into_iter()
        .map(|(target, device, source)| {
            let (capacity, allocation) =
                parse_blkinfo(lines_of(&format!("v_disk:{name}:{target}")));
            let from_stats = stat.and_then(|s| s.disks.iter().find(|d| d.target == target));
            VmDisk {
                target,
                device,
                source,
                capacity_bytes: if capacity > 0 {
                    capacity
                } else {
                    from_stats.map(|d| d.capacity_bytes).unwrap_or(0)
                },
                allocation_bytes: if allocation > 0 {
                    allocation
                } else {
                    from_stats.map(|d| d.allocation_bytes).unwrap_or(0)
                },
                format: from_stats.map(|d| d.format.clone()).unwrap_or_default(),
            }
        })
        // 空光驱没有容量也没有源，但「这里有个光驱」本身是信息，保留。
        .filter(|d| d.capacity_bytes > 0 || !d.source.is_empty() || d.device == "cdrom")
        .collect()
}

/// `virsh domifaddr` → 网卡名到地址（去掉 `/24` 前缀长度，优先 ipv4）。
fn parse_addrs(lines: &[&str]) -> HashMap<String, String> {
    let mut out: HashMap<String, String> = HashMap::new();
    for line in lines {
        let text = line.trim();
        if text.is_empty() || text.starts_with("Name") || text.chars().all(|c| c == '-') {
            continue;
        }
        let f: Vec<&str> = text.split_whitespace().collect();
        if f.len() < 4 {
            continue;
        }
        let (nic, proto, addr) = (f[0], f[2], f[3]);
        if proto != "ipv4" && out.contains_key(nic) {
            continue;
        }
        let addr = addr.split('/').next().unwrap_or(addr).to_string();
        out.insert(nic.to_string(), addr);
    }
    out
}

/// `balloon.rss` 的单位在 libvirt 各版本间不统一（多数是字节，也有按 KiB 给的）。
///
/// 判据：比「当前气球大小」大一个量级的就是字节，否则按 KiB 认。0 表示没采到。
fn used_kib(rss: u64, current_kib: u64) -> u64 {
    if rss == 0 {
        return 0;
    }
    let looks_like_bytes =
        (current_kib > 0 && rss > current_kib.saturating_mul(2)) || rss > 1_000_000_000;
    if looks_like_bytes {
        rss / 1024
    } else {
        rss
    }
}

fn build_item(
    name: &str,
    id: Option<i64>,
    state: &str,
    info: &HashMap<String, String>,
    stat: Option<&DomStats>,
) -> VmItem {
    let memory_kib = info.get("Max memory").map(|v| first_number(v)).unwrap_or(0);
    let empty = DomStats::default();
    let stat = stat.unwrap_or(&empty);

    let nets = stat
        .nets
        .iter()
        .map(|(nic, (rx, tx))| VmNet {
            target: nic.clone(),
            rx_bytes: *rx,
            tx_bytes: *tx,
            ..Default::default()
        })
        .collect::<Vec<_>>();

    VmItem {
        name: name.to_string(),
        id,
        uuid: info.get("UUID").cloned().unwrap_or_default(),
        state: state.to_string(),
        vcpu: info
            .get("CPU(s)")
            .and_then(|v| v.trim().parse::<u32>().ok())
            .unwrap_or(0),
        memory_kib,
        memory_used_kib: used_kib(stat.balloon_rss, stat.balloon_current_kib),
        cpu_time_ns: stat.cpu_time_ns,
        autostart: info
            .get("Autostart")
            .map(|v| v == "enable")
            .unwrap_or(false),
        persistent: info.get("Persistent").map(|v| v == "yes").unwrap_or(false),
        os_type: info.get("OS Type").cloned().unwrap_or_default(),
        disks: stat
            .disks
            .iter()
            .filter(|d| d.capacity_bytes > 0 || !d.source.is_empty())
            .cloned()
            .collect(),
        nets,
        net_rx_bytes: stat.nets.values().map(|(rx, _)| rx).sum(),
        net_tx_bytes: stat.nets.values().map(|(_, tx)| tx).sum(),
    }
}

/// 列表输出 → `VmList`。
pub fn parse_list(raw: &str) -> VmList {
    let sections = split_sections(raw);
    let mut list = VmList::default();

    let Some(engine_lines) = sections.get("v_engine") else {
        list.degraded.push("libvirt".to_string());
        return list;
    };
    if engine_lines.iter().any(|l| l.trim() == "__no_virsh__") {
        return list;
    }
    list.engine = engine_lines
        .iter()
        .map(|l| l.trim())
        .find(|l| !l.is_empty())
        .map(str::to_string);
    if list.engine.is_none() {
        list.engine = Some("libvirt".to_string());
    }

    let doms = parse_dom_list(sections.get("v_list").map(Vec::as_slice).unwrap_or(&[]));
    if doms.is_empty() && !sections.contains_key("v_list") {
        list.degraded.push("虚拟机列表".to_string());
    }
    let stats = parse_domstats(sections.get("v_stats").map(Vec::as_slice).unwrap_or(&[]));

    for (name, id, state) in doms {
        let info_key = format!("v_info:{name}");
        let info = kv_map(sections.get(&info_key).map(Vec::as_slice).unwrap_or(&[]));
        let if_key = format!("v_if:{name}");
        let ip_key = format!("v_ip:{name}");
        let ips = parse_addrs(sections.get(&ip_key).map(Vec::as_slice).unwrap_or(&[]));
        let ifaces = parse_ifaces(sections.get(&if_key).map(Vec::as_slice).unwrap_or(&[]));

        let mut item = build_item(&name, id, &state, &info, stats.get(&name));
        item.disks = build_disks(&name, &sections, stats.get(&name));
        // `domstats` 只给网卡名与收发字节；桥接、型号、MAC、IP 来自 `domiflist`/`domifaddr`。
        if !ifaces.is_empty() {
            item.nets = ifaces
                .iter()
                .map(|i| {
                    let (rx, tx) = stats
                        .get(&name)
                        .and_then(|s| s.nets.get(&i.target).copied())
                        .unwrap_or((0, 0));
                    VmNet {
                        target: i.target.clone(),
                        kind: i.kind.clone(),
                        source: i.source.clone(),
                        model: i.model.clone(),
                        mac: i.mac.clone(),
                        ip: ips.get(&i.target).cloned().unwrap_or_default(),
                        rx_bytes: rx,
                        tx_bytes: tx,
                    }
                })
                .collect();
        } else if !ips.is_empty() {
            for net in &mut item.nets {
                if let Some(ip) = ips.get(&net.target) {
                    net.ip = ip.clone();
                }
            }
        }
        list.items.push(item);
    }

    list.items.sort_by(|a, b| a.name.cmp(&b.name));
    list.total = list.items.len();
    list.running = list.items.iter().filter(|i| i.state == "running").count();
    list.paused = list.items.iter().filter(|i| i.state == "paused").count();
    list.shutoff = list.items.iter().filter(|i| i.state == "shut off").count();
    list.crashed = list.items.iter().filter(|i| i.state == "crashed").count();
    if !stats.is_empty() && list.items.iter().all(|i| i.disks.is_empty()) {
        list.degraded.push("磁盘".to_string());
    }
    list
}

/// 从 `dumpxml` 里取一个属性，单双引号都认。
fn xml_attr(line: &str, name: &str) -> String {
    let needle = format!("{name}=");
    let Some(pos) = line.find(&needle) else {
        return String::new();
    };
    let rest = &line[pos + needle.len()..];
    let Some(quote) = rest.chars().next() else {
        return String::new();
    };
    if quote != '\'' && quote != '"' {
        return String::new();
    }
    rest[1..]
        .split(quote)
        .next()
        .unwrap_or_default()
        .to_string()
}

/// 磁盘的 `driver type`（qcow2 / raw）。只认 `<driver name='qemu' type='…'>`。
fn parse_disk_formats(xml: &str) -> HashMap<String, String> {
    let mut out: HashMap<String, String> = HashMap::new();
    let mut format = String::new();
    for line in xml.lines() {
        let text = line.trim();
        if text.starts_with("<disk") {
            format.clear();
        }
        if text.starts_with("<driver") && text.contains("name='qemu'") {
            format = xml_attr(text, "type");
        }
        if let Some(rest) = text.strip_prefix("<target dev=") {
            let device = rest
                .split(['\'', '"'])
                .nth(1)
                .unwrap_or_default()
                .to_string();
            if !device.is_empty() && !format.is_empty() {
                out.insert(device, format.clone());
            }
        }
    }
    out
}

/// 详情输出 → `VmDetail`。
pub fn parse_detail(raw: &str) -> Result<VmDetail, String> {
    let sections = split_sections(raw);
    let lines_of = |key: &str| sections.get(key).map(Vec::as_slice).unwrap_or(&[]);

    let info = kv_map(lines_of("v_info"));
    let name = info
        .get("Name")
        .cloned()
        .filter(|n| !n.is_empty())
        .ok_or_else(|| "拿不到这台虚拟机的信息（可能已被删除）".to_string())?;
    let state = info.get("State").cloned().unwrap_or_default();
    let id = info.get("Id").and_then(|v| v.trim().parse::<i64>().ok());

    let stats = parse_domstats(lines_of("v_stats"));
    let stat = stats.get(&name).cloned();
    let ips = parse_addrs(lines_of("v_ip"));
    let ifaces = parse_ifaces(lines_of("v_if"));
    let xml = lines_of("v_xml").join("\n");

    let mut item = build_item(&name, id, &state, &info, stat.as_ref());
    item.disks = build_disks(&name, &sections, stat.as_ref());

    // 格式（qcow2 / raw）只有详情档的 `dumpxml` 拿得到。
    let formats = parse_disk_formats(&xml);
    for disk in &mut item.disks {
        if let Some(format) = formats.get(&disk.target) {
            disk.format = format.clone();
        }
    }

    item.nets = ifaces
        .iter()
        .map(|i| {
            let (rx, tx) = stat
                .as_ref()
                .and_then(|s| s.nets.get(&i.target).copied())
                .unwrap_or((0, 0));
            VmNet {
                target: i.target.clone(),
                kind: i.kind.clone(),
                source: i.source.clone(),
                model: i.model.clone(),
                mac: i.mac.clone(),
                ip: ips.get(&i.target).cloned().unwrap_or_default(),
                rx_bytes: rx,
                tx_bytes: tx,
            }
        })
        .collect();

    let machine = xml
        .lines()
        .find(|l| l.contains("<type") && l.contains("machine="))
        .map(|l| xml_attr(l, "machine"))
        .unwrap_or_default();
    let firmware = if xml.contains("<loader") || xml.contains("pflash") {
        "UEFI".to_string()
    } else {
        "BIOS".to_string()
    };
    let display = lines_of("v_display")
        .iter()
        .map(|l| l.trim())
        .find(|l| !l.is_empty())
        .map(str::to_string);

    Ok(VmDetail {
        item,
        machine,
        firmware,
        display,
    })
}

/// 动作输出 → `(提示, 状态)`。提示取 `virsh` 自己的输出，状态取回读的 `domstate`。
pub fn parse_action(raw: &str) -> (String, String) {
    let sections = split_sections(raw);
    let message = sections
        .get("v_action")
        .map(|lines| {
            lines
                .iter()
                .map(|l| l.trim())
                .filter(|l| !l.is_empty())
                .collect::<Vec<_>>()
                .join("；")
        })
        .unwrap_or_default();
    let state = sections
        .get("v_state")
        .and_then(|lines| lines.iter().map(|l| l.trim()).find(|l| !l.is_empty()))
        .unwrap_or_default()
        .to_string();
    (message, state)
}

/// `autostart` 的输出里没有 `domstate`，回读的是 `dominfo`；只关心是否成功。
pub fn parse_autostart(raw: &str) -> (bool, String, bool) {
    let sections = split_sections(raw);
    let message = sections
        .get("v_action")
        .map(|lines| {
            lines
                .iter()
                .map(|l| l.trim())
                .map(str::to_string)
                .collect::<Vec<_>>()
                .join("；")
        })
        .unwrap_or_default();
    let info = kv_map(sections.get("v_info").map(Vec::as_slice).unwrap_or(&[]));
    let enabled = info
        .get("Autostart")
        .map(|v| v == "enable")
        .unwrap_or(false);
    let ok = info.contains_key("Autostart");
    (ok, message, enabled)
}

#[cfg(test)]
mod tests {
    use super::*;

    const LIST_SAMPLE: &str = "@@v_engine
10.0.0
@@v_list
 Id   Name           State
---------------------------------
 12   win11-dev      running
 -    debian-build   shut off
 3    openwrt        paused
---------------------------------
@@v_stats
Domain: 'win11-dev'
  Domain: id=12
  state.state=1
  cpu.time=42000000000
  balloon.current=16777216
  balloon.rss=15400000000
  block.count=1
  block.0.name=vda
  block.0.path=/mnt/vm/win11.qcow2
  block.0.capacity=128849018880
  block.0.allocation=80000000000
  net.count=1
  net.0.name=vnet0
  net.0.rx.bytes=12000
  net.0.tx.bytes=8000
Domain: 'debian-build'
  Domain: id=-1
  state.state=5
  balloon.current=4194304
  balloon.rss=0
@@v_info:win11-dev
Id:             12
Name:           win11-dev
UUID:           8f2c1111-2222-3333-4444-555566667777
OS Type:        hvm
State:          running
CPU(s):         8
CPU time:       41d 7h
Max memory:     16777216 KiB
Used memory:    15564000 KiB
Persistent:     yes
Autostart:      enable
@@v_if:win11-dev
 Interface   Type      Source    Model    MAC
------------------------------------------------------------
 vnet0       network   default   virtio   52:54:00:aa:bb:cc
@@v_ip:win11-dev
 Name       MAC address          Protocol     Address
-------------------------------------------------------------------------------
 vnet0      52:54:00:aa:bb:cc   ipv4         192.168.122.31/24
@@v_info:debian-build
Id:             -
Name:           debian-build
UUID:           11112222-3333-4444-5555-666677778888
OS Type:        hvm
State:          shut off
CPU(s):         4
Max memory:     4194304 KiB
Persistent:     yes
Autostart:      disable
@@v_if:debian-build
 Interface   Type      Source    Model    MAC
------------------------------------------------------------
 vnet1       network   default   virtio   52:54:00:dd:ee:ff
@@v_ip:debian-build
 Name       MAC address          Protocol     Address
-------------------------------------------------------------------------------
@@v_info:openwrt
Id:             3
Name:           openwrt
UUID:           99990000-1111-2222-3333-444455556666
OS Type:        hvm
State:          paused
CPU(s):         2
Max memory:     524288 KiB
Persistent:     yes
Autostart:      disable
@@v_end
";

    #[test]
    fn list_script_is_read_only() {
        let text = list_script();
        for banned in [
            "virsh start",
            "virsh destroy",
            "virsh shutdown",
            "autostart",
        ] {
            assert!(!text.contains(banned), "列表脚本里不该出现 {banned}");
        }
        assert!(text.contains("virsh list --all"));
        assert!(text.contains("virsh domblklist"));
        assert!(text.contains("virsh domblkinfo"));
        // libvirt 11 的 `domstats` 不认 `--inactive`：带上它整条命令报错，
        // CPU / 磁盘 / 网速会一起归零（真机踩过）。
        assert!(!text.contains("domstats --inactive"));
        // 不给 `--cpu-total` 就没有 `cpu.time`，CPU 环永远是「—」（真机踩过）。
        assert!(text.contains("--cpu-total"));
    }

    /// 磁盘清单以 `domblklist` 为准：关机的域也要有容量，且不能靠 `domstats`。
    #[test]
    fn parse_list_reads_disks_for_shut_off_domain() {
        let sample = "@@v_engine
11.2.0
@@v_list
 Id   Name      State
---------------------------
 -    proxy     shut off
---------------------------
@@v_stats
@@v_info:proxy
Id:             -
Name:           proxy
UUID:           abc
OS Type:        hvm
State:          shut off
CPU(s):         4
Max memory:     4194304 KiB
Persistent:     yes
Autostart:      disable
@@v_blk:proxy
 Type   Device   Target   Source
----------------------------------------
 file   disk     hdc      /mnt/deva/domains/proxy/win10.img
 file   cdrom    sda      -
@@v_disk:proxy:hdc
Capacity:       107374182400
Allocation:     22942743040
Physical:       45898137600
@@v_end
";
        let list = parse_list(sample);
        let proxy = list.items.iter().find(|i| i.name == "proxy").unwrap();
        assert_eq!(proxy.disks.len(), 2);
        assert_eq!(proxy.disks[0].target, "hdc");
        assert_eq!(proxy.disks[0].device, "disk");
        assert_eq!(proxy.disks[0].capacity_bytes, 107374182400);
        assert_eq!(proxy.disks[0].source, "/mnt/deva/domains/proxy/win10.img");
        assert_eq!(proxy.disks[1].device, "cdrom");
        assert_eq!(proxy.disks[1].source, "", "空光驱的 `-` 不该当路径");
        assert!(
            !list.degraded.iter().any(|d| d == "磁盘"),
            "拿到磁盘就不该再报降级"
        );
    }

    #[test]
    fn scripts_never_contain_single_quotes() {
        let cases = [
            list_script(),
            detail_script("win11-dev").unwrap(),
            action_script(VmAction::Shutdown, "win11-dev").unwrap(),
            autostart_script("win11-dev", true).unwrap(),
        ];
        for script in cases {
            // 整段被 sh -c '...' 包裹，正文里再出现单引号就会截断。
            assert_eq!(script.matches('\'').count(), 2, "{script}");
        }
    }

    #[test]
    fn validate_name_rejects_injection() {
        assert!(validate_name("a; rm -rf /").is_err());
        assert!(validate_name("a'b").is_err());
        assert!(validate_name("").is_err());
        assert!(validate_name(&"x".repeat(81)).is_err());
        assert!(validate_name("win11-dev_2").is_ok());
    }

    #[test]
    fn action_verbs_are_closed() {
        assert_eq!(action_verb(VmAction::Destroy), "destroy");
        // 注入串过不了白名单，不会拼进脚本。
        assert!(action_script(VmAction::Start, "x; reboot").is_err());
    }

    #[test]
    fn allowed_actions_by_state() {
        assert!(allowed_actions("running").contains(&VmAction::Shutdown));
        assert!(!allowed_actions("running").contains(&VmAction::Start));
        assert_eq!(
            allowed_actions("paused"),
            &[VmAction::Resume, VmAction::Destroy][..]
        );
        assert_eq!(allowed_actions("shut off"), &[VmAction::Start][..]);
        assert_eq!(allowed_actions("crashed"), &[VmAction::Destroy][..]);
    }

    #[test]
    fn parse_list_reads_states_memory_and_network() {
        let list = parse_list(LIST_SAMPLE);
        assert_eq!(list.engine.as_deref(), Some("10.0.0"));
        assert_eq!(list.total, 3);
        assert_eq!(list.running, 1);
        assert_eq!(list.paused, 1);
        assert_eq!(list.shutoff, 1);
        assert_eq!(list.crashed, 0);

        let win = list.items.iter().find(|i| i.name == "win11-dev").unwrap();
        assert_eq!(win.id, Some(12));
        assert_eq!(win.vcpu, 8);
        assert_eq!(win.memory_kib, 16777216);
        // 字节口径的 rss → KiB。
        assert_eq!(win.memory_used_kib, 15039062);
        assert!(win.autostart);
        assert_eq!(win.disks.len(), 1);
        assert_eq!(win.disks[0].target, "vda");
        assert_eq!(win.disks[0].capacity_bytes, 128849018880);
        assert_eq!(win.nets[0].source, "default");
        assert_eq!(win.nets[0].ip, "192.168.122.31");
        assert_eq!(win.net_rx_bytes, 12000);

        let debian = list
            .items
            .iter()
            .find(|i| i.name == "debian-build")
            .unwrap();
        assert_eq!(debian.id, None);
        assert_eq!(debian.memory_used_kib, 0);
        assert!(!debian.autostart);
    }

    #[test]
    fn parse_list_without_virsh_is_empty_not_error() {
        let list = parse_list("@@v_engine\n__no_virsh__\n@@v_list\n");
        assert!(list.engine.is_none());
        assert_eq!(list.total, 0);
        assert!(list.degraded.is_empty());
    }

    #[test]
    fn parse_detail_reads_machine_firmware_and_disk_format() {
        let raw = "@@v_xml
<domain type='kvm'>
  <name>win11-dev</name>
  <os>
    <type arch='x86_64' machine='pc-q35-8.2'>hvm</type>
    <loader readonly='yes' type='pflash'>/usr/share/OVMF.fd</loader>
  </os>
  <devices>
    <disk type='file' device='disk'>
      <driver name='qemu' type='qcow2' cache='none'/>
      <source file='/mnt/vm/win11.qcow2'/>
      <target dev='vda' bus='virtio'/>
    </disk>
  </devices>
</domain>
@@v_info
Id:             12
Name:           win11-dev
UUID:           8f2c1111-2222-3333-4444-555566667777
OS Type:        hvm
State:          running
CPU(s):         8
Max memory:     16777216 KiB
Persistent:     yes
Autostart:      enable
@@v_blk
 Type   Device   Target   Source
---------------------------------
 file   disk     vda      /mnt/vm/win11.qcow2
@@v_if
 Interface   Type      Source    Model    MAC
------------------------------------------------------------
 vnet0       network   default   virtio   52:54:00:aa:bb:cc
@@v_ip
 Name       MAC address          Protocol     Address
-------------------------------------------------------------------------------
 vnet0      52:54:00:aa:bb:cc   ipv4         192.168.122.31/24
@@v_stats
Domain: 'win11-dev'
  cpu.time=100
  balloon.current=16777216
  balloon.rss=8000000000
  block.0.name=vda
  block.0.path=/mnt/vm/win11.qcow2
  block.0.capacity=128849018880
  block.0.allocation=80000000000
@@v_display
vnc://127.0.0.1:5901
@@v_end
";
        let detail = parse_detail(raw).unwrap();
        assert_eq!(detail.item.name, "win11-dev");
        assert_eq!(detail.machine, "pc-q35-8.2");
        assert_eq!(detail.firmware, "UEFI");
        assert_eq!(detail.display.as_deref(), Some("vnc://127.0.0.1:5901"));
        assert_eq!(detail.item.disks[0].device, "disk");
        assert_eq!(detail.item.disks[0].format, "qcow2");
        assert_eq!(detail.item.nets[0].ip, "192.168.122.31");
    }

    #[test]
    fn parse_action_returns_message_and_state() {
        let raw = "@@v_action\nDomain 'win11-dev' is being shutdown\n@@v_state\nrunning\n@@v_end\n";
        let (message, state) = parse_action(raw);
        assert!(message.contains("shutdown"));
        assert_eq!(state, "running");
    }

    #[test]
    fn parse_autostart_reports_effective_flag() {
        let raw =
            "@@v_action\n\n@@v_info\nName:           win11-dev\nAutostart:      enable\n@@v_end\n";
        let (ok, _, enabled) = parse_autostart(raw);
        assert!(ok);
        assert!(enabled);
    }
}
