import { useCallback, useEffect, useMemo, useState } from "react";
import { ipc } from "../lib/ipc";
import { fmt } from "../lib/format";
import type { DiskInfo, ResultScope, ScanItem } from "../lib/types";
import { useApp, type Page } from "../lib/store";
import { Icon, type IconName } from "../components/Icon";
import { defaultSelected } from "../components/ui";

interface CatAgg {
  name: string;
  size: number;
  count: number;
}

function aggregate(items: ScanItem[]): CatAgg[] {
  const m = new Map<string, CatAgg>();
  for (const it of items) {
    const a = m.get(it.category) ?? { name: it.category, size: 0, count: 0 };
    a.size += it.size_bytes;
    a.count += 1;
    m.set(it.category, a);
  }
  return [...m.values()].sort((a, b) => b.size - a.size);
}

const CAT_ICON: Record<string, string> = {
  浏览器缓存: "folder",
  系统日志: "doc",
  安装包: "file",
  Docker: "container",
  Conda缓存: "cpu",
};

/** 「本次扫描摘要」= 全量体检的四个模块入口：每行讲清数量含义，点击直达对应明细页 */
interface SummaryMod {
  scope: ResultScope;
  page: Page;
  label: string;
  icon: IconName;
  bg: string;
  fg: string;
  /** 右侧体量与副标题的口径解释（鼠标悬停可见） */
  tip: string;
  readySub: (n: number) => string;
}
const SUMMARY_MODS: SummaryMod[] = [
  {
    scope: "all",
    page: "clean",
    label: "缓存与应用数据",
    icon: "clean",
    bg: "var(--brand-50)",
    fg: "var(--brand)",
    tip: "开发者/浏览器缓存、应用缓存与应用数据等垃圾；右侧为默认可安全回收的空间",
    readySub: (n) => `${n} 项，默认可安全清理`,
  },
  {
    scope: "large",
    page: "analysis",
    label: "磁盘大文件",
    icon: "analysis",
    bg: "var(--cache-50)",
    fg: "var(--cache)",
    tip: "单个 ≥100MB 的超大文件（视频 / 镜像 / 压缩包 / AI 模型等）总大小；进磁盘分析可切换看大目录占用",
    readySub: (n) => `${n} 个大文件`,
  },
  {
    scope: "dup",
    page: "dup",
    label: "重复文件",
    icon: "dup",
    bg: "var(--safe-50)",
    fg: "var(--safe)",
    tip: "内容完全相同的文件分组，每组保留一份、删除多余副本；右侧为可释放空间",
    readySub: (n) => `${n} 组重复`,
  },
  {
    scope: "apps",
    page: "uninstall",
    label: "已安装应用",
    icon: "uninstall",
    bg: "var(--caution-50)",
    fg: "var(--caution)",
    tip: "本机已安装的应用及其残留；右侧为占用空间，卸载优先走官方卸载器",
    readySub: (n) => `${n} 个应用`,
  },
];

