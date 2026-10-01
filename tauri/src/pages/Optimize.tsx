import { useCallback, useEffect, useState } from "react";
import { ipc } from "../lib/ipc";
import type { ScanItem } from "../lib/types";
import { useApp } from "../lib/store";
import { Icon } from "../components/Icon";
import { Badge, Empty, PageHeader } from "../components/ui";

const TASK_ICON: Record<string, string> = {
  flush_dns: "wifi",
  clear_memory: "cpu",
  rebuild_spotlight: "search",
  purge_inactive: "zap",
};

export function Optimize() {
  const { toast, confirm, langEn } = useApp();
  const [items, setItems] = useState<ScanItem[]>([]);
  const [loading, setLoading] = useState(true);
  const [busyKey, setBusyKey] = useState<string | null>(null);

  const refresh = useCallback(() => {
    setLoading(true);
    ipc
      .optimizeList()
      .then(setItems)
      .catch((e) => toast("warn", "读取维护任务失败：" + e))
      .finally(() => setLoading(false));
  }, [toast]);
  useEffect(refresh, [refresh]);

  const run = (it: ScanItem) => {
    const advanced = it.recommend === "Advanced" || it.recommend === "Caution";
    confirm({
      title: it.category,
      sub: it.description || "执行系统维护任务",
      warn: advanced
        ? "这是高级任务：会修改系统状态（如重建索引），期间相关功能可能短暂变慢。"
        : "低风险维护任务，可安全执行。",
      items: [it.path],
      confirmText: advanced ? "仍要执行" : "立即执行",
      onConfirm: async () => {
        setBusyKey(it.path);
        try {
          const msg = await ipc.optimizeRun(it.path, langEn);
          toast("success", msg || "任务已完成");
        } catch (e) {
          toast("warn", "任务执行失败：" + String(e));
        } finally {
          setBusyKey(null);
        }
      },
    });
  };

  return (
    <div>
      <PageHeader
        title="系统优化"
        sub="macOS 日常维护脚本（刷新 DNS / 重建聚焦索引 / 释放非活跃内存等）"
      />
      {items.length === 0 ? (
        <Empty icon="optimize" text={loading ? "正在加载维护任务…" : "当前平台没有可用的维护任务"} />
      ) : (
        items.map((it) => (
          <div className="opt-row" key={it.path}>
            <span
              className="ic"
              style={{
                background:
                  it.recommend === "Safe" ? "var(--safe-50)" : "var(--caution-50)",
                color: it.recommend === "Safe" ? "var(--safe)" : "var(--caution)",
              }}
            >
              <Icon name={TASK_ICON[it.path] ?? "optimize"} size={18} />
            </span>
            <div className="meta">
              <div className="t" style={{ display: "flex", gap: 8, alignItems: "center" }}>
                {it.category} <Badge r={it.recommend} />
              </div>
              <div className="d">{it.description}</div>
            </div>
            <button
              className={it.recommend === "Safe" ? "btn-secondary" : "btn-primary danger"}
              style={{ height: 34, fontSize: 13 }}
              onClick={() => run(it)}
              disabled={busyKey === it.path}
            >
              {busyKey === it.path ? "执行中…" : "执行"}
            </button>
          </div>
        ))
      )}
    </div>
  );
}
