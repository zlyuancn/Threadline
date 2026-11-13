//! 配置导入导出的判定与落地。
//!
//! **归属**：业务层（编排）。判定规则仍属模型：端口/域名冲突走
//! `TunnelStore::config_conflict`，连接/转发的字段校验走各模型的 `create` / `patch`。
//!
//! 这一层只做三件事：把包内条目映射成模型操作、决定每条的去向、把结果汇总成
//! 可读的报告。**不自己重写一套端口比较**——那正是"预览说能建、落地却失败"的来源。

use std::collections::HashSet;

use anyhow::Result;
use tl_protocol::{
    CreateServerRequest, CreateTunnelRequest, PatchServerRequest, PatchTunnelRequest,
    PlannedForward, PlannedServer, ProfileForward, ProfileImportFailure, ProfileImportRequest,
    ProfileImportResult, ProfileImportServer, ProfilePlan, ProfilePlanRequest,
};

use crate::models::server::ServerStore;
use crate::models::tunnel::{ConflictKind, TunnelStore};

/// 图标上限：与 `normalize_icon` 的 200KB 字符限制留出余量。
/// 超限时丢图标保条目，而不是整条导入失败。
const MAX_ICON_CHARS: usize = 190_000;

/// 只读预判：回答"如果现在导入，每条会怎样"。
pub fn plan(
    servers: &ServerStore,
    tunnels: &TunnelStore,
    req: ProfilePlanRequest,
) -> Result<ProfilePlan> {
    let existing_servers = servers.list()?.items;
    let existing_forwards = tunnels
        .list()?
        .items
        .into_iter()
        .map(|view| view.definition)
        .collect::<Vec<_>>();

    let mut taken_server_names: HashSet<String> =
        existing_servers.iter().map(|s| s.name.clone()).collect();
    let mut planned = Vec::with_capacity(req.servers.len());

    for (index, server) in req.servers.iter().enumerate() {
        let same_id = existing_servers.iter().any(|e| e.id == server.id);
        let same_name = existing_servers.iter().any(|e| e.name == server.name);
        let (action, reason, suggested_name) = if same_id {
            (
                "exists".to_string(),
                Some("本机已存在同一条连接".to_string()),
                None,
            )
        } else if same_name {
            (
                "name_taken".to_string(),
                Some(format!("已存在同名连接「{}」", server.name)),
                Some(unique_name(&server.name, &taken_server_names)),
            )
        } else {
            ("ok".to_string(), None, None)
        };
        taken_server_names.insert(server.name.clone());

        // 包内前序条目：同包两条抢同一地址/域名时，后一条也要报冲突
        let mut accepted: Vec<(String, String, u16, String)> = Vec::new();
        let mut forwards = Vec::with_capacity(server.forwards.len());
        for (f_index, forward) in server.forwards.iter().enumerate() {
            let (f_action, f_reason, occupied_by) =
                plan_forward(tunnels, forward, &existing_forwards, &accepted);
            if f_action == "ok" {
                accepted.push((
                    forward.name.clone(),
                    forward.local_host.clone(),
                    forward.local_port,
                    forward.domain.clone(),
                ));
            }
            forwards.push(PlannedForward {
                index: f_index,
                name: forward.name.clone(),
                action: f_action,
                reason: f_reason,
                occupied_by,
            });
        }

        planned.push(PlannedServer {
            index,
            name: server.name.clone(),
            action,
            reason,
            suggested_name,
            forwards,
        });
    }

    Ok(ProfilePlan { servers: planned })
}

