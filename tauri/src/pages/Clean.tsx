import { useCallback, useEffect, useMemo, useState } from "react";
import { ipc } from "../lib/ipc";
import { fmt, shortPath } from "../lib/format";
import type { CleanItemReq, ScanItem } from "../lib/types";
import { useApp } from "../lib/store";
import { Icon } from "../components/Icon";
import { Badge, Empty, PageHeader, defaultSelected } from "../components/ui";

interface Group {
  name: string;
  items: ScanItem[];
  size: number;
}

const GROUP_ICON: Record<string, string> = {
  浏览器缓存: "folder",
  系统日志: "doc",
  安装包: "file",
  模拟器镜像: "cpu",
  AI模型: "model",
};

function groupIcon(name: string): string {
  if (GROUP_ICON[name]) return GROUP_ICON[name];
  if (name.includes("Docker") || name.includes("K8s")) return "container";
  if (name.includes("模型") || name.includes("缓存")) return "cpu";
  return "folder";
}

export function Clean() {
  const { startScan, scanning, confirm, toast, langEn } = useApp();
  const [items, setItems] = useState<ScanItem[]>([]);
  const [checked, setChecked] = useState<Set<string>>(new Set());
  const [open, setOpen] = useState<Set<string>>(new Set());
  const [busy, setBusy] = useState(false);

  const refresh = useCallback(() => {
    startScan("all", (its) => {
      setItems(its);
      // 默认勾选：安全/缓存组（与 core default_selected 一致）
      const g = new Map<string, boolean>();
      for (const it of its) {
        if (it.deletable && defaultSelected(it.recommend)) g.set(it.category, true);
      }
      setChecked(new Set(g.keys()));
    });
  }, [startScan]);

  useEffect(refresh, [refresh]);

  const groups = useMemo<Group[]>(() => {
    const m = new Map<string, Group>();
    for (const it of items) {
      let g = m.get(it.category);
      if (!g) {
        g = { name: it.category, items: [], size: 0 };
        m.set(it.category, g);
      }
      g.items.push(it);
      if (it.deletable) g.size += it.size_bytes;
    }
    return [...m.values()].sort((a, b) => b.size - a.size);
  }, [items]);

  const selectedItems = useMemo(
    () =>
      items.filter(
        (i) => checked.has(i.category) && i.deletable
      ),
    [items, checked]
  );
  const selectedSize = selectedItems.reduce((s, i) => s + i.size_bytes, 0);
  const hasAdvanced = selectedItems.some(
    (i) => i.recommend === "Advanced" || i.recommend === "Caution"
  );

  const toggleGroup = (name: string) => {
    setChecked((s) => {
      const n = new Set(s);
      if (n.has(name)) n.delete(name);
      else n.add(name);
      return n;
    });
  };
  const toggleOpen = (name: string) =>
    setOpen((s) => {
      const n = new Set(s);
      if (n.has(name)) n.delete(name);
      else n.add(name);
      return n;
    });

  const runClean = async () => {
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
    const finalReqs = reqs.filter((r) => byKey.has(r.path + "|" + r.category));
    const paths = finalReqs.flatMap((r) => [r.path, ...r.batch_paths]);

    if (blocked.length > 0) {
      toast("info", `${blocked.length} 项未通过安全检查，已自动排除`);
    }

    confirm({
      title: "确认清理选中项目？",
      sub: `将把 ${finalReqs.length} 项（含副本共 ${paths.length} 个路径）移入废纸篓，可恢复。`,
      warn: hasAdvanced
        ? "其中包含「注意/高级」风险项：请确认你了解这些文件的用途，删除后可能需要重新下载或登录。"
        : "操作会先经过 maclean-core 安全闸门；系统关键目录会被拦截。",
      items: paths,
      confirmText: `安全清理 ${fmt(selectedSize)}`,
      onConfirm: async () => {
        setBusy(true);
        try {
          const rep = await ipc.cleanExecute(finalReqs, langEn);
          if (rep.cancelled) {
            toast("warn", "清理已取消");
          } else {
            toast(
              "success",
              `已清理 ${rep.deleted} 项${rep.intercepted ? `，拦截 ${rep.intercepted} 项` : ""}${
                rep.need_password ? `，${rep.need_password} 项需要管理员权限（请用桌面版）` : ""
              }`
            );
            refresh();
          }
        } catch (e) {
          toast("warn", "清理失败：" + String(e));
        } finally {
          setBusy(false);
        }
      },
    });
  };

  return (
    <div>
      <PageHeader
        title="智能清理"
        sub="选择要清理的分类；所有删除都会过安全闸门并优先移入废纸篓"
        action={
          <button className="btn-secondary" onClick={refresh} style={{ height: 34 }}>
            <Icon name="refresh" size={14} /> 重新扫描
          </button>
        }
      />

      {groups.length === 0 ? (
        <Empty icon="clean" text={scanning ? "正在扫描可清理项…" : "暂未发现可清理项"} />
      ) : (
        <div className="cat-list">
          {groups.map((g) => {
            const isOn = checked.has(g.name);
            const isOpen = open.has(g.name);
            const deletableCount = g.items.filter((i) => i.deletable).length;
            return (
              <div key={g.name}>
                <div
                  className={`cat-card${isOn ? " checked" : ""}${deletableCount === 0 ? " disabled" : ""}`}
                  onClick={() => deletableCount > 0 && toggleGroup(g.name)}
                >
                  <span className="chek">
                    <Icon name="check" size={12} />
                  </span>
                  <span
                    className="ico"
                    style={{ background: "var(--brand-50)", color: "var(--brand)" }}
                  >
                    <Icon name={groupIcon(g.name)} size={20} />
                  </span>
                  <div className="meta">
                    <div className="t">{g.name}</div>
                    <div className="d">
                      {g.items.length} 项
                      {deletableCount < g.items.length
                        ? `（${g.items.length - deletableCount} 项受保护不可删）`
                        : ""}
                    </div>
                  </div>
                  <span className="sz">{fmt(g.size)}</span>
                  <span
                    className="chev-rt"
                    onClick={(e) => {
                      e.stopPropagation();
                      toggleOpen(g.name);
                    }}
                  >
                    <Icon
                      name="chev"
                      size={16}
                      style={{
                        transition: "transform .2s",
                        transform: isOpen ? "rotate(90deg)" : "none",
                      }}
                    />
                  </span>
                </div>

                {isOpen && (
                  <div style={{ margin: "4px 0 4px 66px" }}>
                    {g.items.map((it) => (
                      <div
                        key={it.path}
                        className="dup-file"
                        style={{ paddingLeft: 10, opacity: it.deletable ? 1 : 0.55 }}
                      >
                        <Badge r={it.recommend} />
                        <span className="fp" title={it.path}>
                          {shortPath(it.path)}
                        </span>
                        {!it.deletable && (
                          <span className="muted" style={{ fontSize: 11 }}>
                            {it.undeletable_reason || "受保护"}
                          </span>
                        )}
                        <span className="sz">{fmt(it.size_bytes)}</span>
                      </div>
                    ))}
                  </div>
                )}
              </div>
            );
          })}
        </div>
      )}

      {selectedItems.length > 0 && (
        <div className="summary-bar">
          <span className="sel">
            已选 <b>{selectedItems.length}</b> 项 · 共 <b>{fmt(selectedSize)}</b>
          </span>
          <div className="grow" />
          <button className="btn-primary danger" onClick={runClean} disabled={busy}>
            <Icon name="trash" size={16} />
            {busy ? "正在清理…" : `安全清理 ${fmt(selectedSize)}`}
          </button>
        </div>
      )}
    </div>
  );
}
