//! WinINET `ProxyOverride` 的纯字符串 reconcile。
//!
//! 归属：**契约 crate**。零依赖、零 Win32、零 IO，Core 与 Agent 共用，
//! 保证「Core 算出要管哪些域名」与「Agent 实际增删」用的是同一套规则。
//!
//! 规则：
//! - 分号分隔；空项丢弃；两端空白裁剪。
//! - **只动管理集里的项**：用户自己写的条目、`<local>` 一律原样保留。
//! - 大小写不敏感去重（注册表里的域名不区分大小写）。
//! - 结果超过 [`MAX_LEN`] 直接拒绝，不自动归并（归并会误伤同后缀域名）。

/// WinINET 对 `ProxyOverride` 的实际上限（超出会被截断）。
pub const MAX_LEN: usize = 1024;

/// 把原始值拆成条目。
pub fn parse(raw: &str) -> Vec<String> {
    raw.split(';')
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(str::to_string)
        .collect()
}

/// 计算写入后的完整条目列表。
///
/// - `current`：注册表里的原值；
/// - `desired`：本次希望托管的域名（来自当前配置）；
/// - `managed`：上次由我们写入的域名（用于区分「用户的项」与「我们的项」）。
pub fn reconcile(
    current: &str,
    desired: &[String],
    managed: &[String],
) -> Result<Vec<String>, String> {
    let managed_lower: Vec<String> = managed.iter().map(|item| item.to_lowercase()).collect();
    let desired_lower: Vec<String> = desired.iter().map(|item| item.to_lowercase()).collect();

    let mut next: Vec<String> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();

    for item in parse(current) {
        let lower = item.to_lowercase();
        let ours = managed_lower.contains(&lower);
        // 我们的项：只在仍然 desired 时保留；用户的项：无条件保留。
        if ours && !desired_lower.contains(&lower) {
            continue;
        }
        if seen.insert(lower) {
            next.push(item);
        }
    }

    for item in desired {
        let lower = item.to_lowercase();
        if seen.insert(lower) {
            next.push(item.clone());
        }
    }

    let joined = next.join(";");
    if joined.len() > MAX_LEN {
        return Err(format!(
            "代理例外超过 {MAX_LEN} 字符（当前 {}），请减少使用域名的转发",
            joined.len()
        ));
    }
    Ok(next)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(items: &[&str]) -> Vec<String> {
        items.iter().map(|item| item.to_string()).collect()
    }

    #[test]
    fn adds_desired_into_empty_value() {
        let next = reconcile("", &s(&["home.example.com", "git.example.com"]), &[]).expect("ok");
        assert_eq!(next, s(&["home.example.com", "git.example.com"]));
    }

    #[test]
    fn keeps_user_entries_and_local() {
        let current = "<local>;*.corp.example;home.old.m";
        let next = reconcile(current, &s(&["home.example.com"]), &[]).expect("ok");
        assert_eq!(
            next,
            s(&[
                "<local>",
                "*.corp.example",
                "home.old.m",
                "home.example.com"
            ])
        );
    }

    #[test]
    fn removes_only_managed_entries_that_are_no_longer_desired() {
        let current = "<local>;home.example.com;git.example.com;user.example";
        let next = reconcile(
            current,
            &s(&["home.example.com"]),
            &s(&["home.example.com", "git.example.com"]),
        )
        .expect("ok");
        assert_eq!(next, s(&["<local>", "home.example.com", "user.example"]));
    }

    #[test]
    fn does_not_duplicate_existing_desired_entry() {
        let next = reconcile("home.example.com", &s(&["home.example.com"]), &[]).expect("ok");
        assert_eq!(next, s(&["home.example.com"]));
    }

    #[test]
    fn dedupes_case_insensitively_and_keeps_first_spelling() {
        let next = reconcile("HOME.example.com", &s(&["home.example.com"]), &[]).expect("ok");
        assert_eq!(next, s(&["HOME.example.com"]));
    }

    #[test]
    fn is_idempotent() {
        let desired = s(&["home.example.com", "git.example.com"]);
        let once = reconcile("<local>", &desired, &[]).expect("first");
        let text = once.join(";");
        let twice = reconcile(&text, &desired, &desired).expect("second");
        assert_eq!(once, twice);
    }

    #[test]
    fn rejects_overlong_result() {
        let long: Vec<String> = (0..80)
            .map(|n| format!("service-{n:03}.very-long-internal-domain.example"))
            .collect();
        assert!(reconcile("", &long, &[]).is_err());
    }

    #[test]
    fn empty_desired_and_empty_managed_keeps_user_value_untouched() {
        let next = reconcile("<local>;user.example", &[], &[]).expect("ok");
        assert_eq!(next, s(&["<local>", "user.example"]));
    }
}
