import { createApp } from 'vue';
import { createPinia } from 'pinia';
import ElementPlus from 'element-plus';
import 'element-plus/dist/index.css';
import 'element-plus/theme-chalk/dark/css-vars.css';
import App from './App.vue';
import { refreshBootstrap } from './api/native';
import { applyTheme, readStoredTheme } from './models/theme';
import './styles.css';

// 挂载之前先把发现契约拿到手：页面刷新（F5）会清掉上一次注入的全局变量，
// 不先拉一次的话，首屏所有请求都会因为没有 baseUrl 而失败，显示「核心不可达」。
// 用函数包一层而不是顶层 await —— 默认构建目标不支持 top-level await。
async function bootstrap(): Promise<void> {
  // 主题要在首帧之前套上，否则会先闪一下默认配色再变。
  // `dark` 类的增删也由 applyTheme 负责（亮色主题不能挂它）。
  applyTheme(readStoredTheme());
  await refreshBootstrap();
  createApp(App).use(createPinia()).use(ElementPlus).mount('#app');
}

void bootstrap();
