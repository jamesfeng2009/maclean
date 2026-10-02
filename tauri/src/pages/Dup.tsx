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
    runningScopes,
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
  // 仅在重复文件模块自身扫描时禁用勾选/删除；其它模块的后台扫描不影响本页
  const dupBusy = runningScopes.includes("dup");
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

  const selectable = useMemo(() => items.filter((i) => i.deletable), [items]);
  const selected = useMemo(
    () => items.filter((i) => checked.has(i.path) && i.deletable),
    [items, checked]
  );
  const selectedSize = selected.reduce((s, i) => s + i.size_bytes, 0);
  // 勾选项里将被删除的「副本」文件总数（每组 = batch_paths 数量，保留 1 份）
  const selectedCopyCount = selected.reduce((n, i) => n + i.batch_paths.length, 0);

  const selectAll = () =>
    setChecked(new Set(selectable.map((i) => i.path)));
  const clearAll = () => setChecked(new Set());

  const runDedup = async () => {
    const reqs: CleanItemReq[] = selected.map((i) => ({
      path: i.path,
      category: i.category,
      batch_paths: i.batch_paths,
      size_bytes: i.size_bytes,
      recommend: i.recommend,
    }));

    // Rust 侧 dry-run 复核：safety 闸门 + 当前可删除性（与磁盘分析一致，预检在前）
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
      toast("warn", "所选重复组均未通过安全检查，未执行删除");
      return;
    }
    const byKey = new Map(allowed.map((p) => [p.path + "|" + p.category, p]));
    const finalReqs = reqs.filter((x) => byKey.has(x.path + "|" + x.category));
    // 仅汇总通过预检组的副本路径
    const copyPaths = finalReqs.flatMap((r) => r.batch_paths);
    const copySize = finalReqs.reduce((s, r) => s + r.size_bytes, 0);
    if (blocked.length > 0) {
      toast("info", `${blocked.length} 组未通过安全检查，已自动排除`);
    }

    confirm({
      title: `删除选中的 ${finalReqs.length} 组重复副本？`,
      sub: `将删除 ${copyPaths.length} 个重复副本、回收约 ${fmt(
        copySize
      )}；每组保留 1 份最新 / 项目内文件，副本移入废纸篓，误删可恢复。`,
      warn: "只会删除下列「副本」文件，每组标记为「保留」的那一份不会动。请确认保留的是你需要的版本后再继续。",
      items: copyPaths,
      confirmText: `删除副本 ${fmt(copySize)}`,
      onConfirm: async () => {
        setBusy(true);
        try {
          const rep = await ipc.cleanExecute(finalReqs, langEn);
          if (rep.cancelled) {
            toast("warn", "已取消");
          } else {
            toast(
              "success",
              `已删除 ${rep.deleted} 组副本${rep.intercepted ? `，拦截 ${rep.intercepted} 个` : ""}`
            );
            // 从全局缓存移除已删组，不自动重扫；需要最新列表请手动重新扫描
            removePaths("dup", new Set(finalReqs.map((r) => r.path)));
            setChecked(new Set());
          }
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
        <>
          {selectable.length > 0 && (
            <div className="select-bar">
              <span className="select-info">
                {selectable.length} 个可清理重复组 · 已选 {selected.length} 组 ·{" "}
                {selectedCopyCount} 个副本 · 可回收 {fmt(selectedSize)}
              </span>
              <div className="grow" />
              <button
                className="btn-secondary select-btn"
                onClick={selectAll}
                disabled={dupBusy || busy}
                title="勾选全部可清理重复组（每组仍保留 1 份）"
              >
                全选
              </button>
              <button
                className="btn-secondary select-btn"
                onClick={clearAll}
                disabled={dupBusy || busy || checked.size === 0}
              >
                清空
              </button>
            </div>
          )}

          {items.map((it) => {
            const k = it.path;
            const isOn = checked.has(k);
            const isOpen = open.has(k);
            return (
              <div
                key={k}
                className={`dup-group${!it.deletable ? " disabled" : ""}`}
              >
                <div
                  className={`dup-head${isOpen ? " open" : ""}`}
                  onClick={() => setOpen((s) => toggle(s, k))}
                >
                  <Icon name="chev" size={13} className="chev" />
                  {it.deletable ? (
                    <span
                      className={`chek${isOn ? " on" : ""}`}
                      title="选择后可删除多余副本（每组保留 1 份）"
                      onClick={(e) => {
                        e.stopPropagation();
                        setChecked((s) => toggle(s, k));
                      }}
                    >
                      <Icon name="check" size={11} />
                    </span>
                  ) : (
                    <span
                      className="chek disabled"
                      title={it.undeletable_reason || "受保护，不可删除"}
                      onClick={(e) => e.stopPropagation()}
                    >
                      <Icon name="lock" size={10} />
                    </span>
                  )}
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
                  {!it.deletable && (
                    <div className="dup-reason">
                      {it.undeletable_reason || "受系统保护，该组不可删除"}
                    </div>
                  )}
                </div>
              </div>
            );
          })}
        </>
      )}

      {selected.length > 0 && (
        <div className="summary-bar">
          <span className="sel">
            已选 <b>{selected.length}</b> 组 · <b>{selectedCopyCount}</b> 个副本 · 可回收{" "}
            <b>{fmt(selectedSize)}</b>
          </span>
          <span className="sel-risk">
            <Icon name="warning" size={14} />
            每组保留 1 份最新 / 项目内文件，仅删除多余副本并移入废纸篓，可恢复
          </span>
          <div className="grow" />
          <button className="btn-primary danger" onClick={runDedup} disabled={busy || dupBusy}>
            <Icon name="trash" size={16} /> {busy ? "正在删除…" : `删除副本 ${fmt(selectedSize)}`}
          </button>
        </div>
      )}
    </div>
  );
}
