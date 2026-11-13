//! 拥有：Windows 任务计划（per-machine 注册物）的查询与增删。
//! 提供：`validate`（前置校验，字段级报错）/ `build_create_args` / `query` / `create` / `delete` / `set_enabled`。
//! 允许调用：`std::process`（调用 `schtasks.exe`）、`std::path`。
//!
//! 纪律（CAW R5 / AP10）：任务计划是 **per-machine** 注册物——只有提权核心、安装器或
//! 一次性提权进程可以写；用户身份的 Core 与 Agent 只能读或要求提权。
//! 前置校验 MUST 在创建**之前**做，否则会留下指向错误路径的任务计划而无法回滚。

use std::path::{Path, PathBuf};
use std::process::Command;

/// 任务计划名（固定，便于排障与卸载清理）。
pub const TASK_NAME: &str = "ThreadlineCore";

/// 注册层操作失败：`field` 有值即为字段级错误（CAW R5 要求带字段名）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegistryError {
    pub field: Option<&'static str>,
    pub message: String,
}

impl RegistryError {
    fn with_field(field: &'static str, message: impl Into<String>) -> Self {
        Self {
            field: Some(field),
            message: message.into(),
        }
    }

    fn plain(message: impl Into<String>) -> Self {
        Self {
            field: None,
            message: message.into(),
        }
    }
}

impl std::fmt::Display for RegistryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.field {
            Some(field) => write!(f, "[{field}] {}", self.message),
            None => write!(f, "{}", self.message),
        }
    }
}

impl std::error::Error for RegistryError {}

/// 任务计划状态（`exists = false` 时 `enabled` 无意义）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskStatus {
    pub exists: bool,
    pub enabled: bool,
}

impl TaskStatus {
    pub fn absent() -> Self {
        Self {
            exists: false,
            enabled: false,
        }
    }
}

/// 前置校验：路径全绝对、无用户环境依赖、日志去向显式（CAW R5）。
///
/// 校验通过才允许创建任务计划；任何一条不满足都必须**带字段名**拒绝。
pub fn validate(exe_path: &Path, log_dir: &Path) -> Result<(), RegistryError> {
    check_absolute(exe_path, "exe_path")?;
    check_absolute(log_dir, "log_dir")?;

    let exe = exe_path.to_string_lossy();
    if exe.contains('~') {
        return Err(RegistryError::with_field(
            "exe_path",
            "路径不得包含 ~（不得依赖用户主目录展开）",
        ));
    }
    if exe.contains('%') {
        return Err(RegistryError::with_field(
            "exe_path",
            "路径不得包含环境变量（%VAR%）：SYSTEM 身份下它会展开成另一个值",
        ));
    }
    let log = log_dir.to_string_lossy();
    if log.contains('~') || log.contains('%') {
        return Err(RegistryError::with_field(
            "log_dir",
            "日志目录不得包含 ~ 或环境变量",
        ));
    }
    Ok(())
}

fn check_absolute(path: &Path, field: &'static str) -> Result<(), RegistryError> {
    if path.as_os_str().is_empty() {
        return Err(RegistryError::with_field(field, "路径不能为空"));
    }
    if !path.is_absolute() {
        return Err(RegistryError::with_field(field, "路径必须是绝对路径"));
    }
    Ok(())
}

/// 构造 `schtasks /create` 的参数（纯函数，便于单测与排障时复现）。
pub fn build_create_args(exe_path: &Path, log_dir: &Path) -> Vec<String> {
    vec![
        "/create".into(),
        "/tn".into(),
        TASK_NAME.into(),
        "/tr".into(),
        format!(
            "\"{}\" --boot --log-dir \"{}\"",
            exe_path.display(),
            log_dir.display()
        ),
        "/sc".into(),
        "onstart".into(),
        "/ru".into(),
        "SYSTEM".into(),
        "/rl".into(),
        "HIGHEST".into(),
        "/f".into(),
    ]
}

fn schtasks(args: &[String]) -> Result<std::process::Output, RegistryError> {
    let mut command = Command::new("schtasks.exe");
    command.args(args);
    // `schtasks.exe` 是控制台程序：核心通常以 `DETACHED_PROCESS` 运行（没有控制台），
    // 子进程若不加这个标志，Windows 会为它**新开一个控制台窗口**，表现成
    // "每隔几秒闪一下黑框"（面板每 5 秒轮询 /whoami，而 /whoami 会查任务计划）。
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    command
        .output()
        .map_err(|e| RegistryError::plain(format!("无法执行 schtasks.exe：{e}")))
}

/// 查询任务是否存在与是否启用。
pub fn query(task_name: &str) -> Result<TaskStatus, RegistryError> {
    let out = schtasks(&[
        "/query".into(),
        "/tn".into(),
        task_name.into(),
        "/xml".into(),
        "ONE".into(),
    ])?;
    if !out.status.success() {
        // 任务不存在是正常状态，不是错误。
        return Ok(TaskStatus::absent());
    }
    let xml = String::from_utf8_lossy(&out.stdout);
    Ok(TaskStatus {
        exists: true,
        enabled: !xml.contains("<Enabled>false</Enabled>"),
    })
}

