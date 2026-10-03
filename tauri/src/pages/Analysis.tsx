import { useCallback, useMemo, useState } from "react";
import { ipc } from "../lib/ipc";
import { fmt, shortPath } from "../lib/format";
import type { CleanItemReq } from "../lib/types";
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
  const {
    startScan,
    scanning,
    runningScopes,
    results,
    ready,
    startFullScan,
    confirm,
    toast,
    removePaths,
    executeClean,
  } = useApp();
  // 复用全局缓存的 large 模块
  const items = results.large;
  const hasScanned = ready.large;
  // 大文件模块是否仍在扫描。磁盘分析页的勾选/删除只依赖 large 自身状态：
  // 一键全量时缓存(all)/应用(apps)模块可能仍在后台继续跑，但 large 通常最早
  // 完成；large 一就绪就应允许用户勾选并移入废纸篓，不必等待最慢的缓存模块。
  const largeBusy = runningScopes.includes("large");
  // 视图：大文件（递归找到的超大单文件，概览入口默认）/ 大目录（主目录顶层占用）
  const [view, setView] = useState<"files" | "dirs">("files");
  const [open, setOpen] = useState<Set<string>>(new Set());
  // 大文件/大目录默认全部不勾（均为「高级」风险，需用户逐个确认）
  const [checked, setChecked] = useState<Set<string>>(new Set());
  const [busy, setBusy] = useState(false);

  // 页头「重新扫描」只刷新磁盘大文件模块
  const refresh = useCallback(() => {
    void startScan("large");
  }, [startScan]);

  // large 模块同时产出两类：category === "目录" 的是大目录排行，其余是按文件
  // 类型（视频/磁盘镜像/压缩包/AI模型…）分类的超大单文件。
  const files = useMemo(() => items.filter((i) => i.category !== "目录"), [items]);
  const dirs = useMemo(() => items.filter((i) => i.category === "目录"), [items]);
  const list = view === "files" ? files : dirs;

  const filesSize = files.reduce((s, i) => s + i.size_bytes, 0);
  const dirsSize = dirs.reduce((s, i) => s + i.size_bytes, 0);
  const total = list.reduce((s, i) => s + i.size_bytes, 0);

  const segName = useCallback(
    (it: (typeof items)[number]) =>
      view === "files" ? it.category : shortPath(it.path).split("/").pop() || it.category,
    [view]
  );

  const segs = useMemo<Seg[]>(() => {
    const m = new Map<string, number>();
    for (const it of list) m.set(segName(it), (m.get(segName(it)) ?? 0) + it.size_bytes);
    return [...m.entries()]
      .sort((a, b) => b[1] - a[1])
      .slice(0, 10)
      .map(([name, size], idx) => ({
        name,
        size,
        pct: total ? (size / total) * 100 : 0,
        color: COLORS[idx % COLORS.length],
      }));
  }, [list, total, segName]);

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

  const selectable = useMemo(() => list.filter((i) => i.deletable), [list]);
  const selectedItems = useMemo(
    () => list.filter((i) => checked.has(i.path) && i.deletable),
    [list, checked]
  );
  const selectedSize = selectedItems.reduce((s, i) => s + i.size_bytes, 0);

  const toggleCheck = (path: string) =>
    setChecked((s) => {
      const n = new Set(s);
      if (n.has(path)) n.delete(path);
      else n.add(path);
      return n;
    });

  const runDelete = async () => {
    const reqs: CleanItemReq[] = selectedItems.map((i) => ({
      path: i.path,
      category: i.category,
      batch_paths: i.batch_paths,
      size_bytes: i.size_bytes,
      recommend: i.recommend,
    }));

    // Rust 侧 dry-run 复核：safety 闸门 + 当前可删除性
    let preview;
    try {
      preview = await ipc.cleanPreview(reqs);
    } catch (e) {
      toast("warn", "安全预检失败：" + String(e));
      return;
    }
    const blocked = preview.filter((p) => !p.allowed);
    const allowed = preview.filter((p) => p.allowed);
    if (allowed.length === 0) {
      toast("warn", "所选项目均未通过安全检查，未执行删除");
      return;
    }
    const byKey = new Map(allowed.map((p) => [p.path + "|" + p.category, p]));
    const finalReqs = reqs.filter((x) => byKey.has(x.path + "|" + x.category));
    const finalPaths = finalReqs.map((x) => x.path);
    if (blocked.length > 0) {
      toast("info", `${blocked.length} 项未通过安全检查，已自动排除`);
    }

    const noun = view === "files" ? "大文件" : "大目录";
    confirm({
      title: `删除选中的 ${finalReqs.length} 个${noun}？`,
      sub: `将把它们移入废纸篓（共 ${fmt(
        selectedSize
      )}），误删可从废纸篓恢复。`,
      warn:
        view === "files"
          ? "这些是你的个人超大文件（视频 / 磁盘镜像 / 压缩包 / AI 模型等），删除后可能需要重新下载。请确认你了解每一项的用途后再继续。"
          : "这些是占用较大的文件夹，可能包含项目代码、虚拟机镜像、开发环境或数据集；删除后可能需要重新下载、重新构建或重新登录。请确认你了解每一项的用途后再继续。",
      items: finalPaths,
      confirmText: `移入废纸篓 ${fmt(selectedSize)}`,
      onConfirm: async () => {
        setBusy(true);
        try {
          const rep = await executeClean(finalReqs);
          if (rep.cancelled) {
            toast("warn", "已取消");
          } else {
            toast(
              "success",
              `已移入废纸篓 ${rep.deleted} 项${rep.intercepted ? `，拦截 ${rep.intercepted} 项` : ""}${
                rep.need_password ? `，${rep.need_password} 项需要管理员权限` : ""
              }`
            );
            // 从全局缓存移除已删项，不自动重扫；需要最新占用请手动重新扫描
            removePaths("large", new Set(finalReqs.map((x) => x.path)));
            setChecked(new Set());
          }
        } catch (e) {
          toast("warn", "删除失败：" + String(e));
        } finally {
          setBusy(false);
        }
      },
    });
  };

  return (
    <div>
      <PageHeader
        title="磁盘分析"
        sub="大文件（单个 ≥100MB）与大目录空间占用；勾选后可移入废纸篓"
        action={
          <button
            className="btn-secondary"
            onClick={hasScanned ? refresh : startFullScan}
            style={{ height: 34 }}
          >
            <Icon name="refresh" size={14} /> {hasScanned ? "重新扫描" : "开始扫描"}
          </button>
        }
      />

      {items.length === 0 ? (
        <Empty
          icon="analysis"
          text={
            hasScanned
              ? "暂未发现大文件"
              : largeBusy
                ? "正在分析磁盘占用，大文件结果即将呈现（缓存等其它模块会在后台继续，不影响此处操作）…"
                : scanning
                  ? "正在全盘体检，磁盘大文件模块完成后即可在此操作…"
                  : "尚未扫描，点击下方按钮开始一次全盘体检（只读扫描）"
          }
          action={
            !scanning && !hasScanned ? (
              <button className="btn-primary" onClick={startFullScan}>
                <Icon name="zap" size={15} /> 开始扫描
              </button>
            ) : undefined
          }
        />
      ) : (
        <>
          <div className="view-switch" role="tablist" aria-label="磁盘分析视图">
            <button
              type="button"
              role="tab"
              aria-selected={view === "files"}
              className={view === "files" ? "active" : ""}
              onClick={() => setView("files")}
              title="递归找到的单个超大文件（视频 / 镜像 / 压缩包 / AI 模型等）"
            >
              <Icon name="file" size={15} />
              大文件
              <span className="vs-meta">
                {files.length} 个 · {fmt(filesSize)}
              </span>
            </button>
            <button
              type="button"
              role="tab"
              aria-selected={view === "dirs"}
              className={view === "dirs" ? "active" : ""}
              onClick={() => setView("dirs")}
              title="主目录下各顶层文件夹的占用排行"
            >
              <Icon name="folder" size={15} />
              大目录
              <span className="vs-meta">
                {dirs.length} 个 · {fmt(dirsSize)}
              </span>
            </button>
          </div>

          {selectable.length > 0 && (
            <div className="select-bar">
              <span className="select-info">
                {selectable.length} 个可处理{view === "files" ? "大文件" : "大目录"} · 已选{" "}
                {selectedItems.length} 项 · {fmt(selectedSize)}
              </span>
              <div className="grow" />
              <button
                className="btn-secondary select-btn"
                onClick={() => setChecked(new Set(selectable.map((i) => i.path)))}
                disabled={largeBusy || busy}
                title={`均为高级风险项，全选${view === "files" ? "大文件" : "大目录"}后请逐项确认用途`}
              >
                全选
              </button>
              <button
                className="btn-secondary select-btn"
                onClick={() => setChecked(new Set())}
                disabled={largeBusy || busy || checked.size === 0}
              >
                清空
              </button>
            </div>
          )}

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
                  <span className="sub">{view === "files" ? "大文件合计" : "大目录合计"}</span>
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
                {view === "files" ? "占用最大的大文件" : "占用最大的大目录"}
              </div>
              <div className="dirtree">
                {list.length === 0 ? (
                  <div className="view-empty">
                    <Icon name="analysis" size={26} />
                    <div className="muted" style={{ marginTop: 8 }}>
                      {view === "files"
                        ? "未发现单个 ≥100MB 的大文件"
                        : "未发现可展示的大目录"}
                    </div>
                    {view === "files" && dirs.length > 0 && (
                      <button
                        type="button"
                        className="btn-secondary select-btn"
                        style={{ marginTop: 10 }}
                        onClick={() => setView("dirs")}
                      >
                        切换到「大目录」查看文件夹占用
                      </button>
                    )}
                  </div>
                ) : (
                  list.slice(0, 40).map((it, idx) => {
                  const k = it.path;
                  const isOpen = open.has(k);
                  const isChecked = checked.has(k);
                  const maxSize = list[0]?.size_bytes || 1;
                  const pctv = total ? Math.min(100, (it.size_bytes / maxSize) * 100) : 0;
                  const baseName = shortPath(it.path).split("/").pop();
                  return (
                    <div key={k}>
                      <div
                        className={`dir-row${isOpen ? " open" : ""}${isChecked ? " checked" : ""}${it.deletable ? "" : " disabled"}`}
                        onClick={() => toggle(k)}
                      >
                        {it.deletable ? (
                          <span
                            className={`chek${isChecked ? " on" : ""}`}
                            title="选择后可移入废纸篓"
                            onClick={(e) => {
                              e.stopPropagation();
                              toggleCheck(k);
                            }}
                          >
                            <Icon name="check" size={12} />
                          </span>
                        ) : (
                          <span className="chek disabled" title={it.undeletable_reason || "受保护，不可删除"}>
                            <Icon name="lock" size={11} />
                          </span>
                        )}
                        <span className="chev-click">
                          <Icon name="chev" size={13} className="chev" />
                        </span>
                        <span className="nm" title={it.path}>
                          {baseName}
                        </span>
                        {view === "files" && (
                          <span className="badge ghost ftype">{it.category}</span>
                        )}
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
                          {!it.deletable && (
                            <div className="muted" style={{ fontSize: 11.5, marginTop: 3, color: "var(--caution)" }}>
                              {it.undeletable_reason || "受系统保护，不可删除"}
                            </div>
                          )}
                        </div>
                      )}
                    </div>
                  );
                  })
                )}
              </div>
            </div>
          </div>
        </>
      )}

      {selectedItems.length > 0 && (
        <div className="summary-bar">
          <span className="sel">
            已选 <b>{selectedItems.length}</b> 项 · 共 <b>{fmt(selectedSize)}</b>
          </span>
          <span className="sel-risk">
            <Icon name="warning" size={14} />
            {view === "files"
              ? "均为你的个人超大文件，删除后可能需重新下载"
              : "均为高级风险项，可能是项目 / 虚拟机 / 开发环境，删除后可能需重新下载或构建"}
          </span>
          <div className="grow" />
          <button
            className="btn-primary danger"
            onClick={runDelete}
            disabled={busy || largeBusy}
            title="缓存等其它模块仍在后台扫描不影响此处；仅在大文件模块刷新时暂禁"
          >
            <Icon name="trash" size={16} />
            {busy ? "正在删除…" : `移入废纸篓 ${fmt(selectedSize)}`}
          </button>
        </div>
      )}
    </div>
  );
}
