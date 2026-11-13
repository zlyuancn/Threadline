//! 转发流量计数。
//!
//! 归属：`tunnel` 模型内部文件组。只做"数了多少字节"，不关心速率——
//! 速率需要时间窗口，那是界面的事（后端只保证计数单调、不被清零）。
//!
//! 为什么放在模型里而不是引擎里：引擎每建一次会话都会重建连接循环，
//! 计数要跨会话累加；而状态又是模型算出来的，两者的宿主只能是同一处。

use std::sync::atomic::{AtomicU64, Ordering};

/// 一条转发的累计字节数。
#[derive(Debug, Default)]
pub struct Traffic {
    /// 本机 → 远端（用户发出去的请求）。
    up: AtomicU64,
    /// 远端 → 本机（拿回来的数据）。
    down: AtomicU64,
}

impl Traffic {
    pub fn add_up(&self, bytes: u64) {
        self.up.fetch_add(bytes, Ordering::Relaxed);
    }

    pub fn add_down(&self, bytes: u64) {
        self.down.fetch_add(bytes, Ordering::Relaxed);
    }

    /// 读一对累计值。界面对两次采样做差得到速率。
    pub fn snapshot(&self) -> (u64, u64) {
        (
            self.up.load(Ordering::Relaxed),
            self.down.load(Ordering::Relaxed),
        )
    }

    /// 归零：转发重新启动时调用，避免把上一段的流量算进新的连接。
    pub fn reset(&self) {
        self.up.store(0, Ordering::Relaxed);
        self.down.store(0, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counters_accumulate_and_reset() {
        let traffic = Traffic::default();
        traffic.add_up(100);
        traffic.add_down(250);
        assert_eq!(traffic.snapshot(), (100, 250));

        traffic.reset();
        assert_eq!(traffic.snapshot(), (0, 0), "重启后不该带着上一段的数字");
    }
}
