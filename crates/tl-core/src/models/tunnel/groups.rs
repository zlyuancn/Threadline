//! 应用页分组清单。
//!
//! 归属：`tunnel` 模型内部文件组。拥有 `app_groups.json`（组名 + 顺序 + **空组**）。
//! 提供：`app_groups` / `set_app_groups` / `rename_app_group` / `delete_app_group` / `reorder_app`。
//! 允许调用：`foundation::store`、`tl_protocol`。
//!
//! 纪律：
//! - 成员的**归属**仍由 `TunnelDto.group`（组名）决定，是唯一真相；这里只管组的顺序与存在性。
//! - `未分组` 是隐含组：不落盘、永远排最后、不可改名/删除。
//! - 清单之外、但转发上出现的组名在读取时**自动收录**——手改配置文件不会让组消失。
//! - 归零口径：`app_order` 只被应用页读，转发逻辑一律不看（改布局不动任何一条隧道）。

use std::collections::HashSet;
use std::path::PathBuf;

use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use tl_protocol::{AppGroup, AppGroupList};

use super::TunnelStore;
use crate::foundation::store;

/// 分组清单文件的版本（与 `tunnels.json` 各自独立）。
const GROUPS_SCHEMA_VERSION: u32 = 1;

/// 隐含的分组名：转发 `group` 为空时归到这里，不可作为自定义组名。
pub const UNGROUPED: &str = "未分组";

#[derive(Debug, Clone, Serialize, Deserialize)]
struct AppGroupsFile {
    schema_version: u32,
    items: Vec<AppGroup>,
}

impl Default for AppGroupsFile {
    fn default() -> Self {
        Self {
            schema_version: GROUPS_SCHEMA_VERSION,
            items: Vec::new(),
        }
    }
}

/// 按数组顺序重排 `order`：数组顺序就是唯一真相，`order` 只是它的投影。
fn renumber(items: &mut [AppGroup]) {
    for (index, group) in items.iter_mut().enumerate() {
        group.order = index as i32;
    }
}

/// 组名校验（新建 / 改名共用）。
fn normalize_group_name(raw: &str) -> Result<String> {
    let name = raw.trim();
    if name.is_empty() {
        return Err(anyhow!("分组名不能为空"));
    }
    if name == UNGROUPED {
        return Err(anyhow!("「{UNGROUPED}」是保留分组，不能作为自定义组名"));
    }
    if name.chars().count() > 32 {
        return Err(anyhow!("分组名最多 32 个字符"));
    }
    Ok(name.to_string())
}

impl TunnelStore {
    fn groups_path(&self) -> PathBuf {
        self.path().with_file_name("app_groups.json")
    }

    fn load_groups(&self) -> Result<AppGroupsFile> {
        Ok(store::read_json::<AppGroupsFile>(&self.groups_path())?.unwrap_or_default())
    }

    fn save_groups(&self, file: &AppGroupsFile) -> Result<()> {
        store::write_json_atomic(&self.groups_path(), file)
    }

    /// 把转发上出现、但清单里没有的组名补到末尾。返回是否发生了变更。
    fn absorb_tunnel_groups(&self, file: &mut AppGroupsFile) -> Result<bool> {
        let tunnels = self.load()?;
        let mut changed = false;
        for def in &tunnels.items {
            let name = def.group.trim();
            if name.is_empty() || file.items.iter().any(|g| g.name == name) {
                continue;
            }
            file.items.push(AppGroup {
                name: name.to_string(),
                order: file.items.len() as i32,
            });
            changed = true;
        }
        Ok(changed)
    }

    /// 分组清单（首个可用版本也是迁移：文件不存在时由现有转发派生）。
    pub fn app_groups(&self) -> Result<AppGroupList> {
        let mut file = self.load_groups()?;
        let absorbed = self.absorb_tunnel_groups(&mut file)?;
        let had_orders = file
            .items
            .iter()
            .enumerate()
            .any(|(i, g)| g.order != i as i32);
        if absorbed || file.items.is_empty() || had_orders {
            // 首次读取即落盘：之后即便转发被清空，组（含空组）也还在。
            renumber(&mut file.items);
            let _ = self.save_groups(&file);
        }
        Ok(AppGroupList { items: file.items })
    }

