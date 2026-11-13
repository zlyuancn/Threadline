//! 拥有：一个**单实例文件锁**的句柄（跨 SYSTEM 与用户身份生效）。
//! 提供：`InstanceLock::acquire`（有界轮询）/ `InstanceLock::is_held`。
//! 允许调用：`std::fs`、`fs2`、`anyhow`。
//!
//! 为什么不用命名互斥体：创建 `Global\...` 内核对象需要 `SeCreateGlobalPrivilege`，
//! 标准用户会直接 `ERROR_ACCESS_DENIED`（SDDL 只约束"打开已有对象"，不解决"创建"）。
//! 文件锁由 OS 在句柄关闭时自动释放，进程崩溃也不会留死锁。

use std::fs::{File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};
use std::thread::sleep;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use fs2::FileExt;

/// 抢锁结果。
#[derive(Debug)]
pub enum Acquire {
    /// 抢到了锁（持有时其他实例会得到 `Held`）。
    Acquired(InstanceLock),
    /// 已有实例持锁。
    Held,
}

/// 持锁句柄；drop 时自动释放。
#[derive(Debug)]
pub struct InstanceLock {
    file: File,
    path: PathBuf,
}

impl InstanceLock {
    /// 在 `path` 上抢独占锁：以 `poll_interval` 轮询，直到 `timeout`，仍抢不到返回 `Held`。
    pub fn acquire(path: &Path, timeout: Duration, poll_interval: Duration) -> Result<Acquire> {
        let file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(path)
            .with_context(|| format!("打开锁文件失败：{}", path.display()))?;

        let deadline = Instant::now() + timeout;
        loop {
            match file.try_lock_exclusive() {
                Ok(()) => {
                    return Ok(Acquire::Acquired(InstanceLock {
                        file,
                        path: path.to_path_buf(),
                    }))
                }
                Err(e) if is_contended(&e) => {
                    if Instant::now() >= deadline {
                        return Ok(Acquire::Held);
                    }
                    sleep(poll_interval);
                }
                Err(e) => {
                    return Err(e).with_context(|| format!("加锁失败：{}", path.display()));
                }
            }
        }
    }

    /// 锁文件路径（用于日志与诊断）。
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for InstanceLock {
    fn drop(&mut self) {
        let _ = fs2::FileExt::unlock(&self.file);
    }
}

fn is_contended(e: &io::Error) -> bool {
    // Windows: ERROR_LOCK_VIOLATION(33)；Unix: EWOULDBLOCK(11)
    matches!(e.raw_os_error(), Some(33) | Some(11)) || e.kind() == io::ErrorKind::WouldBlock
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_lock_path(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("threadline-lock-{tag}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        dir.join("core.lock")
    }

    #[test]
    fn first_acquire_wins_second_reports_held() {
        let path = temp_lock_path("held");
        let first =
            InstanceLock::acquire(&path, Duration::from_millis(50), Duration::from_millis(10))
                .expect("acquire first");
        let lock = match first {
            Acquire::Acquired(l) => l,
            Acquire::Held => panic!("第一次必须抢到"),
        };

        let second =
            InstanceLock::acquire(&path, Duration::from_millis(50), Duration::from_millis(10))
                .expect("acquire second");
        assert!(matches!(second, Acquire::Held));

        drop(lock);

        let third =
            InstanceLock::acquire(&path, Duration::from_millis(50), Duration::from_millis(10))
                .expect("acquire third");
        assert!(matches!(third, Acquire::Acquired(_)), "释放后应能重新抢到");

        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn missing_directory_is_an_error_not_a_panic() {
        let path = std::env::temp_dir()
            .join("threadline-lock-no-such-dir")
            .join("nested")
            .join("core.lock");
        assert!(
            InstanceLock::acquire(&path, Duration::from_millis(10), Duration::from_millis(5))
                .is_err()
        );
    }
}
