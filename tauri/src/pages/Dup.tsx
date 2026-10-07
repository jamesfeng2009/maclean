import { useCallback, useEffect, useMemo, useState } from "react";
import { ipc } from "../lib/ipc";
import { fmt, shortPath } from "../lib/format";
import type { CleanItemReq, ScanItem } from "../lib/types";
import { useApp } from "../lib/store";
import { useDeleteStrategy, planDelete, deleteSubText } from "../lib/deletePolicy";
import { Icon } from "../components/Icon";
import { Empty, PageHeader } from "../components/ui";

/** 文件类型分类（对标 MangoDisk 的 All/Video/Audio/Images/Documents/Archives/AI models/Other） */
const DUP_KINDS = [
  { key: "all", label: "All" },
  { key: "video", label: "Video" },
  { key: "audio", label: "Audio" },
  { key: "images", label: "Images" },
  { key: "documents", label: "Documents" },
  { key: "archives", label: "Archives" },
  { key: "ai", label: "AI models" },
  { key: "other", label: "Other" },
] as const;
type DupKind = (typeof DUP_KINDS)[number]["key"];

const VIDEO_EXT = new Set(["mp4", "mov", "mkv", "avi", "webm", "flv", "wmv", "m4v", "mts", "m2ts"]);
const AUDIO_EXT = new Set(["mp3", "wav", "flac", "aac", "m4a", "ogg", "opus", "wma", "aiff"]);
const IMAGE_EXT = new Set(["jpg", "jpeg", "png", "gif", "webp", "heic", "heif", "tiff", "bmp", "svg", "raw", "avif"]);
const DOC_EXT = new Set([
  "pdf", "doc", "docx", "xls", "xlsx", "ppt", "pptx", "txt", "md", "csv",
  "json", "html", "htm", "epub", "rtf", "pages", "key", "numbers",
]);
const ARCHIVE_EXT = new Set(["zip", "rar", "7z", "tar", "gz", "bz2", "xz", "tgz", "dmg", "iso", "zst", "cab"]);
const AI_EXT = new Set(["onnx", "gguf", "safetensors", "pt", "pth", "caffemodel", "tflite", "pb", "h5", "torch", "bin"]);
const AI_PATH_RE = /(huggingface|torchhub|\.cache\/models|models--|deepset|mlc-llm)/i;

function kindOf(it: ScanItem): Exclude<DupKind, "all"> {
  const name = it.path.split("/").pop()?.toLowerCase() ?? "";
  const ex = name.includes(".") ? name.split(".").pop()! : "";
  if (VIDEO_EXT.has(ex)) return "video";
  if (AUDIO_EXT.has(ex)) return "audio";
  if (IMAGE_EXT.has(ex)) return "images";
  if (DOC_EXT.has(ex)) return "documents";
  if (ARCHIVE_EXT.has(ex)) return "archives";
  if (AI_EXT.has(ex) || AI_PATH_RE.test(it.path)) return "ai";
  return "other";
}

/** 最小大小筛选（本地过滤显示；扫描器下限为 200KB） */
const SIZE_FILTERS: Array<{ key: string; label: string; bytes: number }> = [
  { key: "all", label: "全部大小", bytes: 0 },
  { key: "200k", label: "≥ 200 KB", bytes: 200 * 1024 },
  { key: "1m", label: "≥ 1 MB", bytes: 1024 * 1024 },
  { key: "10m", label: "≥ 10 MB", bytes: 10 * 1024 * 1024 },
  { key: "100m", label: "≥ 100 MB", bytes: 100 * 1024 * 1024 },
];

const fmtMtime = (s: number) => {
  if (!s) return "—";
  const d = new Date(s * 1000);
  const p = (n: number) => String(n).padStart(2, "0");
  return `${p(d.getMonth() + 1)}/${p(d.getDate())}/${d.getFullYear()} ${p(d.getHours())}:${p(
    d.getMinutes()
  )}`;
};

