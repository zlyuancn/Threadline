//! 拥有：Windows hosts 文件的路径、按字节读取、托管区块渲染、块外冲突检测、
//! 原子写入、权限判定与 DNS 缓存刷新。
//! 提供：`hosts_path` / `render` / `conflicts` / `read_raw` / `write_raw` /
//! `is_elevated` / `flush_dns`。
//! 允许调用：`std::fs` / `std::process` / `foundation::atomic`、`windows-sys`（仅令牌查询）。
//!
//! 纪律：
//! - **按字节处理，不转码**。hosts 里可能有 GBK 注释或 BOM，读成 String 再写回会把它们弄坏。
//! - 只改自己两行标记之间的内容；块外逐字节保留。
//! - 损坏区块（有开始标记、没有结束标记）只摘自己的行，**不吞**后续用户条目。

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::foundation::atomic;

pub const BEGIN_MARKER: &str = "# >>> threadline domain access (managed) >>>";
pub const END_MARKER: &str = "# <<< threadline domain access (managed) <<<";

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Entry {
    pub address: String,
    pub domain: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Conflict {
    pub domain: String,
    /// 块外那行原本指向的地址（用于界面提示「已有一行指向 X」）。
    pub existing: String,
}

/// hosts 的真实路径。用 `SystemRoot` 而不是硬编码盘符。
pub fn hosts_path() -> PathBuf {
    let root = std::env::var_os("SystemRoot")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"C:\Windows"));
    root.join("System32")
        .join("drivers")
        .join("etc")
        .join("hosts")
}

pub fn read_raw(path: &Path) -> Result<Vec<u8>> {
    fs::read(path).with_context(|| format!("读取 hosts 失败：{}", path.display()))
}

pub fn write_raw(path: &Path, bytes: &[u8]) -> Result<()> {
    atomic::write_atomic(path, bytes)
}

/// 渲染托管区块。`entries` 为空时连标记一起删除。
///
/// 保留 BOM、原换行风格（首个换行是 `\r\n` 就用 CRLF）与块外全部字节。
pub fn render(existing: &[u8], entries: &[Entry]) -> Vec<u8> {
    let (bom, body) = if existing.starts_with(&[0xEF, 0xBB, 0xBF]) {
        (&existing[..3], &existing[3..])
    } else {
        (&[][..], existing)
    };
    let lines = split_lines(body);
    let begin = lines.iter().position(|(c, _)| is_marker(c, BEGIN_MARKER));
    let end = lines.iter().position(|(c, _)| is_marker(c, END_MARKER));

    // 损坏区块：只有开始标记。只摘开始行与紧随其后的「我们写的地址行」。
    let corrupt = matches!((begin, end), (Some(_), None));
    let block_range = match (begin, end) {
        (Some(b), Some(e)) if e >= b => Some((b, e)),
        (Some(b), None) => Some((b, b)),
        _ => None,
    };

    let mut sorted = entries.to_vec();
    sorted.sort();
    sorted.dedup();

    let mut block: Vec<String> = Vec::new();
    if !sorted.is_empty() {
        block.push(BEGIN_MARKER.to_string());
        for entry in &sorted {
            block.push(format!("{}\t{}", entry.address, entry.domain));
        }
        block.push(END_MARKER.to_string());
    }

    let uses_crlf = lines.iter().any(|(_, ending)| *ending == b"\r\n");
    let newline: &[u8] = if uses_crlf { b"\r\n" } else { b"\n" };

    let mut out: Vec<u8> = Vec::new();
    out.extend_from_slice(bom);

    match block_range {
        Some((start, stop)) => {
            for (index, (content, ending)) in lines.iter().enumerate() {
                if index == start {
                    for line in &block {
                        out.extend_from_slice(line.as_bytes());
                        out.extend_from_slice(newline);
                    }
                    continue;
                }
                if index > start && index <= stop {
                    continue;
                }
                if corrupt && index > start && looks_like_our_address_line(content) {
                    continue;
                }
                out.extend_from_slice(content);
                out.extend_from_slice(ending);
            }
        }
        None => {
            for (content, ending) in &lines {
                out.extend_from_slice(content);
                out.extend_from_slice(ending);
            }
            if !block.is_empty() {
                if !out.is_empty() && !out.ends_with(b"\n") {
                    out.extend_from_slice(newline);
                }
                for line in &block {
                    out.extend_from_slice(line.as_bytes());
                    out.extend_from_slice(newline);
                }
            }
        }
    }

    out
}