fn plan_forward(
    tunnels: &TunnelStore,
    forward: &ProfileForward,
    existing: &[tl_protocol::TunnelDto],
    accepted: &[(String, String, u16, String)],
) -> (String, Option<String>, Option<String>) {
    // 名字重复（只提示，不阻止：create 会按名字拒绝，界面可用建议名）
    if let Some(other) = existing.iter().find(|t| t.name == forward.name) {
        return (
            "name_taken".to_string(),
            Some(format!("已存在同名转发「{}」", forward.name)),
            Some(other.name.clone()),
        );
    }
    // 包内重名
    if let Some((name, _, _, _)) = accepted.iter().find(|(name, ..)| *name == forward.name) {
        return (
            "name_taken".to_string(),
            Some(format!("同一个包里出现两条「{}」", name)),
            Some(name.clone()),
        );
    }

    // 包内地址/域名冲突
    if !forward.domain.is_empty() {
        if let Some((name, _, _, _)) = accepted
            .iter()
            .find(|(_, _, _, domain)| !domain.is_empty() && *domain == forward.domain)
        {
            return (
                "domain_taken".to_string(),
                Some(format!("同一个包里域名 {} 重复", forward.domain)),
                Some(name.clone()),
            );
        }
    } else if let Some((name, _, _, _)) = accepted.iter().find(|(_, host, port, domain)| {
        domain.is_empty() && *host == forward.local_host && *port == forward.local_port
    }) {
        return (
            "port_taken".to_string(),
            Some(format!(
                "同一个包里本机 {}:{} 重复",
                forward.local_host, forward.local_port
            )),
            Some(name.clone()),
        );
    }

    // 与现有配置比较：规则在模型里，这里只做翻译
    // （`config_conflict` 在内部按 create 的方式模拟域名回环地址分配）
    match tunnels.config_conflict(
        &forward.local_host,
        forward.local_port,
        &forward.domain,
        None,
    ) {
        Ok(None) => ("ok".to_string(), None, None),
        Ok(Some((ConflictKind::Address, name))) => (
            "port_taken".to_string(),
            Some(format!(
                "本机 {}:{} 已被转发「{}」占用",
                forward.local_host, forward.local_port, name
            )),
            Some(name),
        ),
        Ok(Some((ConflictKind::Domain, name))) => (
            "domain_taken".to_string(),
            Some(format!("域名 {} 已被转发「{}」占用", forward.domain, name)),
            Some(name),
        ),
        Err(e) => ("invalid".to_string(), Some(e.to_string()), None),
    }
}

/// 逐条落地。
pub fn import(
    servers: &ServerStore,
    tunnels: &TunnelStore,
    req: ProfileImportRequest,
) -> Result<ProfileImportResult> {
    let mut result = ProfileImportResult {
        created_servers: 0,
        created_forwards: 0,
        skipped: 0,
        failed: Vec::new(),
    };
    let mut server_ids: std::collections::HashMap<String, String> =
        std::collections::HashMap::new();

    for server in &req.servers {
        match server.action.as_str() {
            "skip" => {
                result.skipped += 1;
                // 连接本身不导入，但它的转发可能仍要落地：挂到本地同 id 或同名的
                // 连接上。常见场景——包来自另一台机器，连接已存在，只补转发。
                // 找不到目标连接时这些转发无处可去，才一并计入跳过。
                match find_local_server(servers, server) {
                    Ok(Some(id)) => {
                        server_ids.insert(server.source_id.clone(), id);
                    }
                    Ok(None) => {
                        result.skipped += server.forwards.len();
                        continue;
                    }
                    Err(e) => {
                        result.failed.push(ProfileImportFailure {
                            name: server.name.clone(),
                            reason: e.to_string(),
                        });
                        continue;
                    }
                }
            }
            "overwrite" => {
                let local = match find_local_server(servers, server) {
                    Ok(Some(id)) => id,
                    Ok(None) => {
                        result.failed.push(ProfileImportFailure {
                            name: server.name.clone(),
                            reason: "本机没有可覆盖的连接".to_string(),
                        });
                        continue;
                    }
                    Err(e) => {
                        result.failed.push(ProfileImportFailure {
                            name: server.name.clone(),
                            reason: e.to_string(),
                        });
                        continue;
                    }
                };
                match overwrite_server(servers, &local, server) {
                    Ok(id) => {
                        // 覆盖也计入「写入」：否则界面会显示「0 条连接」，
                        // 用户会以为覆盖没生效（实际字段在下面一行改了）
                        result.created_servers += 1;
                        server_ids.insert(server.source_id.clone(), id);
                    }
                    Err(e) => {
                        result.failed.push(ProfileImportFailure {
                            name: server.name.clone(),
                            reason: e.to_string(),
                        });
                        continue;
                    }
                }
            }
            _ => match create_server(servers, server) {
                Ok(id) => {
                    result.created_servers += 1;
                    server_ids.insert(server.source_id.clone(), id);
                }
                Err(e) => {
                    result.failed.push(ProfileImportFailure {
                        name: server.name.clone(),
                        reason: e.to_string(),
                    });
                    continue;
                }
            },
        }

        let Some(server_id) = server_ids.get(&server.source_id).cloned() else {
            continue;
        };
        for forward in &server.forwards {
            match forward.action.as_str() {
                "skip" => result.skipped += 1,
                "overwrite" => match overwrite_forward(tunnels, &server_id, forward) {
                    Ok(()) => result.created_forwards += 1,
                    Err(e) => result.failed.push(ProfileImportFailure {
                        name: forward.name.clone(),
                        reason: e.to_string(),
                    }),
                },
                _ => match create_forward(tunnels, &server_id, forward) {
                    Ok(()) => result.created_forwards += 1,
                    Err(e) => result.failed.push(ProfileImportFailure {
                        name: forward.name.clone(),
                        reason: e.to_string(),
                    }),
                },
            }
        }
    }

    Ok(result)
}

