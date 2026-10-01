import { useCallback, useEffect, useState } from "react";
import { ipc } from "../lib/ipc";
import { shortPath } from "../lib/format";
import type { StartupItem } from "../lib/types";
import { useApp } from "../lib/store";
import { Icon } from "../components/Icon";
import { Empty, PageHeader } from "../components/ui";

export function Startup() {
  const { toast } = useApp();
  const [items, setItems] = useState<StartupItem[]>([]);
  const [loading, setLoading] = useState(true);
  const [busyKey, setBusyKey] = useState<string | null>(null);

  const refresh = useCallback(() => {
    setLoading(true);
    ipc
      .startups()
      .then(setItems)
      .catch((e) => toast("warn", "读取启动项失败：" + e))
      .finally(() => setLoading(false));
  }, [toast]);

  useEffect(refresh, [refresh]);

  const toggle = async (it: StartupItem) => {
    const target = !it.enabled;
    setBusyKey(it.plist);
    try {
      const msg = await ipc.startupSetEnabled(it.label, it.plist, it.scope, target);
      setItems((list) =>
        list.map((x) => (x.plist === it.plist ? { ...x, enabled: target } : x))
      );
      toast("success", msg || (target ? "已设为开机启动" : "已禁用开机启动"));
    } catch (e) {
      toast("warn", (target ? "启用" : "禁用") + "失败：" + String(e));
    } finally {
      setBusyKey(null);
    }
  };

  const enabledCount = items.filter((i) => i.enabled).length;

  return (
    <div>
      <PageHeader
        title="启动项"
        sub={`当前 ${items.length} 个登录项，${enabledCount} 个处于启用状态；禁用只移动配置，不删除文件`}
        action={
          <button className="btn-secondary" onClick={refresh} style={{ height: 34 }}>
            <Icon name="refresh" size={14} /> 刷新
          </button>
        }
      />

      {items.length === 0 ? (
        <Empty icon="startup" text={loading ? "正在读取登录项…" : "未发现可管理的登录项"} />
      ) : (
        items.map((it) => (
          <div className="launch-row" key={it.plist}>
            <span
              className="ic"
              style={{ background: "var(--brand-50)", color: "var(--brand)" }}
            >
              <Icon name="startup" size={17} />
            </span>
            <div className="meta">
              <div className="t">{it.label}</div>
              <div className="d" title={it.plist}>
                {shortPath(it.plist)}
              </div>
            </div>
            <span className="kind">{it.scope === "user" ? "用户" : "系统"}</span>
            <button
              className={`switch${it.enabled ? " on" : ""}`}
              disabled={busyKey === it.plist}
              onClick={() => toggle(it)}
              aria-label={it.enabled ? "禁用" : "启用"}
            />
          </div>
        ))
      )}
    </div>
  );
}
