#!/usr/bin/env node
// Threadline 架构机检（MSO §6 可机检子集 + CAW 可机检不变量）。
//
// 用法：
//   node tools/msocheck/msocheck.mjs            # 检查仓库
//   node tools/msocheck/msocheck.mjs --selftest # 自测规则本身
//
// 规则（设计文档 §10）：
//   R1 依赖方向  src/ 下只允许 foundation|models|business|interface|shell + 顶层 main.rs/lib.rs；
//                禁反向 import；禁两层以上 super::；覆盖组合导入、`crate ::` 空白、#[path] 重定向
//   R2 数据独占  禁跨模型 internal（含 glob、组合导入、`server::internal::x()` 间接形态）
//   R3 前端收口  src/views/** 禁 axios（含 import()）/ fetch（含取引用 `window.fetch`）
//   R4 核心无头  核心/唤醒不得依赖 GUI 库（含 iced/slint）；windows crate 禁 Win32_UI*/Graphics 特性；
//                覆盖 crate 的 Cargo.toml、build.rs 与根 [workspace.dependencies]
//   R5 壳不施放  src/main.rs 与 src/shell/** 禁止任何 spawn*/make*/activate*/apply* 形态
//                （调用、关联函数、turbofish、取函数引用、方法调用）
//   R6 环与终止  模型调用图成环时，环上至少一个模型必须**在代码中**（剥离注释后）调用
//                foundation::{retry,limiter,timeout}::，或声明 `pub const TERMINATION: &str = "phase";`
//
// 机检是必要条件而非充分：拦得住结构违规，拦不住语义越权。

import { readFileSync, readdirSync, statSync, existsSync } from 'node:fs';
import { join, relative, sep } from 'node:path';

const ROOT = process.cwd();
const LAYERS = ['foundation', 'models', 'business', 'interface', 'shell'];
const level = (name) => LAYERS.indexOf(name);
const SRC_DIRS = [
  join(ROOT, 'crates', 'tl-core', 'src'),
  join(ROOT, 'crates', 'tl-wake', 'src'),
];

/** 去掉行注释与块注释（机检不能被注释糊弄）。 */
export function stripComments(source) {
  return source
    .replace(/\/\*[\s\S]*?\*\//g, '')
    .replace(/^\s*\/\/.*$/gm, '')
    .replace(/\s\/\/.*$/gm, '');
}

function referencedLayers(source) {
  const refs = new Set();
  const norm = source.replace(/\s*::\s*/g, '::');
  for (const m of norm.matchAll(/\buse\s+crate::(\w+)/g)) refs.add(m[1]);
  for (const m of norm.matchAll(/\bcrate::(\w+)::/g)) refs.add(m[1]);
  for (const m of norm.matchAll(/\buse\s+crate::\{([^}]*)\}/g)) {
    for (const part of m[1].split(',')) {
      const head = part.trim().split('::')[0].trim();
      if (head) refs.add(head);
    }
  }
  return refs;
}

/** R1：分层与依赖方向。 */
export function checkDependencyDirection(relPath, rawSource) {
  const violations = [];
  const source = stripComments(rawSource);
  const norm = source.replace(/\s*::\s*/g, '::');
  const parts = relPath.split('/');

  if (parts.length === 1) {
    if (parts[0] !== 'main.rs' && parts[0] !== 'lib.rs') {
      violations.push(
        `${relPath}: src/ 顶层禁止平铺文件（只允许 main.rs / lib.rs），请归入 foundation/models/business/interface/shell`,
      );
    }
  } else if (!LAYERS.includes(parts[0])) {
    violations.push(`${relPath}: 未知层目录 "${parts[0]}"（只允许 ${LAYERS.join(' / ')}）`);
  }

  if (/#\[path\s*=/.test(source)) {
    violations.push(`${relPath}: 禁止用 #[path] 重定向模块（绕过分层检查）`);
  }

  const self = LAYERS.includes(parts[0]) ? parts[0] : 'shell';
  for (const ref of referencedLayers(source)) {
    if (level(ref) < 0) continue;
    if (level(ref) > level(self)) {
      violations.push(`${relPath}: ${self} 层禁止依赖上层 ${ref}（依赖方向只能向下）`);
    }
  }

  if (/super\s*::\s*super\s*::/.test(source)) {
    violations.push(`${relPath}: 禁止两层以上 super::（请用 crate:: 绝对路径）`);
  }
  return violations;
}

