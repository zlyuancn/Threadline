/**
 * 通配地址折成回环地址。
 *
 * 监听 `0.0.0.0` / `::` 时，浏览器或客户端该敲 `127.0.0.1`——把通配地址
 * 直接交出去是连不上的，而这个转换很容易在界面里各写一份、写歪。
 */
export function loopbackHost(host: string): string {
  const wildcard = host === '' || host === '0.0.0.0' || host === '::' || host === '[::]';
  return wildcard ? '127.0.0.1' : host;
}

/** 「打开本地页面」用的地址。 */
export function localUrl(host: string, port: number, scheme: 'http' | 'https' = 'http'): string {
  return `${scheme}://${loopbackHost(host)}:${port}/`;
}
