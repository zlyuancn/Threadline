import { describe, expect, it } from 'vitest';
import type { ServerDto, TunnelDto } from '../api/types';
import {
  buildProfile,
  packageFileName,
  readPackage,
  shortHash,
  writePackage,
} from './profile';
import { unzip, zip } from './zip';

const servers: ServerDto[] = [
  {
    id: 'srv_a',
    name: 'mypc',
    host: '192.168.28.213',
    port: 22,
    username: 'root',
    credential_kind: 'password',
    host_key_fingerprint: 'SHA256:known',
    keepalive_secs: 30,
    enabled: true,
  },
  {
    id: 'srv_b',
    name: 'nas',
    host: '192.168.28.9',
    port: 22,
    username: 'admin',
    host_key_fingerprint: null,
    keepalive_secs: 30,
    enabled: false,
  },
];

const forwards: TunnelDto[] = [
  {
    id: 'fwd_rdp',
    name: 'mypc远程桌面',
    server_id: 'srv_a',
    local_host: '127.0.0.1',
    local_port: 3389,
    remote_host: '192.168.28.213',
    remote_port: 3389,
    auto_start: true,
    scheme: 'rdp',
    open_path: '/dash',
    in_app: false,
  },
  {
    id: 'fwd_web',
    name: 'nas面板',
    server_id: 'srv_b',
    local_host: '127.0.0.1',
    local_port: 5000,
    remote_host: '192.168.28.9',
    remote_port: 5000,
    auto_start: false,
    scheme: 'http',
    in_app: true,
  },
];

const PNG = 'data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==';

describe('buildProfile', () => {
  it('brings the whole server when the server is selected', () => {
    const built = buildProfile({
      servers,
      forwards,
      serverIds: ['srv_a'],
      forwardIds: [],
      includeIcons: true,
      includeFingerprint: true,
    });
    expect(built.document.servers).toHaveLength(1);
    expect(built.document.servers[0].forwards.map((f) => f.id)).toEqual(['fwd_rdp']);
  });

  it('pulls in the owning server when only a forward is selected', () => {
    const built = buildProfile({
      servers,
      forwards,
      serverIds: [],
      forwardIds: ['fwd_web'],
      includeIcons: true,
      includeFingerprint: true,
    });
    expect(built.document.servers.map((s) => s.id)).toEqual(['srv_b']);
    expect(built.document.servers[0].forwards.map((f) => f.id)).toEqual(['fwd_web']);
  });

  it('deduplicates a forward selected twice by server and by itself', () => {
    const built = buildProfile({
      servers,
      forwards,
      serverIds: ['srv_a'],
      forwardIds: ['fwd_rdp'],
      includeIcons: true,
      includeFingerprint: true,
    });
    expect(built.document.servers[0].forwards).toHaveLength(1);
  });

  it('drops fingerprints and icons when the switches are off', () => {
    const built = buildProfile({
      servers: [{ ...servers[0] }],
      forwards: [{ ...forwards[0], icon: PNG }],
      serverIds: ['srv_a'],
      forwardIds: [],
      includeIcons: false,
      includeFingerprint: false,
    });
    expect(built.document.servers[0].host_key_fingerprint).toBeNull();
    expect(built.document.servers[0].forwards[0].icon).toBe('');
    expect(built.icons.size).toBe(0);
  });

  it('stores a data-uri icon as a flat file and shares it between identical icons', () => {
    const built = buildProfile({
      servers: [{ ...servers[0] }],
      forwards: [
        { ...forwards[0], icon: PNG },
        { ...forwards[0], id: 'fwd_copy', icon: PNG },
      ],
      serverIds: ['srv_a'],
      forwardIds: [],
      includeIcons: true,
      includeFingerprint: true,
    });
    const iconPaths = built.document.servers[0].forwards.map((f) => f.icon);
    expect(iconPaths[0]).toMatch(/^icons\/[a-z0-9]{7}\.png$/);
    expect(iconPaths[1]).toBe(iconPaths[0]);
    expect(built.icons.size).toBe(1);
  });

  it('keeps an http icon as-is instead of downloading it', () => {
    const built = buildProfile({
      servers: [{ ...servers[0] }],
      forwards: [{ ...forwards[0], icon: 'https://example.com/logo.png' }],
      serverIds: ['srv_a'],
      forwardIds: [],
      includeIcons: true,
      includeFingerprint: true,
    });
    expect(built.document.servers[0].forwards[0].icon).toBe('https://example.com/logo.png');
    expect(built.icons.size).toBe(0);
  });
});

