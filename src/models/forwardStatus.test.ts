import { describe, expect, it } from 'vitest';
import { forwardBlockedReason } from './forwardStatus';

const disabledTunnel = { id: 's1', name: 'dev-a', enabled: false };
const enabledTunnel = { id: 's1', name: 'dev-a', enabled: true };

describe('forwardBlockedReason', () => {
  it('运行中不说话', () => {
    expect(
      forwardBlockedReason({ state: 'running', last_error: '旧错误' }, enabledTunnel),
    ).toBe('');
  });

  it('隧道未启用时，直说隧道没启用（而不是摊一个过期错误）', () => {
    // 这正是用户看到的那一幕：开关开着、状态已停止，界面必须解释为什么。
    expect(
      forwardBlockedReason(
        { state: 'stopped', last_error: '无法连接 192.0.2.99:16100：channel closed' },
        disabledTunnel,
      ),
    ).toBe('隧道「dev-a」未启用');
  });

  it('隧道正常时，回落到上次错误', () => {
    expect(
      forwardBlockedReason({ state: 'error', last_error: '认证被拒' }, enabledTunnel),
    ).toBe('认证被拒');
  });

  it('既没启用问题也没错误时，不编造原因', () => {
    expect(forwardBlockedReason({ state: 'stopped', last_error: null }, enabledTunnel)).toBe('');
  });

  it('隧道信息还没加载时，不崩也不瞎猜', () => {
    expect(
      forwardBlockedReason({ state: 'stopped', last_error: '本地端口绑定失败' }, undefined),
    ).toBe('本地端口绑定失败');
  });
});