fn create_server(servers: &ServerStore, server: &ProfileImportServer) -> Result<String> {
    let dto = servers.create(CreateServerRequest {
        name: server.name.clone(),
        host: server.host.clone(),
        port: server.port,
        // 导入的连接没有密码：留空让模型标成「待填」，界面据此提示
        credential_id: String::new(),
        username: server.username.clone(),
        kind: Some(server.credential_kind),
        secret: None,
        passphrase: None,
        host_key_fingerprint: server.host_key_fingerprint.clone(),
        keepalive_secs: server.keepalive_secs,
        enabled: false,
    })?;
    Ok(dto.id)
}

/// 包里的条目不一定是本机的那一条（来自另一台机器时 id 不同），
/// 所以先按 id、再按名称找本地对应项。
fn find_local_server(
    servers: &ServerStore,
    server: &ProfileImportServer,
) -> Result<Option<String>> {
    let items = servers.list()?.items;
    Ok(items
        .iter()
        .find(|item| item.id == server.source_id)
        .or_else(|| items.iter().find(|item| item.name == server.name))
        .map(|item| item.id.clone()))
}

fn overwrite_server(
    servers: &ServerStore,
    local_id: &str,
    server: &ProfileImportServer,
) -> Result<String> {
    // 走模型层而非 `business::patch_server`：后者在「只改用户名」时会重建登录材料，
    // 覆盖导入不该动对方的密码与指纹。
    servers.patch(
        local_id,
        PatchServerRequest {
            name: Some(server.name.clone()),
            host: Some(server.host.clone()),
            port: Some(server.port),
            username: Some(server.username.clone()),
            kind: Some(server.credential_kind),
            keepalive_secs: Some(server.keepalive_secs),
            ..Default::default()
        },
    )?;
    Ok(local_id.to_string())
}

fn create_forward(
    tunnels: &TunnelStore,
    server_id: &str,
    forward: &tl_protocol::ProfileImportForward,
) -> Result<()> {
    tunnels.create(CreateTunnelRequest {
        name: forward.name.clone(),
        server_id: server_id.to_string(),
        local_host: forward.local_host.clone(),
        local_port: forward.local_port,
        remote_host: forward.remote_host.clone(),
        remote_port: forward.remote_port,
        auto_start: forward.auto_start,
        note: forward.note.clone(),
        open_path: forward.open_path.clone(),
        scheme: forward.scheme.clone(),
        domain: forward.domain.clone(),
        remote_scheme: forward.remote_scheme.clone(),
        icon: bounded_icon(&forward.icon),
        group: forward.group.clone(),
        in_app: forward.in_app,
    })?;
    Ok(())
}

fn overwrite_forward(
    tunnels: &TunnelStore,
    server_id: &str,
    forward: &tl_protocol::ProfileImportForward,
) -> Result<()> {
    let Some(target) = forward.target_id.as_deref().filter(|id| !id.is_empty()) else {
        // 没有目标 id 时降级为新建：界面上"覆盖"选项只在同 id 时出现
        return create_forward(tunnels, server_id, forward);
    };
    tunnels.patch(
        target,
        PatchTunnelRequest {
            name: Some(forward.name.clone()),
            server_id: Some(server_id.to_string()),
            local_host: Some(forward.local_host.clone()),
            local_port: Some(forward.local_port),
            remote_host: Some(forward.remote_host.clone()),
            remote_port: Some(forward.remote_port),
            auto_start: Some(forward.auto_start),
            note: Some(forward.note.clone()),
            open_path: Some(forward.open_path.clone()),
            scheme: Some(forward.scheme.clone()),
            domain: Some(forward.domain.clone()),
            remote_scheme: forward.remote_scheme.clone(),
            icon: Some(bounded_icon(&forward.icon)),
            group: Some(forward.group.clone()),
            in_app: Some(forward.in_app),
        },
    )?;
    Ok(())
}

