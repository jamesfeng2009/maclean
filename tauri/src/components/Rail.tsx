import { useApp, PAGE_ICON, PAGE_LABEL } from "../lib/store";
import { Icon } from "./Icon";

const NAV = [
  "overview",
  "analysis",
  "clean",
  "dup",
  "uninstall",
  "startup",
  "optimize",
  "settings",
] as const;

export function Rail() {
  const { page, setPage, toast } = useApp();
  return (
    <aside className="rail">
      <div className="logo">
        <svg viewBox="0 0 24 24" fill="none" stroke="#fff" strokeWidth="2.2">
          <path
            d="M5 13l4 4L19 7"
            strokeLinecap="round"
            strokeLinejoin="round"
          />
        </svg>
      </div>
      {NAV.map((id) => (
        <button
          key={id}
          className={page === id ? "active" : ""}
          onClick={() => setPage(id)}
          aria-label={PAGE_LABEL[id]}
        >
          <Icon name={PAGE_ICON[id]} />
          <span className="tip">{PAGE_LABEL[id]}</span>
        </button>
      ))}
      <div className="spacer" />
      <button
        onClick={() => toast("info", "当前为 Tauri 骨架版，更新通道接入中")}
        aria-label="检查更新"
      >
        <Icon name="refresh" />
        <span className="tip">检查更新</span>
      </button>
    </aside>
  );
}
