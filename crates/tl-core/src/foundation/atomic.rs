//! 拥有：单文件「临时文件 + 原子替换」机制，含 Windows 占用重试。
//! 提供：`write_atomic` / `replace_with_retry`。
//! 允许调用：`std::fs` / `std::io`、`windows-sys`（仅文件替换 API）。
//!
//! 为什么需要这一层：配置 JSON 与 hosts 都会遇到「目标正被别的进程打开」的失败窗口
//! （杀毒、索引器、备份）。调用方各写一套重试必然走样，所以统一放在底座。

use std::collections::HashSet;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};

use anyhow::{Context, Result};

/// 进程内同路径串行表。
///
/// `ReplaceFileW` 的两个并发替换同一目标时会撞上它自己的中间态（1175/1176/1177），
/// 重试也不保证收敛。跨进程靠重试，同进程直接排队——配置写入本来就是低频操作。
static BUSY: LazyLock<Mutex<HashSet<PathBuf>>> = LazyLock::new(|| Mutex::new(HashSet::new()));

struct PathGuard {
    path: PathBuf,
}

impl Drop for PathGuard {
    fn drop(&mut self) {
        let mut busy = BUSY.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        busy.remove(&self.path);
    }
}

fn acquire(path: &Path) -> PathGuard {
    let key = path.to_path_buf();
    loop {
        let mut busy = BUSY.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if busy.insert(key.clone()) {
            return PathGuard { path: key };
        }
        drop(busy);
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
}

/// 原子写入任意字节：同目录临时文件 → fsync → 替换目标。
pub fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    let _guard = acquire(path);
    let tmp = temp_path(path);
    {
        let mut file = fs::File::create(&tmp)
            .with_context(|| format!("创建临时文件失败：{}", tmp.display()))?;
        file.write_all(bytes)
            .with_context(|| format!("写入临时文件失败：{}", tmp.display()))?;
        file.sync_all().context("fsync 临时文件失败")?;
    }

    match replace_with_retry(&tmp, path) {
        Ok(()) => Ok(()),
        Err(e) => {
            let _ = fs::remove_file(&tmp);
            // 保留 io::Error 作为 source：调用方要靠 raw_os_error 区分「没权限」与「其它失败」。
            Err(anyhow::Error::new(e).context(format!(
                "原子替换失败：{} -> {}",
                tmp.display(),
                path.display()
            )))
        }
    }
}

/// 用 `from` 替换 `to`，遇到 Windows 的共享冲突短暂重试。
///
/// 覆盖被占用文件的失败窗口实测能到 1 秒以上（杀毒/索引器扫新文件），
/// 所以退避上限放到 ~2s：交互操作等一下可以接受，直接报 500 不行。
pub fn replace_with_retry(from: &Path, to: &Path) -> io::Result<()> {
    const ATTEMPTS: u32 = 20;
    let mut last = None;
    for attempt in 0..ATTEMPTS {
        match replace_once(from, to) {
            Ok(()) => return Ok(()),
            Err(e) if is_transient(&e) => {
                last = Some(e);
                let backoff = 10 * u64::from(attempt + 1);
                std::thread::sleep(std::time::Duration::from_millis(backoff.min(150)));
            }
            Err(e) => return Err(e),
        }
    }
    Err(last.expect("循环至少尝试一次"))
}

#[cfg(windows)]
fn replace_once(from: &Path, to: &Path) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::ReplaceFileW;

    // 目标不存在时 ReplaceFileW 会失败，直接 rename。
    if !to.exists() {
        return fs::rename(from, to);
    }
    let wide = |p: &Path| -> Vec<u16> {
        p.as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect()
    };
    let to_w = wide(to);
    let from_w = wide(from);
    // ReplaceFileW 保留目标原有的安全描述符（hosts 的自定义 ACL 不会被继承 ACL 覆盖）。
    let ok = unsafe {
        ReplaceFileW(
            to_w.as_ptr(),
            from_w.as_ptr(),
            std::ptr::null(),
            0,
            std::ptr::null(),
            std::ptr::null(),
        )
    };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(not(windows))]
fn replace_once(from: &Path, to: &Path) -> io::Result<()> {
    fs::rename(from, to)
}

/// 只重试「共享冲突」这一类：其余错误（磁盘满、路径不存在）重试也没用。
///
/// 1175/1176/1177 是 `ReplaceFileW` 自己的三个中间态错误码（无法移动/无法删除被替换文件），
/// 在另一个进程正替换同一目标时出现，退避后即可成功。
fn is_transient(error: &io::Error) -> bool {
    matches!(
        error.raw_os_error(),
        Some(5) | Some(32) | Some(33) | Some(1175) | Some(1176) | Some(1177)
    )
}

fn temp_path(path: &Path) -> PathBuf {
    // 临时文件名必须唯一。固定叫 `<file>.tmp` 时，两个写入方会互相搬走对方的临时文件，
    // 后者 rename 直接报 os error 2（文件不存在）。
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let seq = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(format!(".tmp-{}-{seq}", std::process::id()));
    path.with_file_name(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("threadline-atomic-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("dir");
        dir
    }

    #[test]
    fn writes_then_replaces_and_leaves_no_temp_files() {
        let dir = temp_dir("roundtrip");
        let path = dir.join("data.bin");
        write_atomic(&path, b"first").expect("first");
        assert_eq!(fs::read(&path).expect("read"), b"first");
        write_atomic(&path, b"second").expect("second");
        assert_eq!(fs::read(&path).expect("read"), b"second");

        let leftovers: Vec<_> = fs::read_dir(&dir)
            .expect("list")
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().contains(".tmp-"))
            .collect();
        assert!(leftovers.is_empty(), "临时文件应已全部替换: {leftovers:?}");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn replacing_missing_target_creates_it() {
        let dir = temp_dir("create");
        let path = dir.join("new.bin");
        write_atomic(&path, b"hello").expect("write");
        assert_eq!(fs::read(&path).expect("read"), b"hello");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn concurrent_writes_to_different_files_all_succeed() {
        // 同一文件的并发替换依赖调用方串行（ReplaceFileW 的中间态不保证并发安全），
        // 这里验证的是「多线程各写各的文件不会互相搬走临时文件」。
        let dir = temp_dir("race");
        let handles: Vec<_> = (0..8)
            .map(|i| {
                let path = dir.join(format!("data-{i}.bin"));
                std::thread::spawn(move || {
                    for _ in 0..5 {
                        write_atomic(&path, format!("writer-{i}").as_bytes()).expect("write");
                    }
                    path
                })
            })
            .collect();
        for handle in handles {
            let path = handle.join().expect("join");
            assert!(fs::read(&path).expect("read").starts_with(b"writer-"));
        }
        let _ = fs::remove_dir_all(&dir);
    }
}