/// 托管区块之外、已经指向目标域名的行。命中即冲突，不覆盖。
pub fn conflicts(existing: &[u8], domains: &[String]) -> Vec<Conflict> {
    if domains.is_empty() {
        return Vec::new();
    }
    let wanted: HashSet<&str> = domains.iter().map(String::as_str).collect();
    let mut in_block = false;
    let mut seen: HashSet<String> = HashSet::new();
    let mut out = Vec::new();

    for (content, _) in split_lines(existing) {
        let trimmed = trim_ascii(content);
        if is_marker(trimmed, BEGIN_MARKER) {
            in_block = true;
            continue;
        }
        if is_marker(trimmed, END_MARKER) {
            in_block = false;
            continue;
        }
        if in_block || trimmed.is_empty() || trimmed.starts_with(b"#") {
            continue;
        }
        let mut parts = trimmed
            .split(|byte| *byte == b' ' || *byte == b'\t')
            .filter(|part| !part.is_empty());
        let Some(address) = parts.next() else {
            continue;
        };
        let address = String::from_utf8_lossy(address).to_string();
        for part in parts {
            let domain = String::from_utf8_lossy(part).to_ascii_lowercase();
            if wanted.contains(domain.as_str()) && seen.insert(domain.clone()) {
                out.push(Conflict {
                    domain,
                    existing: address.clone(),
                });
            }
        }
    }
    out
}

/// 当前进程是否在提升令牌下运行。
///
/// 用令牌查询判断，**绝不**用「以写方式打开 hosts」探测——`File::create` 会截断文件。
#[cfg(windows)]
pub fn is_elevated() -> bool {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::Security::{
        GetTokenInformation, TokenElevation, TOKEN_ELEVATION, TOKEN_QUERY,
    };
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

    unsafe {
        let mut token = std::ptr::null_mut();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
            return false;
        }
        let mut elevation: TOKEN_ELEVATION = std::mem::zeroed();
        let mut len = 0u32;
        let ok = GetTokenInformation(
            token,
            TokenElevation,
            &mut elevation as *mut _ as *mut _,
            std::mem::size_of::<TOKEN_ELEVATION>() as u32,
            &mut len,
        );
        CloseHandle(token);
        ok != 0 && elevation.TokenIsElevated != 0
    }
}

#[cfg(not(windows))]
pub fn is_elevated() -> bool {
    false
}

/// 刷新系统 DNS 缓存。失败只影响「改完立刻生效」的速度，不当致命错误。
pub fn flush_dns() -> Result<()> {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let status = std::process::Command::new("ipconfig")
            .arg("/flushdns")
            .creation_flags(CREATE_NO_WINDOW)
            .status()
            .context("执行 ipconfig /flushdns 失败")?;
        if !status.success() {
            return Err(anyhow::anyhow!(
                "ipconfig /flushdns 退出码 {:?}",
                status.code()
            ));
        }
    }
    Ok(())
}

fn split_lines(bytes: &[u8]) -> Vec<(&[u8], &'static [u8])> {
    let mut out = Vec::new();
    let mut start = 0;
    for i in 0..bytes.len() {
        if bytes[i] == b'\n' {
            let mut content_end = i;
            let ending: &'static [u8] = if content_end > start && bytes[content_end - 1] == b'\r' {
                content_end -= 1;
                b"\r\n"
            } else {
                b"\n"
            };
            out.push((&bytes[start..content_end], ending));
            start = i + 1;
        }
    }
    if start < bytes.len() {
        out.push((&bytes[start..], b""));
    }
    out
}

fn is_marker(line: &[u8], marker: &str) -> bool {
    trim_ascii(line) == marker.as_bytes()
}

fn trim_ascii(mut line: &[u8]) -> &[u8] {
    while let Some((first, rest)) = line.split_first() {
        if first.is_ascii_whitespace() {
            line = rest;
        } else {
            break;
        }
    }
    while let Some((last, rest)) = line.split_last() {
        if last.is_ascii_whitespace() {
            line = rest;
        } else {
            break;
        }
    }
    line
}

