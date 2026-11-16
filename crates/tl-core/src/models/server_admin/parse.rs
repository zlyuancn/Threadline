//! 概览输出解析。
//!
//! 归属：`server_admin` 模型内部文件组。拥有：分节格式的解析规则与指标计算口径。
//! 提供：`interpret(raw) -> ParsedOverview`（纯函数，无 IO）。
//! 允许调用：`tl_protocol`（DTO）、标准库。
//!
//! 纪律：**容错优先**。远端是别人的机器，`df`、`ps`、`docker` 都可能因为发行版
//! 或权限给出不同输出；解析不出来的块只让那一块缺省并记进 `degraded`，
//! 绝不因为一行畸形把整页打空。

use std::collections::{HashMap, HashSet};

use tl_protocol::{
    BlockDisk, ContainerRow, ContainerSummary, CpuStat, DiskIo, DiskUsage, GpuStat, LoadStat,
    MemBreakdown, MemoryStat, NetInterface, ProcessRow, SystemInfo, ZfsPool,
};

/// 解析后的概览（不含 `collected_at` / `elapsed_ms` / `modules`——那些由编排层补）。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ParsedOverview {
    pub system: SystemInfo,
    pub cpu: CpuStat,
    pub load: LoadStat,
    pub memory: MemoryStat,
    pub disks: Vec<DiskUsage>,
    pub block_disks: Vec<BlockDisk>,
    pub zfs_pools: Vec<ZfsPool>,
    pub mem_breakdown: MemBreakdown,
    pub gpus: Vec<GpuStat>,
    pub network: Vec<NetInterface>,
    pub disk_io: Vec<DiskIo>,
    pub processes: Vec<ProcessRow>,
    pub containers: ContainerSummary,
    pub degraded: Vec<String>,
}

/// 把脚本输出切成节：`@@name` 之后、下一个 `@@` 之前的所有行。
pub fn split_sections(raw: &str) -> HashMap<String, Vec<&str>> {
    let mut out: HashMap<String, Vec<&str>> = HashMap::new();
    let mut current: Option<String> = None;
    for line in raw.lines() {
        if let Some(name) = line.strip_prefix("@@") {
            let key = name.trim().to_string();
            out.entry(key.clone()).or_default();
            current = Some(key);
            continue;
        }
        if let Some(key) = &current {
            if let Some(lines) = out.get_mut(key) {
                lines.push(line);
            }
        }
    }
    out
}

fn kv<'a>(lines: &[&'a str]) -> HashMap<&'a str, &'a str> {
    lines
        .iter()
        .filter_map(|line| line.split_once('='))
        .map(|(k, v)| (k.trim(), v.trim()))
        .collect()
}

/// `/proc/stat` 的 cpu 行。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct CpuSample {
    user: u64,
    nice: u64,
    system: u64,
    idle: u64,
    iowait: u64,
    irq: u64,
    softirq: u64,
    steal: u64,
}

impl CpuSample {
    fn parse(line: &str) -> Option<Self> {
        let mut parts = line.split_whitespace();
        if parts.next()? != "cpu" {
            return None;
        }
        let mut v = [0u64; 8];
        for slot in v.iter_mut() {
            match parts.next().and_then(|s| s.parse::<u64>().ok()) {
                Some(n) => *slot = n,
                None => break,
            }
        }
        Some(Self {
            user: v[0],
            nice: v[1],
            system: v[2],
            idle: v[3],
            iowait: v[4],
            irq: v[5],
            softirq: v[6],
            steal: v[7],
        })
    }

    fn total(&self) -> u64 {
        self.user
            + self.nice
            + self.system
            + self.idle
            + self.iowait
            + self.irq
            + self.softirq
            + self.steal
    }

    fn idle_all(&self) -> u64 {
        self.idle + self.iowait
    }
}

fn percent(part: u64, total: u64) -> f64 {
    if total == 0 {
        0.0
    } else {
        part as f64 * 100.0 / total as f64
    }
}

/// 两次采样的实际间隔（秒）。`date +%s%N` 不可用时回退脚本里写死的 0.3s。
fn sample_seconds(sections: &HashMap<String, Vec<&str>>) -> f64 {
    let stamp = |key: &str| {
        sections
            .get(key)
            .and_then(|lines| lines.first())
            .and_then(|s| s.trim().parse::<u64>().ok())
    };
    match (stamp("t1"), stamp("t2")) {
        (Some(a), Some(b)) if b > a => (b - a) as f64 / 1e9,
        _ => 0.3,
    }
}

fn meminfo_kb(lines: &[&str]) -> HashMap<String, u64> {
    let mut map = HashMap::new();
    for line in lines {
        if let Some((key, rest)) = line.split_once(':') {
            let kb = rest
                .split_whitespace()
                .next()
                .and_then(|n| n.parse::<u64>().ok())
                .unwrap_or(0);
            map.insert(key.trim().to_string(), kb);
        }
    }
    map
}

/// 伪文件系统与容器挂载：占满屏且没有信息量。
fn is_pseudo_filesystem(fs: &str) -> bool {
    [
        "tmpfs", "devtmpfs", "devfs", "shm", "overlay", "squashfs", "udev", "none", "proc",
        "sysfs", "cgroup", "efivarfs",
    ]
    .iter()
    .any(|prefix| fs.starts_with(prefix))
}

fn parse_df(lines: &[&str]) -> Vec<DiskUsage> {
    let mut out: Vec<DiskUsage> = Vec::new();
    // 同一个文件系统挂在多处（bind mount）只留第一条。
    let mut seen: HashSet<String> = HashSet::new();
    for line in lines.iter().skip(1) {
        let fields: Vec<&str> = line.split_whitespace().collect();
        if fields.len() < 6 {
            continue;
        }
        let filesystem = fields[0];
        if is_pseudo_filesystem(filesystem) {
            continue;
        }
        if !seen.insert(filesystem.to_string()) {
            continue;
        }
        let total_kb: u64 = fields[1].parse().unwrap_or(0);
        if total_kb == 0 {
            continue;
        }
        let disk = DiskUsage {
            filesystem: filesystem.to_string(),
            mount: fields[5..].join(" "),
            total_kb,
            used_kb: fields[2].parse().unwrap_or(0),
            available_kb: fields[3].parse().unwrap_or(0),
            use_percent: fields[4].trim_end_matches('%').parse().unwrap_or(0.0),
        };
        if is_shadow_mount(&disk, &out) {
            continue;
        }
        out.push(disk);
    }
    out.truncate(12);
    out
}

