// 配置导入导出的端点封装。
//
// 只有两个端点：`plan` 只读预判（导入面板据此渲染清单），`import` 逐条落地。
// 判定规则全在 Core——这里不做任何"端口能不能用"的判断，否则一定和落地结果漂移。

import type { ProfileServer } from '../models/profile';
import { http } from './client';
import type {
  ProfileImportRequest,
  ProfileImportResult,
  ProfilePlan,
} from './types';

export const profileApi = {
  plan: (servers: ProfileServer[]) =>
    http.post<ProfilePlan>('/profile/plan', { servers }),

  /** 批量落地：走 60s 超时，200 条不会把界面吊在 15s 的超时上。 */
  import: (req: ProfileImportRequest) =>
    http.postSlow<ProfileImportResult>('/profile/import', req),
};