/// 损坏区块里，紧跟开始标记的那些行才可能是我们写的。
fn looks_like_our_address_line(line: &[u8]) -> bool {
    trim_ascii(line).starts_with(b"127.0.0.")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(address: &str, domain: &str) -> Entry {
        Entry {
            address: address.to_string(),
            domain: domain.to_string(),
        }
    }

    #[test]
    fn appends_block_and_keeps_foreign_lines() {
        let original = b"192.0.2.98  dev-a.example.com\r\n";
        let out = render(original, &[entry("127.0.0.5", "home.example.com")]);
        let text = String::from_utf8(out).expect("utf8");
        assert!(text.starts_with("192.0.2.98  dev-a.example.com\r\n"));
        assert!(text.contains(BEGIN_MARKER));
        assert!(text.contains("127.0.0.5\thome.example.com"));
        assert!(text.ends_with(&format!("{END_MARKER}\r\n")));
    }

    #[test]
    fn render_is_idempotent() {
        let original = b"# keep me\r\n127.0.0.1 localhost\r\n";
        let entries = vec![entry("127.0.0.5", "home.example.com")];
        let once = render(original, &entries);
        let twice = render(&once, &entries);
        assert_eq!(once, twice);
    }

    #[test]
    fn empty_entries_remove_the_whole_block() {
        let original = b"192.0.2.98 dev-a.example.com\r\n";
        let with_block = render(original, &[entry("127.0.0.5", "home.example.com")]);
        let cleaned = render(&with_block, &[]);
        assert_eq!(cleaned, original.to_vec());
    }

    #[test]
    fn block_outside_content_is_preserved_byte_for_byte() {
        // 含 GBK 字节与 BOM 的块外内容，必须原样保留
        let mut original = vec![0xEF, 0xBB, 0xBF];
        original
            .extend_from_slice(b"# \xD6\xD0\xCE\xC4\xD7\xA2\xCA\xCD\r\n127.0.0.1 localhost\r\n");
        let out = render(&original, &[entry("127.0.0.5", "home.example.com")]);
        assert!(out.starts_with(&original[..3]));
        assert!(out.windows(original.len() - 3).any(|w| w == &original[3..]));
    }

    #[test]
    fn corrupt_block_only_removes_our_address_lines() {
        // 开始标记在、结束标记丢：只摘 127.0.0.x 行，后面的用户条目保留
        let original =
            b"192.0.2.1 keep.example.com\r\n# >>> threadline domain access (managed) >>>\r\n127.0.0.5 old.example.com\r\n192.0.2.9 user.example.com\r\n";
        let out = render(original, &[]);
        let text = String::from_utf8(out).expect("utf8");
        assert!(
            !text.contains("old.example.com"),
            "自己的旧行应被摘掉: {text}"
        );
        assert!(
            text.contains("192.0.2.9 user.example.com"),
            "用户行必须保留: {text}"
        );
        assert!(text.contains("192.0.2.1 keep.example.com"));
        assert!(!text.contains(BEGIN_MARKER));
    }

    #[test]
    fn conflicts_ignore_block_and_comments() {
        let original = b"# 127.0.0.5 home.example.com\r\n192.0.2.98 dev-a.example.com\r\n";
        let found = conflicts(
            original,
            &[
                "home.example.com".to_string(),
                "dev-a.example.com".to_string(),
            ],
        );
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].domain, "dev-a.example.com");
        assert_eq!(found[0].existing, "192.0.2.98");
    }

    #[test]
    fn conflicts_inside_managed_block_are_not_conflicts() {
        let original = format!("{BEGIN_MARKER}\r\n127.0.0.5\thome.example.com\r\n{END_MARKER}\r\n");
        let found = conflicts(original.as_bytes(), &["home.example.com".to_string()]);
        assert!(found.is_empty());
    }

    #[test]
    fn multiple_domains_on_one_foreign_line_are_all_detected() {
        let original = b"192.0.2.2 a.example.com b.example.com\r\n";
        let found = conflicts(
            original,
            &["a.example.com".to_string(), "b.example.com".to_string()],
        );
        assert_eq!(found.len(), 2);
    }

    #[test]
    fn lf_file_stays_lf() {
        let original = b"127.0.0.1 localhost\n";
        let out = render(original, &[entry("127.0.0.5", "home.example.com")]);
        let text = String::from_utf8(out).expect("utf8");
        assert!(!text.contains("\r\n"));
    }
}
