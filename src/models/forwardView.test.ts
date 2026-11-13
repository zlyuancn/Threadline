import { describe, expect, it } from 'vitest';
import {
  canOpen,
  connLabel,
  endpointLabel,
  faviconUrl,
  isActive,
  isWeb,
  matchesQuery,
  pillOf,
  schemeLabel,
  schemeOf,
  urlOf,
} from './forwardView';

function view(overrides: Record<string, unknown> = {}) {
  return {
    definition: {
      local_host: '127.0.0.1',
      local_port: 16100,
      remote_host: '192.0.2.99',
      remote_port: 16100,
      scheme: 'tcp',
      name: 'HA',
      note: '测试面板',
      ...overrides,
    },
    status: { state: 'running', active_connections: 0 },
  };
}

describe('forwardView', () => {
  it('only http/https offer an openable url', () => {
    expect(isWeb(view())).toBe(false);
    expect(isWeb(view({ scheme: 'http' }))).toBe(true);
    expect(isWeb(view({ scheme: 'https' }))).toBe(true);
  });

  it('builds the url with the right scheme and loopback fallback', () => {
    expect(urlOf(view({ scheme: 'http' }))).toBe('http://127.0.0.1:16100/');
    expect(urlOf(view({ scheme: 'https' }))).toBe('https://127.0.0.1:16100/');
    expect(urlOf(view({ scheme: 'http', local_host: '0.0.0.0' }))).toBe('http://127.0.0.1:16100/');
  });

  it('appends the configured path to the open url', () => {
    // 「打开本地页面」要能直接落到具体页面，而不是永远停在根路径。
    expect(urlOf(view({ scheme: 'http', open_path: '/lovelace/0' }))).toBe(
      'http://127.0.0.1:16100/lovelace/0',
    );
    // 老数据/手改文件里可能没有前导斜杠，拼接时要兜住
    expect(urlOf(view({ scheme: 'http', open_path: 'lovelace/0' }))).toBe(
      'http://127.0.0.1:16100/lovelace/0',
    );
    // 空路径与 "/" 都等于根，不能拼出 "//"
    expect(urlOf(view({ scheme: 'http', open_path: '/' }))).toBe('http://127.0.0.1:16100/');
    expect(urlOf(view({ scheme: 'http', open_path: '' }))).toBe('http://127.0.0.1:16100/');
  });

  it('normalizes the local scheme and only web schemes open directly', () => {
    expect(schemeOf(view() as never)).toBe('tcp');
    expect(schemeOf(view({ scheme: 'HTTP' }) as never)).toBe('http');
    expect(schemeOf(view({ scheme: 'ssh' }) as never)).toBe('ssh');
    // 脏数据统一回退成 tcp，避免图标/按钮被一个拼错的字符串带偏
    expect(schemeOf(view({ scheme: 'websocket' }) as never)).toBe('tcp');
    expect(schemeLabel(view({ scheme: 'rdp' }) as never)).toBe('RDP');

    expect(canOpen(view({ scheme: 'http' }) as never)).toBe(true);
    expect(canOpen(view({ scheme: 'https' }) as never)).toBe(true);
    expect(canOpen(view({ scheme: 'ssh' }) as never)).toBe(false);
    expect(canOpen(view({ scheme: 'rdp' }) as never)).toBe(false);
  });

  it('shows the local endpoint, loopback for wildcard hosts', () => {
    expect(endpointLabel(view({ scheme: 'http' }) as never)).toBe('http://127.0.0.1:16100');
    expect(endpointLabel(view({ scheme: 'http', domain: 'ha.lan' }) as never)).toBe(
      'http://ha.lan:16100',
    );
    expect(endpointLabel(view({ scheme: 'tcp', local_host: '0.0.0.0' }) as never)).toBe(
      '127.0.0.1:16100',
    );
    expect(endpointLabel(view({ scheme: 'ssh' }) as never)).toBe('127.0.0.1:16100');
    expect(endpointLabel(view({ scheme: 'rdp' }) as never)).toBe('127.0.0.1:16100');
  });

  it('prefers the domain for non-web entries too', () => {
    // 配了域名后客户端连的是域名：回环地址只是内部实现，显示出来会误导
    expect(endpointLabel(view({ scheme: 'rdp', domain: 'mypc.zly.m' }) as never)).toBe(
      'mypc.zly.m:16100',
    );
    expect(endpointLabel(view({ scheme: 'ssh', domain: 'box.lan' }) as never)).toBe(
      'box.lan:16100',
    );
    // 443 是默认端口，写出来反而奇怪
    expect(
      endpointLabel(view({ scheme: 'tcp', domain: 'db.lan', local_port: 443 }) as never),
    ).toBe('db.lan');
  });

  it('derives the favicon from the local entry, never from a hardcoded list', () => {
    expect(faviconUrl(view({ scheme: 'http' }) as never)).toBe(
      'http://127.0.0.1:16100/favicon.ico',
    );
    // 域名入口优先：浏览器里实际访问的就是它
    expect(faviconUrl(view({ scheme: 'https', domain: 'ha.lan' }) as never)).toBe(
      'https://ha.lan:16100/favicon.ico',
    );
    // 非 web 没有可加载的图标，交给协议图标兜底
    expect(faviconUrl(view({ scheme: 'ssh' }) as never)).toBe('');
  });

  it('treats transitional states as active so the menu says stop', () => {
    expect(isActive('running')).toBe(true);
    expect(isActive('reconnecting')).toBe(true);
    expect(isActive('backoff')).toBe(true);
    expect(isActive('needs_trust')).toBe(true);
    expect(isActive('stopped')).toBe(false);
    expect(isActive('error')).toBe(false);
  });

  it('maps states to pill tones', () => {
    expect(pillOf('running')).toBe('is-on');
    expect(pillOf('error')).toBe('is-bad');
    expect(pillOf('connecting')).toBe('is-warn');
    expect(pillOf('stopped')).toBe('is-off');
  });

  it('describes an idle forward instead of saying 0 connections', () => {
    expect(connLabel(view())).toBe('暂无连接');
    expect(
      connLabel({ ...view(), status: { state: 'running', active_connections: 3 } }),
    ).toBe('累计 3 个连接');
  });

  it('searches name, note, ip and port', () => {
    const v = view() as never;
    expect(matchesQuery(v, ['HA', '测试面板'], 'ha')).toBe(true);
    expect(matchesQuery(v, ['HA', '测试面板'], '面板')).toBe(true);
    expect(matchesQuery(v, ['HA', '测试面板'], '192.0.2')).toBe(true);
    expect(matchesQuery(v, ['HA', '测试面板'], '16100')).toBe(true);
    expect(matchesQuery(v, ['HA', '测试面板'], '不存在')).toBe(false);
    // 空关键词等于不过滤
    expect(matchesQuery(v, ['HA'], '   ')).toBe(true);
  });
});
