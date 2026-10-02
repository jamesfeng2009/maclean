import type { ReactNode } from "react";
import type { Recommend } from "../lib/types";
import { Icon, type IconName } from "./Icon";

export function Badge({ r }: { r: Recommend | string }) {
  const map: Record<string, { cls: string; t: string }> = {
    Safe: { cls: "safe", t: "安全" },
    CacheOnly: { cls: "cache", t: "缓存" },
    Caution: { cls: "caution", t: "注意" },
    Advanced: { cls: "danger", t: "高级" },
    Protected: { cls: "protected", t: "受保护" },
  };
  const m = map[r] ?? { cls: "ghost", t: r };
  return <span className={`badge ${m.cls}`}>{m.t}</span>;
}

export function PageHeader({
  title,
  sub,
  action,
}: {
  title: string;
  sub: string;
  action?: ReactNode;
}) {
  return (
    <div
      style={{
        display: "flex",
        alignItems: "flex-end",
        justifyContent: "space-between",
        marginBottom: 18,
        gap: 12,
      }}
    >
      <div>
        <div className="h2">{title}</div>
        <div className="sub" style={{ marginTop: 3 }}>
          {sub}
        </div>
      </div>
      {action}
    </div>
  );
}

export function Empty({
  icon = "file",
  text,
  action,
}: {
  icon?: IconName;
  text: string;
  action?: ReactNode;
}) {
  return (
    <div className="empty">
      <div
        style={{
          width: 52,
          height: 52,
          borderRadius: 16,
          background: "var(--panel)",
          border: "1px solid var(--line)",
          display: "flex",
          alignItems: "center",
          justifyContent: "center",
          color: "var(--text-3)",
          margin: "0 auto 12px",
        }}
      >
        <Icon name={icon} size={24} />
      </div>
      <div style={{ marginBottom: 12 }}>{text}</div>
      {action}
    </div>
  );
}

/** 是否默认勾选（与 core Recommend::default_selected 对齐） */
export function defaultSelected(r: Recommend): boolean {
  return r === "Safe" || r === "CacheOnly";
}