/** R2：跨模型访问 internal。 */
export function checkModelIsolation(relPath, rawSource) {
  const violations = [];
  const source = stripComments(rawSource);
  const norm = source.replace(/\s*::\s*/g, '::');
  const selfModel = relPath.match(/models[/\\](\w+)[/\\]/)?.[1];
  const hit = (target) => target && target !== selfModel;

  for (const m of norm.matchAll(/crate::models::(\w+)::internal\b/g)) {
    if (hit(m[1])) {
      violations.push(`${relPath}: 跨模型访问内部实现 models::${m[1]}::internal（只能走对方 port）`);
    }
  }
  for (const m of norm.matchAll(/use\s+crate::models::(\w+)::\*/g)) {
    if (hit(m[1])) {
      violations.push(`${relPath}: 禁止 use crate::models::${m[1]}::*（glob 绕开数据独占）`);
    }
  }
  for (const m of norm.matchAll(/\buse\s+crate::models::(\w+)::\{([^}]*)\}/g)) {
    if (hit(m[1]) && /internal|\*/.test(m[2])) {
      violations.push(`${relPath}: 禁止用组合导入访问 models::${m[1]} 的 internal / glob`);
    }
  }
  // use crate::models::{b::*} / {b::internal::Foo}
  for (const m of norm.matchAll(/\buse\s+crate::models::\{([^}]*)\}/g)) {
    if (!/internal|\*/.test(m[1])) continue;
    const targets = m[1]
      .split(',')
      .map((s) => s.trim().split('::')[0].trim())
      .filter(Boolean);
    if (targets.some((t) => hit(t))) {
      violations.push(`${relPath}: 禁止用组合导入访问其它模型的 internal / glob（${m[1].trim()}）`);
    }
  }
  // 间接形态：use crate::models::{server}; … server::internal::x()
  if (/\binternal::\w+\s*\(/.test(norm) && !/models\/(\w+)\//.test(relPath.replaceAll(sep, '/'))) {
    violations.push(`${relPath}: 出现 internal::… 调用，疑似跨模型翻内部实现`);
  }
  return violations;
}

/** R3：views 层禁止直连 HTTP。 */
export function checkViewHttp(relPath, rawSource) {
  const violations = [];
  if (!/(^|\/)views\//.test(relPath.replaceAll(sep, '/'))) return violations;
  const source = stripComments(rawSource);
  if (/from\s+['"]axios['"]|require\(['"]axios['"]\)|import\(\s*['"]axios['"]\s*\)/.test(source)) {
    violations.push(`${relPath}: views 层禁止直接使用 axios（唯一出口 src/api）`);
  }
  // 直接调用、取函数引用（window.fetch / const f = fetch）都算
  if (/(^|[^.\w])(window\.)?fetch\b/.test(source) || /\bfetch\s*[;),]/.test(source)) {
    violations.push(`${relPath}: views 层禁止直接使用 fetch（唯一出口 src/api）`);
  }
  return violations;
}

