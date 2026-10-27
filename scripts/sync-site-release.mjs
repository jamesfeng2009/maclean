#!/usr/bin/env node
// 把一次 CLI 发布同步到官网仓库 maclean-site。
//
// 一次发布，官网有三处会漂移（2026-10-27 之前全靠手工改，改漏就是
// 页面版本号与 Cargo.toml 对不上）：
//
//   src/data/site.ts     version / updated          —— 页脚、JSON-LD softwareVersion
//   src/i18n/en.ts       changelog.entries[0]       —— 更新日志英文条目
//   src/i18n/zh.ts       changelog.entries[0]       —— 更新日志中文条目
//
// 用法:
//   node scripts/sync-site-release.mjs <site-dir> <version> [YYYY-MM-DD]
//
// 条目正文来源（可选）：release-notes/<version>.json
//   { "en": ["第一行", "第二行"], "zh": ["第一行", "第二行"] }
// 没有这个文件时写入一条指向 GitHub Release 的占位条目，并在 stdout 打印
// PLACEHOLDER，让 CI 打 warning。机器不编造营销文案 —— 占位比编造诚实。
//
// 幂等：目标版本已经在 site.ts 里存在时直接退出 0，不产生任何改动。

import { readFileSync, writeFileSync, existsSync } from 'node:fs';
import { join } from 'node:path';

const [siteDir, version, dateArg] = process.argv.slice(2);

if (!siteDir || !version) {
  console.error('用法: node scripts/sync-site-release.mjs <site-dir> <version> [YYYY-MM-DD]');
  process.exit(2);
}
if (!/^\d+\.\d+\.\d+(-[0-9A-Za-z.-]+)?$/.test(version)) {
  console.error(`错误：'${version}' 不是合法 semver`);
  process.exit(2);
}

const today = dateArg || new Date().toISOString().slice(0, 10);
if (!/^\d{4}-\d{2}-\d{2}$/.test(today)) {
  console.error(`错误：日期 '${today}' 不是 YYYY-MM-DD`);
  process.exit(2);
}

const siteTsPath = join(siteDir, 'src/data/site.ts');
const enPath = join(siteDir, 'src/i18n/en.ts');
const zhPath = join(siteDir, 'src/i18n/zh.ts');

for (const p of [siteTsPath, enPath, zhPath]) {
  if (!existsSync(p)) {
    console.error(`错误：找不到 ${p}（site-dir 指向的不是 maclean-site 仓库？）`);
    process.exit(2);
  }
}

function bail(msg) {
  console.error(`同步失败：${msg}`);
  process.exit(1);
}

// 仓库里 i18n 文件统一用单引号字符串。JSON.stringify 会产出双引号，
// 混进去以后 prettier/xo 之类一跑就是全文件 diff，所以自己转义。
function q(s) {
  return `'${String(s).replace(/\\/g, '\\\\').replace(/'/g, "\\'")}'`;
}

// ---------------------------------------------------------------- site.ts
const siteSrc = readFileSync(siteTsPath, 'utf8');
const versionRe = /(^ {2}version: ')([^']+)(')/m;
const updatedRe = /(^ {2}updated: ')([^']+)(')/m;

const currentVersion = siteSrc.match(versionRe);
if (!currentVersion) bail('site.ts 里找不到 `  version: \'...\'`');

if (currentVersion[2] === version) {
  console.log(`site.ts 已经是 ${version}，无需改动`);
  // 仍然校验语言文件，避免"版本对但 changelog 漏了"的半同步状态
  const enHas = readFileSync(enPath, 'utf8').includes(`version: '${version}'`);
  const zhHas = readFileSync(zhPath, 'utf8').includes(`version: '${version}'`);
  if (enHas && zhHas) {
    console.log('changelog 也已有该版本，整体幂等退出');
    process.exit(0);
  }
  console.log('但 changelog 缺少该版本条目，继续补齐');
} else {
  const nextSite = siteSrc
    .replace(versionRe, `$1${version}$3`)
    .replace(updatedRe, `$1${today}$3`);
  if (nextSite === siteSrc) bail('site.ts 替换后内容未变化（updated 字段缺失？）');
  writeFileSync(siteTsPath, nextSite, 'utf8');
  console.log(`site.ts: ${currentVersion[2]} -> ${version} (updated ${today})`);
}

// ------------------------------------------------------------ 变更条目正文
const notesPath = join(
  process.env.GITHUB_WORKSPACE || process.cwd(),
  'release-notes',
  `${version}.json`
);

let items = {
  en: [`See the GitHub release notes for the full list of changes in v${version}.`],
  zh: [`v${version} 的完整变更见 GitHub Release 说明。`],
};
let placeholder = true;

if (existsSync(notesPath)) {
  try {
    const parsed = JSON.parse(readFileSync(notesPath, 'utf8'));
    const ok =
      Array.isArray(parsed.en) &&
      Array.isArray(parsed.zh) &&
      parsed.en.length > 0 &&
      parsed.en.length === parsed.zh.length;
    if (!ok) {
      bail(`release-notes/${version}.json 需要 en/zh 两个等长非空数组`);
    }
    items = { en: parsed.en, zh: parsed.zh };
    placeholder = false;
  } catch (err) {
    bail(`解析 release-notes/${version}.json 失败: ${err.message}`);
  }
}

// --------------------------------------------------- i18n changelog entries
function syncChangelog(path, currentTag, prevTag, list, label) {
  const src = readFileSync(path, 'utf8');

  const changelogAt = src.indexOf('\n  changelog: {');
  if (changelogAt === -1) bail(`${label} 里找不到 changelog 段`);
  const entriesAt = src.indexOf('    entries: [', changelogAt);
  if (entriesAt === -1) bail(`${label} 里找不到 changelog.entries`);

  // 先把旧的"当前版本"降级，再做插入 —— 顺序颠倒会把自己刚插入的那条降级。
  const tagAt = src.indexOf(`tag: '${currentTag}',`, entriesAt);
  if (tagAt === -1) bail(`${label} 里找不到 tag: '${currentTag}'`);

  let out = src.slice(0, tagAt) + `tag: '${prevTag}',` + src.slice(tagAt + `tag: '${currentTag}',`.length);

  const body = list.map((i) => `          ${q(i)},`).join('\n');
  const entry =
    `    entries: [\n` +
    `      {\n` +
    `        version: ${q(version)},\n` +
    `        date: ${q(today)},\n` +
    `        tag: '${currentTag}',\n` +
    `        items: [\n` +
    `${body}\n` +
    `        ],\n` +
    `      },`;

  out = out.replace('    entries: [', entry);

  // 结构自检：新版本的条目必须在旧版本之前
  const newIdx = out.indexOf(`version: '${version}'`);
  const oldIdx = out.indexOf(`version: '${currentVersion[2]}'`);
  if (newIdx === -1) bail(`${label} 插入后找不到新版本条目`);
  if (oldIdx !== -1 && newIdx > oldIdx) bail(`${label} 新条目没有排在旧版本之前`);

  writeFileSync(path, out, 'utf8');
  console.log(`${label}: 已插入 ${version}，${currentVersion[2]} 标记为 ${prevTag}`);
}

syncChangelog(enPath, 'Current', 'Previous', items.en, 'en.ts');
syncChangelog(zhPath, '当前版本', '上一版', items.zh, 'zh.ts');

if (placeholder) {
  console.log(
    `PLACEHOLDER: 未找到 release-notes/${version}.json，已写入指向 GitHub Release 的占位条目`
  );
}
