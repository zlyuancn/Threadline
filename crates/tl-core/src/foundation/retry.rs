//! 拥有：一条**有界退避**策略的状态（重试次数、下次延迟）。
//! 提供：`Backoff::next_delay` / `Backoff::reset` / `Backoff::exhausted`。
//! 允许调用：仅标准库。
//!
//! 为什么在底座：MSO 明确禁止模型自建私有重试轮子（各写各的会互相不知情、同时开火打挂下游）。
//! 所有需要"重连/重试"的模型都必须用这里的实例，并把参数（序列、次数上限）作为声明传入。

/// 默认退避序列（秒）：1 → 2 → 4 → 8 → 16 → 30，之后固定 30。
pub const DEFAULT_SCHEDULE_SECS: &[u64] = &[1, 2, 4, 8, 16, 30];

/// 默认最大连续尝试次数：用完即进入不可自愈的错误态，等人工重试。
pub const DEFAULT_MAX_ATTEMPTS: u32 = 10;

#[derive(Debug, Clone)]
pub struct Backoff {
    schedule: &'static [u64],
    max_attempts: u32,
    attempts: u32,
}

impl Default for Backoff {
    fn default() -> Self {
        Self::new(DEFAULT_SCHEDULE_SECS, DEFAULT_MAX_ATTEMPTS)
    }
}

impl Backoff {
    pub fn new(schedule: &'static [u64], max_attempts: u32) -> Self {
        assert!(!schedule.is_empty(), "退避序列不能为空");
        assert!(max_attempts > 0, "最大尝试次数必须大于 0");
        Self {
            schedule,
            max_attempts,
            attempts: 0,
        }
    }

    /// 已经尝试过的次数。
    pub fn attempts(&self) -> u32 {
        self.attempts
    }

    /// 预算是否已耗尽（耗尽后不应再自动重试）。
    pub fn exhausted(&self) -> bool {
        self.attempts >= self.max_attempts
    }

    /// 取下一次等待秒数并自增计数；已耗尽则返回 `None`。
    pub fn next_delay(&mut self) -> Option<u64> {
        if self.exhausted() {
            return None;
        }
        let idx = (self.attempts as usize).min(self.schedule.len() - 1);
        self.attempts += 1;
        Some(self.schedule[idx])
    }

    /// 连接成功后复位（下次断线从最短延迟重新开始）。
    pub fn reset(&mut self) {
        self.attempts = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schedule_grows_then_caps() {
        let mut b = Backoff::new(DEFAULT_SCHEDULE_SECS, 10);
        let delays: Vec<u64> = (0..7).filter_map(|_| b.next_delay()).collect();
        assert_eq!(delays, vec![1, 2, 4, 8, 16, 30, 30]);
    }

    #[test]
    fn budget_is_bounded_and_reports_exhaustion() {
        let mut b = Backoff::new(DEFAULT_SCHEDULE_SECS, 3);
        assert_eq!(b.next_delay(), Some(1));
        assert_eq!(b.next_delay(), Some(2));
        assert_eq!(b.next_delay(), Some(4));
        assert!(b.exhausted());
        assert_eq!(b.next_delay(), None);
        assert_eq!(b.attempts(), 3);
    }

    #[test]
    fn reset_restarts_from_the_beginning() {
        let mut b = Backoff::new(DEFAULT_SCHEDULE_SECS, 2);
        assert_eq!(b.next_delay(), Some(1));
        assert_eq!(b.next_delay(), Some(2));
        assert!(b.exhausted());

        b.reset();
        assert!(!b.exhausted());
        assert_eq!(b.next_delay(), Some(1));
    }
}