/** R5：壳层禁止施放型调用（任何形态）。 */
export function checkShellDispatch(relPath, rawSource) {
  const violations = [];
  const isShell = relPath === 'main.rs' || /(^|[/\\])shell[/\\]/.test(relPath);
  if (!isShell) return violations;
  const source = stripComments(rawSource);
  // 框架/标准库的任务启动不是"模型施放"：先剥离，避免把 tokio::spawn 误判成违规。
  const sanitized = source.replace(
    /\b(tokio|std|futures|task|thread|rayon)::(spawn|spawn_blocking|spawn_local)\s*\(/g,
    '',
  );
  const pattern =
    /::(spawn|make|activate|apply)\b|\.(spawn|make|activate|apply)\w*\s*\(|\b(spawn|make|activate|apply)_\w+/;
  if (pattern.test(sanitized)) {
    violations.push(
      `${relPath}: 壳层禁止任何 spawn*/make*/activate*/apply* 形态（调用/关联函数/turbofish/取引用），应经业务层统一入口`,
    );
  }
  return violations;
}

/** R4：核心/唤醒不得链接 GUI 库，也不得启用 windows 的 UI 特性。 */
const GUI_LIBS = [
  'tauri',
  'wry',
  'tao',
  'winit',
  'eframe',
  'egui',
  'iced',
  'slint',
  'webview2',
  'webkit2gtk',
  'gtk',
  'notify-rust',
];
const UI_FEATURES = /Win32_(UI|Graphics|Media|Web)\w*/i;

export function checkCoreHasNoUi(label, rawSource) {
  const violations = [];
  const banned = new RegExp(`^\\s*(${GUI_LIBS.join('|')})\\s*=`, 'i');
  for (const line of rawSource.split('\n')) {
    if (banned.test(line)) {
      violations.push(`${label}: 核心/唤醒进程禁止依赖 GUI 库 —— ${line.trim()}`);
    }
  }
  for (const m of rawSource.matchAll(/features\s*=\s*\[([^\]]*)\]/g)) {
    const f = m[1].match(UI_FEATURES);
    if (f) {
      violations.push(`${label}: 核心/唤醒禁止启用 windows UI 特性 —— ${f[0]}`);
    }
  }
  return violations;
}

/** 代码中（剥离注释后）是否真的调用了底座终止组件。 */
export function declaresTermination(rawSource) {
  const source = stripComments(rawSource);
  return /foundation\s*::\s*(retry|limiter|timeout)\s*::/.test(source);
}

/** 代码级相位声明（常量，不会被注释掉）。 */
export function declaresPhase(rawSource) {
  const source = stripComments(rawSource);
  return /pub\s+const\s+TERMINATION\s*:\s*&str\s*=\s*"phase"\s*;/.test(source);
}

/** R6：模型调用图环检测 + 终止保障。 */
export function checkCycles(files) {
  const graph = new Map();
  for (const [rel, raw] of files) {
    const selfModel = rel.match(/models\/(\w+)\//)?.[1];
    if (!selfModel) continue;
    if (!graph.has(selfModel)) graph.set(selfModel, new Set());
    const source = stripComments(raw).replace(/\s*::\s*/g, '::');
    for (const m of source.matchAll(/crate::models::(\w+)/g)) {
      if (m[1] !== selfModel) graph.get(selfModel).add(m[1]);
    }
    for (const m of source.matchAll(/\buse\s+crate::models::\{([^}]*)\}/g)) {
      for (const part of m[1].split(',')) {
        const name = part.trim().split('::')[0].trim();
        if (name && name !== selfModel) graph.get(selfModel).add(name);
      }
    }
  }

  const cycles = [];
  const state = new Map();
  const stack = [];
  const dfs = (node) => {
    state.set(node, 1);
    stack.push(node);
    for (const next of graph.get(node) ?? []) {
      if (!graph.has(next)) continue;
      const s = state.get(next) ?? 0;
      if (s === 0) dfs(next);
      else if (s === 1) cycles.push(stack.slice(stack.indexOf(next)).concat(next));
    }
    stack.pop();
    state.set(node, 2);
  };
  for (const node of graph.keys()) if ((state.get(node) ?? 0) === 0) dfs(node);

  const violations = [];
  const seen = new Set();
  for (const cycle of cycles) {
    const key = [...new Set(cycle)].sort().join('->');
    if (seen.has(key)) continue;
    seen.add(key);
    const members = [...new Set(cycle.slice(0, -1))];
    const ok = members.some((model) =>
      [...files.entries()]
        .filter(([rel]) => rel.startsWith(`models/${model}/`))
        .some(([, src]) => declaresTermination(src) || declaresPhase(src)),
    );
    if (!ok) {
      violations.push(
        `环 ${members.join(' → ')} → ${members[0]}: 没有任何模型声明终止保障（代码中调用 foundation::retry|limiter|timeout，或 pub const TERMINATION = "phase"）`,
      );
    }
  }
  return violations;
}

function walk(dir) {
  const out = [];
  if (!existsSync(dir)) return out;
  for (const name of readdirSync(dir)) {
    if (name === 'target' || name === 'node_modules' || name.startsWith('.')) continue;
    const full = join(dir, name);
    if (statSync(full).isDirectory()) out.push(...walk(full));
    else out.push(full);
  }
  return out;
}

function collect(srcDir, ext) {
  const files = new Map();
  for (const file of walk(srcDir)) {
    if (!file.endsWith(ext)) continue;
    files.set(relative(srcDir, file).replaceAll(sep, '/'), readFileSync(file, 'utf8'));
  }
  return files;
}

export function run() {
  const violations = [];

  for (const srcDir of SRC_DIRS) {
    const files = collect(srcDir, '.rs');
    for (const [rel, source] of files) {
      violations.push(...checkDependencyDirection(rel, source));
      violations.push(...checkModelIsolation(rel, source));
      violations.push(...checkShellDispatch(rel, source));
    }
    violations.push(...checkCycles(files));
  }

  for (const crate of ['tl-core', 'tl-wake']) {
    const toml = join(ROOT, 'crates', crate, 'Cargo.toml');
    if (existsSync(toml)) {
      violations.push(...checkCoreHasNoUi(`crates/${crate}/Cargo.toml`, readFileSync(toml, 'utf8')));
    }
    const buildRs = join(ROOT, 'crates', crate, 'build.rs');
    if (existsSync(buildRs)) {
      violations.push(...checkCoreHasNoUi(`crates/${crate}/build.rs`, readFileSync(buildRs, 'utf8')));
    }
  }
  const wsToml = join(ROOT, 'Cargo.toml');
  if (existsSync(wsToml)) {
    const src = readFileSync(wsToml, 'utf8');
    const wsDep = src.split('[workspace.dependencies]')[1] ?? '';
    violations.push(...checkCoreHasNoUi('Cargo.toml [workspace.dependencies]', wsDep));
  }

  for (const ext of ['.vue', '.ts']) {
    for (const [rel, source] of collect(join(ROOT, 'src'), ext)) {
      violations.push(...checkViewHttp(`src/${rel}`, source));
    }
  }

  if (violations.length) {
    console.error('msocheck 失败：');
    for (const v of violations) console.error('  ✗ ' + v);
    process.exit(1);
  }
  console.log(
    'msocheck 通过：分层与依赖方向 / 模型隔离 / 前端收口 / 核心无头 / 壳不施放 / 环与终止保障',
  );
}

function selftest() {
  const cases = [
    // R1
    ['R1 反向依赖被抓', () => checkDependencyDirection('models/tunnel/x.rs', 'use crate::interface::http::Router;').length === 1],
    ['R1 正向依赖放行', () => checkDependencyDirection('business/start.rs', 'use crate::models::tunnel::Tunnel;').length === 0],
    ['R1 同层放行', () => checkDependencyDirection('models/a/x.rs', 'use crate::models::b::Thing;').length === 0],
    ['R1 顶层平铺被抓', () => checkDependencyDirection('utils.rs', '// hi').length === 1],
    ['R1 顶层 main.rs 放行', () => checkDependencyDirection('main.rs', 'use crate::interface::Api;').length === 0],
    ['R1 顶层 lib.rs 放行', () => checkDependencyDirection('lib.rs', '').length === 0],
    ['R1 未知层目录被抓', () => checkDependencyDirection('helpers/x.rs', '// hi').length === 1],
    ['R1 组合导入被抓', () => checkDependencyDirection('models/a/x.rs', 'use crate::{interface::Api, models::b::Y};').length === 1],
    ['R1 crate :: 空白绕过被抓', () => checkDependencyDirection('models/a/x.rs', 'use crate :: interface::Api;').length === 1],
    ['R1 #[path] 重定向被抓', () => checkDependencyDirection('models/a/x.rs', '#[path = "../interface/http.rs"] mod http;').length === 1],
    ['R1 注释里的违规不算', () => checkDependencyDirection('models/a/x.rs', '// use crate::interface::Api;').length === 0],
    ['R1 深 super 被抓', () => checkDependencyDirection('models/a/x.rs', 'use super :: super :: Thing;').length === 1],
    // R2
    ['R2 跨模型内部被抓', () => checkModelIsolation('models/a/x.rs', 'use crate::models::b::internal::Foo;').length === 1],
    ['R2 本模型内部放行', () => checkModelIsolation('models/b/x.rs', 'use crate::models::b::internal::Foo;').length === 0],
    ['R2 glob 被抓', () => checkModelIsolation('models/a/x.rs', 'use crate::models::b::*;').length === 1],
    ['R2 组合 internal 被抓', () => checkModelIsolation('models/a/x.rs', 'use crate::models::b::{internal::Foo};').length === 1],
    ['R2 组合 glob 被抓', () => checkModelIsolation('models/a/x.rs', 'use crate::models::{b::*};').length === 1],
    // R3
    ['R3 views 直连 axios 被抓', () => checkViewHttp('src/views/Home.vue', "import axios from 'axios';").length === 1],
    ['R3 views 动态 import axios 被抓', () => checkViewHttp('src/views/Home.vue', "await import('axios');").length === 1],
    ['R3 views 调 fetch 被抓', () => checkViewHttp('src/views/Home.vue', 'await fetch("/api");').length === 1],
    ['R3 views 取 fetch 引用被抓', () => checkViewHttp('src/views/Home.vue', 'const f = window.fetch; f("/api");').length === 1],
    ['R3 api 层放行', () => checkViewHttp('src/api/tunnel.ts', "import axios from 'axios';").length === 0],
    // R5
    ['R5 shell/ 施放被抓', () => checkShellDispatch('shell/main.rs', 'crate::models::tunnel::spawn_tunnel();').length === 1],
    ['R5 顶层 main.rs 施放被抓', () => checkShellDispatch('main.rs', 'spawn_tunnel();').length === 1],
    ['R5 别名调用被抓', () => checkShellDispatch('main.rs', 'use crate::models::tunnel::spawn_tunnel; spawn_tunnel();').length === 1],
    ['R5 关联函数被抓', () => checkShellDispatch('main.rs', 'Tunnel::activate(id);').length === 1],
    ['R5 方法调用被抓', () => checkShellDispatch('main.rs', 'tunnel_model.spawn();').length === 1],
    ['R5 turbofish 被抓', () => checkShellDispatch('main.rs', 'crate::models::tunnel::spawn::<Tunnel>();').length === 1],
    ['R5 取函数引用被抓', () => checkShellDispatch('main.rs', 'let f = crate::models::tunnel::apply_plan; f();').length === 1],
    ['R5 非壳放行', () => checkShellDispatch('business/start.rs', 'crate::models::tunnel::spawn_tunnel();').length === 0],
    ['R5 框架 spawn 不算违规', () => checkShellDispatch('main.rs', 'tokio::spawn(async move { work().await });').length === 0],
    // R4
    ['R4 GUI 依赖被抓', () => checkCoreHasNoUi('Cargo.toml', 'tauri = "2"').length === 1],
    ['R4 winit 被抓', () => checkCoreHasNoUi('Cargo.toml', 'winit = "0.30"').length === 1],
    ['R4 iced 被抓', () => checkCoreHasNoUi('Cargo.toml', 'iced = "0.13"').length === 1],
    ['R4 windows UI 特性被抓', () => checkCoreHasNoUi('Cargo.toml', 'windows = { version = "0.58", features = ["Win32_UI_WindowsAndMessaging"] }').length === 1],
    ['R4 windows 非 UI 特性放行', () => checkCoreHasNoUi('Cargo.toml', 'windows = { version = "0.58", features = ["Win32_Foundation"] }').length === 0],
    ['R4 正常依赖放行', () => checkCoreHasNoUi('Cargo.toml', 'tokio = "1"').length === 0],
    // R6
    [
      'R6 无终止声明的环被抓',
      () => checkCycles(new Map([
        ['models/a/x.rs', 'use crate::models::b::Foo;'],
        ['models/b/x.rs', 'use crate::models::a::Bar;'],
      ])).length > 0,
    ],
    [
      'R6 注释不算终止声明',
      () => checkCycles(new Map([
        ['models/a/x.rs', 'use crate::models::b::Foo; // foundation::retry::backoff()'],
        ['models/b/x.rs', 'use crate::models::a::Bar;'],
      ])).length > 0,
    ],
    [
      'R6 注释里的 phase 也不算',
      () => checkCycles(new Map([
        ['models/a/x.rs', 'use crate::models::b::Foo; // pub const TERMINATION: &str = "phase";'],
        ['models/b/x.rs', 'use crate::models::a::Bar;'],
      ])).length > 0,
    ],
    [
      'R6 代码里真的调用终止组件则放行',
      () => checkCycles(new Map([
        ['models/a/x.rs', 'use crate::models::b::Foo; fn f(){ foundation::retry::backoff(); }'],
        ['models/b/x.rs', 'use crate::models::a::Bar;'],
      ])).length === 0,
    ],
    [
      'R6 代码级 phase 常量可放行',
      () => checkCycles(new Map([
        ['models/a/x.rs', 'use crate::models::b::Foo; pub const TERMINATION: &str = "phase";'],
        ['models/b/x.rs', 'use crate::models::a::Bar;'],
      ])).length === 0,
    ],
    [
      'R6 组合导入建边',
      () => checkCycles(new Map([
        ['models/a/x.rs', 'use crate::models::{b};'],
        ['models/b/x.rs', 'use crate::models::a::Bar;'],
      ])).length > 0,
    ],
    [
      'R6 无环不报错',
      () => checkCycles(new Map([['models/a/x.rs', 'use crate::models::b::Foo;']])).length === 0,
    ],
  ];
  let failed = 0;
  for (const [name, fn] of cases) {
    let ok = false;
    try {
      ok = fn();
    } catch (e) {
      console.error(`      ${e.message}`);
    }
    console.log(`${ok ? 'ok  ' : 'FAIL'} ${name}`);
    if (!ok) failed++;
  }
  if (failed) {
    console.error(`selftest 失败 ${failed} 项`);
    process.exit(1);
  }
  console.log(`selftest 通过（${cases.length} 项）`);
}

if (process.argv.includes('--selftest')) selftest();
else run();
