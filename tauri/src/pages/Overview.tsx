import { useEffect, useMemo, useState } from "react";
import { ipc } from "../lib/ipc";
import { fmt } from "../lib/format";
import type { DiskInfo, ScanItem } from "../lib/types";
import { useApp } from "../lib/store";
import { Icon } from "../components/Icon";
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

export function Overview() {
  const { scanning, setPage, toast, results, ready, startFullScan } = useApp();
  const [disk, setDisk] = useState<DiskInfo | null>(null);

  // 可回收 / 摘要复用全局缓存的 all 模块（全量体检产出，扫一次所有页共享）
  const items = results.all;
  const hasScanned = ready.all;

  // 启动仅读取磁盘容量（只读系统信息），绝不自动扫描；扫描必须由用户点击触发
  useEffect(() => {
    ipc
      .diskInfo()
      .then(setDisk)
      .catch((e) => toast("warn", "读取磁盘信息失败：" + e));
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
                    onClick={() => setPage("clean")}
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
                    </div>
                    <span className="rs" style={{ color: "var(--brand)" }}>
                      {fmt(a.size)}
                    </span>
                    <span className="rv">
                      <span className="badge ghost">{a.count} 项</span>
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
            <div className="h3" style={{ marginBottom: 6 }}>
              本次扫描摘要
            </div>
            <div className="row-item">
              <span className="ic" style={{ background: "var(--brand-50)", color: "var(--brand)" }}>
                <Icon name="file" size={15} />
              </span>
              <span style={{ flex: 1 }}>发现项目</span>
              <b>{hasScanned ? items.length : "—"}</b>
            </div>
            <div className="row-item">
              <span className="ic" style={{ background: "var(--safe-50)", color: "var(--safe)" }}>
                <Icon name="check" size={15} />
              </span>
              <span style={{ flex: 1 }}>安全可清理</span>
              <b>{hasScanned ? safeCount : "—"}</b>
            </div>
            <div className="row-item">
              <span className="ic" style={{ background: "var(--cache-50)", color: "var(--cache)" }}>
                <Icon name="folder" size={15} />
              </span>
              <span style={{ flex: 1 }}>类别数</span>
              <b>{hasScanned ? cats.length : "—"}</b>
            </div>
            <div className="row-item">
              <span className="ic" style={{ background: "var(--caution-50)", color: "var(--caution)" }}>
                <Icon name="zap" size={15} />
              </span>
              <span style={{ flex: 1 }}>预计可回收</span>
              <b style={{ color: "var(--brand)" }}>{hasScanned ? fmt(reclaim) : "—"}</b>
            </div>
          </div>
        </div>
      </div>
    </div>
  );
}