describe('readPackage', () => {
  const roundtrip = async (built = sample()) => {
    const bytes = writePackage(built);
    return readPackage(bytes);
  };

  const sample = () =>
    buildProfile({
      servers,
      forwards: [{ ...forwards[0], icon: PNG }],
      serverIds: ['srv_a'],
      forwardIds: [],
      includeIcons: true,
      includeFingerprint: true,
      preferences: { theme: 'jade', hotkey: 'Ctrl+Alt+T' },
    });

  it('round-trips structure, icons and preferences', async () => {
    const parsed = await roundtrip();
    const forward = parsed.document.servers[0].forwards[0];
    expect(forward.open_path).toBe('/dash');
    expect(forward.auto_start).toBe(true);
    expect(parsed.icons.get(forward.icon!)).toBe(PNG);
    expect(parsed.document.preferences?.theme).toBe('jade');
  });

  it('rejects a wrong kind or a future version', async () => {
    const built = sample();
    await expect(
      readPackage(writePackage({ ...built, document: { ...built.document, kind: 'other' as never } })),
    ).rejects.toThrow(/不是 Threadline/);
    await expect(
      readPackage(writePackage({ ...built, document: { ...built.document, version: 99 } })),
    ).rejects.toThrow(/版本不支持/);
  });

  it('drops an oversized icon and clears the reference', async () => {
    // 128KB 以上的图标不该进内存，更不该让条目本身失败
    const big = `data:image/png;base64,${btoa('x'.repeat(200 * 1024))}`;
    const built = buildProfile({
      servers: [{ ...servers[0] }],
      forwards: [{ ...forwards[0], icon: big }],
      serverIds: ['srv_a'],
      forwardIds: [],
      includeIcons: true,
      includeFingerprint: true,
    });
    // buildProfile 已经在打包前丢掉它
    expect(built.document.servers[0].forwards[0].icon).toBe('');

    const parsed = await readPackage(writePackage(built));
    expect(parsed.document.servers[0].forwards[0].icon).toBe('');
  });

  it('ignores entries outside icons/ and nested icon paths', async () => {
    const built = sample();
    const bytes = writePackage(built);
    // 正常包不会带这些东西；模拟别人手工往包里塞
    const files = await unzip(bytes);
    files.set('evil.exe', new Uint8Array([1, 2, 3]));
    files.set('icons/../escape.png', new Uint8Array([1, 2, 3]));
    const tampered = zip([...files].map(([name, data]) => ({ name, data })));
    const parsed = await readPackage(tampered);
    expect([...parsed.icons.keys()].every((name) => !name.includes('..'))).toBe(true);
  });

  it('refuses a file that is not a package', async () => {
    await expect(readPackage(new TextEncoder().encode('hello'))).rejects.toThrow();
  });
});

describe('naming helpers', () => {
  it('names the file by date', () => {
    expect(packageFileName(new Date(2026, 10, 13))).toBe('2026-11-13-threadline.threadline');
  });

  it('hashes stably and within 7 chars', () => {
    expect(shortHash('abc')).toBe(shortHash('abc'));
    expect(shortHash('abc')).not.toBe(shortHash('abd'));
    expect(shortHash('abc')).toHaveLength(7);
  });
});