/// 同一个存储池被重复挂成很多层（Unraid 的 shfs 把每个共享都挂一遍）。
///
/// 判定保守：父挂载本身**不是块设备**（`/dev/*`）时，它的子路径只是视图；
/// 真实分区的独立挂载（父是 `/dev/sdb1`）一律保留。
fn is_shadow_mount(disk: &DiskUsage, earlier: &[DiskUsage]) -> bool {
    if disk.mount == "/" || disk.filesystem.starts_with("/dev/") {
        return false;
    }
    earlier.iter().any(|parent| {
        parent.mount != "/"
            && !parent.filesystem.starts_with("/dev/")
            && disk.mount.starts_with(&format!("{}/", parent.mount))
    })
}

/// 只统计真实块设备：`dm-*`/`loop*`/`ram*` 要么重复计数，要么没有意义。
fn is_physical_disk(name: &str) -> bool {
    const PREFIXES: [&str; 6] = ["sd", "nvme", "vd", "hd", "xvd", "mmcblk"];
    PREFIXES.iter().any(|p| name.starts_with(p))
}

/// 解析 GPU 行。两种来源的单位不同：
///
/// - `nvidia|型号|占用%|显存已用MB|显存总量MB|温度`
/// - `sysfs|型号|占用%|显存已用字节|显存总量字节|0`
///
/// 统一折算成 kB；`[N/A]`、空串一律当 0——老驱动不给温度、核显没有独立显存都是常态，
/// 界面据此判断这一行要不要显示，而不是画一个假的 0。
fn parse_gpus(lines: &[&str]) -> Vec<GpuStat> {
    let mut out = Vec::new();
    for line in lines {
        let fields: Vec<&str> = line.split('|').collect();
        if fields.len() < 6 {
            continue;
        }
        let name = fields[1].trim();
        if name.is_empty() {
            continue;
        }
        // nvidia 报 MB（× 1024 得 kB），sysfs 报字节（÷ 1024 得 kB）
        let scale = if fields[0].trim() == "nvidia" {
            1024.0
        } else {
            1.0 / 1024.0
        };
        let number = |index: usize| -> f64 { fields[index].trim().parse::<f64>().unwrap_or(0.0) };
        out.push(GpuStat {
            name: name.to_string(),
            usage_percent: number(2).clamp(0.0, 100.0),
            memory_used_kb: (number(3) * scale) as u64,
            memory_total_kb: (number(4) * scale) as u64,
            temperature_c: number(5).max(0.0) as u32,
        });
    }
    out
}

/// 解析 `lsblk -P` 的一行：`NAME="sda" TYPE="disk" SIZE="1000" MOUNTPOINT="/"`。
///
/// 值一律被双引号包住，所以按引号配对切，不用管空格与 `=` 出现在值里。
fn parse_lsblk_line(line: &str) -> HashMap<String, String> {
    let mut out = HashMap::new();
    let mut rest = line;
    while let Some(eq) = rest.find('=') {
        let key = rest[..eq].trim();
        let after = &rest[eq + 1..];
        let Some(open) = after.find('"') else { break };
        let Some(len) = after[open + 1..].find('"') else {
            break;
        };
        if !key.is_empty() {
            out.insert(key.to_string(), after[open + 1..open + 1 + len].to_string());
        }
        rest = &after[open + 1 + len + 1..];
    }
    out
}

/// 内存去向：`@@memdetail` 的键值对。缺项一律 0（没装 ZFS / docker / 没有虚拟机）。
fn parse_mem_breakdown(meta: &HashMap<&str, &str>) -> MemBreakdown {
    let get = |key: &str| -> u64 {
        meta.get(key)
            .and_then(|value| value.trim().parse::<u64>().ok())
            .unwrap_or(0)
    };
    MemBreakdown {
        vm_kb: get("vm_kb"),
        container_kb: get("container_kb"),
        zfs_arc_kb: get("zfs_arc_kb"),
    }
}

/// 解析 `zpool list -vHp`：不缩进的池行 + 缩进的成员行。
///
/// 返回池清单，以及「成员设备 → 池名」的映射（设备名去掉 `/dev/` 前缀）。
fn parse_zpools(lines: &[&str]) -> (Vec<ZfsPool>, HashMap<String, String>) {
    let mut pools: Vec<ZfsPool> = Vec::new();
    let mut pool_of: HashMap<String, String> = HashMap::new();
    let mut current = String::new();

    for line in lines {
        if line.trim().is_empty() {
            continue;
        }
        let fields: Vec<&str> = line.split_whitespace().collect();
        if fields.is_empty() {
            continue;
        }
        let indented = line.starts_with(' ') || line.starts_with('\t');
        if !indented {
            let name = fields[0].to_string();
            pools.push(ZfsPool {
                name: name.clone(),
                size_bytes: fields.get(1).and_then(|v| v.parse().ok()).unwrap_or(0),
                alloc_bytes: fields.get(2).and_then(|v| v.parse().ok()).unwrap_or(0),
            });
            current = name;
            continue;
        }
        if current.is_empty() {
            continue;
        }
        // vdev 容器（mirror-0 / raidz1-0 / draid…）不是设备，跳过
        let device = fields[0].trim_start_matches("/dev/");
        if device.starts_with("mirror")
            || device.starts_with("raidz")
            || device.starts_with("draid")
            || device.starts_with("spare")
        {
            continue;
        }
        pool_of.insert(device.to_string(), current.clone());
    }

    (pools, pool_of)
}

