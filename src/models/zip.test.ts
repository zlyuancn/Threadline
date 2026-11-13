import { deflateRawSync } from 'node:zlib';
import { describe, expect, it } from 'vitest';
import { crc32, unzip, zip } from './zip';

const utf8 = (s: string) => new TextEncoder().encode(s);

describe('zip', () => {
  it('round-trips text and binary entries', async () => {
    const bytes = zip([
      { name: 'profile.json', data: utf8('{"kind":"threadline.profile"}') },
      { name: 'icons/a1b2.png', data: new Uint8Array([0x89, 0x50, 0x4e, 0x47]) },
    ]);
    const out = await unzip(bytes);
    expect(new TextDecoder().decode(out.get('profile.json')!)).toBe(
      '{"kind":"threadline.profile"}',
    );
    expect([...out.get('icons/a1b2.png')!]).toEqual([0x89, 0x50, 0x4e, 0x47]);
  });

  it('writes a checksum that matches the reference CRC32', async () => {
    // 已知向量：ASCII "123456789" 的 CRC32 是 0xCBF43926
    expect(crc32(utf8('123456789'))).toBe(0xcbf43926);
    // 包里存的 crc 与之一致（zip 结构里的第 14 字节起）
    const bytes = zip([{ name: 'x.txt', data: utf8('123456789') }]);
    const view = new DataView(bytes.buffer);
    expect(view.getUint32(14, true)).toBe(0xcbf43926);
  });

  it('reads deflate entries produced by other tools', async () => {
    const out = await unzip(deflatedZip('profile.json', '{"a":1}'));
    expect(new TextDecoder().decode(out.get('profile.json')!)).toBe('{"a":1}');
  });

  it('rejects bytes that are not a zip at all', async () => {
    await expect(unzip(utf8('definitely not a zip'))).rejects.toThrow(/有效的/);
  });

  it('keeps an empty package readable', async () => {
    const out = await unzip(zip([]));
    expect(out.size).toBe(0);
  });
});

/** 手工拼一个 method=8 的包：模拟 7-Zip / 资源管理器重新打包后的文件。 */
function deflatedZip(name: string, text: string): Uint8Array {
  const raw = Buffer.from(text);
  const packed = deflateRawSync(raw);
  const nameBytes = Buffer.from(name, 'utf8');
  const crc = crc32(new Uint8Array(raw));

  const local = Buffer.alloc(30 + nameBytes.length);
  local.writeUInt32LE(0x04034b50, 0);
  local.writeUInt16LE(20, 4);
  local.writeUInt16LE(0x0800, 6);
  local.writeUInt16LE(8, 8);
  local.writeUInt32LE(crc, 14);
  local.writeUInt32LE(packed.length, 18);
  local.writeUInt32LE(raw.length, 22);
  local.writeUInt16LE(nameBytes.length, 26);
  nameBytes.copy(local, 30);

  const central = Buffer.alloc(46 + nameBytes.length);
  central.writeUInt32LE(0x02014b50, 0);
  central.writeUInt16LE(20, 4);
  central.writeUInt16LE(20, 6);
  central.writeUInt16LE(0x0800, 8);
  central.writeUInt16LE(8, 10);
  central.writeUInt32LE(crc, 16);
  central.writeUInt32LE(packed.length, 20);
  central.writeUInt32LE(raw.length, 24);
  central.writeUInt16LE(nameBytes.length, 28);
  central.writeUInt32LE(0, 42);
  nameBytes.copy(central, 46);

  const cdOffset = local.length + packed.length;
  const eocd = Buffer.alloc(22);
  eocd.writeUInt32LE(0x06054b50, 0);
  eocd.writeUInt16LE(1, 8);
  eocd.writeUInt16LE(1, 10);
  eocd.writeUInt32LE(central.length, 12);
  eocd.writeUInt32LE(cdOffset, 16);

  return new Uint8Array(Buffer.concat([local, packed, central, eocd]));
}
