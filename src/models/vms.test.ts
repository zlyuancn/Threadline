import { describe, expect, it } from 'vitest';
import {
  allowedActions,
  dataDisks,
  diskTotalBytes,
  formatKib,
  memoryPercent,
  splitKib,
  stateLabel,
  stateTone,
} from './vms';
import type { VmItem } from '../api/types';

function item(over: Partial<VmItem> = {}): VmItem {
  return {
    name: 'win11-dev',
    id: 12,
    uuid: '8f2c1111-2222-3333-4444-555566667777',
    state: 'running',
    vcpu: 8,
    memory_kib: 16 * 1024 * 1024,
    memory_used_kib: 8 * 1024 * 1024,
    cpu_time_ns: 0,
    autostart: true,
    persistent: true,
    os_type: 'hvm',
    disks: [
      {
        target: 'vda',
        device: 'disk',
        source: '/mnt/vm/win11.qcow2',
        capacity_bytes: 120 * 1024 ** 3,
        allocation_bytes: 60 * 1024 ** 3,
        format: 'qcow2',
      },
    ],
    nets: [],
    net_rx_bytes: 0,
    net_tx_bytes: 0,
    ...over,
  };
}

// 这张表决定菜单给哪几项，核心的 `vms.rs::allowed_actions` 决定请求放不放行。
// 两头不一致时，用户看到的是「菜单里有、点了报错」——所以这里逐条钉死。
describe('allowedActions', () => {
  it('offers shutdown / reboot / suspend / destroy while running', () => {
    expect(allowedActions('running')).toEqual(['shutdown', 'reboot', 'suspend', 'destroy']);
  });

  it('only offers resume and destroy when paused', () => {
    expect(allowedActions('paused')).toEqual(['resume', 'destroy']);
  });

  it('only offers start when shut off', () => {
    expect(allowedActions('shut off')).toEqual(['start']);
  });

  it('treats crashed and pmsuspended as clean-up only', () => {
    expect(allowedActions('crashed')).toEqual(['destroy']);
    expect(allowedActions('pmsuspended')).toEqual(['destroy']);
  });

  it('falls back to start for states we do not know', () => {
    expect(allowedActions('')).toEqual(['start']);
    expect(allowedActions('some-new-state')).toEqual(['start']);
  });
});

describe('memoryPercent', () => {
  it('computes used over allocated', () => {
    expect(memoryPercent(item())).toBeCloseTo(50, 5);
  });

  it('returns 0 when nothing was sampled', () => {
    expect(memoryPercent(item({ memory_used_kib: 0 }))).toBe(0);
  });

  it('clamps balloon overshoot to 100', () => {
    expect(memoryPercent(item({ memory_used_kib: 20 * 1024 * 1024 }))).toBe(100);
  });
});

describe('size text', () => {
  it('splits a size into number and unit for the ring', () => {
    expect(splitKib(16 * 1024 * 1024)).toEqual(['16.0', 'GB']);
  });

  it('keeps MiB-sized values readable', () => {
    expect(formatKib(512 * 1024)).toBe('512.0 MB');
  });
});

describe('disks', () => {
  it('leaves cdrom out of the data disks', () => {
    const vm = item({
      disks: [
        ...item().disks,
        {
          target: 'sda',
          device: 'cdrom',
          source: '/mnt/iso/win.iso',
          capacity_bytes: 5 * 1024 ** 3,
          allocation_bytes: 0,
          format: 'raw',
        },
      ],
    });
    expect(dataDisks(vm)).toHaveLength(1);
    expect(diskTotalBytes(vm)).toBe(120 * 1024 ** 3);
  });
});

describe('state wording', () => {
  it('translates libvirt states', () => {
    expect(stateLabel('running')).toBe('运行中');
    expect(stateLabel('shut off')).toBe('已关机');
    expect(stateLabel('crashed')).toBe('已崩溃');
  });

  it('maps states to tones', () => {
    expect(stateTone('running')).toBe('ok');
    expect(stateTone('paused')).toBe('warn');
    expect(stateTone('crashed')).toBe('bad');
    expect(stateTone('shut off')).toBe('muted');
  });
});