/// 把物理盘（`lsblk` TYPE=disk）与 `df` 的用量对上，并标出 ZFS 池归属。
///
/// 只认同一个盘的**分区**（名字以盘名开头），md/raid/lvm 这类聚合设备不归属到任何
/// 物理盘上——它们的用量算到哪块盘都是错的，宁可不显示。
///
/// ZFS 是个例外：成员盘自己**没有挂载点**（只有池有），光看 `df` 会以为它们"未挂载"。
/// 这里把 `zpool` 的成员关系贴回盘上，前端据此显示"ZFS 池 xxx"并用池的用量。
fn parse_block_disks(
    lines: &[&str],
    disks: &[DiskUsage],
    pool_of: &HashMap<String, String>,
) -> Vec<BlockDisk> {
    let by_mount: HashMap<&str, &DiskUsage> = disks
        .iter()
        .map(|disk| (disk.mount.as_str(), disk))
        .collect();

    let mut out: Vec<BlockDisk> = Vec::new();
    for line in lines {
        let fields = parse_lsblk_line(line);
        if fields.get("TYPE").map(String::as_str) != Some("disk") {
            continue;
        }
        let name = fields.get("NAME").cloned().unwrap_or_default();
        let size_bytes = fields
            .get("SIZE")
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or(0);
        // zram 是内核拿内存做的交换盘，列进"磁盘"只会让人困惑。
        if name.is_empty() || size_bytes == 0 || name.starts_with("zram") {
            continue;
        }
        let fstype = fields.get("FSTYPE").cloned().unwrap_or_default();
        // 池名优先取 zpool 的成员关系（成员可能报成 sdb1，这里按前缀兜一层）；
        // 退一步认 lsblk 的 zfs_member 标记，这样即使 zpool 命令不可用，
        // 也知道"这块盘不是没用，是给 ZFS 用了"。
        let pool = pool_of
            .get(&name)
            .cloned()
            .or_else(|| {
                pool_of
                    .iter()
                    .find(|(device, _)| device.starts_with(&name))
                    .map(|(_, pool)| pool.clone())
            })
            .unwrap_or_else(|| {
                if fstype.contains("zfs") {
                    "zfs".to_string()
                } else {
                    String::new()
                }
            });
        out.push(BlockDisk {
            name,
            size_bytes,
            fstype,
            pool,
            ..Default::default()
        });
    }

    // 第二遍：分区的挂载量汇总到所属盘。
    for line in lines {
        let fields = parse_lsblk_line(line);
        if matches!(
            fields.get("TYPE").map(String::as_str),
            Some("disk" | "loop" | "rom")
        ) {
            continue;
        }
        let mount = fields
            .get("MOUNTPOINT")
            .map(|value| value.replace("\\x20", " "))
            .unwrap_or_default();
        if mount.is_empty() {
            continue;
        }
        let Some(usage) = by_mount.get(mount.as_str()) else {
            continue;
        };
        let name = fields.get("NAME").cloned().unwrap_or_default();
        // 归属到名字最长（最具体）的那块盘：nvme0n1 与 nvme0n1p1 都得落到 nvme0n1。
        let owner = out
            .iter_mut()
            .filter(|disk| name.starts_with(&disk.name))
            .max_by_key(|disk| disk.name.len());
        if let Some(disk) = owner {
            disk.total_kb += usage.total_kb;
            disk.used_kb += usage.used_kb;
            if disk.mount.is_empty() {
                disk.mount = mount;
            }
        }
    }

    // 大的排前面：一眼看到"盘有多大"比按字母序有用。
    out.sort_by(|a, b| {
        b.size_bytes
            .cmp(&a.size_bytes)
            .then_with(|| a.name.cmp(&b.name))
    });
    out
}

fn parse_diskstats(lines: &[&str]) -> HashMap<String, (u64, u64)> {
    // 先收一遍设备名：判断"这是不是分区"要看父设备在不在同一份输出里。
    let mut known: HashSet<&str> = HashSet::new();
    for line in lines {
        let f: Vec<&str> = line.split_whitespace().collect();
        if f.len() < 10 || !is_physical_disk(f[2]) {
            continue;
        }
        known.insert(f[2]);
    }

    let mut map = HashMap::new();
    for line in lines {
        let f: Vec<&str> = line.split_whitespace().collect();
        // 1..=17 是老内核的字段数；取到「写入扇区」需要至少 10 列。
        if f.len() < 10 {
            continue;
        }
        let name = f[2];
        if !is_physical_disk(name) || is_partition_of(name, &known) {
            continue;
        }
        let sectors_read: u64 = f[5].parse().unwrap_or(0);
        let sectors_written: u64 = f[9].parse().unwrap_or(0);
        map.insert(
            name.to_string(),
            (sectors_read * 512, sectors_written * 512),
        );
    }
    map
}

/// `sda1` / `nvme0n1p1` / `mmcblk0p1` 是分区：父设备也在列表里时只留父设备。
///
/// 否则一块盘会在界面上出现两次，累计读写还一模一样——那不是"更详细"，是噪音。
fn is_partition_of(name: &str, known: &HashSet<&str>) -> bool {
    let trimmed = name.trim_end_matches(|c: char| c.is_ascii_digit());
    if trimmed.is_empty() || trimmed.len() == name.len() {
        return false;
    }
    if known.contains(trimmed) {
        return true;
    }
    // nvme / mmcblk 的分区名在编号前多了个 p。
    match trimmed.strip_suffix('p') {
        Some(parent) => known.contains(parent),
        None => false,
    }
}

