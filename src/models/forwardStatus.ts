/**
 * 端口转发的「为什么没在跑」。
 *
 * 界面同时展示两件不同的事：**开关=用户意图**（持久化）与**状态=现实**。
 * 两者不一致时（开关开着、状态已停止），必须就地说明原因，否则用户看到的
 * 就是自相矛盾的两行字。这段判定抽出来是为了能被测试覆盖。
 */

export interface ForwardStatusLike {
  state: string;
  last_error: string | null;
}

export interface TunnelLike {
  id: string;
  name: string;
  enabled: boolean;
}

/**
 * 返回一句人能读懂的原因；没有可解释的阻塞点时返回空串。
 *
 * 优先级：**当前**的阻塞点（隧道没启用）＞ 上次错误。
 * 否则会把一条过期错误当成现在的原因——那是另一种撒谎。
 */
export function forwardBlockedReason(
  status: ForwardStatusLike,
  tunnel: TunnelLike | undefined,
): string {
  if (status.state === 'running') {
    return '';
  }
  if (tunnel && !tunnel.enabled) {
    return `隧道「${tunnel.name}」未启用`;
  }
  return status.last_error ?? '';
}
