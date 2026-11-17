//! 服务器管理（只读）。
//!
//! 归属：**模型层**。拥有：管理模块清单、远端只读采集脚本、输出解析与指标口径。
//! 提供：`overview_script()` / `interpret(raw)` / `modules(&ContainerSummary)`。
//! 允许调用：`tl_protocol`（DTO）、标准库。
//!
//! 纪律：
//! - **无状态**：不落盘、不缓存，每次请求现采现解析（会话归 `tunnel`，本模型不持有）；
//! - **只读**：脚本里没有写操作，写操作类模块（进程结束、文件删除、防火墙）另立契约；
//! - **不假装能用**：未实现模块在清单里标 `Planned`，界面据此显示"规划中"。

pub mod collect;
pub mod containers;
pub mod parse;

use tl_protocol::{AdminModule, AdminModuleState, ContainerSummary};

pub use collect::{Probe, EXEC_BUDGET};
pub use containers::{
    action_script, action_verb, allowed_actions, console_command, detail_script,
    image_users_script, list_script, logs_script, merge_inspect, merge_stats, parse_detail,
    parse_image_users, parse_list, parse_logs, parse_stats, remove_script, resolve_cid, rmi_script,
    stats_script, validate_cid, ConsoleMode, ACTION_BUDGET, STATS_BUDGET,
};
pub use parse::{interpret, interpret_with, split_sections, Extras, ParsedOverview};

/// 概览采集脚本（一次往返）。
///
/// 按 `probe` 拆成"每秒 / 每 5 秒 / 每 10 秒"三种口径，互不牵连：
/// 每秒那条不跑 `df`（挂载了网络盘时会卡住整个脚本），也不跑 `ps` / `docker ps`。
pub fn overview_script(probe: Probe) -> String {
    collect::script(probe)
}

