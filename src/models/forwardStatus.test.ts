import { describe, expect, it } from 'vitest';
import { forwardBlockedReason } from './forwardStatus';

const disconnected = { id: 's1', name: 'dev-a', enabled: false };
const connected = { id: 's1', name: 'dev-a', enabled: true };

describe('forwardBlockedReason', () => {
  it('运行中不说话', () => {
    expect(forwardBlockedReason({ state: 'running', last_error: '旧错误' }, connected)).toBe('');
  });

  it('连接断开时，直说连接断了（而不是摊一个过期错误）', () => {
    // 这正是用户看到的那一幕：转发的开关开着、状态已停止，顶部连接卡片却写着「已连接」。
    expect(
      forwardBlockedReason(
        { state: 'stopped', last_error: '无法连接 192.0.2.99:16100：channel closed' },
        disconnected,
      ),
    ).toBe('连接「dev-a」已断开：转发挂在它上面，先打开上面的连接开关');
  });

  it('连接正常时，回落到上次错误', () => {
    expect(forwardBlockedReason({ state: 'error', last_error: '认证被拒' }, connected)).toBe(
      '认证被拒',
    );
  });

  it('既没启用问题也没错误时，不编造原因', () => {
    expect(forwardBlockedReason({ state: 'stopped', last_error: null }, connected)).toBe('');
  });

  it('连接信息还没加载时，不崩也不瞎猜', () => {
    expect(
      forwardBlockedReason({ state: 'stopped', last_error: '本地端口绑定失败' }, undefined),
    ).toBe('本地端口绑定失败');
  });
});