/// 回环与虚拟网卡会掩盖真实流量，默认不展示。
///
/// `macvtap` / `vhost` / `vnet` 是虚拟机与容器造出来的影子接口，
/// 它们和宿主网卡报同一份字节数，一起显示只会让人以为流量翻倍。
fn is_virtual_iface(name: &str) -> bool {
    name == "lo"
        || [
            "docker", "veth", "br-", "virbr", "cni", "flannel", "tun", "tap", "macvtap", "vhost",
            "vnet", "ifb", "dummy",
        ]
        .iter()
        .any(|prefix| name.starts_with(prefix))
}

fn parse_netdev(lines: &[&str]) -> HashMap<String, (u64, u64)> {
    let mut map = HashMap::new();
    for line in lines {
        let Some((name, rest)) = line.split_once(':') else {
            continue;
        };
        let name = name.trim();
        if name.is_empty() || is_virtual_iface(name) {
            continue;
        }
        let f: Vec<&str> = rest.split_whitespace().collect();
        // rx 在前 8 列，tx 从第 9 列开始。
        if f.len() < 16 {
            continue;
        }
        map.insert(
            name.to_string(),
            (f[0].parse().unwrap_or(0), f[8].parse().unwrap_or(0)),
        );
    }
    map
}

fn parse_ps(lines: &[&str]) -> Vec<ProcessRow> {
    let mut out = Vec::new();
    for line in lines {
        let f: Vec<&str> = line.split_whitespace().collect();
        if f.len() < 5 {
            continue;
        }
        let Ok(pid) = f[0].parse::<u32>() else {
            continue; // 表头 PID %CPU %MEM RSS COMMAND
        };
        out.push(ProcessRow {
            pid,
            cpu_percent: f[1].parse().unwrap_or(0.0),
            mem_percent: f[2].parse().unwrap_or(0.0),
            rss_kb: f[3].parse().unwrap_or(0),
            command: f[4..].join(" "),
        });
    }
    out.truncate(10);
    out
}

fn parse_containers(lines: &[&str]) -> Option<ContainerSummary> {
    if lines.iter().any(|l| l.trim() == "__no_engine__") {
        return None;
    }
    let mut items = Vec::new();
    for line in lines {
        let f: Vec<&str> = line.split('|').collect();
        if f.len() < 5 {
            continue;
        }
        items.push(ContainerRow {
            id: f[0].trim().to_string(),
            name: f[1].trim().to_string(),
            image: f[2].trim().to_string(),
            state: f[3].trim().to_string(),
            status: f[4].trim().to_string(),
        });
    }
    let running = items.iter().filter(|c| c.state == "running").count();
    Some(ContainerSummary {
        engine: Some("docker".to_string()),
        total: items.len(),
        running,
        items,
    })
}

fn rate(now: u64, before: u64, seconds: f64) -> f64 {
    if seconds <= 0.0 || now < before {
        return 0.0;
    }
    (now - before) as f64 / seconds
}

/// 本次采集带不带"重块"（进程列表、容器）。
///
/// 轻量采集（每秒刷新用）故意不跑 `ps` / `docker ps`，所以这两块缺失是**预期**，
/// 不该被记成"部分数据缺失"——否则界面每秒都闪一行黄字。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Extras {
    Full,
    Lite,
}

/// 解析一次采集输出（完整版）。
pub fn interpret(raw: &str) -> ParsedOverview {
    interpret_with(raw, Extras::Full)
}

