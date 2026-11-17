import { describe, expect, it } from 'vitest';
import {
  allowedActions,
  levelClass,
  memDetail,
  memPercent,
  memText,
  shortImage,
  splitImage,
  trafficPair,
} from './containers';
import type { ContainerItem } from '../api/types';

function item(over: Partial<ContainerItem> = {}): ContainerItem {
  return {
    id: 'a1f93c2e7b10',
    name: 'immich_server',
    image: 'ghcr.io/immich-app/immich-server:v1.119.0',
    state: 'running',
    status: 'Up 3 days (healthy)',
    created_at: '2026-11-03 21:04',
    restarts: 0,
    ports: [],
    mounts: [],
    networks: [],
    cpu_percent: 0,
    mem_bytes: 0,
    mem_limit_bytes: 0,
    net_rx_bytes: 0,
    net_tx_bytes: 0,
    ...over,
  };
}

// 这张表决定菜单给哪几项，核心的 `containers.rs::allowed_actions` 决定请求放不放行。
// 两头不一致时，用户看到的是「菜单里有、点了报错」——所以这里逐条钉死。
describe('allowedActions', () => {
  it('offers stop and restart for a running container', () => {
    expect(allowedActions('running')).toEqual(['restart', 'stop']);
  });

  it('only offers stop for a container paused from the CLI', () => {
    // 本产品不做暂停：被外部冻住的容器只能停掉再启动，不给「继续」
    expect(allowedActions('paused')).toEqual(['stop']);
  });

  it('only allows stop while restarting', () => {
    expect(allowedActions('restarting')).toEqual(['stop']);
  });

  it('treats exited / created / unknown as startable', () => {
    expect(allowedActions('exited')).toEqual(['start']);
    expect(allowedActions('created')).toEqual(['start']);
    expect(allowedActions('dead')).toEqual(['start']);
    expect(allowedActions('')).toEqual(['start']);
  });
});

describe('splitImage', () => {
  it('strips the registry and keeps the tag separate', () => {
    expect(splitImage('ghcr.io/immich-app/immich-server:v1.119.0')).toEqual({
      repo: 'immich-app/immich-server',
      tag: ':v1.119.0',
    });
  });

  it('keeps a tagless image whole', () => {
    expect(splitImage('nginx')).toEqual({ repo: 'nginx', tag: '' });
    expect(splitImage('immich-app/immich-server')).toEqual({
      repo: 'immich-app/immich-server',
      tag: '',
    });
  });

  it('does not mistake a registry port for a tag', () => {
    // `registry:5000/app` 里的冒号属于 registry，不是 tag 分隔符
    expect(splitImage('registry:5000/app')).toEqual({ repo: 'app', tag: '' });
    expect(splitImage('localhost:5000/app:v1')).toEqual({ repo: 'app', tag: ':v1' });
  });

  it('keeps local images without a registry dotted host', () => {
    expect(splitImage('myimg/app:v2')).toEqual({ repo: 'myimg/app', tag: ':v2' });
  });
});

describe('shortImage', () => {
  it('drops the path but keeps the tag', () => {
    expect(shortImage('ghcr.io/immich-app/immich-server:v1.119.0')).toBe(
      'immich-server:v1.119.0',
    );
  });

  it('handles tagless images', () => {
    expect(shortImage('nginx')).toBe('nginx');
  });
});

describe('memory display', () => {
  it('shows a dash when the container reports nothing', () => {
    expect(memText(item())).toBe('—');
    expect(memDetail(item())).toBe('—');
    // 上限为 0 是 docker 的「不限制」，不该画成 0% 的空条
    expect(memPercent(item())).toBe(0);
  });

  it('prints usage with its limit when both are known', () => {
    const row = item({ mem_bytes: 1024 * 1024, mem_limit_bytes: 1024 * 1024 * 2 });
    expect(memText(row)).toBe('1.0 MB');
    expect(memDetail(row)).toBe('1.0 MB / 2.0 MB');
    expect(memPercent(row)).toBe(50);
  });

  it('prints usage alone when the limit is unknown', () => {
    const row = item({ mem_bytes: 1024, mem_limit_bytes: 0 });
    expect(memDetail(row)).toBe('1.0 KB');
    expect(memPercent(row)).toBe(0);
  });

  it('rounds the percentage to one decimal and caps at 100', () => {
    expect(memPercent(item({ mem_bytes: 1, mem_limit_bytes: 3 }))).toBe(33.3);
    expect(memPercent(item({ mem_bytes: 500, mem_limit_bytes: 100 }))).toBe(100);
  });
});

describe('levelClass', () => {
  it('matches the warning / danger thresholds used elsewhere', () => {
    expect(levelClass(0)).toBe('is-ok');
    expect(levelClass(69.9)).toBe('is-ok');
    expect(levelClass(70)).toBe('is-warn');
    expect(levelClass(89.9)).toBe('is-warn');
    expect(levelClass(90)).toBe('is-bad');
  });
});

describe('trafficPair', () => {
  it('keeps one unit for the pair to save width', () => {
    expect(trafficPair(0, 0)).toBe('↑0 ↓0 B');
    expect(trafficPair(1024, 512)).toBe('↑0.5 ↓1.0 KB');
    expect(trafficPair(1024 * 1024 * 2, 1024 * 1024)).toBe('↑1.0 ↓2.0 MB');
  });

  it('uplink first, downlink second', () => {
    // 参数是 (rx, tx)：界面上 ↑ 是发出去，↓ 是收进来
    expect(trafficPair(100, 200)).toBe('↑200 ↓100 B');
  });
});
