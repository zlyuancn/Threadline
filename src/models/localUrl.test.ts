import { describe, expect, it } from 'vitest';
import { localUrl } from './localUrl';

describe('localUrl', () => {
  it('keeps an explicit loopback address', () => {
    expect(localUrl('127.0.0.1', 8123)).toBe('http://127.0.0.1:8123/');
  });

  it('maps the wildcard address to loopback', () => {
    // 把 0.0.0.0 原样丢给浏览器是打不开的——这是最容易写歪的一处。
    expect(localUrl('0.0.0.0', 16100)).toBe('http://127.0.0.1:16100/');
  });

  it('maps the IPv6 any-address to loopback', () => {
    expect(localUrl('::', 8080)).toBe('http://127.0.0.1:8080/');
    expect(localUrl('[::]', 8080)).toBe('http://127.0.0.1:8080/');
  });

  it('keeps a specific LAN address as-is', () => {
    expect(localUrl('192.0.2.5', 3000)).toBe('http://192.0.2.5:3000/');
  });

  it('keeps the given scheme', () => {
    // https 的转发点"打开本地页面"必须走 https，否则浏览器会当成明文的另一个服务。
    expect(localUrl('127.0.0.1', 8443, 'https')).toBe('https://127.0.0.1:8443/');
  });

  it('always ends with a slash so relative paths work', () => {
    expect(localUrl('127.0.0.1', 1).endsWith('/')).toBe(true);
  });
});
