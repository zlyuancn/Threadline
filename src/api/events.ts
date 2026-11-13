import { bootstrap } from './client';

/**
 * 状态变更事件流（SSE）。
 *
 * 为什么不用 `EventSource`：它不能自定义请求头，而核心的控制 API 要求
 * `Authorization: Bearer …`。用 fetch 读流自己做解析，就不用把 token 塞进 URL
 * （URL 会进日志、进历史，是最容易漏的一处）。
 */

export interface EventStreamHandle {
  close: () => void;
}

const RETRY_MIN_MS = 500;
const RETRY_MAX_MS = 5000;

/**
 * 订阅核心的状态变更；`onChange` 在每次收到事件时调用。
 *
 * 断线会自动重连（指数退避到 5s 封顶）。返回的 handle 用于停止订阅——
 * 组件卸载时必须调，否则页面切走后连接会一直挂着。
 */
export function subscribeEvents(onChange: () => void): EventStreamHandle {
  let closed = false;
  let retry = RETRY_MIN_MS;
  let timer: number | null = null;
  let controller: AbortController | null = null;

  function scheduleReconnect(): void {
    if (closed) {
      return;
    }
    timer = window.setTimeout(() => {
      timer = null;
      void connect();
    }, retry);
    retry = Math.min(retry * 2, RETRY_MAX_MS);
  }

  async function connect(): Promise<void> {
    if (closed) {
      return;
    }
    const { baseUrl, token } = bootstrap();
    controller = new AbortController();

    try {
      const response = await fetch(`${baseUrl}/api/v1/events`, {
        headers: { Authorization: `Bearer ${token}`, Accept: 'text/event-stream' },
        signal: controller.signal,
      });
      if (!response.ok || !response.body) {
        throw new Error(`events: HTTP ${response.status}`);
      }
      // 连上了就重置退避，别让一次断线把后续重连都拖慢
      retry = RETRY_MIN_MS;

      const reader = response.body.getReader();
      const decoder = new TextDecoder();
      let buffer = '';

      for (;;) {
        const { value, done } = await reader.read();
        if (done) {
          break;
        }
        buffer += decoder.decode(value, { stream: true });

        // SSE 以空行分隔事件；只关心"有没有事发生"，不解析载荷
        let boundary = buffer.indexOf('\n\n');
        while (boundary !== -1) {
          const chunk = buffer.slice(0, boundary);
          buffer = buffer.slice(boundary + 2);
          if (chunk.includes('event: changed')) {
            onChange();
          }
          boundary = buffer.indexOf('\n\n');
        }
      }
    } catch {
      // 连接被拒/中断都走重连；具体原因交给轮询那条线去判断"核心是否在线"
    }

    scheduleReconnect();
  }

  void connect();

  return {
    close(): void {
      closed = true;
      if (timer !== null) {
        window.clearTimeout(timer);
        timer = null;
      }
      controller?.abort();
    },
  };
}
