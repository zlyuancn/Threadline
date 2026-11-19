import { describe, expect, it } from 'vitest';
import { clockText, formatBytes, formatKb, formatUptime, levelClass, percent, pickableServers, rateText, usagePercent } from './serverAdmin';

describe('formatBytes', () => {
  it('keeps bytes as integers', () => {
    expect(formatBytes(512)).toBe('512 B');
  });

  it('steps up units at 1024', () => {
    expect(formatBytes(1024)).toBe('1.0 KB');
    expect(formatBytes(1536)).toBe('1.5 KB');
    expect(formatBytes(1024 * 1024 * 3)).toBe('3.0 MB');
    expect(formatBytes(1024 ** 4)).toBe('1.0 TB');
  });

  it('treats missing or negative values as zero', () => {
    expect(formatBytes(0)).toBe('0 B');
    expect(formatBytes(-1)).toBe('0 B');
    expect(formatBytes(Number.NaN)).toBe('0 B');
  });
});

describe('formatKb / rateText', () => {
  it('converts meminfo kilobytes to readable size', () => {
    expect(formatKb(16 * 1024 * 1024)).toBe('16.0 GB');
  });

  it('prints zero rate instead of an empty cell', () => {
    expect(rateText(0)).toBe('0 B/s');
    expect(rateText(2048)).toBe('2.00 KB/s');
  });
});

describe('formatUptime', () => {
  it('keeps seconds so the number visibly ticks every second', () => {
    expect(formatUptime(3 * 86400 + 4 * 3600 + 59)).toBe('3 天 4 小时 0 分 59 秒');
  });

  it('falls back to hours then minutes', () => {
    expect(formatUptime(3600 * 5 + 60)).toBe('5 小时 1 分 0 秒');
    expect(formatUptime(90)).toBe('1 分 30 秒');
  });

  it('shows a dash when unknown', () => {
    expect(formatUptime(0)).toBe('—');
    expect(formatUptime(Number.NaN)).toBe('—');
  });
});

describe('percent helpers', () => {
  it('clamps and rounds', () => {
    expect(percent(34.567)).toBe(34.6);
    expect(percent(-5)).toBe(0);
    expect(percent(180)).toBe(100);
    expect(percent(Number.NaN)).toBe(0);
  });

  it('computes usage without dividing by zero', () => {
    expect(usagePercent(50, 100)).toBe(50);
    expect(usagePercent(5, 0)).toBe(0);
  });

  it('maps levels to the shared thresholds', () => {
    expect(levelClass(10)).toBe('is-ok');
    expect(levelClass(75)).toBe('is-warn');
    expect(levelClass(95)).toBe('is-bad');
  });
});

describe('clockText', () => {
  it('formats an iso timestamp to local hh:mm:ss', () => {
    const text = clockText('2026-11-16T06:07:08.000Z');
    expect(text).toMatch(/^\d{2}:\d{2}:\d{2}$/);
  });

  it('returns the input when unparsable', () => {
    expect(clockText('not-a-date')).toBe('not-a-date');
  });
});

describe('pickableServers', () => {
  const servers = [
    { id: 'deva', name: 'deva', host: '192.168.28.98' },
    { id: 'local', name: '本机', host: '127.0.0.1', kind: 'local' },
    { id: 'devb', name: 'devb', host: '10.0.0.5' },
  ];

  it('drops the built-in local entry', () => {
    expect(pickableServers(servers).map((s) => s.id)).toEqual(['deva', 'devb']);
  });

  it('keeps the caller order given by the sidebar', () => {
    const reordered = [servers[2], servers[0], servers[1]];
    expect(pickableServers(reordered).map((s) => s.id)).toEqual(['devb', 'deva']);
  });

  it('filters by name or host, ignoring case and surrounding spaces', () => {
    expect(pickableServers(servers, ' DEV ').map((s) => s.id)).toEqual(['deva', 'devb']);
    expect(pickableServers(servers, '10.0.0').map((s) => s.id)).toEqual(['devb']);
    expect(pickableServers(servers, '192.168').map((s) => s.id)).toEqual(['deva']);
  });

  it('returns every ssh server for a blank keyword', () => {
    expect(pickableServers(servers, '   ').map((s) => s.id)).toEqual(['deva', 'devb']);
  });
});
