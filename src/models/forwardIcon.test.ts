import { describe, expect, it } from 'vitest';
import { resolveIconSrc } from './forwardIcon';

const FAVICON = 'https://example.com/favicon.ico';

describe('forwardIcon', () => {
  it('prefers the custom icon over the site favicon', () => {
    // 自定义图标是用户明确选的，跟运行状态无关，永远优先
    expect(resolveIconSrc('data:image/png;base64,AA', true, true, FAVICON)).toBe(
      'data:image/png;base64,AA',
    );
    expect(resolveIconSrc('data:image/png;base64,AA', true, false, FAVICON)).toBe(
      'data:image/png;base64,AA',
    );
  });

  it('treats a blank custom icon as unset', () => {
    expect(resolveIconSrc('   ', true, true, FAVICON)).toBe(FAVICON);
    expect(resolveIconSrc('', true, true, FAVICON)).toBe(FAVICON);
  });

  it('loads the site favicon only for a running web forward', () => {
    expect(resolveIconSrc(undefined, true, true, FAVICON)).toBe(FAVICON);
    // 停止时去请求只会拿到错误页，不如图标回退
    expect(resolveIconSrc(undefined, true, false, FAVICON)).toBe('');
    // 非 web 没有 favicon 这个概念
    expect(resolveIconSrc(undefined, false, true, '')).toBe('');
  });
});
