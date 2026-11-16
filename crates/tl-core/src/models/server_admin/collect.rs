//! 概览采集脚本。
//!
//! 归属：`server_admin` 模型内部文件组。拥有：**只读**采集脚本的正文。
//! 提供：`script(probe)`（一条命令，一次往返采完所需指标）。
//! 允许调用：无（纯字符串拼接）。
//!
//! 纪律：
//! - **只读**：脚本里不允许出现任何写操作（重定向写文件、kill、rm、systemctl 等）；
//! - **POSIX**：只用 `sh` 与 `/proc`，不依赖 GNU 专有参数；
//! - **分节**：每节以 `@@name` 开头，解析侧按节容错——某节缺失只影响那一块；
//! - **无单引号**：整段被 `sh -c '...'` 包裹，脚本内不能出现单引号。

use std::time::Duration;

/// 一次采样的预算：脚本里有二次采样，正常 1s 内返回；慢机器给足 12s。
pub const EXEC_BUDGET: Duration = Duration::from_secs(12);

/// 采什么。
///
/// 界面按刷新频率拆成三种：CPU/内存/流量每秒、磁盘 IO 每 5 秒、磁盘容量每 10 秒。
/// 拆开的理由很实在——每秒跑一次 `df` 会在挂载了网络盘时直接卡住整个脚本，
/// 而磁盘容量本来也不是每秒变的东西。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Probe {
    /// 全部（进页面、手动刷新）。
    Full,
    /// CPU / 内存 / 网络流量（两次采样算瞬时值）。
    Live,
    /// 磁盘 IO（两次采样算瞬时速率）。
    DiskIo,
    /// 磁盘容量（`df`，单次）。
    Disks,
}

impl Probe {
    /// 接口上的 `?probe=`：未知值一律按完整采集处理。
    pub fn parse(text: &str) -> Probe {
        match text {
            "live" => Probe::Live,
            "disk-io" => Probe::DiskIo,
            "disks" => Probe::Disks,
            _ => Probe::Full,
        }
    }
}

const META: &str = r#"echo @@meta
echo "hostname=$(hostname 2>/dev/null || cat /proc/sys/kernel/hostname 2>/dev/null)"
echo "os=$(grep -m1 "^PRETTY_NAME=" /etc/os-release 2>/dev/null | cut -d= -f2- | tr -d "\"")"
echo "kernel=$(uname -r 2>/dev/null)"
echo "arch=$(uname -m 2>/dev/null)"
echo "uptime=$(cut -d" " -f1 /proc/uptime 2>/dev/null)"
ip=$(hostname -I 2>/dev/null | tr " " "\n" | grep -v "^127\." | head -n1)
[ -z "$ip" ] && ip=$(ip -4 -o addr show scope global 2>/dev/null | head -n1 | sed "s/.*inet //" | cut -d/ -f1)
echo "ip=$ip"
"#;

const CPUINFO: &str = r#"echo @@cpuinfo
echo "cores=$(grep -c "^processor" /proc/cpuinfo 2>/dev/null)"
echo "model=$(grep -m1 "^model name" /proc/cpuinfo 2>/dev/null | cut -d: -f2-)"
"#;

const T1: &str = "echo @@t1\ndate +%s%N 2>/dev/null\n";

const STAT1: &str = "echo @@stat1\nhead -n1 /proc/stat 2>/dev/null\n";

const MEMINFO: &str = "echo @@meminfo\ncat /proc/meminfo 2>/dev/null\n";

const DF: &str = "echo @@df\ndf -kP 2>/dev/null\n";

/// 物理盘清单。`-P` 输出 `KEY="value"`，比默认表格好解析。
///
/// `df` 只有"挂载点"的概念（Unraid 会把每个共享挂一遍），要看"这台机器有几块盘、
/// 各多大"只能问 `lsblk`。
const LSBLK: &str = "echo @@lsblk\nlsblk -b -P -o NAME,TYPE,SIZE,FSTYPE,MOUNTPOINT 2>/dev/null\n";

/// ZFS 池清单。`-v` 会连带列出成员设备，`-H` 去表头，`-p` 用字节。
///
/// 解析约定：不缩进的行是池（`池名 总量 已分配`），缩进的行是成员（第一列是设备名）。
const ZPOOL: &str = "echo @@zpool\nif command -v zpool >/dev/null 2>&1; then zpool list -vHp -o name,size,alloc 2>/dev/null; fi\n";

/// GPU。优先 `nvidia-smi`（唯一能报显存与温度的通用途径），否则退回 `/sys/class/drm`。
///
/// 每行带上来源前缀：两者的容量单位不同（nvidia 是 MB、sysfs 是字节），
/// 解析侧必须知道该乘 1024 还是不乘。没有 GPU 时这一节是空的，不报错。
const GPU: &str = r#"echo @@gpu
if command -v nvidia-smi >/dev/null 2>&1; then
nvidia-smi --query-gpu=name,utilization.gpu,memory.used,memory.total,temperature.gpu --format=csv,noheader,nounits 2>/dev/null | sed "s/^/nvidia|/;s/, /|/g"
else
for d in /sys/class/drm/card[0-9]*/device; do
[ -f "$d/gpu_busy_percent" ] || continue
c=$(echo "$d" | sed "s|.*/drm/||;s|/device||")
drv=$(grep -m1 "^DRIVER=" "$d/uevent" 2>/dev/null | cut -d= -f2)
[ -z "$drv" ] && drv="unknown"
case "$drv" in
i915|xe) label="Intel 核显" ;;
amdgpu|radeon) label="AMD 显卡" ;;
nouveau) label="NVIDIA 显卡" ;;
*) label="$drv" ;;
esac
echo "sysfs|$label ($c)|$(cat "$d/gpu_busy_percent" 2>/dev/null)|$(cat "$d/mem_info_vram_used" 2>/dev/null)|$(cat "$d/mem_info_vram_total" 2>/dev/null)|0"
done
fi
"#;

