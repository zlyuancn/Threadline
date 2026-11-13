//! 运行态模型。
//!
//! **拥有**：`control.json`（发现契约）、`install.json`（只读）、**开机自启开关**的注册层视图，
//! 以及当前进程的降级集合。判定规则：能否改自启（身份是否有权）、当前是否降级。
//! **提供**：`whoami` / `autostart_status` / `set_autostart` / `write_control` / `mark_stopped` /
//! `degradations` / `raise` / `clear`。
//! **允许调用**：`foundation::{store, registry, token}`、`tl_protocol`。
//!
//! 纪律（CAW I5 / AP10）：自启开关只改**注册层**，绝不停正在跑的核心；
//! 任务计划是 per-machine 注册物，只有 SYSTEM 身份的核心（或一次性提权进程）能写。

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use tl_protocol::{
    AutostartStatus, ControlFile, ControlInfo, ControlStopped, Degradation, InstallInfo, RunMode,
    WhoamiResponse,
};

use crate::foundation::{registry, store};

/// 改自启开关的失败原因。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AutostartError {
    /// 当前身份无权改 per-machine 注册物，需要一次提权（CAW R6：一次授权，不循环弹）。
    PrivilegeRequired,
    /// 注册层操作失败（含字段级错误）。
    Registry(registry::RegistryError),
}

impl std::fmt::Display for AutostartError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AutostartError::PrivilegeRequired => write!(f, "需要以管理员权限修改开机自启"),
            AutostartError::Registry(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for AutostartError {}

impl From<registry::RegistryError> for AutostartError {
    fn from(e: registry::RegistryError) -> Self {
        AutostartError::Registry(e)
    }
}

/// 当前进程的静态身份信息（由壳层在启动时确定，进程生命周期内不变）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessIdentity {
    pub instance_id: String,
    pub version: String,
    pub run_mode: RunMode,
    /// `SYSTEM` 或 `user:<account>`。
    pub identity: String,
}

impl ProcessIdentity {
    pub fn is_system(&self) -> bool {
        self.identity.eq_ignore_ascii_case("SYSTEM")
    }
}

/// 运行态仓库：数据目录下的 `control.json` 与注册层视图。
#[derive(Debug)]
pub struct RuntimeStore {
    dir: PathBuf,
    process: ProcessIdentity,
    degradations: Mutex<Vec<Degradation>>,
}

