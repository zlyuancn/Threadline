//! 拥有：时间戳格式化的纯实现（UTC ISO-8601）。
//! 提供：`now_iso8601` / `to_iso8601`。
//! 允许调用：仅标准库。
//!
//! 为什么自己写：只为了给 `control.json` / 观测指纹写一个人类可读时间戳，
//! 为此引入整个日期库不划算。算法是公开的 civil-from-days，已用已知时间点锁定测试。

use std::time::{SystemTime, UNIX_EPOCH};

/// 当前时刻（UTC，`YYYY-MM-DDTHH:MM:SSZ`）。
pub fn now_iso8601() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    to_iso8601(secs)
}

/// Unix 秒 → `YYYY-MM-DDTHH:MM:SSZ`（UTC）。
pub fn to_iso8601(secs_since_epoch: i64) -> String {
    let days = secs_since_epoch.div_euclid(86_400);
    let secs_of_day = secs_since_epoch.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    let hour = secs_of_day / 3600;
    let minute = (secs_of_day % 3600) / 60;
    let second = secs_of_day % 60;
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z")
}

/// Howard Hinnant 的 civil_from_days：自 1970-01-01 起的天数 → 公历年月日。
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32; // [1, 12]
    (if m <= 2 { y + 1 } else { y }, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn epoch_is_1970() {
        assert_eq!(to_iso8601(0), "1970-01-01T00:00:00Z");
    }

    #[test]
    fn known_timestamps_map_correctly() {
        // 2026-10-19T00:00:00Z
        assert_eq!(to_iso8601(1_792_368_000), "2026-10-19T00:00:00Z");
        // 2000-03-01T12:34:56Z（跨闰年边界）
        assert_eq!(to_iso8601(951_914_096), "2000-03-01T12:34:56Z");
        // 2024-02-29T23:59:59Z（闰日）
        assert_eq!(to_iso8601(1_709_251_199), "2024-02-29T23:59:59Z");
    }

    #[test]
    fn now_is_well_formed() {
        let now = now_iso8601();
        assert_eq!(now.len(), 20, "格式长度固定：{now}");
        assert!(now.ends_with('Z'));
        assert_eq!(&now[4..5], "-");
        assert_eq!(&now[10..11], "T");
    }
}
