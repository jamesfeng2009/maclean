import { useCallback, useMemo, useState } from "react";
import { fmt, shortPath } from "../lib/format";
import type { ScanItem } from "../lib/types";
import { useApp } from "../lib/store";
import { Icon } from "../components/Icon";
import { Badge, Empty, PageHeader } from "../components/ui";

const COLORS = ["#4F46E5", "#10B981", "#3B82F6", "#F59E0B", "#8B5CF6", "#EC4899", "#14B8A6", "#F97316", "#6366F1", "#64748B"];

interface Seg {
  name: string;
  size: number;
  pct: number;
  color: string;
}

export function Analysis() {
  const { startScan, scanning } = useApp();
  const [items, setItems] = useState<ScanItem[]>([]);
  const [open, setOpen] = useState<Set<string>>(new Set());
  const [hasScanned, setHasScanned] = useState(false);

  // 不做挂载自动扫描，仅按钮触发
  const refresh = useCallback(() => {
    void startScan("large", (its) => {
      setItems(its);
      setHasScanned(true);
    });
  }, [startScan]);

  const total = items.reduce((s, i) => s + i.size_bytes, 0);

  const segs = useMemo<Seg[]>(() => {
    const m = new Map<string, number>();
    for (const it of items) m.set(it.category, (m.get(it.category) ?? 0) + it.size_bytes);
    return [...m.entries()]
      .sort((a, b) => b[1] - a[1])
      .slice(0, 10)
      .map(([name, size], idx) => ({
        name,
        size,
        pct: total ? (size / total) * 100 : 0,
        color: COLORS[idx % COLORS.length],
      }));
  }, [items, total]);

  // donut：用 stroke-dasharray 多段拼接
  const r = 80;
  const c = 2 * Math.PI * r;
  let acc = 0;
  const circles = segs.map((s) => {
    const frac = s.pct / 100;
    const dash = `${frac * c} ${c - frac * c}`;
    const off = -acc * c;
    acc += frac;
    return { ...s, dash, off };
  });

  const toggle = (k: string) =>
    setOpen((s) => {
      const n = new Set(s);
      if (n.has(k)) n.delete(k);
      else n.add(k);
      return n;
    });

  return (
    <div>
      <PageHeader
        title="磁盘分析"
        sub="大文件与大目录空间占用（按顶层分类聚合）"
        action={
          <button className="btn-secondary" onClick={refresh} style={{ height: 34 }}>
            <Icon name="refresh" size={14} /> {hasScanned ? "重新扫描" : "开始扫描"}
          </button>
        }
      />

      {items.length === 0 ? (
        <Empty
          icon="analysis"
          text={
            scanning
              ? "正在分析磁盘占用…"
              : hasScanned
                ? "暂未发现大文件"
                : "尚未扫描，点击下方按钮分析磁盘占用（只读扫描）"
          }
          action={
            !scanning && !hasScanned ? (
              <button className="btn-primary" onClick={refresh}>
                <Icon name="zap" size={15} /> 开始扫描
              </button>
            ) : undefined
          }
        />
      ) : (
        <div className="analysis-grid">
          <div className="card donut-wrap">
            <div className="h3" style={{ alignSelf: "flex-start" }}>
              空间占比
            </div>
            <div className="donut">
              <svg width="210" height="210" viewBox="0 0 200 200">
                <circle cx="100" cy="100" r={r} fill="none" stroke="var(--line)" strokeWidth="22" />
                {circles.map((s) => (
                  <circle
                    key={s.name}
                    cx="100"
                    cy="100"
                    r={r}
                    fill="none"
                    stroke={s.color}
                    strokeWidth="22"
                    strokeDasharray={s.dash}
                    strokeDashoffset={s.off}
                  />
                ))}
              </svg>
              <div className="center">
                <span className="n">{fmt(total)}</span>
                <span className="sub">大文件合计</span>
              </div>
            </div>
            <div className="legend-col">
              {segs.map((s) => (
                <div className="legend-row" key={s.name}>
                  <span className="sw" style={{ background: s.color }} />
                  <span className="grow">{s.name}</span>
                  <b>{fmt(s.size)}</b>
                  <span className="pct">{s.pct.toFixed(1)}%</span>
                </div>
              ))}
            </div>
          </div>

          <div className="card">
            <div className="h3" style={{ marginBottom: 8 }}>
              占用最大的项目
            </div>
            <div className="dirtree">
              {items.slice(0, 40).map((it, idx) => {
                const k = it.path;
                const isOpen = open.has(k);
                const pctv = total ? Math.min(100, (it.size_bytes / items[0].size_bytes) * 100) : 0;
                return (
                  <div key={k}>
                    <div className={`dir-row${isOpen ? " open" : ""}`} onClick={() => toggle(k)}>
                      <Icon name="chev" size={13} className="chev" />
                      <span className="nm" title={it.path}>
                        {shortPath(it.path).split("/").pop()}
                      </span>
                      <Badge r={it.recommend} />
                      <span className="mini">
                        <i
                          style={{
                            width: pctv + "%",
                            background: COLORS[idx % COLORS.length],
                          }}
                        />
                      </span>
                      <span className="sz">{fmt(it.size_bytes)}</span>
                    </div>
                    {isOpen && (
                      <div className="dir-children" style={{ paddingBottom: 8 }}>
                        <div className="sub" style={{ fontSize: 12, wordBreak: "break-all" }}>
                          {shortPath(it.path)}
                        </div>
                        {it.description && (
                          <div className="sub" style={{ fontSize: 12, marginTop: 3 }}>
                            {it.description}
                          </div>
                        )}
                      </div>
                    )}
                  </div>
                );
              })}
            </div>
          </div>
        </div>
      )}
    </div>
  );
}
