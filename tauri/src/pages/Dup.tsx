import { useCallback, useEffect, useMemo, useState } from "react";
import { ipc } from "../lib/ipc";
import { fmt, shortPath } from "../lib/format";
import type { CleanItemReq } from "../lib/types";
import { useApp } from "../lib/store";
import { Icon } from "../components/Icon";
import { Empty, PageHeader } from "../components/ui";

export function Dup() {
  const {
    startScan,
    scanning,
    confirm,
    toast,
    langEn,
    results,
    ready,
    scanTick,
    lastScope,
    removePaths,
  } = useApp();
  // 复用全局缓存的 dup 模块
  const items = results.dup;
  const hasScanned = ready.dup;
  const [checked, setChecked] = useState<Set<string>>(new Set());
  const [open, setOpen] = useState<Set<string>>(new Set());
  const [busy, setBusy] = useState(false);

  // 页头「重新扫描」只刷新重复文件模块
  const refresh = useCallback(() => {
    void startScan("dup");
  }, [startScan]);

  // dup 模块结果写入后默认勾选全部可清理组（扫描器保证保留最新/项目内副本）
  useEffect(() => {
    if (lastScope === "dup" && results.dup.length > 0) {
      setChecked(new Set(results.dup.filter((i) => i.deletable).map((i) => i.path)));
    }
    // 仅以扫描完成计数为触发
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [scanTick]);

  const toggle = (set: Set<string>, k: string) => {
    const n = new Set(set);
    if (n.has(k)) n.delete(k);
    else n.add(k);
    return n;
  };

  const selected = useMemo(
    () => items.filter((i) => checked.has(i.path) && i.deletable),
    [items, checked]
  );
  const selectedSize = selected.reduce((s, i) => s + i.size_bytes, 0);

  const runDedup = () => {
    const reqs: CleanItemReq[] = selected.map((i) => ({
      path: i.path,
      category: i.category,
      batch_paths: i.batch_paths,
      size_bytes: i.size_bytes,
      recommend: i.recommend,
    }));
    const copyPaths = reqs.flatMap((r) => r.batch_paths);
    confirm({
      title: "确认删除重复副本？",
      sub: `将删除 ${copyPaths.length} 个重复副本，每组保留 1 份最新文件（移入废纸篓，可恢复）。`,
      warn: "core 只会删除 batch_paths 中的副本，保留每组标记为「保留」的文件。",
      items: copyPaths,
      confirmText: `删除副本 ${fmt(selectedSize)}`,
      onConfirm: async () => {
        setBusy(true);
        try {
          let preview;
          try {
            preview = await ipc.cleanPreview(reqs);
          } catch (e) {
            toast("warn", "安全预检失败：" + String(e));
            return;
          }
          const okKeys = new Set(
            preview.filter((p) => p.allowed).map((p) => p.path + "|" + p.category)
          );
          const finalReqs = reqs.filter((r) =>
            okKeys.has(r.path + "|" + r.category)
          );
          if (finalReqs.length === 0) {
            toast("warn", "副本均未通过安全检查，未执行");
            return;
          }
          const rep = await ipc.cleanExecute(finalReqs, langEn);
          toast(
            "success",
            `已删除 ${rep.deleted} 组副本${rep.intercepted ? `，拦截 ${rep.intercepted} 个` : ""}`
          );
          // 从全局缓存移除已删组，不自动重扫；需要最新列表请手动重新扫描
          const removed = new Set(finalReqs.map((r) => r.path));
          removePaths("dup", removed);
          setChecked(new Set());
        } catch (e) {
          toast("warn", "去重失败：" + String(e));
        } finally {
          setBusy(false);
        }
      },
    });
  };

  return (
    <div>
      <PageHeader
        title="重复文件"
        sub="相同内容的文件只保留 1 份（最新 / 项目内优先），其余移入废纸篓"
        action={
          <button
            className="btn-secondary"
            onClick={refresh}
            style={{ height: 34 }}
          >
            <Icon name="refresh" size={14} /> {hasScanned ? "重新扫描" : "开始扫描"}
          </button>
        }
      />

      {items.length === 0 ? (
        <Empty
          icon="dup"
          text={
            hasScanned
              ? "没有发现重复文件"
              : scanning
                ? "正在查找重复文件（逐文件比对，文件较多时可能需要几分钟）…"
                : "重复文件需逐文件内容比对，计算较重、耗时较长，建议在需要时单独扫描（只读扫描）"
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
        items.map((it) => {
          const k = it.path;
          const isOn = checked.has(k);
          const isOpen = open.has(k);
          return (
            <div
              key={k}
              className={`dup-group${!it.deletable ? " disabled" : ""}`}
              style={!it.deletable ? { opacity: 0.55 } : undefined}
            >
              <div
                className={`dup-head${isOpen ? " open" : ""}`}
                onClick={() => setOpen((s) => toggle(s, k))}
              >
                <Icon name="chev" size={13} className="chev" />
                <span
                  className="chek"
                  style={{
                    width: 18,
                    height: 18,
                    borderRadius: 5,
                    border: "1.5px solid var(--line-2)",
                    display: "flex",
                    alignItems: "center",
                    color: "#fff",
                    background: isOn ? "var(--brand)" : "transparent",
                    borderColor: isOn ? "var(--brand)" : "var(--line-2)",
                  }}
                  onClick={(e) => {
                    e.stopPropagation();
                    if (it.deletable) setChecked((s) => toggle(s, k));
                  }}
                >
                  <Icon name="check" size={11} />
                </span>
                <span className="nm" title={it.path}>
                  {shortPath(it.path).split("/").pop()}
                </span>
                <span className="badge ghost">{it.batch_paths.length + 1} 个副本</span>
                <span style={{ fontWeight: 700, fontFamily: "ui-monospace,Menlo,monospace", fontSize: 13 }}>
                  {fmt(it.size_bytes)}
                </span>
              </div>
              <div className="dup-body" style={{ display: isOpen ? "flex" : "none" }}>
                <div className="dup-file">
                  <span className="fp" title={it.path}>
                    {shortPath(it.path)}
                  </span>
                  <span className="keep">保留</span>
                  <span className="sz">{fmt(it.size_bytes / (it.batch_paths.length + 1))}</span>
                </div>
                {it.batch_paths.map((bp) => (
                  <div key={bp} className="dup-file">
                    <span className="fp" title={bp}>
                      {shortPath(bp)}
                    </span>
                    <span className="sz">副本 · 删除</span>
                  </div>
                ))}
              </div>
            </div>
          );
        })
      )}

      {selected.length > 0 && (
        <div className="summary-bar">
          <span className="sel">
            已选 <b>{selected.length}</b> 组 · 可回收 <b>{fmt(selectedSize)}</b>
          </span>
          <div className="grow" />
          <button className="btn-primary danger" onClick={runDedup} disabled={busy || scanning}>
            <Icon name="trash" size={16} /> {busy ? "正在删除…" : "删除重复副本"}
          </button>
        </div>
      )}
    </div>
  );
}