/// 创建（覆盖）开机自启任务：ONSTART + SYSTEM + 最高权限。
pub fn create(exe_path: &Path, log_dir: &Path) -> Result<(), RegistryError> {
    validate(exe_path, log_dir)?;
    let out = schtasks(&build_create_args(exe_path, log_dir))?;
    if !out.status.success() {
        return Err(RegistryError::plain(format!(
            "创建任务计划失败：{}",
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    Ok(())
}

/// 删除任务；不存在视为成功（幂等）。
pub fn delete(task_name: &str) -> Result<(), RegistryError> {
    let out = schtasks(&[
        "/delete".into(),
        "/tn".into(),
        task_name.into(),
        "/f".into(),
    ])?;
    if out.status.success() {
        return Ok(());
    }
    let err = String::from_utf8_lossy(&out.stderr).to_lowercase();
    if err.contains("cannot find") || err.contains("找不到") {
        return Ok(());
    }
    Err(RegistryError::plain(format!(
        "删除任务计划失败：{}",
        String::from_utf8_lossy(&out.stderr).trim()
    )))
}

/// 启用/禁用任务（**不删除**：删除对运行中实例的影响未定义，禁用可逆且安全）。
pub fn set_enabled(task_name: &str, enabled: bool) -> Result<(), RegistryError> {
    let flag = if enabled { "/enable" } else { "/disable" };
    let out = schtasks(&[
        "/change".into(),
        "/tn".into(),
        task_name.into(),
        flag.into(),
    ])?;
    if !out.status.success() {
        return Err(RegistryError::plain(format!(
            "修改任务状态失败：{}",
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    Ok(())
}

/// 任务计划实际写入的路径（排障用：与期望值比对可发现路径漂移）。
pub fn task_command(task_name: &str) -> Result<Option<String>, RegistryError> {
    let out = schtasks(&[
        "/query".into(),
        "/tn".into(),
        task_name.into(),
        "/v".into(),
        "/fo".into(),
        "LIST".into(),
    ])?;
    if !out.status.success() {
        return Ok(None);
    }
    let text = String::from_utf8_lossy(&out.stdout).to_string();
    Ok(text
        .lines()
        .find(|line| line.contains("Task To Run") || line.contains("要运行的任务"))
        .map(|line| {
            line.split_once(':')
                .map(|(_, v)| v.trim())
                .unwrap_or(line.trim())
                .to_string()
        }))
}

/// 常用路径：`%ProgramData%\threadline\logs`（这里只做字符串拼接，不访问环境变量）。
pub fn default_log_dir(program_data: &Path) -> PathBuf {
    program_data.join("threadline").join("logs")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_relative_exe_path_with_named_field() {
        let err = validate(
            Path::new("tl-core.exe"),
            Path::new("C:\\ProgramData\\threadline\\logs"),
        )
        .expect_err("相对路径必须被拒");
        assert_eq!(err.field, Some("exe_path"));
    }

    #[test]
    fn rejects_relative_log_dir_with_named_field() {
        let err = validate(
            Path::new("C:\\Program Files\\Threadline\\tl-core.exe"),
            Path::new("logs"),
        )
        .expect_err("相对日志目录必须被拒");
        assert_eq!(err.field, Some("log_dir"));
    }

    #[test]
    fn rejects_home_and_env_dependence() {
        let err = validate(
            Path::new("C:\\Users\\x\\~backup\\tl-core.exe"),
            Path::new("C:\\logs"),
        )
        .expect_err("含 ~ 必须被拒");
        assert_eq!(err.field, Some("exe_path"));

        let err = validate(
            Path::new("C:\\Users\\%USERNAME%\\tl-core.exe"),
            Path::new("C:\\logs"),
        )
        .expect_err("含环境变量必须被拒");
        assert_eq!(err.field, Some("exe_path"));
    }

    #[test]
    fn accepts_absolute_paths() {
        validate(
            Path::new("C:\\Program Files\\Threadline\\tl-core.exe"),
            Path::new("C:\\ProgramData\\threadline\\logs"),
        )
        .expect("绝对路径应通过");
    }

    #[test]
    fn create_args_pin_identity_and_trigger() {
        let args = build_create_args(
            Path::new("C:\\Program Files\\Threadline\\tl-core.exe"),
            Path::new("C:\\ProgramData\\threadline\\logs"),
        );
        let joined = args.join(" ");
        assert!(joined.contains("/sc onstart"), "必须是开机触发：{joined}");
        assert!(
            joined.contains("/ru SYSTEM"),
            "必须以 SYSTEM 运行：{joined}"
        );
        assert!(joined.contains("/rl HIGHEST"), "必须是最高权限：{joined}");
        assert!(joined.contains("--boot"), "必须带无头身份信号：{joined}");
        assert!(joined.contains("/f"), "必须覆盖旧条目：{joined}");
        assert!(
            joined.contains("\"C:\\Program Files\\Threadline\\tl-core.exe\""),
            "路径必须加引号：{joined}"
        );
    }

    #[test]
    fn absent_task_is_not_an_error_shape() {
        let status = TaskStatus::absent();
        assert!(!status.exists);
        assert!(!status.enabled);
    }

    #[test]
    fn default_log_dir_is_under_threadline() {
        let dir = default_log_dir(Path::new("C:\\ProgramData"));
        assert_eq!(dir, PathBuf::from("C:\\ProgramData\\threadline\\logs"));
    }
}
