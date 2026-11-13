/**
 * 隧道会话状态的三态合并。
 *
 * "查不到"和"没连上"是两件事：核心刚重启、token 轮换、暂时不可达时，
 * 会话查询会失败。把失败当成 `connected: false`，界面就会在一切正常时
 * 显示"未连接"，而且没有任何提示——这正是用户遇到的那一幕。
 */

export interface SessionState {
  /** 会话是否连着（仅在 known 为真时有意义）。 */
  connected: boolean;
  /** 是否真的查到了。 */
  known: boolean;
}

/** 会话查询的结果：失败时为 null。 */
export type SessionView = { connected: boolean } | null;

/**
 * 合并一次刷新的结果：
 * - 查到了 → 用新值
 * - 没查到 → 保留上一次（包括"未知"），不把未知写成"没连上"
 */
export function mergeSessionStates(
  previous: Record<string, SessionState>,
  ids: string[],
  views: SessionView[],
): Record<string, SessionState> {
  const next: Record<string, SessionState> = { ...previous };
  ids.forEach((id, index) => {
    const view = views[index];
    if (view) {
      next[id] = { connected: view.connected, known: true };
    } else if (!next[id]) {
      next[id] = { connected: false, known: false };
    }
  });
  return next;
}