export function Overview() {
  const {
    scanning,
    setPage,
    focusClean,
    toast,
    results,
    ready,
    fullRunning,
    runningScopes,
    singleScope,
    startFullScan,
    cleanNonce,
    sessionTrashBytes,
  } = useApp();
  const [disk, setDisk] = useState<DiskInfo | null>(null);

  // 可回收 / 摘要复用全局缓存的 all 模块（全量体检产出，扫一次所有页共享）
  const items = results.all;
  const hasScanned = ready.all;
  const hasAnyScanned = Object.values(ready).some(Boolean);

  // 仅读取磁盘容量（只读系统信息），绝不自动扫描；扫描必须由用户点击触发。
  // 删除默认「移入废纸篓」，同卷下 df 已用 / 可用不会立即变——待用户清空废纸篓
  // 后才下降，因此在窗口重新激活时也重拉一次，做到「清空 → 回到 app」数字即更新。
  const refreshDisk = useCallback(() => {
    ipc.diskInfo().then(setDisk).catch(() => undefined);
  }, []);

  // 挂载时、以及每次成功清理（cleanNonce 递增）后刷新
  useEffect(() => {
    ipc
      .diskInfo()
      .then(setDisk)
      .catch((e) => toast("warn", "读取磁盘信息失败：" + e));
  }, [toast, cleanNonce]);

  // 用户去 Finder 清空废纸篓后回到窗口：重新激活时刷新，已用 / 可用立即闭环下降
  useEffect(() => {
    let last = 0;
    const onFocus = () => {
      const now = Date.now();
      if (now - last < 2000) return; // 简单节流，避免 focus/visibilitychange 连发
      last = now;
      refreshDisk();
    };
    window.addEventListener("focus", onFocus);
    document.addEventListener("visibilitychange", onFocus);
    return () => {
      window.removeEventListener("focus", onFocus);
      document.removeEventListener("visibilitychange", onFocus);
    };
  }, [refreshDisk]);

  const openTrash = useCallback(() => {
    ipc
      .revealTrash()
      .catch((e) => toast("warn", "打开废纸篓失败：" + e));
  }, [toast]);

  const cats = useMemo(() => aggregate(items), [items]);
  const top = cats.slice(0, 4);
  const reclaim = useMemo(
    () =>
      items
        .filter((i) => i.deletable && defaultSelected(i.recommend))
        .reduce((s, i) => s + i.size_bytes, 0),
    [items]
  );
  const safeCount = items.filter((i) => i.recommend === "Safe").length;

  // 环形
  const r = 58;
  const c = 2 * Math.PI * r;
  const pct = disk?.usage_pct ?? 0;

  return (
    <div className="grid" style={{ gap: 16 }}>
      <div className="hero">
        {/* 左：磁盘 + 可回收 */}
        <div className="hero-left">
          <div className="card diskcard">
            <div className="ring">
              <svg width="150" height="150" viewBox="0 0 150 150">
                <circle cx="75" cy="75" r={r} fill="none" stroke="var(--line)" strokeWidth="13" />
                <circle
                  cx="75"
                  cy="75"
                  r={r}
                  fill="none"
                  stroke="url(#diskGrad)"
                  strokeWidth="13"
                  strokeLinecap="round"
                  strokeDasharray={c}
                  strokeDashoffset={c * (1 - pct / 100)}
                />
                <defs>
                  <linearGradient id="diskGrad" x1="0" y1="0" x2="1" y2="1">
                    <stop offset="0%" stopColor="var(--brand)" />
                    <stop offset="100%" stopColor="#7C3AED" />
                  </linearGradient>
                </defs>
              </svg>
              <div className="ring-center">
                <span className="num">{pct.toFixed(0)}%</span>
                <span className="lab">磁盘已用</span>
              </div>
            </div>
            <div className="disk-info">
              <div className="sub" style={{ marginBottom: 2 }}>
                启动磁盘 · Macintosh HD
              </div>
              <div className="big">{fmt(disk?.used_bytes ?? 0)}</div>
              <div className="bar">
                <i style={{ width: pct + "%" }} />
              </div>
              <div className="disk-legend">
                <span>
                  <i style={{ background: "var(--brand)" }} />
                  已用 {fmt(disk?.used_bytes ?? 0)}
                </span>
                <span>
                  <i style={{ background: "var(--line-2)" }} />
                  可用 {fmt(disk?.free_bytes ?? 0)}
                </span>
                <span>
                  <i style={{ background: "var(--safe)" }} />
                  可清理 {hasScanned ? fmt(reclaim) : "—"}
                </span>
              </div>
              {sessionTrashBytes > 0 && (
                <button
                  type="button"
                  className="trash-hint"
                  onClick={openTrash}
                  title="清理的文件已移入废纸篓，仍占用启动磁盘，所以上方已用 / 可用暂时不变；点击在访达中打开废纸篓，清空后已用空间即会下降（回到本窗口会自动刷新）"
                >
                  <Icon name="trash" size={13} />
                  本次已移入废纸篓约 {fmt(sessionTrashBytes)} · 清空后释放，点击打开
                </button>
              )}
            </div>
          </div>

          <div>
            <div className="row" style={{ justifyContent: "space-between", marginBottom: 10 }}>
              <div className="h3">可回收空间（按类别）</div>
              <button className="btn-secondary" onClick={startFullScan} style={{ height: 30, fontSize: 12.5 }}>
                <Icon name="refresh" size={13} /> {hasScanned ? "重新扫描" : "开始扫描"}
              </button>
            </div>
            {top.length === 0 ? (
              <div className="card empty" style={{ padding: 26 }}>
                {scanning ? (
                  "正在全盘体检…可切换到其它页面，缓存模块完成后这里会自动呈现"
                ) : hasScanned ? (
                  "暂未发现可清理项"
                ) : (
                  <div style={{ display: "grid", gap: 12, justifyItems: "center" }}>
                    <span>尚未扫描，点击开始一次全盘体检（只读扫描，不会删除任何文件）</span>
                    <button className="btn-primary" onClick={startFullScan}>
                      <Icon name="zap" size={14} /> 开始扫描
                    </button>
                  </div>
                )}
              </div>
            ) : (
              <div className="reclaim-grid">
                {top.map((a) => (
                  <div
                    key={a.name}
                    className="reclaim-item"
                    role="button"
                    tabIndex={0}
                    title={`查看「${a.name}」的占用明细`}
                    onClick={() => focusClean(a.name)}
                    onKeyDown={(e) => {
                      if (e.key === "Enter" || e.key === " ") {
                        e.preventDefault();
                        focusClean(a.name);
                      }
                    }}
                  >
                    <div className="rt">
                      <span className="rn">
                        <Icon
                          name={CAT_ICON[a.name] ?? "folder"}
                          size={15}
                          style={{ verticalAlign: "-2px", marginRight: 5 }}
                        />
                        {a.name}
                      </span>
                      <Icon name="chev" size={14} className="reclaim-go" />
                    </div>
                    <span className="rs" style={{ color: "var(--brand)" }}>
                      {fmt(a.size)}
                    </span>
                    <span className="rv">
                      <span className="badge ghost">{a.count} 项</span>
                      <span className="reclaim-detail-hint">点击查看占用明细</span>
                    </span>
                  </div>
                ))}
              </div>
            )}
          </div>
        </div>

        {/* 右：保护状态 + 本次扫描摘要 */}
        <div className="hero-right">
          <div className="card status-card">
            <div className="h3" style={{ marginBottom: 2 }}>
              安全保护
            </div>
            {[
              ["safe", "危险路径拦截", "系统关键目录永不被删", "var(--safe)"],
              ["trash", "废纸篓优先", "误删可从废纸篓恢复", "var(--cache)"],
              ["backup", "M-2 恢复清单", "每次删除记录可回溯", "var(--brand)"],
              ["uninst", "官方卸载器", "应用卸载优先走原生流程", "var(--caution)"],
            ].map(([, t, d, color]) => (
              <div className="status-item" key={t}>
                <span className="dot" style={{ background: color }} />
                <span className="grow">
                  <b>{t}</b>
                  <div className="sub" style={{ fontSize: 12 }}>
                    {d}
                  </div>
                </span>
                <Icon name="shield" size={15} style={{ color: "var(--safe)" }} />
              </div>
            ))}
          </div>

          <div className="card recent">
            <div
              className="row"
              style={{ justifyContent: "space-between", alignItems: "center", marginBottom: 6 }}
            >
              <div className="h3">本次扫描摘要</div>
              {hasAnyScanned && (
                <span className="sub" style={{ fontSize: 11 }}>
                  点击查看明细
                </span>
              )}
            </div>
            {SUMMARY_MODS.map((m) => {
              const list = results[m.scope];
              // large 模块同时产出大文件与大目录；概览「磁盘大文件」只取真实
              // 大文件（单个 ≥100MB）口径，大目录占用进磁盘分析后切换视图查看。
              const countList =
                m.scope === "large" ? list.filter((i) => i.category !== "目录") : list;
              const isReady = ready[m.scope];
              const isRun =
                (fullRunning && runningScopes.includes(m.scope)) ||
                (!fullRunning && singleScope === m.scope && scanning);
              const pending = fullRunning && !isReady && !isRun;
              // all 行右侧展示「默认可安全回收」，与磁盘卡口径一致；其余模块展示相关占用体量
              const size =
                m.scope === "all"
                  ? reclaim
                  : countList.reduce((s, i) => s + i.size_bytes, 0);
              const sub = isReady
                ? m.scope === "all"
                  ? `${list.length} 项 · ${safeCount} 项可安全清理`
                  : m.readySub(countList.length)
                : isRun
                  ? "正在扫描…"
                  : pending
                    ? "等待扫描"
                    : "尚未扫描";
              return (
                <div
                  key={m.scope}
                  className="row-item link"
                  role="button"
                  tabIndex={0}
                  title={m.tip}
                  onClick={() => setPage(m.page)}
                  onKeyDown={(e) => {
                    if (e.key === "Enter" || e.key === " ") {
                      e.preventDefault();
                      setPage(m.page);
                    }
                  }}
                >
                  <span className="ic" style={{ background: m.bg, color: m.fg }}>
                    <Icon name={m.icon} size={15} />
                  </span>
                  <span style={{ flex: 1, minWidth: 0 }}>
                    <span className="rn-label">{m.label}</span>
                    <div className="sub" style={{ fontSize: 11.5, marginTop: 1 }}>
                      {sub}
                    </div>
                  </span>
                  {isReady ? (
                    <b style={{ color: m.scope === "all" ? "var(--brand)" : "var(--text)" }}>
                      {fmt(size)}
                    </b>
                  ) : (
                    <b className="dim">—</b>
                  )}
                  <span className="chev">
                    <Icon name="chev" size={14} />
                  </span>
                </div>
              );
            })}
          </div>
        </div>
      </div>
    </div>
  );
}