impl RuntimeStore {
    pub fn new(dir: impl Into<PathBuf>, process: ProcessIdentity) -> Self {
        Self {
            dir: dir.into(),
            process,
            degradations: Mutex::new(Vec::new()),
        }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn process(&self) -> &ProcessIdentity {
        &self.process
    }

    fn control_path(&self) -> PathBuf {
        self.dir.join("control.json")
    }

    fn install_path(&self) -> PathBuf {
        self.dir.join("install.json")
    }

    /// 读安装元数据（取不到 → `None`，例如绿色免安装部署）。
    pub fn install_info(&self) -> Option<InstallInfo> {
        store::read_json::<InstallInfo>(&self.install_path())
            .ok()
            .flatten()
    }

    /// 写发现契约（Core 启动后立即调用）。
    pub fn write_control(&self, port: u16, token: &str, started_at: &str) -> anyhow::Result<()> {
        store::write_json_atomic(
            &self.control_path(),
            &ControlFile::Running(ControlInfo {
                version: self.process.version.clone(),
                instance_id: self.process.instance_id.clone(),
                pid: std::process::id(),
                identity: self.process.identity.clone(),
                run_mode: self.process.run_mode,
                host: "127.0.0.1".to_string(),
                port,
                token: token.to_string(),
                started_at: started_at.to_string(),
            }),
        )
    }

    /// 优雅退出时把发现契约改成「已停止」，避免 Wake 对着死端口重试。
    pub fn mark_stopped(&self) -> anyhow::Result<()> {
        store::write_json_atomic(
            &self.control_path(),
            &ControlFile::Stopped(ControlStopped {
                version: self.process.version.clone(),
            }),
        )
    }

    pub fn read_control(&self) -> Option<ControlInfo> {
        store::read_json::<ControlFile>(&self.control_path())
            .ok()
            .flatten()
            .and_then(|f| f.running().cloned())
    }

    /// 自启开关的当前状态（只读注册层，不改任何东西）。
    ///
    /// `privilege_required` 表示**当前身份**改这个开关是否要提权：只有 SYSTEM 能直接改 per-machine 任务。
    pub fn autostart_status(&self) -> Result<AutostartStatus, registry::RegistryError> {
        let task = registry::query(registry::TASK_NAME)?;
        Ok(AutostartStatus {
            enabled: task.exists && task.enabled,
            task_exists: task.exists,
            task_enabled: task.enabled,
            privilege_required: !self.process.is_system(),
        })
    }

    /// 改自启开关：**只动注册层，不碰正在跑的隧道**（CAW I5）。
    ///
    /// 关闭走 `delete`（幂等）；创建走 `create`（内部先做字段级前置校验）。
    pub fn set_autostart(
        &self,
        enabled: bool,
        exe_path: &Path,
        log_dir: &Path,
    ) -> Result<AutostartStatus, AutostartError> {
        if !self.process.is_system() {
            return Err(AutostartError::PrivilegeRequired);
        }
        if enabled {
            registry::create(exe_path, log_dir)?;
        } else {
            registry::delete(registry::TASK_NAME)?;
        }
        Ok(self.autostart_status()?)
    }

    /// 组合出 `/whoami` 响应。
    pub fn whoami(&self) -> Result<WhoamiResponse, registry::RegistryError> {
        Ok(WhoamiResponse {
            instance_id: self.process.instance_id.clone(),
            version: self.process.version.clone(),
            run_mode: self.process.run_mode,
            identity: self.process.identity.clone(),
            degradations: self.degradations(),
            autostart: self.autostart_status()?,
        })
    }

    /// 当前降级集合（去重、稳定顺序）。
    pub fn degradations(&self) -> Vec<Degradation> {
        let mut list = self.degradations.lock().expect("degradations lock").clone();
        list.sort_by_key(|d| degradation_rank(*d));
        list.dedup();
        list
    }

    /// 标记一个降级（CAW I9：任何降级都必须可见）。
    pub fn raise(&self, degradation: Degradation) {
        let mut list = self.degradations.lock().expect("degradations lock");
        if !list.contains(&degradation) {
            list.push(degradation);
        }
    }

    /// 解除一个降级。
    pub fn clear(&self, degradation: Degradation) {
        self.degradations
            .lock()
            .expect("degradations lock")
            .retain(|d| *d != degradation);
    }

    /// 依据运行档位补齐「必然成立」的降级标记（启动时调用一次）。
    pub fn sync_run_mode_degradations(&self) {
        match self.process.run_mode {
            RunMode::StandIn => self.raise(Degradation::StandInCore),
            RunMode::Fallback => self.raise(Degradation::FallbackRun),
            RunMode::Boot | RunMode::Manual => {}
        }
    }
}

fn degradation_rank(d: Degradation) -> u8 {
    match d {
        Degradation::StandInCore => 0,
        Degradation::FallbackRun => 1,
        Degradation::CredentialAclRelaxed => 2,
        Degradation::CredentialUnreadable => 3,
        Degradation::LocalPortLost => 4,
    }
}

/// 生成进程身份（壳层启动时调用一次）。
pub fn new_instance_id() -> String {
    crate::foundation::token::random_token(4).unwrap_or_else(|_| "00000000".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("threadline-runtime-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create dir");
        dir
    }

    fn store_in(dir: &Path, run_mode: RunMode, identity: &str) -> RuntimeStore {
        RuntimeStore::new(
            dir,
            ProcessIdentity {
                instance_id: "inst1234".into(),
                version: "0.1.0".into(),
                run_mode,
                identity: identity.into(),
            },
        )
    }

    #[test]
    fn control_file_roundtrips_and_marks_stopped() {
        let dir = temp_dir("control");
        let store = store_in(&dir, RunMode::Boot, "SYSTEM");

        store
            .write_control(18921, "tok", "2026-10-19T09:00:00+08:00")
            .expect("write");
        let read = store.read_control().expect("read");
        assert_eq!(read.port, 18921);
        assert_eq!(read.instance_id, "inst1234");
        assert_eq!(read.run_mode, RunMode::Boot);

        store.mark_stopped().expect("stopped");
        assert!(store.read_control().is_none(), "已停止后不应再报告运行信息");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_install_info_is_none_not_error() {
        let dir = temp_dir("install");
        let store = store_in(&dir, RunMode::Manual, "user:alice");
        assert!(store.install_info().is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn user_identity_cannot_toggle_per_machine_task() {
        let dir = temp_dir("privilege");
        let store = store_in(&dir, RunMode::Manual, "user:alice");
        let err = store
            .set_autostart(
                true,
                Path::new("C:\\Program Files\\Threadline\\tl-core.exe"),
                Path::new("C:\\logs"),
            )
            .expect_err("用户身份必须被拒");
        assert_eq!(err, AutostartError::PrivilegeRequired);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn standalone_autostart_status_is_readable() {
        // 只读查询不能因为任务存在与否而报错。
        //
        // **不要断言 enabled 的具体值**：它读的是这台机器上的真实任务计划，
        // 装了自启的机器上必然是 true。之前这条断言让测试随环境变红
        // （开发机装了自启任务 → 全量测试挂掉），而代码其实没问题。
        let dir = temp_dir("status");
        let store = store_in(&dir, RunMode::Manual, "user:alice");
        let status = store.autostart_status().expect("query must not fail");
        assert!(
            status.privilege_required,
            "用户身份改开关需要提权（与任务是否存在无关）"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn degradations_are_deduped_and_mode_synced() {
        let dir = temp_dir("degradation");
        let store = store_in(&dir, RunMode::StandIn, "user:alice");
        store.sync_run_mode_degradations();
        store.raise(Degradation::StandInCore);
        store.raise(Degradation::CredentialAclRelaxed);
        assert_eq!(
            store.degradations(),
            vec![Degradation::StandInCore, Degradation::CredentialAclRelaxed],
            "去重且稳定排序"
        );

        store.clear(Degradation::StandInCore);
        assert!(!store.degradations().contains(&Degradation::StandInCore));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn whoami_carries_run_mode_identity_and_autostart() {
        let dir = temp_dir("whoami");
        let store = store_in(&dir, RunMode::StandIn, "user:alice");
        store.sync_run_mode_degradations();
        let who = store.whoami().expect("whoami");
        assert_eq!(who.run_mode, RunMode::StandIn);
        assert_eq!(who.identity, "user:alice");
        assert!(who.degradations.contains(&Degradation::StandInCore));
        assert!(who.autostart.privilege_required);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn instance_ids_are_unique() {
        let a = new_instance_id();
        let b = new_instance_id();
        assert_ne!(a, b);
        assert!(!a.is_empty());
    }
}
