/**
 * 最小 ZIP 读写。
 *
 * 只做 `.threadline` 包需要的那部分：
 * - 写入一律用 **stored**（包内是 PNG 与 JSON，PNG 已压缩过，再压一遍没收益，
 *   还省掉一个压缩库依赖）
 * - 读取同时支持 stored 与 **deflate**（别人可能用 7-Zip / 资源管理器重新打包）
 *
 * 不做 ZIP64：条目数与单文件大小都有上限（见 `profile.ts`），远达不到 4GB。
 */

const LOCAL_SIG = 0x04034b50;
const CENTRAL_SIG = 0x02014b50;
const EOCD_SIG = 0x06054b50;
const EOCD_SIZE = 22;
const UTF8_FLAG = 0x0800;

export interface ZipEntry {
  name: string;
  data: Uint8Array;
}

const CRC_TABLE = (() => {
  const table = new Uint32Array(256);
  for (let i = 0; i < 256; i += 1) {
    let c = i;
    for (let k = 0; k < 8; k += 1) {
      c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
    }
    table[i] = c >>> 0;
  }
  return table;
})();

export function crc32(data: Uint8Array): number {
  let c = 0xffffffff;
  for (let i = 0; i < data.length; i += 1) {
    c = CRC_TABLE[(c ^ data[i]) & 0xff] ^ (c >>> 8);
  }
  return (c ^ 0xffffffff) >>> 0;
}

function dosTime(d: Date): number {
  return (d.getHours() << 11) | (d.getMinutes() << 5) | Math.floor(d.getSeconds() / 2);
}

function dosDate(d: Date): number {
  // ZIP 的年份从 1980 起算
  const year = Math.max(0, d.getFullYear() - 1980);
  return (year << 9) | ((d.getMonth() + 1) << 5) | d.getDate();
}

/** 打包成 zip 字节流。 */
export function zip(entries: ZipEntry[], now = new Date()): Uint8Array {
  const encoder = new TextEncoder();
  const time = dosTime(now);
  const date = dosDate(now);
  const locals: Uint8Array[] = [];
  const centrals: Uint8Array[] = [];
  let offset = 0;

  for (const entry of entries) {
    const name = encoder.encode(entry.name);
    const crc = crc32(entry.data);
    const size = entry.data.length;

    const local = new Uint8Array(30 + name.length);
    const lv = new DataView(local.buffer);
    lv.setUint32(0, LOCAL_SIG, true);
    lv.setUint16(4, 20, true);
    lv.setUint16(6, UTF8_FLAG, true);
    lv.setUint16(8, 0, true); // stored
    lv.setUint16(10, time, true);
    lv.setUint16(12, date, true);
    lv.setUint32(14, crc, true);
    lv.setUint32(18, size, true);
    lv.setUint32(22, size, true);
    lv.setUint16(26, name.length, true);
    lv.setUint16(28, 0, true);
    local.set(name, 30);

    const central = new Uint8Array(46 + name.length);
    const cv = new DataView(central.buffer);
    cv.setUint32(0, CENTRAL_SIG, true);
    cv.setUint16(4, 20, true);
    cv.setUint16(6, 20, true);
    cv.setUint16(8, UTF8_FLAG, true);
    cv.setUint16(10, 0, true);
    cv.setUint16(12, time, true);
    cv.setUint16(14, date, true);
    cv.setUint32(16, crc, true);
    cv.setUint32(20, size, true);
    cv.setUint32(24, size, true);
    cv.setUint16(28, name.length, true);
    cv.setUint16(30, 0, true); // extra
    cv.setUint16(32, 0, true); // comment
    cv.setUint16(34, 0, true); // disk
    cv.setUint16(36, 0, true); // internal attrs
    cv.setUint32(38, 0, true); // external attrs
    cv.setUint32(42, offset, true);
    central.set(name, 46);

    locals.push(local, entry.data);
    centrals.push(central);
    offset += local.length + size;
  }

  const centralSize = centrals.reduce((n, c) => n + c.length, 0);
  const eocd = new Uint8Array(EOCD_SIZE);
  const ev = new DataView(eocd.buffer);
  ev.setUint32(0, EOCD_SIG, true);
  ev.setUint16(8, entries.length, true);
  ev.setUint16(10, entries.length, true);
  ev.setUint32(12, centralSize, true);
  ev.setUint32(16, offset, true);

  const out = new Uint8Array(offset + centralSize + EOCD_SIZE);
  let p = 0;
  for (const chunk of [...locals, ...centrals, eocd]) {
    out.set(chunk, p);
    p += chunk.length;
  }
  return out;
}

