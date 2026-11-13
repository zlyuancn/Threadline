import { describe, expect, it } from 'vitest';
import { PROTOCOL_GLYPHS, protocolGlyph, resolveIconSrc } from './forwardIcon';

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

describe('protocolGlyph', () => {
  it('covers every protocol the UI accepts', () => {
    // 卡片与磁贴共用这张表：漏一个协议，那一类转发就会退回别的图标
    expect(Object.keys(PROTOCOL_GLYPHS).sort()).toEqual(['http', 'https', 'rdp', 'ssh', 'tcp']);
    for (const glyph of Object.values(PROTOCOL_GLYPHS)) {
      expect(glyph.startsWith('<svg')).toBe(true);
    }
  });

  it('draws rdp and ssh differently from a bare tcp forward', () => {
    // RDP 必须是显示器：和 tcp 共用「两条线」正是首页与磁贴图标打架的原因
    expect(protocolGlyph('rdp')).not.toBe(protocolGlyph('tcp'));
    expect(protocolGlyph('ssh')).not.toBe(protocolGlyph('tcp'));
    expect(protocolGlyph('rdp')).not.toBe(protocolGlyph('ssh'));
  });

  it('uses the same glyph for plain http and https', () => {
    expect(protocolGlyph('https')).toBe(protocolGlyph('http'));
  });
});