    /// 全量覆盖清单：新建、删除**空**组、调整顺序。
    ///
    /// 删除仍有成员的组会被拒绝——那会让成员处于"组名不在清单里"的游离态，
    /// 必须走 `delete_app_group`，由它把成员回落到「未分组」。
    pub fn set_app_groups(&self, items: Vec<AppGroup>) -> Result<AppGroupList> {
        let mut file = self.load_groups()?;
        self.absorb_tunnel_groups(&mut file)?;
        let tunnels = self.load()?;

        let mut next: Vec<String> = Vec::new();
        let mut seen: HashSet<String> = HashSet::new();
        for item in items {
            let name = normalize_group_name(&item.name)?;
            if !seen.insert(name.clone()) {
                return Err(anyhow!("分组名重复：{name}"));
            }
            next.push(name);
        }

        for existing in &file.items {
            if next.contains(&existing.name) {
                continue;
            }
            if tunnels.items.iter().any(|t| t.group == existing.name) {
                return Err(anyhow!(
                    "分组「{}」已被成员引用，请用删除分组（成员会回到「{UNGROUPED}」）",
                    existing.name
                ));
            }
        }

        let mut items: Vec<AppGroup> = next
            .into_iter()
            .map(|name| AppGroup { name, order: 0 })
            .collect();
        renumber(&mut items);
        self.save_groups(&AppGroupsFile {
            schema_version: GROUPS_SCHEMA_VERSION,
            items: items.clone(),
        })?;
        self.notify_changed();
        Ok(AppGroupList { items })
    }

    /// 分组改名：清单与成员一起改。
    ///
    /// 两个文件各自原子写，顺序是「先成员、后清单」：万一中间崩溃，结果是
    /// 成员已在新名下、清单还是旧名 —— 下次读取会把新名自动收录，不会丢数据。
    pub fn rename_app_group(&self, from: &str, to: &str) -> Result<AppGroupList> {
        let from = from.trim();
        if from.is_empty() {
            return Err(anyhow!("要改名的分组不能为空"));
        }
        if from == UNGROUPED {
            return Err(anyhow!("「{UNGROUPED}」是保留分组，不能改名"));
        }
        let to = normalize_group_name(to)?;

        let mut groups = self.load_groups()?;
        self.absorb_tunnel_groups(&mut groups)?;
        if !groups.items.iter().any(|g| g.name == from) {
            return Err(anyhow!("分组不存在：{from}"));
        }
        if from != to && groups.items.iter().any(|g| g.name == to) {
            return Err(anyhow!("分组名已存在：{to}"));
        }

        let mut tunnels = self.load()?;
        let mut touched = false;
        for def in tunnels.items.iter_mut() {
            if def.group == from {
                def.group = to.clone();
                touched = true;
            }
        }
        if touched {
            self.save(&tunnels)?;
        }

        for group in groups.items.iter_mut() {
            if group.name == from {
                group.name = to.clone();
            }
        }
        renumber(&mut groups.items);
        self.save_groups(&groups)?;
        self.notify_changed();
        Ok(AppGroupList {
            items: groups.items,
        })
    }

    /// 删除分组：清单移除，成员回落到「未分组」。
    pub fn delete_app_group(&self, name: &str) -> Result<AppGroupList> {
        let name = name.trim();
        if name.is_empty() {
            return Err(anyhow!("要删除的分组不能为空"));
        }
        if name == UNGROUPED {
            return Err(anyhow!("「{UNGROUPED}」不能删除"));
        }

        let mut groups = self.load_groups()?;
        self.absorb_tunnel_groups(&mut groups)?;
        if !groups.items.iter().any(|g| g.name == name) {
            return Err(anyhow!("分组不存在：{name}"));
        }

        let mut tunnels = self.load()?;
        let mut touched = false;
        for def in tunnels.items.iter_mut() {
            if def.group == name {
                def.group.clear();
                touched = true;
            }
        }
        if touched {
            self.save(&tunnels)?;
        }

        groups.items.retain(|g| g.name != name);
        renumber(&mut groups.items);
        self.save_groups(&groups)?;
        self.notify_changed();
        Ok(AppGroupList {
            items: groups.items,
        })
    }

