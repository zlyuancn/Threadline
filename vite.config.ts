import { defineConfig } from 'vitest/config';
import vue from '@vitejs/plugin-vue';
import pkg from './package.json';

// 开发模式：`/api` 转发到本机核心（端口可能顺延，用 TL_DEV_URL 覆盖）。
// 打包后：Tauri 注入 `window.__THREADLINE__ = { baseUrl, token }`，不走代理。
export default defineConfig({
  plugins: [vue()],
  // 版本号只有一处真相：package.json。界面角落与导出包的元数据都用它，
  // 免得发版时忘了改另一个写死的 "0.1.0"。
  define: {
    __APP_VERSION__: JSON.stringify(pkg.version),
  },
  server: {
    port: 5173,
    strictPort: true,
    proxy: {
      '/api': {
        target: process.env.TL_DEV_URL ?? 'http://127.0.0.1:18921',
        changeOrigin: false,
      },
    },
  },
  build: {
    outDir: 'dist',
    emptyOutDir: true,
  },
  test: {
    environment: 'node',
    include: ['src/**/*.test.ts'],
  },
});
