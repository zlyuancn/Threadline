//! 域名访问的编排：把「隧道配置里的域名」变成 hosts 托管区块与代理例外清单。
//!
//! 归属：**业务层**。只按顺序调度：读配置 → 渲染 → 尝试写 → 记录状态。
//! 「哪些域名要写」「区块长什么样」「算不算冲突」分别由 `models::tunnel` 与
//! `foundation::hosts` 回答，本模块不藏规则。
//!
//! 允许调用：`models::tunnel`（只读）、`foundation::{hosts, store, clock}`。

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
use tl_protocol::{
    HostsConflict, HostsStatus, ManagedHostEntry, ProxyOverrideStatus, ProxySyncResult,
};

use crate::foundation::{clock, hosts, store};
use crate::models::tunnel::TunnelStore;

/// 代理例外管理集（上次由 Agent 写入的域名）。
const PROXY_FILE: &str = "proxy-overrides.json";

#[derive(Debug, Default)]
struct Inner {
    last_error: Option<String>,
    last_synced_at: Option<String>,
    proxy_managed: Vec<String>,
    proxy_last_error: Option<String>,
    proxy_last_synced_at: Option<String>,
    proxy_enabled: Option<bool>,
    proxy_pac: Option<bool>,
}

pub struct HostsSyncService {
    tunnels: Arc<TunnelStore>,
    data_dir: PathBuf,
    inner: Mutex<Inner>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SyncOutcome {
    pub written: bool,
    pub needs_elevation: bool,
    pub conflicts: usize,
}

impl HostsSyncService {
    pub fn new(tunnels: Arc<TunnelStore>, data_dir: impl Into<PathBuf>) -> Arc<Self> {
        let data_dir = data_dir.into();
        let proxy_file = data_dir.join(PROXY_FILE);
        let managed = store::read_json::<Vec<String>>(&proxy_file)
            .ok()
            .flatten()
            .unwrap_or_default();
        Arc::new(Self {
            tunnels,
            data_dir,
            inner: Mutex::new(Inner {
                proxy_managed: managed,
                ..Inner::default()
            }),
        })
    }

    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }

    /// 当前配置里所有「有域名」的条目（按域名排序，保证渲染稳定）。
    pub fn entries(&self) -> Result<Vec<ManagedHostEntry>> {
        let mut items: Vec<ManagedHostEntry> = self
            .tunnels
            .list()?
            .items
            .into_iter()
            .filter(|view| !view.definition.domain.is_empty())
            .map(|view| ManagedHostEntry {
                domain: view.definition.domain,
                address: view.definition.domain_host,
                tunnel_id: view.definition.id,
            })
            .filter(|entry| !entry.address.is_empty())
            .collect();
        items.sort_by(|a, b| a.domain.cmp(&b.domain));
        Ok(items)
    }

    /// 期望进入代理例外的域名（与 hosts 条目同源）。
    pub fn desired_proxy_domains(&self) -> Result<Vec<String>> {
        let mut domains: Vec<String> = self
            .entries()?
            .into_iter()
            .map(|entry| entry.domain)
            .collect();
        domains.sort();
        domains.dedup();
        Ok(domains)
    }

    /// 读 hosts、算冲突、判断是否需要写。**只读**，不触发提权。
    pub fn status(&self) -> Result<HostsStatus> {
        let path = hosts::hosts_path();
        let existing = hosts::read_raw(&path).unwrap_or_default();
        let entries = self.entries()?;
        let domains: Vec<String> = entries.iter().map(|e| e.domain.clone()).collect();
        let conflicts: Vec<HostsConflict> = hosts::conflicts(&existing, &domains)
            .into_iter()
            .map(|conflict| HostsConflict {
                domain: conflict.domain,
                existing: conflict.existing,
            })
            .collect();

        let block_entries: Vec<hosts::Entry> = entries
            .iter()
            .filter(|entry| !conflicts.iter().any(|c| c.domain == entry.domain))
            .map(|entry| hosts::Entry {
                address: entry.address.clone(),
                domain: entry.domain.clone(),
            })
            .collect();
        let desired = hosts::render(&existing, &block_entries);
        let pending = desired != existing;
        let elevated = hosts::is_elevated();
        let inner = self.lock();

        Ok(HostsStatus {
            path: path.display().to_string(),
            managed: entries,
            conflicts,
            needs_elevation: pending && !elevated,
            pending,
            elevated,
            last_error: inner.last_error.clone(),
            last_synced_at: inner.last_synced_at.clone(),
            proxy: self.proxy_status_locked(&inner),
        })
    }

    /// 写一次托管区块。**不自动提权**：写不进去就返回 `needs_elevation`，由界面拉 UAC。
    pub fn sync(&self) -> Result<SyncOutcome> {
        let path = hosts::hosts_path();
        let existing = hosts::read_raw(&path).unwrap_or_default();
        let entries = self.entries()?;
        let domains: Vec<String> = entries.iter().map(|e| e.domain.clone()).collect();
        let conflicts = hosts::conflicts(&existing, &domains);

        let block_entries: Vec<hosts::Entry> = entries
            .iter()
            .filter(|entry| !conflicts.iter().any(|c| c.domain == entry.domain))
            .map(|entry| hosts::Entry {
                address: entry.address.clone(),
                domain: entry.domain.clone(),
            })
            .collect();
        let desired = hosts::render(&existing, &block_entries);

        if desired == existing {
            let mut inner = self.lock();
            inner.last_error = None;
            inner.last_synced_at = Some(clock::now_iso8601());
            return Ok(SyncOutcome {
                written: true,
                needs_elevation: false,
                conflicts: conflicts.len(),
            });
        }

        match hosts::write_raw(&path, &desired) {
            Ok(()) => {
                let _ = hosts::flush_dns();
                let mut inner = self.lock();
                inner.last_error = None;
                inner.last_synced_at = Some(clock::now_iso8601());
                Ok(SyncOutcome {
                    written: true,
                    needs_elevation: false,
                    conflicts: conflicts.len(),
                })
            }
            Err(err) => {
                let needs_elevation = is_access_denied(&err);
                let mut inner = self.lock();
                inner.last_error = Some(err.to_string());
                Ok(SyncOutcome {
                    written: false,
                    needs_elevation,
                    conflicts: conflicts.len(),
                })
            }
        }
    }