    /// 应用页磁贴顺序：按 `ids` 赋 `app_order = 0..n`，缺席的追加到末尾。
    ///
    /// 与 `reorder_servers` 同规则：界面上能看到全部条目，但并发/漏传不该丢条目。
    pub fn reorder_app(&self, ids: Vec<String>) -> Result<()> {
        let mut file = self.load()?;
        let known: HashSet<&str> = file.items.iter().map(|t| t.id.as_str()).collect();

        let mut order: Vec<String> = Vec::new();
        let mut seen: HashSet<String> = HashSet::new();
        for id in ids {
            if known.contains(id.as_str()) && seen.insert(id.clone()) {
                order.push(id);
            }
        }
        for def in &file.items {
            if seen.insert(def.id.clone()) {
                order.push(def.id.clone());
            }
        }

        for (index, id) in order.iter().enumerate() {
            if let Some(def) = file.items.iter_mut().find(|t| &t.id == id) {
                def.app_order = index as i32;
            }
        }
        self.save(&file)?;
        self.notify_changed();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn group_names_reject_empty_reserved_and_too_long() {
        assert!(normalize_group_name("  ").is_err());
        assert!(normalize_group_name(UNGROUPED).is_err());
        assert!(normalize_group_name(&"组".repeat(33)).is_err());
        assert_eq!(normalize_group_name(" 工具 ").expect("ok"), "工具");
    }

    #[test]
    fn renumber_follows_array_order() {
        let mut items = vec![
            AppGroup {
                name: "b".into(),
                order: 9,
            },
            AppGroup {
                name: "a".into(),
                order: 3,
            },
        ];
        renumber(&mut items);
        assert_eq!(items[0].order, 0);
        assert_eq!(items[1].order, 1);
    }

    mod store_side {
        use super::*;
        use std::sync::Arc;

        use tl_protocol::{TunnelDto, TunnelsFile, SCHEMA_VERSION};

        use crate::models::credential::CredentialStore;
        use crate::models::server::ServerStore;

        struct Fixture {
            dir: PathBuf,
            tunnels: TunnelStore,
        }

        impl Drop for Fixture {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.dir);
            }
        }