/// 管理页导航清单。
///
/// 清单由核心给：新增模块时只改这里 + 前端加一个组件，界面结构不用动。
/// `containers` 用来把"这台机器没有容器引擎"变成 `Unavailable` 而不是空白页。
pub fn modules(containers: &ContainerSummary) -> Vec<AdminModule> {
    let module = |id: &str, title: &str, state: AdminModuleState, note: &str| AdminModule {
        id: id.to_string(),
        title: title.to_string(),
        state,
        note: Some(note.to_string()),
    };

    vec![
        module(
            "overview",
            "概览",
            AdminModuleState::Ready,
            "系统信息与实时指标（一次采集）",
        ),
        module(
            "processes",
            "进程管理",
            AdminModuleState::Planned,
            "全量进程、按 CPU/内存排序、结束进程",
        ),
        // 容器模块已实现：有引擎就是 Ready，没引擎仍是 Unavailable（不是"规划中"）。
        module(
            "containers",
            "容器管理",
            if containers.engine.is_some() {
                AdminModuleState::Ready
            } else {
                AdminModuleState::Unavailable
            },
            if containers.engine.is_some() {
                "容器列表、日志、启停与进入容器"
            } else {
                "未检测到 docker"
            },
        ),
        // 虚拟机：位置留好，核心尚未实现（P4），侧边栏紧跟在容器下面。
        module(
            "vms",
            "虚拟机",
            AdminModuleState::Planned,
            "virsh 列表、启停与串口控制台（后续开发）",
        ),
        module(
            "files",
            "文件管理",
            AdminModuleState::Planned,
            "目录浏览、上传下载、删除与重命名（SFTP）",
        ),
        module(
            "terminal",
            "终端",
            AdminModuleState::Planned,
            "浏览器内的交互式 shell",
        ),
        module(
            "network",
            "网络管理",
            AdminModuleState::Planned,
            "网卡、路由、监听端口与 DNS",
        ),
        module(
            "firewall",
            "防火墙",
            AdminModuleState::Planned,
            "ufw / firewalld / iptables 规则查看与开关",
        ),
        module(
            "services",
            "服务与自启",
            AdminModuleState::Planned,
            "systemd 单元列表、启停与开机项",
        ),
        module(
            "logs",
            "日志",
            AdminModuleState::Planned,
            "journalctl 与指定日志文件的跟随",
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overview_is_always_ready() {
        let modules = modules(&ContainerSummary::default());
        let overview = modules
            .iter()
            .find(|m| m.id == "overview")
            .expect("overview");
        assert_eq!(overview.state, AdminModuleState::Ready);
    }

    #[test]
    fn containers_without_engine_are_flagged_unavailable() {
        let modules = modules(&ContainerSummary::default());
        let containers = modules
            .iter()
            .find(|m| m.id == "containers")
            .expect("containers");
        assert_eq!(containers.state, AdminModuleState::Unavailable);
        assert!(containers.note.as_deref().unwrap_or("").contains("docker"));
    }

    #[test]
    fn containers_with_engine_are_ready() {
        let summary = ContainerSummary {
            engine: Some("docker".to_string()),
            ..Default::default()
        };
        let modules = modules(&summary);
        let containers = modules
            .iter()
            .find(|m| m.id == "containers")
            .expect("containers");
        assert_eq!(containers.state, AdminModuleState::Ready);
    }

    #[test]
    fn vms_are_planned_and_follow_containers() {
        let summary = ContainerSummary {
            engine: Some("docker".to_string()),
            ..Default::default()
        };
        let modules = modules(&summary);
        let at = |id: &str| modules.iter().position(|m| m.id == id).expect(id);
        assert_eq!(modules[at("vms")].state, AdminModuleState::Planned);
        // 侧边栏顺序：虚拟机紧跟在容器管理后面
        assert_eq!(at("vms"), at("containers") + 1);
    }

    #[test]
    fn planned_modules_still_carry_a_note() {
        let modules = modules(&ContainerSummary::default());
        for m in modules
            .iter()
            .filter(|m| m.state != AdminModuleState::Ready)
        {
            assert!(m.note.is_some(), "{} 应说明规划内容或不可用原因", m.id);
        }
    }

    #[test]
    fn script_is_read_only_and_single_quote_free() {
        for probe in [Probe::Full, Probe::Live, Probe::DiskIo, Probe::Disks] {
            let script = overview_script(probe);
            // 整段被 `sh -c '...'` 包裹：**正文**里再出现单引号会把命令截断。
            let body = script
                .strip_prefix("sh -c '")
                .and_then(|s| s.strip_suffix('\''))
                .expect("脚本应被 sh -c 单引号包裹");
            assert!(!body.contains('\''), "脚本正文不能含单引号");
            for forbidden in ["rm ", "kill ", "systemctl ", " > /", ">>/"] {
                assert!(
                    !body.contains(forbidden),
                    "采集脚本必须是只读的：{forbidden}"
                );
            }
        }
    }

    #[test]
    fn each_probe_only_carries_what_it_needs() {
        // 每秒那条：CPU / 内存 / 网络 / GPU / 磁盘 IO 都读 /proc，一起采最省；
        // 但**不能带 df**——挂了网络盘时 `df` 会把整条命令拖住，而容量也不是每秒变的。
        let live = overview_script(Probe::Live);
        assert!(!live.contains("@@df"), "每秒那条不该跑 df");
        assert!(!live.contains("@@ps"));
        assert!(!live.contains("@@docker"));
        assert!(!live.contains("@@zpool"));
        assert!(
            !live.contains("@@memdetail"),
            "内存分源要扫进程表，跟 10 秒那档走"
        );
        assert!(live.contains("@@stat1") && live.contains("@@stat2"));
        assert!(live.contains("@@netdev1") && live.contains("@@netdev2"));
        // 磁盘 IO 并进了每秒这一档：`/proc/diskstats` 只是一次 cat
        assert!(live.contains("@@diskstats1") && live.contains("@@diskstats2"));
        assert!(live.contains("@@gpu"));

        // 磁盘 IO 单独那条保留（接口仍然可用），只采 diskstats 两次。
        let io = overview_script(Probe::DiskIo);
        assert!(io.contains("@@diskstats1") && io.contains("@@diskstats2"));
        assert!(!io.contains("@@df"));
        assert!(!io.contains("@@stat1"));
        assert!(!io.contains("@@netdev1"));

        // 磁盘容量 + 内存分源：没有二次采样，不用 sleep。
        let disks = overview_script(Probe::Disks);
        assert!(disks.contains("@@df"));
        assert!(disks.contains("@@lsblk"));
        assert!(disks.contains("@@zpool"));
        assert!(disks.contains("@@memdetail"));
        assert!(!disks.contains("@@t1"));
        assert!(!disks.contains("sleep"));

        let full = overview_script(Probe::Full);
        assert!(full.contains("@@ps"));
        assert!(full.contains("@@docker"));
        assert!(full.contains("@@df"));
    }
}