/// 解析一次采集输出，按 `extras` 决定"缺进程/容器"算不算降级。
pub fn interpret_with(raw: &str, extras: Extras) -> ParsedOverview {
    let sections = split_sections(raw);
    let lines_of = |key: &str| sections.get(key).map(Vec::as_slice).unwrap_or(&[]);
    let mut out = ParsedOverview::default();
    let mut degraded: Vec<String> = Vec::new();

    // ── 系统信息 ───────────────────────────────────────────────
    let meta = kv(lines_of("meta"));
    out.system.hostname = meta
        .get("hostname")
        .copied()
        .unwrap_or_default()
        .to_string();
    out.system.os = meta.get("os").copied().unwrap_or_default().to_string();
    out.system.kernel = meta.get("kernel").copied().unwrap_or_default().to_string();
    out.system.arch = meta.get("arch").copied().unwrap_or_default().to_string();
    out.system.uptime_secs = meta
        .get("uptime")
        .and_then(|v| v.trim().parse::<f64>().ok())
        .unwrap_or(0.0) as u64;
    out.system.host_ip = meta
        .get("ip")
        .copied()
        .unwrap_or_default()
        .trim()
        .to_string();

    // ── CPU ───────────────────────────────────────────────────
    let cpuinfo = kv(lines_of("cpuinfo"));
    out.cpu.cores = cpuinfo
        .get("cores")
        .and_then(|v| v.trim().parse::<u32>().ok())
        .unwrap_or(0);
    out.cpu.model = cpuinfo
        .get("model")
        .copied()
        .unwrap_or_default()
        .to_string();

    let first_cpu_line = |key: &str| lines_of(key).first().copied().unwrap_or_default();
    match (
        CpuSample::parse(first_cpu_line("stat1")),
        CpuSample::parse(first_cpu_line("stat2")),
    ) {
        (Some(before), Some(after)) => {
            let total = after.total().saturating_sub(before.total());
            if total > 0 {
                let idle = after.idle_all().saturating_sub(before.idle_all());
                out.cpu.usage_percent = percent(total.saturating_sub(idle), total);
                out.cpu.user_percent = percent(
                    (after.user + after.nice).saturating_sub(before.user + before.nice),
                    total,
                );
                out.cpu.system_percent = percent(
                    (after.system + after.irq + after.softirq)
                        .saturating_sub(before.system + before.irq + before.softirq),
                    total,
                );
                out.cpu.iowait_percent = percent(after.iowait.saturating_sub(before.iowait), total);
            }
        }
        _ => degraded.push("CPU".to_string()),
    }

    // ── 负载 ───────────────────────────────────────────────────
    if let Some(line) = lines_of("loadavg").first() {
        let f: Vec<&str> = line.split_whitespace().collect();
        if f.len() >= 4 {
            out.load.one = f[0].parse().unwrap_or(0.0);
            out.load.five = f[1].parse().unwrap_or(0.0);
            out.load.fifteen = f[2].parse().unwrap_or(0.0);
            if let Some((running, total)) = f[3].split_once('/') {
                out.load.running = running.parse().unwrap_or(0);
                out.load.total = total.parse().unwrap_or(0);
            }
        }
    }

    // ── 内存 / Swap ────────────────────────────────────────────
    let mem = meminfo_kb(lines_of("meminfo"));
    out.memory.total_kb = *mem.get("MemTotal").unwrap_or(&0);
    out.memory.buffers_kb = *mem.get("Buffers").unwrap_or(&0);
    out.memory.cached_kb = *mem.get("Cached").unwrap_or(&0);
    // Cached 里含 tmpfs/shmem，那部分删不掉也回收不了，界面要用它来算"真可回收"
    out.memory.shmem_kb = *mem.get("Shmem").unwrap_or(&0);
    let free = *mem.get("MemFree").unwrap_or(&0);
    out.memory.available_kb = match mem.get("MemAvailable") {
        Some(v) if *v > 0 => *v,
        // 老内核没有 MemAvailable：退回 free + buffers + cached 的经典口径。
        _ => free + out.memory.buffers_kb + out.memory.cached_kb,
    };
    out.memory.used_kb = out.memory.total_kb.saturating_sub(out.memory.available_kb);
    out.memory.swap_total_kb = *mem.get("SwapTotal").unwrap_or(&0);
    let swap_free = *mem.get("SwapFree").unwrap_or(&0);
    out.memory.swap_used_kb = out.memory.swap_total_kb.saturating_sub(swap_free);

    // ── 磁盘 ───────────────────────────────────────────────────
    out.disks = parse_df(lines_of("df"));
    let (zfs_pools, pool_of) = parse_zpools(lines_of("zpool"));
    out.zfs_pools = zfs_pools;
    out.block_disks = parse_block_disks(lines_of("lsblk"), &out.disks, &pool_of);
    out.mem_breakdown = parse_mem_breakdown(&kv(lines_of("memdetail")));
    out.gpus = parse_gpus(lines_of("gpu"));

    // ── 网络流量 / 磁盘 IO（两次采样差分）────────────────────
    let seconds = sample_seconds(&sections);
    let net_before = parse_netdev(lines_of("netdev1"));
    let net_after = parse_netdev(lines_of("netdev2"));
    for (name, (rx, tx)) in &net_after {
        let (rx0, tx0) = net_before.get(name).copied().unwrap_or((0, 0));
        out.network.push(NetInterface {
            name: name.clone(),
            rx_bytes: *rx,
            tx_bytes: *tx,
            rx_rate: rate(*rx, rx0, seconds),
            tx_rate: rate(*tx, tx0, seconds),
        });
    }
    out.network.sort_by(|a, b| a.name.cmp(&b.name));

    let io_before = parse_diskstats(lines_of("diskstats1"));
    let io_after = parse_diskstats(lines_of("diskstats2"));
    for (device, (read, write)) in &io_after {
        let (r0, w0) = io_before.get(device).copied().unwrap_or((0, 0));
        out.disk_io.push(DiskIo {
            device: device.clone(),
            read_bytes: *read,
            write_bytes: *write,
            read_rate: rate(*read, r0, seconds),
            write_rate: rate(*write, w0, seconds),
        });
    }
    out.disk_io.sort_by(|a, b| a.device.cmp(&b.device));

    // ── 进程 / 容器 ────────────────────────────────────────────
    out.processes = parse_ps(lines_of("ps"));
    match sections.get("docker") {
        Some(lines) => match parse_containers(lines) {
            Some(summary) => out.containers = summary,
            None => out.containers = ContainerSummary::default(),
        },
        None => {
            if extras == Extras::Full {
                degraded.push("容器".to_string());
            }
        }
    }

    // ── 缺块汇总 ───────────────────────────────────────────────
    if sections.is_empty() {
        degraded.insert(0, "没有拿到任何输出（仅支持 Linux 服务器）".to_string());
    } else if lines_of("meta").is_empty() && lines_of("stat1").is_empty() {
        degraded.insert(0, "未找到 /proc：概览仅支持 Linux 服务器".to_string());
    }
    if lines_of("meminfo").is_empty() {
        degraded.push("内存".to_string());
    }
    if lines_of("df").is_empty() {
        degraded.push("磁盘".to_string());
    }
    if out.processes.is_empty() && extras == Extras::Full {
        degraded.push("进程列表".to_string());
    }
    out.degraded = degraded;
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "@@meta
hostname=web-01
os=Ubuntu 24.04.1 LTS
kernel=6.8.0-45-generic
arch=x86_64
uptime=123456.78
ip=192.168.28.99
@@cpuinfo
cores=4
model= Intel(R) Xeon(R) CPU E5-2680 v4 @ 2.40GHz
@@t1
1700000000000000000
@@stat1
cpu  1000 10 500 8000 100 0 20 0 0 0
@@meminfo
MemTotal:       16384000 kB
MemFree:         2048000 kB
MemAvailable:   12288000 kB
Buffers:          512000 kB
Cached:          4096000 kB
SwapTotal:       2048000 kB
SwapFree:        1536000 kB
@@loadavg
0.52 0.58 0.59 2/345 6789
@@df
Filesystem     1024-blocks     Used Available Capacity Mounted on
/dev/sda1        102400000 51200000  46080000      53% /
tmpfs              8192000        0   8192000       0% /dev/shm
/dev/sdb1        204800000 10240000 183500000       6% /data
@@diskstats1
   8       0 sda 1000 0 20000 100 2000 0 40000 200 0 0 0
   8      16 sdb 10 0 200 1 20 0 400 2 0 0 0
   7       0 loop0 5 0 40 0 0 0 0 0 0 0 0
@@netdev1
Inter-|   Receive                                                |  Transmit
 face |bytes    packets errs drop fifo frame compressed multicast|bytes    packets errs drop fifo colls carrier compressed
    lo: 1000000 100 0 0 0 0 0 0 1000000 100 0 0 0 0 0 0
  eth0: 5000000 4000 0 0 0 0 0 0 2000000 2000 0 0 0 0 0 0
@@ps
    PID %CPU %MEM   RSS COMMAND
   1234 25.0  3.1 51200 nginx
    567  5.5  1.2 20480 sshd
@@t2
1700000000300000000
@@stat2
cpu  1100 10 560 8300 110 0 20 0 0 0
@@diskstats2
   8       0 sda 1100 0 22000 110 2100 0 44000 210 0 0 0
   8      16 sdb 10 0 200 1 20 0 400 2 0 0 0
   7       0 loop0 5 0 40 0 0 0 0 0 0 0 0
@@netdev2
Inter-|   Receive                                                |  Transmit
 face |bytes    packets errs drop fifo frame compressed multicast|bytes    packets errs drop fifo colls carrier compressed
    lo: 1000000 100 0 0 0 0 0 0 1000000 100 0 0 0 0 0 0
  eth0: 5030000 4010 0 0 0 0 0 0 2010000 2005 0 0 0 0 0 0
@@docker
abc123456789|web|nginx:1.27|running|Up 3 hours
def987654321|db|postgres:16|exited|Exited (0) 2 days ago
@@end
";

    #[test]
    fn parses_system_and_cpu() {
        let parsed = interpret(SAMPLE);
        assert_eq!(parsed.system.hostname, "web-01");
        assert_eq!(parsed.system.os, "Ubuntu 24.04.1 LTS");
        assert_eq!(parsed.system.kernel, "6.8.0-45-generic");
        assert_eq!(parsed.system.uptime_secs, 123456);
        assert_eq!(parsed.system.host_ip, "192.168.28.99");
        assert_eq!(parsed.cpu.cores, 4);
        assert_eq!(
            parsed.cpu.model,
            "Intel(R) Xeon(R) CPU E5-2680 v4 @ 2.40GHz"
        );
    }

    #[test]
    fn cpu_usage_comes_from_two_samples() {
        let parsed = interpret(SAMPLE);
        // 总增量：user 100 + system 60 + idle 300 + iowait 10 + softirq 0 = 470
        // 空闲增量：300 + 10 = 310 → 使用率 (470 - 310) / 470 ≈ 34.04%
        assert!(
            (parsed.cpu.usage_percent - 34.04).abs() < 0.1,
            "{}",
            parsed.cpu.usage_percent
        );
        assert!((parsed.cpu.user_percent - 21.28).abs() < 0.1);
        assert!((parsed.cpu.iowait_percent - 2.13).abs() < 0.1);
    }

    #[test]
    fn memory_uses_mem_available() {
        let parsed = interpret(SAMPLE);
        assert_eq!(parsed.memory.total_kb, 16_384_000);
        assert_eq!(parsed.memory.available_kb, 12_288_000);
        assert_eq!(parsed.memory.used_kb, 4_096_000);
        assert_eq!(parsed.memory.swap_used_kb, 512_000);
        assert!(parsed.degraded.is_empty(), "{:?}", parsed.degraded);
    }

    #[test]
    fn disks_skip_pseudo_filesystems() {
        let parsed = interpret(SAMPLE);
        let mounts: Vec<&str> = parsed.disks.iter().map(|d| d.mount.as_str()).collect();
        assert_eq!(mounts, vec!["/", "/data"]);
        assert!((parsed.disks[0].use_percent - 53.0).abs() < f64::EPSILON);
    }

    #[test]
    fn network_and_disk_io_are_rates_over_sampled_interval() {
        let parsed = interpret(SAMPLE);
        let eth0 = parsed
            .network
            .iter()
            .find(|n| n.name == "eth0")
            .expect("eth0");
        // 增量 30000 字节 / 0.3s = 100000 B/s
        assert!((eth0.rx_rate - 100_000.0).abs() < 1.0, "{}", eth0.rx_rate);
        assert!((eth0.tx_rate - 33_333.33).abs() < 1.0);
        // lo 与虚拟网卡不出现
        assert!(!parsed.network.iter().any(|n| n.name == "lo"));

        let sda = parsed
            .disk_io
            .iter()
            .find(|d| d.device == "sda")
            .expect("sda");
        // 扇区增量 2000 × 512 = 1MB / 0.3s
        assert!(
            (sda.read_rate - 3_413_333.3).abs() < 10.0,
            "{}",
            sda.read_rate
        );
        assert!(!parsed.disk_io.iter().any(|d| d.device == "loop0"));
    }

    #[test]
    fn containers_are_parsed_with_state() {
        let parsed = interpret(SAMPLE);
        let containers = &parsed.containers;
        assert_eq!(containers.engine.as_deref(), Some("docker"));
        assert_eq!(containers.total, 2);
        assert_eq!(containers.running, 1);
        assert_eq!(containers.items[0].name, "web");
        assert_eq!(containers.items[1].state, "exited");
    }

    #[test]
    fn missing_docker_is_not_a_degradation() {
        let raw = "@@meta\nhostname=x\n@@docker\n__no_engine__\n@@end\n";
        let parsed = interpret(raw);
        assert!(parsed.containers.engine.is_none());
        assert_eq!(parsed.containers.total, 0);
    }

    #[test]
    fn same_filesystem_mounted_twice_is_listed_once() {
        // Unraid 的 shfs：同一个文件系统挂在 /mnt/deva 与它下面的共享目录。
        let raw = "@@df
Filesystem 1024-blocks Used Available Capacity Mounted on
shfs 2048000000 20480000 2027520000 1% /mnt/deva
shfs 2048000000 61440000 1986560000 3% /mnt/deva/media
/dev/sda1 102400 20480 81920 20% /boot
@@end
";
        let parsed = interpret(raw);
        let mounts: Vec<&str> = parsed.disks.iter().map(|d| d.mount.as_str()).collect();
        assert_eq!(mounts, vec!["/mnt/deva", "/boot"]);
    }

    #[test]
    fn nested_mounts_of_the_same_pool_are_collapsed() {
        // Unraid 的 shfs：每个共享都被挂一遍，容量几乎相同，只有父挂载有意义。
        let raw = "@@df
Filesystem 1024-blocks Used Available Capacity Mounted on
rootfs 11400000 3200000 8200000 28% /
/dev/sda1 29000000 1100000 27900000 4% /boot
deva 2400000000 24000000 2376000000 1% /mnt/deva
deva/media 2442000000 73260000 2368740000 3% /mnt/deva/media
deva/temp 2379000000 23790000 2355210000 1% /mnt/deva/temp
@@end
";
        let parsed = interpret(raw);
        let mounts: Vec<&str> = parsed.disks.iter().map(|d| d.mount.as_str()).collect();
        assert_eq!(mounts, vec!["/", "/boot", "/mnt/deva"]);
    }

    #[test]
    fn real_partition_mounted_under_a_directory_is_kept() {
        // 父挂载本身是块设备：子目录挂的是另一块盘，不能当"视图"吞掉。
        let raw = "@@df
Filesystem 1024-blocks Used Available Capacity Mounted on
/dev/sda1 100000 20000 80000 20% /
/dev/sdb1 200000 20000 180000 10% /data
/dev/sdc1 300000 20000 280000 7% /data/backup
@@end
";
        let parsed = interpret(raw);
        let mounts: Vec<&str> = parsed.disks.iter().map(|d| d.mount.as_str()).collect();
        assert_eq!(mounts, vec!["/", "/data", "/data/backup"]);
    }

    #[test]
    fn gpus_from_nvidia_and_sysfs_are_normalised_to_kb() {
        let raw = "@@gpu
nvidia|NVIDIA GeForce RTX 4090|12|2048|24564|45
sysfs|GPU (card1)|7|1073741824|8589934592|0
@@end
";
        let parsed = interpret(raw);
        assert_eq!(parsed.gpus.len(), 2);

        let rtx = &parsed.gpus[0];
        assert_eq!(rtx.name, "NVIDIA GeForce RTX 4090");
        assert_eq!(rtx.usage_percent, 12.0);
        // nvidia-smi 报 MB
        assert_eq!(rtx.memory_used_kb, 2048 * 1024);
        assert_eq!(rtx.memory_total_kb, 24564 * 1024);
        assert_eq!(rtx.temperature_c, 45);

        // sysfs 报字节：1 GiB = 1048576 kB
        let igpu = &parsed.gpus[1];
        assert_eq!(igpu.memory_used_kb, 1024 * 1024);
        assert_eq!(igpu.memory_total_kb, 8 * 1024 * 1024);
    }

    #[test]
    fn gpu_fields_drivers_do_not_report_become_zero() {
        // 老驱动对利用率/显存/温度都可能吐 [N/A]，不能让整行解析失败。
        let raw = "@@gpu
nvidia|Tesla T4|[N/A]|[N/A]|[N/A]|[N/A]
@@end
";
        let parsed = interpret(raw);
        assert_eq!(parsed.gpus.len(), 1);
        assert_eq!(parsed.gpus[0].name, "Tesla T4");
        assert_eq!(parsed.gpus[0].usage_percent, 0.0);
        assert_eq!(parsed.gpus[0].memory_total_kb, 0);
    }

    #[test]
    fn no_gpu_section_yields_no_rows() {
        let parsed = interpret("@@meta\nhostname=x\n@@end\n");
        assert!(parsed.gpus.is_empty());
    }

    #[test]
    fn block_disks_come_from_lsblk_and_borrow_df_usage() {
        let raw = "@@df
Filesystem 1024-blocks Used Available Capacity Mounted on
/dev/sda1 100000 20000 80000 20% /
/dev/sdb1 200000 20000 180000 10% /data
@@lsblk
NAME=\"sda\" TYPE=\"disk\" SIZE=\"128035676160\" MOUNTPOINT=\"\"
NAME=\"sda1\" TYPE=\"part\" SIZE=\"128034551808\" MOUNTPOINT=\"/\"
NAME=\"sdb\" TYPE=\"disk\" SIZE=\"2000398934016\" MOUNTPOINT=\"\"
NAME=\"sdb1\" TYPE=\"part\" SIZE=\"2000397852672\" MOUNTPOINT=\"/data\"
NAME=\"nvme0n1\" TYPE=\"disk\" SIZE=\"500107862016\" MOUNTPOINT=\"\"
NAME=\"loop0\" TYPE=\"loop\" SIZE=\"1024\" MOUNTPOINT=\"/snap/core\"
@@end
";
        let parsed = interpret(raw);
        // 按容量降序；loop 不是物理盘，不进列表
        let names: Vec<&str> = parsed.block_disks.iter().map(|d| d.name.as_str()).collect();
        assert_eq!(names, vec!["sdb", "nvme0n1", "sda"]);

        let sda = parsed
            .block_disks
            .iter()
            .find(|d| d.name == "sda")
            .expect("sda");
        assert_eq!(sda.total_kb, 100000);
        assert_eq!(sda.used_kb, 20000);
        assert_eq!(sda.mount, "/");

        // 没挂载的盘只报容量：用量未知，界面会显示"未挂载"而不是 0%
        let nvme = parsed
            .block_disks
            .iter()
            .find(|d| d.name == "nvme0n1")
            .expect("nvme");
        assert_eq!(nvme.size_bytes, 500107862016);
        assert_eq!(nvme.total_kb, 0);
    }

    #[test]
    fn zfs_member_disks_carry_their_pool() {
        // ZFS 成员盘自己没有挂载点（池才有），但 lsblk 会标 zfs_member，
        // zpool -v 能给出成员归属。
        let raw = "@@lsblk\nNAME=\"nvme0n1\" TYPE=\"disk\" SIZE=\"2000398934016\" FSTYPE=\"zfs_member\" MOUNTPOINT=\"\"\nNAME=\"sda\" TYPE=\"disk\" SIZE=\"128035676160\" FSTYPE=\"\" MOUNTPOINT=\"\"\nNAME=\"sda1\" TYPE=\"part\" SIZE=\"128034551808\" FSTYPE=\"ext4\" MOUNTPOINT=\"/\"\n@@zpool\ntank\t2000398934016\t400000000\n\tnvme0n1\t2000398934016\t-\n@@end\n";
        let parsed = interpret(raw);

        assert_eq!(parsed.zfs_pools.len(), 1);
        assert_eq!(parsed.zfs_pools[0].name, "tank");
        assert_eq!(parsed.zfs_pools[0].alloc_bytes, 400000000);

        let nvme = parsed
            .block_disks
            .iter()
            .find(|d| d.name == "nvme0n1")
            .expect("nvme0n1");
        assert_eq!(nvme.pool, "tank");
        assert_eq!(nvme.fstype, "zfs_member");
        // 池的容量在池上，成员盘自己不报用量
        assert_eq!(nvme.total_kb, 0);

        let sda = parsed
            .block_disks
            .iter()
            .find(|d| d.name == "sda")
            .expect("sda");
        assert_eq!(sda.pool, "");
        assert_eq!(sda.fstype, "");
    }

    #[test]
    fn zfs_marker_alone_still_identifies_the_disk() {
        // 没装 zpool 命令（或权限不够）时，至少别把成员盘当成"空盘"
        let raw = "@@lsblk\nNAME=\"sdb\" TYPE=\"disk\" SIZE=\"1000\" FSTYPE=\"zfs_member\" MOUNTPOINT=\"\"\n@@end\n";
        let parsed = interpret(raw);
        assert_eq!(parsed.block_disks[0].pool, "zfs");
    }

    #[test]
    fn mem_breakdown_reads_optional_sources() {
        let raw = "@@memdetail\nzfs_arc_kb=1048576\ncontainer_kb=524288\nvm_kb=2097152\n@@end\n";
        let parsed = interpret(raw);
        assert_eq!(parsed.mem_breakdown.zfs_arc_kb, 1_048_576);
        assert_eq!(parsed.mem_breakdown.container_kb, 524_288);
        assert_eq!(parsed.mem_breakdown.vm_kb, 2_097_152);

        // 没有 ZFS / docker / 虚拟机时就是 0，不能当成"解析失败"
        let empty = interpret("@@memdetail\nzfs_arc_kb=0\n@@end\n");
        assert_eq!(empty.mem_breakdown.vm_kb, 0);
        assert_eq!(empty.mem_breakdown.container_kb, 0);
    }

    #[test]
    fn partitions_are_not_listed_next_to_their_disk() {
        let raw = "@@t1
1000000000
@@diskstats1
 259       0 nvme0n1 10 0 200 1 20 0 400 2 0 0 0
 259       1 nvme0n1p1 10 0 200 1 20 0 400 2 0 0 0
   8       0 sda 10 0 200 1 20 0 400 2 0 0 0
   8       1 sda1 10 0 200 1 20 0 400 2 0 0 0
@@t2
1300000000
@@diskstats2
 259       0 nvme0n1 20 0 400 2 40 0 800 4 0 0 0
 259       1 nvme0n1p1 20 0 400 2 40 0 800 4 0 0 0
   8       0 sda 20 0 400 2 40 0 800 4 0 0 0
   8       1 sda1 20 0 400 2 40 0 800 4 0 0 0
@@end
";
        let parsed = interpret(raw);
        let devices: Vec<&str> = parsed.disk_io.iter().map(|d| d.device.as_str()).collect();
        assert_eq!(devices, vec!["nvme0n1", "sda"]);
    }

    #[test]
    fn macvtap_and_vhost_shadow_interfaces_are_hidden() {
        let raw = "@@netdev1
  eth0: 1000 1 0 0 0 0 0 0 1000 1 0 0 0 0 0 0
  macvtap68: 1000 1 0 0 0 0 0 0 1000 1 0 0 0 0 0 0
  vhost0: 1000 1 0 0 0 0 0 0 1000 1 0 0 0 0 0 0
@@t1
1000000000
@@netdev2
  eth0: 2000 2 0 0 0 0 0 0 2000 2 0 0 0 0 0 0
  macvtap68: 2000 2 0 0 0 0 0 0 2000 2 0 0 0 0 0 0
  vhost0: 2000 2 0 0 0 0 0 0 2000 2 0 0 0 0 0 0
@@t2
1300000000
@@end
";
        let parsed = interpret(raw);
        let names: Vec<&str> = parsed.network.iter().map(|n| n.name.as_str()).collect();
        assert_eq!(names, vec!["eth0"]);
    }

    #[test]
    fn missing_sections_are_reported_but_do_not_panic() {
        let raw = "@@meta\nhostname=x\n@@end\n";
        let parsed = interpret(raw);
        assert_eq!(parsed.system.hostname, "x");
        assert!(parsed.degraded.iter().any(|d| d == "CPU"));
        assert!(parsed.degraded.iter().any(|d| d == "容器"));
        assert!(parsed.degraded.iter().any(|d| d == "进程列表"));
    }

    #[test]
    fn empty_output_explains_platform_boundary() {
        let parsed = interpret("");
        assert!(parsed.degraded[0].contains("Linux"));
    }

    #[test]
    fn garbage_lines_are_ignored() {
        let raw = "@@df\nFilesystem 1024-blocks Used Available Capacity Mounted on\nnot a df line\n@@ps\n@@netdev1\n@@netdev2\n@@end\n";
        let parsed = interpret(raw);
        assert!(parsed.disks.is_empty());
        assert!(parsed.processes.is_empty());
        assert!(parsed.network.is_empty());
    }
}