/// 图标超限时丢弃图标而不是整条失败（`normalize_icon` 只接受 200KB 字符）。
fn bounded_icon(icon: &str) -> String {
    if icon.chars().count() > MAX_ICON_CHARS {
        String::new()
    } else {
        icon.to_string()
    }
}

/// 「mypc」→「mypc (2)」，跳过已被占用的后缀。
fn unique_name(base: &str, taken: &HashSet<String>) -> String {
    for n in 2..1000 {
        let candidate = format!("{base} ({n})");
        if !taken.contains(&candidate) {
            return candidate;
        }
    }
    format!("{base} (copy)")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::credential::CredentialStore;
    use std::path::PathBuf;
    use std::sync::Arc;
    use tl_protocol::{
        CreateCredentialRequest, CredentialKind, ProfileImportForward, ProfileImportServer,
        ProfileServer,
    };

    struct Fixture {
        dir: PathBuf,
        servers: Arc<ServerStore>,
        tunnels: TunnelStore,
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    fn fixture(tag: &str) -> Fixture {
        let dir =
            std::env::temp_dir().join(format!("threadline-profile-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create dir");
        let credentials = Arc::new(CredentialStore::new(dir.join("credentials.dat")));
        let servers = Arc::new(ServerStore::new(
            dir.join("servers.json"),
            Arc::clone(&credentials),
        ));
        let tunnels = TunnelStore::new(dir.join("tunnels.json"), Arc::clone(&servers));
        Fixture {
            dir,
            servers,
            tunnels,
        }
    }

    /// 建一条「待填凭据」的连接：导入路径就是这个形态，测试不必造凭据。
    fn pending_server(f: &Fixture, name: &str) -> String {
        f.servers
            .create(CreateServerRequest {
                name: name.to_string(),
                host: "192.0.2.10".into(),
                port: 22,
                credential_id: String::new(),
                username: "root".into(),
                kind: None,
                secret: None,
                passphrase: None,
                host_key_fingerprint: None,
                keepalive_secs: 30,
                enabled: false,
            })
            .expect("server")
            .id
    }

    fn add_forward(f: &Fixture, server_id: &str, name: &str, port: u16, domain: &str) {
        f.tunnels
            .create(CreateTunnelRequest {
                name: name.to_string(),
                server_id: server_id.to_string(),
                local_host: "127.0.0.1".into(),
                local_port: port,
                remote_host: "192.0.2.10".into(),
                remote_port: port,
                auto_start: true,
                note: String::new(),
                open_path: String::new(),
                scheme: "tcp".into(),
                domain: domain.to_string(),
                remote_scheme: None,
                icon: String::new(),
                group: String::new(),
                in_app: true,
            })
            .expect("tunnel");
    }

    fn pack_forward(name: &str, port: u16, domain: &str) -> ProfileForward {
        ProfileForward {
            id: format!("pkg-{name}"),
            name: name.to_string(),
            local_host: "127.0.0.1".into(),
            local_port: port,
            remote_host: "192.0.2.10".into(),
            remote_port: port,
            scheme: "tcp".into(),
            domain: domain.to_string(),
            remote_scheme: None,
            open_path: String::new(),
            note: String::new(),
            group: String::new(),
            in_app: true,
            auto_start: true,
        }
    }

    fn pack_server(name: &str, forwards: Vec<ProfileForward>) -> ProfileServer {
        ProfileServer {
            id: format!("pkg-{name}"),
            name: name.to_string(),
            host: "192.0.2.10".into(),
            port: 22,
            username: "root".into(),
            credential_kind: CredentialKind::Password,
            keepalive_secs: 30,
            host_key_fingerprint: None,
            forwards,
        }
    }

    fn import_forward(name: &str, port: u16, action: &str) -> ProfileImportForward {
        ProfileImportForward {
            name: name.to_string(),
            local_host: "127.0.0.1".into(),
            local_port: port,
            remote_host: "192.0.2.10".into(),
            remote_port: port,
            scheme: "tcp".into(),
            domain: String::new(),
            remote_scheme: None,
            open_path: String::new(),
            note: String::new(),
            group: String::new(),
            in_app: true,
            auto_start: true,
            icon: String::new(),
            action: action.to_string(),
            target_id: None,
        }
    }

    #[test]
    fn plan_reports_name_port_and_domain_conflicts() {
        let f = fixture("plan-conflict");
        let srv = pending_server(&f, "a");
        // 域名条目占用的是回环 IP:端口，不是 127.0.0.1；另建一条无域名的来测端口冲突
        add_forward(&f, &srv, "web", 18080, "a.example.com");
        add_forward(&f, &srv, "plain", 18080, "");

        // 包内：同名连接 a + 三条转发（抢端口 / 抢域名 / 干净）
        let plan = plan(
            &f.servers,
            &f.tunnels,
            ProfilePlanRequest {
                servers: vec![pack_server(
                    "a",
                    vec![
                        pack_forward("web", 18080, ""),
                        pack_forward("dup-port", 18080, ""),
                        pack_forward("mine", 18099, "a.example.com"),
                        pack_forward("fresh", 18099, ""),
                    ],
                )],
            },
        )
        .expect("plan");

        let server = &plan.servers[0];
        assert_eq!(server.action, "name_taken");
        assert_eq!(server.suggested_name.as_deref(), Some("a (2)"));

        // 同名优先于端口冲突：界面先要让用户改名
        assert_eq!(server.forwards[0].action, "name_taken");
        assert_eq!(server.forwards[0].occupied_by.as_deref(), Some("web"));
        // 不同名但抢同一端口（127.0.0.1:18080 归 plain）
        assert_eq!(server.forwards[1].action, "port_taken");
        assert_eq!(server.forwards[1].occupied_by.as_deref(), Some("plain"));
        // 抢现有域名
        assert_eq!(server.forwards[2].action, "domain_taken");
        // 干净的一条
        assert_eq!(server.forwards[3].action, "ok");
    }

    #[test]
    fn plan_catches_conflicts_inside_the_package() {
        let f = fixture("plan-self");
        // 空配置：唯一的冲突源是包自己
        let plan = plan(
            &f.servers,
            &f.tunnels,
            ProfilePlanRequest {
                servers: vec![pack_server(
                    "x",
                    vec![
                        pack_forward("one", 18080, ""),
                        pack_forward("two", 18080, ""),
                    ],
                )],
            },
        )
        .expect("plan");
        let forwards = &plan.servers[0].forwards;
        assert_eq!(forwards[0].action, "ok");
        assert_eq!(forwards[1].action, "port_taken");
        assert_eq!(forwards[1].occupied_by.as_deref(), Some("one"));
    }

    #[test]
    fn import_creates_pending_server_and_forwards() {
        let f = fixture("import-create");
        let result = import(
            &f.servers,
            &f.tunnels,
            ProfileImportRequest {
                servers: vec![ProfileImportServer {
                    source_id: "pkg-x".into(),
                    name: "x".into(),
                    host: "192.0.2.10".into(),
                    port: 22,
                    username: "root".into(),
                    credential_kind: CredentialKind::Password,
                    keepalive_secs: 30,
                    host_key_fingerprint: Some("SHA256:shared".into()),
                    action: "create".into(),
                    forwards: vec![
                        import_forward("web", 18080, "create"),
                        import_forward("rdp", 13389, "create"),
                    ],
                }],
                keep_disabled: true,
            },
        )
        .expect("import");

        assert_eq!(result.created_servers, 1);
        assert_eq!(result.created_forwards, 2);
        assert!(result.failed.is_empty());

        let saved = f.servers.list().expect("list").items;
        assert!(saved[0].needs_credential, "导入的连接必须标成待填");
        assert!(!saved[0].enabled, "导入的连接不能自动启用");
        // 指纹按文件带过来（导入方可以改，但默认信任分享者）
        assert_eq!(
            saved[0].host_key_fingerprint.as_deref(),
            Some("SHA256:shared")
        );
    }

    #[test]
    fn overwrite_keeps_existing_credential_and_fingerprint() {
        let f = fixture("import-overwrite");
        let credentials = Arc::new(CredentialStore::new(f.dir.join("credentials.dat")));
        let cred = credentials
            .add(CreateCredentialRequest {
                name: String::new(),
                kind: CredentialKind::Password,
                username: "root".into(),
                secret: "pw".into(),
                passphrase: None,
            })
            .expect("credential");
        let original = f
            .servers
            .create(CreateServerRequest {
                name: "old".into(),
                host: "192.0.2.10".into(),
                port: 22,
                credential_id: cred.id.clone(),
                username: "root".into(),
                kind: None,
                secret: None,
                passphrase: None,
                host_key_fingerprint: Some("SHA256:local".into()),
                keepalive_secs: 30,
                enabled: true,
            })
            .expect("server");

        import(
            &f.servers,
            &f.tunnels,
            ProfileImportRequest {
                servers: vec![ProfileImportServer {
                    source_id: original.id.clone(),
                    name: "new-name".into(),
                    host: "192.0.2.11".into(),
                    port: 2222,
                    username: "ubuntu".into(),
                    credential_kind: CredentialKind::Password,
                    keepalive_secs: 60,
                    // 分享者的指纹不能盖掉本机已信任的那份
                    host_key_fingerprint: Some("SHA256:other".into()),
                    action: "overwrite".into(),
                    forwards: vec![],
                }],
                keep_disabled: true,
            },
        )
        .expect("import");

        let saved = f.servers.list().expect("list").items;
        assert_eq!(saved[0].name, "new-name");
        assert_eq!(saved[0].host, "192.0.2.11");
        assert_eq!(saved[0].credential_id, cred.id, "覆盖不能清掉对方的密码");
        assert_eq!(
            saved[0].host_key_fingerprint.as_deref(),
            Some("SHA256:local"),
            "覆盖不能改写本机的信任状态"
        );
        assert!(saved[0].enabled, "覆盖不动启用状态");
    }

    #[test]
    fn import_is_idempotent_by_skip_action() {
        let f = fixture("import-skip");
        let req = || ProfileImportRequest {
            servers: vec![ProfileImportServer {
                source_id: "pkg-y".into(),
                name: "y".into(),
                host: "192.0.2.10".into(),
                port: 22,
                username: "root".into(),
                credential_kind: CredentialKind::Password,
                keepalive_secs: 30,
                host_key_fingerprint: None,
                action: "skip".into(),
                forwards: vec![import_forward("a", 18080, "skip")],
            }],
            keep_disabled: true,
        };
        let first = import(&f.servers, &f.tunnels, req()).expect("import");
        assert_eq!(first.created_servers, 0);
        assert_eq!(first.skipped, 2, "连接与其转发都计入跳过");
        assert!(f.servers.list().expect("list").items.is_empty());
    }

    /// 包来自另一台机器时，连接 id 与本机不同、名字相同。
    /// 用户对连接选「跳过」、对转发选「新建」——转发必须挂到本机那条连接上。
    #[test]
    fn skipped_server_still_imports_its_new_forwards() {
        let f = fixture("import-skip-server-create-forwards");
        let local = f
            .servers
            .create(CreateServerRequest {
                name: "y".into(),
                host: "192.0.2.10".into(),
                port: 22,
                credential_id: String::new(),
                username: "root".into(),
                kind: Some(CredentialKind::Password),
                secret: None,
                passphrase: None,
                host_key_fingerprint: None,
                keepalive_secs: 30,
                enabled: false,
            })
            .expect("create local server");

        let result = import(
            &f.servers,
            &f.tunnels,
            ProfileImportRequest {
                servers: vec![ProfileImportServer {
                    source_id: "other-machine-id".into(),
                    name: "y".into(),
                    host: "192.0.2.10".into(),
                    port: 22,
                    username: "root".into(),
                    credential_kind: CredentialKind::Password,
                    keepalive_secs: 30,
                    host_key_fingerprint: None,
                    action: "skip".into(),
                    forwards: vec![import_forward("brand-new", 18090, "create")],
                }],
                keep_disabled: true,
            },
        )
        .expect("import");

        assert_eq!(result.created_servers, 0, "连接本身没被新建");
        assert_eq!(result.created_forwards, 1, "转发不该被连接连累");
        assert_eq!(result.skipped, 1, "只有连接算跳过");

        let forwards = f.tunnels.list().expect("list").items;
        assert_eq!(forwards.len(), 1);
        assert_eq!(
            forwards[0].definition.server_id, local.id,
            "转发要挂到本机那条同名连接上"
        );
    }
}
