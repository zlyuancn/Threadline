//! 拥有：单个 JSON 文件的读取与**原子写入**（临时文件 + 同目录 rename 覆盖）。
//! 提供：`read_json` / `write_json_atomic`。
//! 允许调用：`std::fs`、`serde`、`anyhow`。
//!
//! 为什么原子：`control.json` 是发现契约，写到一半就被 Wake 读到会判「不是我们的核心」；
//! 配置文件的半写会丢掉全部隧道。所有落盘都走这里，不允许调用方自己 `fs::write`。

use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use serde::de::DeserializeOwned;
use serde::Serialize;

use crate::foundation::atomic;

/// 读取 JSON。文件不存在返回 `Ok(None)`（不是错误——首跑就是这种状态）。
pub fn read_json<T: DeserializeOwned>(path: &Path) -> Result<Option<T>> {
    match fs::read_to_string(path) {
        Ok(text) => {
            let value = serde_json::from_str(&text)
                .with_context(|| format!("解析 JSON 失败：{}", path.display()))?;
            Ok(Some(value))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e).with_context(|| format!("读取失败：{}", path.display())),
    }
}

/// 原子写入 JSON：先写 `<name>.tmp`，fsync，再 rename 覆盖目标。
pub fn write_json_atomic<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    let bytes = serde_json::to_vec_pretty(value).context("序列化 JSON 失败")?;
    atomic::write_atomic(path, &bytes)
}

/// 删除文件；不存在视为成功（幂等）。
pub fn remove_if_exists(path: &Path) -> Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e).with_context(|| format!("删除失败：{}", path.display())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn concurrent_writes_all_succeed() {
        // 固定临时文件名时，这里会报 os error 2：后到的写入方 rename 时，
        // 临时文件已经被另一个线程改名搬走了。
        let dir =
            std::env::temp_dir().join(format!("threadline-store-race-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("dir");
        let path = dir.join("data.json");

        let handles: Vec<_> = (0..8)
            .map(|i| {
                let path = path.clone();
                std::thread::spawn(move || {
                    for _ in 0..5 {
                        write_json_atomic(&path, &serde_json::json!({ "writer": i }))
                            .expect("write");
                    }
                })
            })
            .collect();
        for handle in handles {
            handle.join().expect("join");
        }

        let value: serde_json::Value = read_json(&path).expect("read").expect("exists");
        assert!(value["writer"].is_number(), "最后一次写入必须完整落盘");

        // 别留下临时文件
        let leftovers: Vec<_> = std::fs::read_dir(&dir)
            .expect("list")
            .filter_map(|entry| entry.ok())
            .filter(|entry| entry.file_name().to_string_lossy().contains(".tmp-"))
            .collect();
        assert!(leftovers.is_empty(), "临时文件应已全部改名: {leftovers:?}");

        let _ = std::fs::remove_dir_all(&dir);
    }
    use serde::Deserialize;

    #[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
    struct Sample {
        id: String,
        port: u16,
    }

    fn temp_dir(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("threadline-store-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    #[test]
    fn missing_file_is_none_not_error() {
        let dir = temp_dir("missing");
        let got: Option<Sample> = read_json(&dir.join("nope.json")).expect("read");
        assert!(got.is_none());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn roundtrip_and_overwrite() {
        let dir = temp_dir("roundtrip");
        let path = dir.join("sample.json");

        let first = Sample {
            id: "a".into(),
            port: 1,
        };
        write_json_atomic(&path, &first).expect("write");
        assert_eq!(read_json::<Sample>(&path).expect("read"), Some(first));

        let second = Sample {
            id: "b".into(),
            port: 2,
        };
        write_json_atomic(&path, &second).expect("overwrite");
        assert_eq!(read_json::<Sample>(&path).expect("read"), Some(second));

        // 不残留临时文件
        assert!(!dir.join("sample.json.tmp").exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn broken_json_is_an_error_so_callers_can_fall_back() {
        let dir = temp_dir("broken");
        let path = dir.join("broken.json");
        fs::write(&path, "{ not json").expect("write raw");
        assert!(read_json::<Sample>(&path).is_err());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn remove_is_idempotent() {
        let dir = temp_dir("remove");
        let path = dir.join("gone.json");
        remove_if_exists(&path).expect("first");
        fs::write(&path, "{}").expect("write");
        remove_if_exists(&path).expect("second");
        assert!(!path.exists());
        let _ = fs::remove_dir_all(&dir);
    }
}