export function Dup() {
  const {
    startScan,
    scanning,
    runningScopes,
    confirm,
    toast,
    results,
    ready,
    scanTick,
    lastScope,
    removePaths,
    removeDupCopy,
    executeClean,
  } = useApp();
  // 复用全局缓存的 dup 模块
  const items = results.dup;
  const hasScanned = ready.dup;
  // 仅在重复文件模块自身扫描时禁用勾选/删除；其它模块的后台扫描不影响本页
  const dupBusy = runningScopes.includes("dup");
  const [checked, setChecked] = useState<Set<string>>(new Set());
  const [open, setOpen] = useState<Set<string>>(new Set());
  const [busy, setBusy] = useState(false);
  const [kind, setKind] = useState<DupKind>("all");
  const [minSize, setMinSize] = useState(0);
  const delStrategy = useDeleteStrategy();

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
  // 当前 类型 + 大小 筛选后的可见项
  const visible = useMemo(
    () =>
      items.filter(
        (i) => (kind === "all" || kindOf(i) === kind) && i.size_bytes >= minSize
      ),
    [items, kind, minSize]
  );
  const kindCounts = useMemo(() => {
    const m = new Map<DupKind, number>();
    for (const i of items) {
      const k = kindOf(i);
      m.set(k, (m.get(k) ?? 0) + 1);
    }
    return m;
  }, [items]);
  // 顶部统计：组数 + 最多可回收（对标 MangoDisk 的 Found N groups · can reclaim up to X）
  const reclaimable = selectable.reduce((s, i) => s + i.size_bytes, 0);

  const selected = useMemo(
    () => items.filter((i) => checked.has(i.path) && i.deletable),
    [items, checked]
  );
  const selectedSize = selected.reduce((s, i) => s + i.size_bytes, 0);
  // 勾选项里将被删除的「副本」文件总数（每组 = batch_paths 数量，保留 1 份）
  const selectedCopyCount = selected.reduce((n, i) => n + i.batch_paths.length, 0);

  const selectAll = () => setChecked(new Set(selectable.map((i) => i.path)));
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

    const plan = planDelete(finalReqs, delStrategy);
    confirm({
      title: `删除选中的 ${finalReqs.length} 组重复副本？`,
      sub: `将删除 ${copyPaths.length} 个重复副本、回收约 ${fmt(
        copySize
      )}；每组保留 1 份最新 / 项目内文件。${deleteSubText(plan)}。`,
      warn: "只会删除下列「副本」文件，每组标记为「保留」的那一份不会动。请确认保留的是你需要的版本后再继续。",
      items: copyPaths,
      confirmText: `删除副本 ${fmt(copySize)}`,
      confirmToggle:
        delStrategy !== "trash" && plan.safeN > 0
          ? {
              label: "本次也把重复副本移入废纸篓（更稳妥、清空后才释放空间）",
              defaultOn: false,
            }
          : undefined,
      onConfirm: async (forceTrashSafe) => {
        setBusy(true);
        try {
          const rep = await executeClean(finalReqs, forceTrashSafe === true);
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

  /** 行级删除单个副本（hover 垃圾桶）：只删这一份，独立过闸，其余副本保留 */
  const removeCopy = async (it: ScanItem, copyPath: string) => {
    if (busy || dupBusy) return;
    // 单份大小（同组等大）：组大小 / 份数
    const per = Math.round(it.size_bytes / (it.batch_paths.length + 1));
    const req: CleanItemReq[] = [
      {
        path: copyPath,
        category: it.category,
        batch_paths: [],
        size_bytes: per,
        recommend: it.recommend,
      },
    ];
    let preview;
    try {
      preview = await ipc.cleanPreview(req);
    } catch (e) {
      toast("warn", "安全预检失败：" + String(e));
      return;
    }
    if (preview[0] && !preview[0].allowed) {
      toast("warn", "该副本未通过安全检查，未删除");
      return;
    }
    const plan = planDelete(req, delStrategy);
    confirm({
      title: "删除这一个重复副本？",
      sub: `将删除 ${copyPath}${deleteSubText(plan) ? `，${deleteSubText(plan)}` : ""}；该组其余副本与保留版本不受影响。`,
      items: [copyPath],
      confirmText: `删除副本 ${fmt(per)}`,
      confirmToggle:
        delStrategy !== "trash" && plan.safeN > 0
          ? {
              label: "本次也移入废纸篓（更稳妥、清空后才释放空间）",
              defaultOn: false,
            }
          : undefined,
      onConfirm: async (forceTrashSafe) => {
        setBusy(true);
        try {
          const rep = await executeClean(req, forceTrashSafe === true);
          if (rep.cancelled) {
            toast("warn", "已取消");
          } else {
            toast("success", `已删除 1 个副本${rep.intercepted ? `，拦截 ${rep.intercepted} 个` : ""}`);
            removeDupCopy(it.path, copyPath);
          }
        } catch (e) {
          toast("warn", "删除副本失败：" + String(e));
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
        sub="相同内容的文件只保留 1 份（最新 / 项目内优先）；副本按删除方式清理，可在设置中选择永久删除或移入废纸篓"
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
          {/* 顶部统计（对标 MangoDisk）：组数 + 最多可回收 */}
          <div className="dup-stats">
            <span className="dup-found">
              Found <b>{items.length}</b> duplicate groups · can reclaim up to{" "}
              <b>{fmt(reclaimable)}</b>
            </span>
            <div className="grow" />
            <label className="dup-size-filter">
              最小大小
              <select
                value={minSize}
                onChange={(e) => setMinSize(Number(e.target.value))}
              >
                {SIZE_FILTERS.map((f) => (
                  <option key={f.key} value={f.bytes}>
                    {f.label}
                  </option>
                ))}
              </select>
            </label>
          </div>

          {/* 文件类型分类标签（对标 MangoDisk） */}
          <div className="dup-kinds">
            {DUP_KINDS.map((k) => (
              <button
                key={k.key}
                className={`dup-kind${kind === k.key ? " on" : ""}`}
                onClick={() => setKind(k.key)}
              >
                {k.label} <span className="dup-kind-n">{k.key === "all" ? items.length : kindCounts.get(k.key as Exclude<DupKind, "all">) ?? 0}</span>
              </button>
            ))}
          </div>

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

          {visible.map((it) => {
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
                    <span className="dup-file-mtime">保留 · 不删除</span>
                    <span className="sz">{fmt(it.size_bytes / (it.batch_paths.length + 1))}</span>
                  </div>
                  {it.batch_paths.map((bp, idx) => (
                    <div key={bp} className="dup-file">
                      <span className="fp" title={bp}>
                        {shortPath(bp)}
                      </span>
                      <span className="dup-file-mtime">{fmtMtime(it.batch_mtimes[idx] ?? 0)}</span>
                      <span className="sz">副本 · 删除</span>
                      <span
                        className="row-del"
                        title="删除这一个副本（其余保留）"
                        onClick={(e) => {
                          e.stopPropagation();
                          void removeCopy(it, bp);
                        }}
                      >
                        <Icon name="trash" size={13} />
                      </span>
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
            每组保留 1 份最新 / 项目内文件，仅删除多余副本；默认永久删除以释放空间，确认时可改为移入废纸篓
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