    /// 启动自检：能写就静默修，不能写只标记（不弹 UAC）。
    pub fn startup_check(&self) {
        let _ = self.sync();
    }

    /// 清空托管区块（连标记一起删），不动块外任何行。
    pub fn clear(&self) -> Result<SyncOutcome> {
        let path = hosts::hosts_path();
        let existing = hosts::read_raw(&path).unwrap_or_default();
        let desired = hosts::render(&existing, &[]);
        if desired == existing {
            return Ok(SyncOutcome {
                written: true,
                needs_elevation: false,
                conflicts: 0,
            });
        }
        match hosts::write_raw(&path, &desired) {
            Ok(()) => {
                let _ = hosts::flush_dns();
                let mut inner = self.lock();
                inner.last_error = None;
                inner.last_synced_at = Some(clock::now_iso8601());
                Ok(SyncOutcome {
                    written: true,
                    needs_elevation: false,
                    conflicts: 0,
                })
            }
            Err(err) => {
                let needs_elevation = is_access_denied(&err);
                let mut inner = self.lock();
                inner.last_error = Some(err.to_string());
                Ok(SyncOutcome {
                    written: false,
                    needs_elevation,
                    conflicts: 0,
                })
            }
        }
    }

    /// 代理例外的同步状态。`desired` 从配置算，`managed` 来自上次 Agent 回报。
    pub fn proxy_status(&self) -> Result<ProxyOverrideStatus> {
        let inner = self.lock();
        Ok(self.proxy_status_locked(&inner))
    }

    fn proxy_status_locked(&self, inner: &Inner) -> ProxyOverrideStatus {
        let desired = self.desired_proxy_domains().unwrap_or_default();
        ProxyOverrideStatus {
            pending: desired != inner.proxy_managed,
            desired,
            managed: inner.proxy_managed.clone(),
            enabled: inner.proxy_enabled,
            pac: inner.proxy_pac,
            last_error: inner.proxy_last_error.clone(),
            last_synced_at: inner.proxy_last_synced_at.clone(),
        }
    }

    /// Agent 回报一次代理例外同步结果：用 `applied` 覆盖管理集。
    pub fn apply_proxy_result(&self, result: ProxySyncResult) -> Result<()> {
        let mut inner = self.lock();
        // 探测结果与同步成败无关：失败的那次也带了真实的系统代理状态。
        if result.enabled.is_some() {
            inner.proxy_enabled = result.enabled;
        }
        if result.pac.is_some() {
            inner.proxy_pac = result.pac;
        }
        if result.ok {
            inner.proxy_managed = result.applied;
            inner.proxy_last_error = None;
            inner.proxy_last_synced_at = Some(result.at);
            let _ = store::write_json_atomic(&self.data_dir.join(PROXY_FILE), &inner.proxy_managed);
        } else {
            inner.proxy_last_error = result.message;
            inner.proxy_last_synced_at = Some(result.at);
        }
        Ok(())
    }

    /// Agent 上报的系统代理状态（Core 是 SYSTEM 时读不到登录用户的设置）。
    pub fn set_proxy_environment(&self, enabled: Option<bool>, pac: Option<bool>) {
        let mut inner = self.lock();
        if enabled.is_some() {
            inner.proxy_enabled = enabled;
        }
        if pac.is_some() {
            inner.proxy_pac = pac;
        }
    }

    /// 清空代理例外的管理集（只清记录，注册表由 Agent 按 `remove` 处理）。
    pub fn clear_proxy_managed(&self) -> Result<()> {
        let mut inner = self.lock();
        inner.proxy_managed.clear();
        inner.proxy_last_error = None;
        inner.proxy_last_synced_at = Some(clock::now_iso8601());
        store::write_json_atomic(&self.data_dir.join(PROXY_FILE), &inner.proxy_managed)
            .context("清空代理例外管理集失败")
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

/// `anyhow` 链里是否有 `ERROR_ACCESS_DENIED`（5）。
fn is_access_denied(err: &anyhow::Error) -> bool {
    err.chain().any(|cause| {
        cause
            .downcast_ref::<std::io::Error>()
            .and_then(std::io::Error::raw_os_error)
            == Some(5)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn access_denied_is_detected_through_context() {
        let io = std::io::Error::from_raw_os_error(5);
        let err = anyhow::Error::new(io).context("原子替换失败");
        assert!(is_access_denied(&err));
    }

    #[test]
    fn other_errors_are_not_access_denied() {
        let io = std::io::Error::from_raw_os_error(32);
        let err = anyhow::Error::new(io);
        assert!(!is_access_denied(&err));
    }
}
