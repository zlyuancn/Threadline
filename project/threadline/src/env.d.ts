/// <reference types="vite/client" />

/**
 * 由 `vite.config.ts` 的 `define` 注入，值取自 `package.json` 的 `version`。
 *
 * 版本号只有这一个真相：界面角落与导出包的元数据都读它，
 * 免得发版时漏掉某个写死的字符串。
 */
declare const __APP_VERSION__: string;
