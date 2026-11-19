import { describe, expect, it } from 'vitest';
import { applySchemeChange, type SchemeEditable } from './forwardForm';

function form(over: Partial<SchemeEditable> = {}): SchemeEditable {
  return { scheme: 'http', local_port: 80, remote_port: 8080, ...over };
}

describe('applySchemeChange', () => {
  it('writes the new scheme back into the form', () => {
    // 回归：原生 select 是单向绑定，曾经漏了这一句，表现为「选了没反应」
    const f = form();
    applySchemeChange(f, 'https');
    expect(f.scheme).toBe('https');
  });

  it('moves the default ports with the scheme', () => {
    const f = form({ scheme: 'http', local_port: 80, remote_port: 8080 });
    applySchemeChange(f, 'https');
    expect(f.local_port).toBe(443);
    expect(f.remote_port).toBe(443);
  });

  it('keeps a hand-typed remote port', () => {
    const f = form({ remote_port: 9999 });
    applySchemeChange(f, 'https');
    expect(f.remote_port).toBe(9999);
    expect(f.local_port).toBe(443);
  });

  it('gives rdp and ssh their own ports', () => {
    const rdp = form();
    applySchemeChange(rdp, 'rdp');
    expect([rdp.scheme, rdp.local_port, rdp.remote_port]).toEqual(['rdp', 3389, 3389]);

    const ssh = form();
    applySchemeChange(ssh, 'ssh');
    expect([ssh.scheme, ssh.local_port, ssh.remote_port]).toEqual(['ssh', 22, 22]);
  });

  it('falls back to the remote port for plain tcp', () => {
    const f = form({ remote_port: 5000 });
    applySchemeChange(f, 'tcp');
    expect(f.scheme).toBe('tcp');
    expect(f.local_port).toBe(5000);
  });
});
