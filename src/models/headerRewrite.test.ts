import { describe, expect, it } from 'vitest';
import {
  defaultHeaderRewrite,
  hostModeOf,
  isDefaultRewrite,
  previewRewrite,
  toDtoRewrite,
  toFormRewrite,
} from './headerRewrite';

const ctx = {
  entryHost: 'home.example.com',
  entryPort: 443,
  scheme: 'https',
  remoteScheme: 'http',
  remoteHost: '127.0.0.1',
  remotePort: 3080,
};

describe('previewRewrite', () => {
  it('默认：Host 换远端地址、补 X-Forwarded、回写 Location', () => {
    const lines = previewRewrite(defaultHeaderRewrite(), ctx);
    expect(lines[0]).toMatchObject({
      label: 'Host',
      from: 'home.example.com',
      to: '127.0.0.1:3080',
      off: false,
    });
    expect(lines[1]).toMatchObject({ label: 'X-Forwarded-Proto', to: 'https', off: false });
    expect(lines[2]).toMatchObject({ label: 'X-Forwarded-Host', to: 'home.example.com' });
    expect(lines[3]).toMatchObject({
      label: 'Location',
      from: '127.0.0.1:3080/…',
      to: 'home.example.com/…',
      off: false,
    });
  });

  it('保留浏览器原 Host 时，from 与 to 相同', () => {
    const rule = { ...defaultHeaderRewrite(), host_mode: 'keep' as const, host: '' };
    const host = previewRewrite(rule, ctx)[0];
    expect(host.from).toBe(host.to);
    expect(host.to).toBe('home.example.com');
  });

  it('自定义 Host 原样显示', () => {
    const rule = { ...defaultHeaderRewrite(), host_mode: 'custom' as const, host: 'edge.example.com' };
    expect(previewRewrite(rule, ctx)[0].to).toBe('edge.example.com');
  });

  it('选了自定义却没填：按自动算，预览不能显示成「保留」', () => {
    const rule = { ...defaultHeaderRewrite(), host_mode: 'custom' as const, host: '   ' };
    expect(previewRewrite(rule, ctx)[0].to).toBe('127.0.0.1:3080');
  });

  it('总开关关闭：预览照实显示「原样透传」，不能谎报改写', () => {
    const lines = previewRewrite(defaultHeaderRewrite(), { ...ctx, enabled: false });
    expect(lines[0].note).toContain('原样透传');
    expect(lines[0].note).toContain('home.example.com');
    // 浏览器本来就不发 X-Forwarded-*，用一句话说清楚，不画箭头误导成「远端收到空值」
    expect(lines[1]).toMatchObject({ label: 'X-Forwarded-Proto', off: true });
    expect(lines[1].note).toContain('不加');
    expect(lines[3].note).toContain('不改写');
  });

  it('关掉两个开关后对应行标记为 off', () => {
    const rule = { ...defaultHeaderRewrite(), forward_headers: false, rewrite_location: false };
    const lines = previewRewrite(rule, ctx);
    expect(lines[1].off).toBe(true);
    expect(lines[2].off).toBe(true);
    expect(lines[3].off).toBe(true);
    expect(lines[0].off).toBe(false);
  });

  it('自定义头追加在最后，空名不显示', () => {
    const rule = {
      ...defaultHeaderRewrite(),
      extra: [
        { name: 'X-Real-IP', value: '1.2.3.4' },
        { name: '   ', value: 'ignored' },
      ],
    };
    const lines = previewRewrite(rule, ctx);
    expect(lines).toHaveLength(5);
    expect(lines[4]).toMatchObject({ label: 'X-Real-IP', to: '1.2.3.4', off: false });
  });
});

describe('表单与契约互转', () => {
  it('去掉空名的自定义头并 trim', () => {
    const form = defaultHeaderRewrite();
    form.extra = [
      { name: ' X-Real-IP ', value: ' 1.2.3.4 ' },
      { name: '  ', value: 'x' },
    ];
    expect(toDtoRewrite(form).extra).toEqual([{ name: 'X-Real-IP', value: '1.2.3.4' }]);
  });

  it('三种 Host 模式各自落到契约里的 null / 空串 / 具体值', () => {
    const auto = toDtoRewrite(defaultHeaderRewrite());
    expect(auto.host).toBeNull();

    const keep = toDtoRewrite({ ...defaultHeaderRewrite(), host_mode: 'keep', host: '' });
    expect(keep.host).toBe('');

    const custom = toDtoRewrite({
      ...defaultHeaderRewrite(),
      host_mode: 'custom',
      host: ' edge.example.com ',
    });
    expect(custom.host).toBe('edge.example.com');
  });

  it('选了自定义却没填：存成 null（自动），而不是空串（保留）', () => {
    const dto = toDtoRewrite({ ...defaultHeaderRewrite(), host_mode: 'custom', host: '  ' });
    expect(dto.host).toBeNull();
    expect(isDefaultRewrite({ ...defaultHeaderRewrite(), host_mode: 'custom', host: '' })).toBe(
      true,
    );
  });

  it('老配置缺字段时按默认补齐', () => {
    const form = toFormRewrite({ host: '', forward_headers: false });
    expect(hostModeOf(form)).toBe('keep');
    expect(form.forward_headers).toBe(false);
    expect(form.rewrite_location).toBe(true);
    expect(form.extra).toEqual([]);
    expect(isDefaultRewrite(form)).toBe(false);
  });

  it('从契约值能反推出模式', () => {
    expect(hostModeOf(toFormRewrite(undefined))).toBe('auto');
    expect(hostModeOf(toFormRewrite({ host: '' }))).toBe('keep');
    expect(hostModeOf(toFormRewrite({ host: 'edge.example.com' }))).toBe('custom');
    expect(hostModeOf(toFormRewrite({ host: null }))).toBe('auto');
  });
});