/** 解包上限：超出即拒绝，不给「解压炸弹」留把内存撑爆的机会。 */
export interface UnzipLimits {
  maxEntries?: number;
  /** 单个条目解压后的大小 */
  maxEntryBytes?: number;
  /** 所有条目解压后的总量 */
  maxTotalBytes?: number;
}

/**
 * 解包。条目顺序与包内一致；同名条目后者覆盖前者。
 *
 * 结构损坏时抛错——调用方据此提示「不是有效的配置文件」，
 * 不要把半解析结果继续往下传。
 */
export async function unzip(
  bytes: Uint8Array,
  limits: UnzipLimits = {},
): Promise<Map<string, Uint8Array>> {
  if (bytes.length < EOCD_SIZE) {
    throw new Error('不是有效的 .threadline 包');
  }
  const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
  const eocd = findEocd(view);
  const count = view.getUint16(eocd + 10, true);
  if (limits.maxEntries !== undefined && count > limits.maxEntries) {
    throw new Error(`压缩包条目太多（${count}）`);
  }
  let p = view.getUint32(eocd + 16, true);
  const out = new Map<string, Uint8Array>();
  let totalBytes = 0;

  for (let i = 0; i < count; i += 1) {
    if (p + 46 > bytes.length || view.getUint32(p, true) !== CENTRAL_SIG) {
      throw new Error('压缩包结构损坏');
    }
    const method = view.getUint16(p + 10, true);
    const size = view.getUint32(p + 20, true);
    // 中央目录里的「解压后大小」：先按它拦，避免真的解出来才检查
    const rawSize = view.getUint32(p + 24, true);
    if (limits.maxEntryBytes !== undefined && rawSize > limits.maxEntryBytes) {
      throw new Error('压缩包里有条目过大');
    }
    totalBytes += rawSize;
    if (limits.maxTotalBytes !== undefined && totalBytes > limits.maxTotalBytes) {
      throw new Error('压缩包解压后过大');
    }
    const nameLen = view.getUint16(p + 28, true);
    const extraLen = view.getUint16(p + 30, true);
    const commentLen = view.getUint16(p + 32, true);
    const localOffset = view.getUint32(p + 42, true);
    const name = new TextDecoder().decode(bytes.subarray(p + 46, p + 46 + nameLen));

    // 本地头里的 name/extra 长度可能与中央目录不同，必须以本地头为准
    if (localOffset + 30 > bytes.length) {
      throw new Error('压缩包结构损坏');
    }
    const localNameLen = view.getUint16(localOffset + 26, true);
    const localExtraLen = view.getUint16(localOffset + 28, true);
    const start = localOffset + 30 + localNameLen + localExtraLen;
    const raw = bytes.subarray(start, start + size);

    out.set(name, method === 0 ? raw : await inflateRaw(raw));
    p += 46 + nameLen + extraLen + commentLen;
  }

  return out;
}

function findEocd(view: DataView): number {
  const maxBack = Math.min(view.byteLength, 0xffff + EOCD_SIZE);
  for (let i = view.byteLength - EOCD_SIZE; i >= view.byteLength - maxBack; i -= 1) {
    if (view.getUint32(i, true) === EOCD_SIG) {
      return i;
    }
  }
  throw new Error('不是有效的 .threadline 包');
}

async function inflateRaw(data: Uint8Array): Promise<Uint8Array> {
  if (typeof DecompressionStream === 'undefined') {
    throw new Error('当前环境不支持解压这个包');
  }
  const stream = new Blob([data as BlobPart])
    .stream()
    .pipeThrough(new DecompressionStream('deflate-raw'));
  return new Uint8Array(await new Response(stream).arrayBuffer());
}
