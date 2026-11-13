import { defineConfig } from 'vitest/config';
import vue from '@vitejs/plugin-vue';

// 开发模式：`/api` 转发到本机核心（端口可能顺延，用 TL_DEV_URL 覆盖）。
// 打包后：Tauri 注入 `window.__THREADLINE__ = { baseUrl, token }`，不走代理。
export default defineConfig({
  plugins: [vue()],
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
