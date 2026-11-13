import { describe, expect, it, vi } from 'vitest';
import {
  applyPreferences,
  diffPreferences,
  type PreferenceApplier,
} from './preferences';

const current = { theme: 'jade', hotkey: 'Ctrl+Alt+T' };

function applier(overrides: Partial<PreferenceApplier> = {}): PreferenceApplier {
  return {
    applyTheme: vi.fn(),
    currentHotkey: () => current.hotkey,
    rebindHotkey: vi.fn(async () => null),
    ...overrides,
  };
}

describe('diffPreferences', () => {
  it('lists nothing when the package carries no preferences', () => {
    expect(diffPreferences(current, undefined)).toEqual([]);
  });

  it('lists only what actually changes', () => {
    expect(diffPreferences(current, { ...current })).toEqual([]);
    const lines = diffPreferences(current, { theme: 'ocean', hotkey: current.hotkey });
    expect(lines).toHaveLength(1);
    expect(lines[0].key).toBe('theme');
    // 显示的是中文名而不是 id：用户不认识 `ink`
    expect(lines[0].label).toContain('→');
    expect(lines[0].label).not.toContain('ocean');
  });

  it('shows both lines when both change', () => {
    const lines = diffPreferences(current, { theme: 'ocean', hotkey: 'Ctrl+Shift+T' });
    expect(lines.map((line) => line.key)).toEqual(['theme', 'hotkey']);
    expect(lines[1].label).toContain('Ctrl+Shift+T');
  });
});

describe('applyPreferences', () => {
  it('applies a known theme and a working hotkey', async () => {
    const deps = applier();
    const failures = await applyPreferences({ theme: 'ocean', hotkey: 'Ctrl+Shift+T' }, deps);
    expect(failures).toEqual([]);
    expect(deps.applyTheme).toHaveBeenCalledWith('ocean');
  });

  it('skips an unknown theme instead of blanking the UI', async () => {
    const deps = applier();
    const failures = await applyPreferences({ theme: 'nope', hotkey: '' }, deps);
    expect(deps.applyTheme).not.toHaveBeenCalled();
    expect(failures[0]).toContain('本机没有');
  });

  it('keeps the old hotkey when rebinding fails', async () => {
    // 被别的程序占用时不能把用户的呼出键改成一个按不出来的键
    const deps = applier({ rebindHotkey: vi.fn(async () => '已被其他程序占用') });
    const failures = await applyPreferences(
      { theme: '', hotkey: 'F12' },
      deps,
    );
    expect(failures[0]).toContain('已');
    expect(deps.rebindHotkey).toHaveBeenCalled();
  });

  it('does not rebind when the hotkey is unchanged', async () => {
    const deps = applier();
    const failures = await applyPreferences({ theme: '', hotkey: current.hotkey }, deps);
    expect(failures).toEqual([]);
    expect(deps.rebindHotkey).not.toHaveBeenCalled();
  });
});
