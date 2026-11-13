//! 拥有：一个**并发预算**的计数状态（当前占用数、上限）。
//! 提供：`ConcurrencyBudget::try_acquire`（返回可自动归还的 `Permit`）。
//! 允许调用：仅标准库。
//!
//! 为什么在底座：单隧道本地连接数必须有上限（超出即拒，不排队堆积），
//! 否则一个失控的本地客户端就能把内存打满。这与退避同在底座，模型只声明参数。

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

/// 并发预算。`Permit` 掉落时自动归还。
#[derive(Debug, Clone)]
pub struct ConcurrencyBudget {
    max: usize,
    active: Arc<AtomicUsize>,
}

/// 一次占用的凭据；drop 时归还。
#[derive(Debug)]
pub struct Permit {
    active: Arc<AtomicUsize>,
}

impl Drop for Permit {
    fn drop(&mut self) {
        self.active.fetch_sub(1, Ordering::SeqCst);
    }
}

impl ConcurrencyBudget {
    pub fn new(max: usize) -> Self {
        assert!(max > 0, "并发上限必须大于 0");
        Self {
            max,
            active: Arc::new(AtomicUsize::new(0)),
        }
    }

    pub fn max(&self) -> usize {
        self.max
    }

    pub fn active(&self) -> usize {
        self.active.load(Ordering::SeqCst)
    }

    /// 尝试占用一个名额；超限返回 `None`（**不排队**，调用方应立即拒绝该连接）。
    pub fn try_acquire(&self) -> Option<Permit> {
        loop {
            let current = self.active.load(Ordering::SeqCst);
            if current >= self.max {
                return None;
            }
            if self
                .active
                .compare_exchange(current, current + 1, Ordering::SeqCst, Ordering::SeqCst)
                .is_ok()
            {
                return Some(Permit {
                    active: Arc::clone(&self.active),
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_beyond_limit_instead_of_queueing() {
        let budget = ConcurrencyBudget::new(2);
        let _a = budget.try_acquire().expect("first");
        let _b = budget.try_acquire().expect("second");
        assert!(budget.try_acquire().is_none(), "第三条必须被立即拒绝");
        assert_eq!(budget.active(), 2);
    }

    #[test]
    fn permit_is_returned_on_drop() {
        let budget = ConcurrencyBudget::new(1);
        {
            let _p = budget.try_acquire().expect("first");
            assert!(budget.try_acquire().is_none());
        }
        assert_eq!(budget.active(), 0);
        assert!(budget.try_acquire().is_some(), "归还后应可再次占用");
    }

    #[test]
    fn budget_is_shared_across_clones() {
        let budget = ConcurrencyBudget::new(1);
        let clone = budget.clone();
        let _p = budget.try_acquire().expect("first");
        assert!(clone.try_acquire().is_none(), "克隆共享同一个计数");
    }
}