        fn fixture(tag: &str) -> Fixture {
            let dir = std::env::temp_dir()
                .join(format!("threadline-groups-{tag}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).expect("dir");
            let credentials = Arc::new(CredentialStore::new(dir.join("credentials.dat")));
            let servers = Arc::new(ServerStore::new(
                dir.join("servers.json"),
                Arc::clone(&credentials),
            ));
            let tunnels = TunnelStore::new(dir.join("tunnels.json"), servers);
            Fixture { dir, tunnels }
        }

        fn dto(id: &str, group: &str, app_order: i32) -> TunnelDto {
            TunnelDto {
                id: id.into(),
                name: id.into(),
                server_id: "srv".into(),
                local_host: "127.0.0.1".into(),
                local_port: 18080,
                remote_host: "127.0.0.1".into(),
                remote_port: 80,
                auto_start: false,
                note: String::new(),
                open_path: String::new(),
                scheme: "tcp".into(),
                domain: String::new(),
                domain_host: String::new(),
                remote_scheme: None,
                icon: String::new(),
                group: group.into(),
                in_app: true,
                app_order,
            }
        }

        fn write_tunnels(f: &Fixture, items: Vec<TunnelDto>) {
            store::write_json_atomic(
                f.tunnels.path(),
                &TunnelsFile {
                    schema_version: SCHEMA_VERSION,
                    items,
                },
            )
            .expect("write tunnels");
        }

        fn names(list: &AppGroupList) -> Vec<&str> {
            list.items.iter().map(|g| g.name.as_str()).collect()
        }

        #[test]
        fn first_read_derives_groups_from_tunnels_in_first_seen_order() {
            let f = fixture("derive");
            write_tunnels(
                &f,
                vec![
                    dto("t1", "工具", 0),
                    dto("t2", "", 1),
                    dto("t3", "ai工具", 2),
                    dto("t4", "工具", 3),
                ],
            );

            let list = f.tunnels.app_groups().expect("groups");
            assert_eq!(names(&list), vec!["工具", "ai工具"]);
            // 首次读取即落盘：之后转发清空，组也还在。
            assert!(f.dir.join("app_groups.json").exists());
        }

        #[test]
        fn empty_groups_survive_and_order_is_normalized() {
            let f = fixture("empty");
            write_tunnels(&f, vec![dto("t1", "工具", 0)]);

            let saved = f
                .tunnels
                .set_app_groups(vec![
                    AppGroup {
                        name: "新组".into(),
                        order: 7,
                    },
                    AppGroup {
                        name: "工具".into(),
                        order: 3,
                    },
                ])
                .expect("set");
            assert_eq!(names(&saved), vec!["新组", "工具"]);
            assert_eq!(saved.items[0].order, 0);
            assert_eq!(saved.items[1].order, 1);

            // 重新读：空组仍在，顺序不变
            let again = f.tunnels.app_groups().expect("groups");
            assert_eq!(names(&again), vec!["新组", "工具"]);
        }

        #[test]
        fn dropping_a_group_with_members_is_rejected() {
            let f = fixture("dropbusy");
            write_tunnels(&f, vec![dto("t1", "工具", 0)]);
            f.tunnels.app_groups().expect("derive");

            let err = f
                .tunnels
                .set_app_groups(vec![AppGroup {
                    name: "其它".into(),
                    order: 0,
                }])
                .expect_err("必须拒绝");
            assert!(err.to_string().contains("已被成员引用"), "{err}");
        }

        #[test]
        fn rename_moves_members_in_one_go() {
            let f = fixture("rename");
            write_tunnels(&f, vec![dto("t1", "工具", 0), dto("t2", "工具", 1)]);
            f.tunnels.app_groups().expect("derive");

            let list = f.tunnels.rename_app_group("工具", "日常").expect("rename");
            assert_eq!(names(&list), vec!["日常"]);

            let items = f.tunnels.list().expect("list").items;
            assert!(items.iter().all(|v| v.definition.group == "日常"));
        }

        #[test]
        fn rename_rejects_conflicts_and_reserved_names() {
            let f = fixture("renamebad");
            write_tunnels(&f, vec![dto("t1", "工具", 0), dto("t2", "网络", 1)]);
            f.tunnels.app_groups().expect("derive");

            assert!(f.tunnels.rename_app_group("工具", "网络").is_err());
            assert!(f.tunnels.rename_app_group("工具", UNGROUPED).is_err());
            assert!(f.tunnels.rename_app_group("不存在的组", "x").is_err());
        }

        #[test]
        fn delete_falls_members_back_to_ungrouped() {
            let f = fixture("delete");
            write_tunnels(&f, vec![dto("t1", "工具", 0), dto("t2", "网络", 1)]);
            f.tunnels.app_groups().expect("derive");

            let list = f.tunnels.delete_app_group("工具").expect("delete");
            assert_eq!(names(&list), vec!["网络"]);

            let items = f.tunnels.list().expect("list").items;
            let moved = items.iter().find(|v| v.definition.id == "t1").expect("t1");
            assert_eq!(moved.definition.group, "");
            let kept = items.iter().find(|v| v.definition.id == "t2").expect("t2");
            assert_eq!(kept.definition.group, "网络");
        }

        #[test]
        fn reorder_assigns_sequence_and_appends_missing() {
            let f = fixture("order");
            write_tunnels(
                &f,
                vec![dto("t1", "", 0), dto("t2", "", 1), dto("t3", "", 2)],
            );

            f.tunnels
                .reorder_app(vec!["t3".into(), "t1".into()])
                .expect("reorder");
            let items = f.tunnels.list().expect("list").items;
            // 只改 `app_order`，配置数组的物理顺序不动（界面按 app_order 排）。
            let order = |id: &str| {
                items
                    .iter()
                    .find(|v| v.definition.id == id)
                    .map(|v| v.definition.app_order)
                    .expect("id")
            };
            assert_eq!(order("t3"), 0);
            assert_eq!(order("t1"), 1);
            // 未传的按原顺序追加到末尾
            assert_eq!(order("t2"), 2);
        }

        #[test]
        fn reorder_ignores_unknown_ids_and_duplicates() {
            let f = fixture("orderbad");
            write_tunnels(&f, vec![dto("t1", "", 0), dto("t2", "", 0)]);

            f.tunnels
                .reorder_app(vec!["nope".into(), "t2".into(), "t2".into()])
                .expect("reorder");
            let items = f.tunnels.list().expect("list").items;
            let order = |id: &str| {
                items
                    .iter()
                    .find(|v| v.definition.id == id)
                    .map(|v| v.definition.app_order)
                    .expect("id")
            };
            assert_eq!(order("t2"), 0);
            assert_eq!(order("t1"), 1);
        }

        #[test]
        fn hand_added_group_in_tunnels_is_absorbed() {
            let f = fixture("absorb");
            write_tunnels(&f, vec![dto("t1", "工具", 0)]);
            f.tunnels.app_groups().expect("derive");

            // 手改配置：多出一个清单里没有的组
            write_tunnels(&f, vec![dto("t1", "工具", 0), dto("t2", "手工加的", 1)]);
            let list = f.tunnels.app_groups().expect("groups");
            assert_eq!(names(&list), vec!["工具", "手工加的"]);
        }
    }
}
