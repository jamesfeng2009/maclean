import { useCallback, useMemo, useState } from "react";
import { ipc } from "../lib/ipc";
import { fmt, shortPath } from "../lib/format";
import type { CleanItemReq } from "../lib/types";
import { useApp } from "../lib/store";
import { Icon } from "../components/Icon";
import { Badge, Empty, PageHeader } from "../components/ui";

const TINTS = ["#4F46E5", "#10B981", "#3B82F6", "#F59E0B", "#8B5CF6", "#EC4899", "#14B8A6", "#F97316"];
function tint(name: string) {
  let h = 0;
  for (let i = 0; i < name.length; i++) h = (h * 31 + name.charCodeAt(i)) >>> 0;
  return TINTS[h % TINTS.length];
}

export function Uninstall() {
  const {
    startScan,
    scanning,
    fullRunning,
    confirm,
    toast,
    langEn,
    results,
    ready,
    startFullScan,
    removePaths,
  } = useApp();
  // 复用全局缓存的 apps 模块
  const items = results.apps;
  const hasScanned = ready.apps;
  const [sel, setSel] = useState<Set<string>>(new Set());
  const [busy, setBusy] = useState(false);

  // 页头「重新扫描」只刷新已安装应用模块
  const refresh = useCallback(() => {
    void startScan("apps");
  }, [startScan]);

  const selected = useMemo(
    () => items.filter((i) => sel.has(i.path) && i.deletable),
    [items, sel]
  );
  const total = selected.reduce((s, i) => s + i.size_bytes, 0);

  const toggle = (p: string) =>
    setSel((s) => {
      const n = new Set(s);
      if (n.has(p)) n.delete(p);
      else n.add(p);
      return n;
    });

  const run = () => {
    const reqs: CleanItemReq[] = selected.map((i) => ({
      path: i.path,
      category: i.category,
      batch_paths: i.batch_paths,
      size_bytes: i.size_bytes,
      recommend: i.recommend,
    }));
    const hasApp = reqs.some((r) => r.category.includes("卸载"));
    confirm({
      title: "确认卸载 / 移除选中项？",
      sub: hasApp
        ? "包含应用本体：maclean 会优先调用该应用的官方卸载器，其余残留移入废纸篓。"
        : "将把选中的应用数据 / 缓存 / 残留移入废纸篓（可恢复）。",
      warn: "应用数据删除后可能丢失登录状态与本地配置，请确认已备份或不再需要。",
      items: reqs.map((r) => r.path),
      confirmText: `卸载并清理 ${fmt(total)}`,
      onConfirm: async () => {
        setBusy(true);
        try {
          const preview = await ipc.cleanPreview(reqs);
          const ok = new Set(
            preview.filter((p) => p.allowed).map((p) => p.path + "|" + p.category)
          );
          const blocked = preview.length - ok.size;
          const finalReqs = reqs.filter((r) => ok.has(r.path + "|" + r.category));
          if (finalReqs.length === 0) {
            toast("warn", "选中项均未通过安全检查，未执行");
            return;
          }
          if (blocked) toast("info", `${blocked} 项未通过安全检查，已排除`);
          const rep = await ipc.cleanExecute(finalReqs, langEn);
          toast("success", `完成 ${rep.deleted} 项${rep.intercepted ? `，拦截 ${rep.intercepted} 项` : ""}`);
          // 从全局缓存移除已处理项，不自动重扫；需要最新列表请手动重新扫描
          const removed = new Set(finalReqs.map((r) => r.path));
          removePaths("apps", removed);
          setSel(new Set());
        } catch (e) {
          toast("warn", "卸载失败：" + String(e));
        } finally {
          setBusy(false);
        }
      },
    });
  };

  return (
    <div>
      <PageHeader
        title="应用卸载"
        sub="应用本体、缓存、数据与卸载残留；删除前逐项过安全闸门"
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
          icon="uninstall"
          text={
            hasScanned
              ? "未发现可卸载的应用"
              : scanning
                ? fullRunning
                  ? "正在全盘体检，已安装应用模块完成后自动呈现…"
                  : "正在扫描已安装应用…"
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
        <div className="app-grid">
          {items.map((it) => {
            const on = sel.has(it.path);
            const name = it.category.replace(/\s*\(卸载\)$/, "").replace(/\s+(数据|缓存)$/, "");
            return (
              <div
                key={it.path}
                className={`app-card${on ? " sel" : ""}${!it.deletable ? " disabled" : ""}`}
                style={!it.deletable ? { opacity: 0.55 } : undefined}
                onClick={() => it.deletable && toggle(it.path)}
              >
                <div className="apl">
                  <span
                    className="ap-ico"
                    style={{ background: tint(name) }}
                  >
                    {name.slice(0, 1)}
                  </span>
                  <div style={{ minWidth: 0 }}>
                    <div className="ap-nm" style={{ whiteSpace: "nowrap", overflow: "hidden", textOverflow: "ellipsis" }}>
                      {name}
                    </div>
                    <div className="ap-v">{shortPath(it.path).split("/").slice(-2)[0]}</div>
                  </div>
                </div>
                <div className="ap-meta">
                  <Badge r={it.recommend} />
                  <b>{fmt(it.size_bytes)}</b>
                </div>
                <div
                  style={{
                    fontSize: 11,
                    color: "var(--text-3)",
                    whiteSpace: "nowrap",
                    overflow: "hidden",
                    textOverflow: "ellipsis",
                  }}
                  title={it.path}
                >
                  {it.category.includes("卸载")
                    ? "应用本体"
                    : it.category.includes("残留")
                      ? "卸载残留"
                      : it.category.includes("数据")
                        ? "应用数据"
                        : "应用缓存"}
                  {!it.deletable ? " · " + (it.undeletable_reason || "受保护") : ""}
                </div>
              </div>
            );
          })}
        </div>
      )}

      {selected.length > 0 && (
        <div className="summary-bar">
          <span className="sel">
            已选 <b>{selected.length}</b> 项 · <b>{fmt(total)}</b>
          </span>
          <div className="grow" />
          <button className="btn-primary danger" onClick={run} disabled={busy || scanning}>
            <Icon name="trash" size={16} /> {busy ? "正在处理…" : "卸载并清理"}
          </button>
        </div>
      )}
    </div>
  );
}