/// 内存去向：VM / 容器 / ZFS ARC。
///
/// 跟着"每 5 秒"那一档走，而不是每秒：`ps -eo rss,comm` 在几千进程的机器上要几十毫秒，
/// 每秒跑一次不划算；内存构成本来也不是每秒剧变的东西。
///
/// 三个数都能缺：没有 ZFS 就没有 `arcstats`、没有 docker 就没有 cgroup 目录、
/// 没有虚拟机就匹配不到 qemu 进程——缺了就是 0，界面对应那段不画。
const MEM_DETAIL: &str = r#"echo @@memdetail
zfs=0
if [ -r /proc/spl/kstat/zfs/arcstats ]; then
zfs=$(awk "/^size / {print \$3}" /proc/spl/kstat/zfs/arcstats 2>/dev/null)
fi
cg=0
for f in /sys/fs/cgroup/system.slice/docker-*.slice/memory.current /sys/fs/cgroup/docker/*/memory.current /sys/fs/cgroup/memory/docker/*/memory.usage_in_bytes; do
[ -f "$f" ] || continue
v=$(cat "$f" 2>/dev/null)
cg=$((cg + ${v:-0}))
done
vm=0
if command -v ps >/dev/null 2>&1; then
set -- $(ps -eo rss,comm 2>/dev/null | grep -E "qemu|kvm" | cut -d" " -f1)
for v in "$@"; do vm=$((vm + v)); done
fi
echo "zfs_arc_kb=$((zfs / 1024))"
echo "container_kb=$((cg / 1024))"
echo "vm_kb=$vm"
"#;

const DISKSTATS1: &str = "echo @@diskstats1\ncat /proc/diskstats 2>/dev/null\n";

const NETDEV1: &str = "echo @@netdev1\ncat /proc/net/dev 2>/dev/null\n";

const PS_SECTION: &str = r#"echo @@ps
(ps -eo pid,pcpu,pmem,rss,comm --sort=-pcpu 2>/dev/null || ps -eo pid,pcpu,pmem,rss,comm 2>/dev/null) | head -n 11
"#;

/// 第二次采样的时间戳：所有速率都靠它和 `t1` 的差值算。
const T2: &str = "echo @@t2\ndate +%s%N 2>/dev/null\n";

const STAT2: &str = "echo @@stat2\nhead -n1 /proc/stat 2>/dev/null\n";

const DISKSTATS2: &str = "echo @@diskstats2\ncat /proc/diskstats 2>/dev/null\n";

const NETDEV2: &str = "echo @@netdev2\ncat /proc/net/dev 2>/dev/null\n";

const DOCKER_SECTION: &str = r#"echo @@docker
if command -v docker >/dev/null 2>&1; then docker ps -a --format "{{.ID}}|{{.Names}}|{{.Image}}|{{.State}}|{{.Status}}" 2>/dev/null; else echo __no_engine__; fi
"#;

const END: &str = "echo @@end\n'";

/// 按 probe 拼出采集脚本。
///
/// 采样窗口：完整 0.3s（多采几块），按需 0.2s（够算差值，又不让每秒那次拖太久）。
pub fn script(probe: Probe) -> String {
    let mut out = String::with_capacity(2048);
    out.push_str("sh -c '\n");
    match probe {
        Probe::Full => {
            out.push_str(META);
            out.push_str(CPUINFO);
            out.push_str(T1);
            out.push_str(STAT1);
            out.push_str(MEMINFO);
            out.push_str(DF);
            out.push_str(LSBLK);
            out.push_str(ZPOOL);
            out.push_str(DISKSTATS1);
            out.push_str(NETDEV1);
            out.push_str(GPU);
            out.push_str(MEM_DETAIL);
            out.push_str(PS_SECTION);
            out.push_str("sleep 0.3\n");
            out.push_str(T2);
            out.push_str(STAT2);
            out.push_str(DISKSTATS2);
            out.push_str(NETDEV2);
            out.push_str(DOCKER_SECTION);
        }
        Probe::Live => {
            out.push_str(META);
            out.push_str(CPUINFO);
            out.push_str(T1);
            out.push_str(STAT1);
            out.push_str(MEMINFO);
            out.push_str(NETDEV1);
            // GPU 使用率也是每秒变的，跟 CPU / 内存走同一条。
            out.push_str(GPU);
            // 磁盘 IO 一起走：`/proc/diskstats` 只是一次 cat，比多开一条隧道往返便宜得多。
            out.push_str(DISKSTATS1);
            out.push_str("sleep 0.2\n");
            out.push_str(T2);
            out.push_str(STAT2);
            out.push_str(NETDEV2);
            out.push_str(DISKSTATS2);
        }
        Probe::DiskIo => {
            out.push_str(T1);
            out.push_str(DISKSTATS1);
            out.push_str("sleep 0.2\n");
            out.push_str(T2);
            out.push_str(DISKSTATS2);
        }
        Probe::Disks => {
            out.push_str(DF);
            out.push_str(LSBLK);
            out.push_str(ZPOOL);
            // 内存分源跟着容量这档（10 秒）：`ps` 扫进程表是这里唯一的重活，
            // 而 VM / 容器 / ARC 的构成也不会每秒剧变。
            out.push_str(MEM_DETAIL);
        }
    }
    out.push_str(END);
    out
}
