import { useApp } from "../lib/store";
import { Icon } from "./Icon";

const ICON = { success: "check", info: "spark", warn: "warning" } as const;

export function ToastHost() {
  const { toasts } = useApp();
  return (
    <div id="toasts">
      {toasts.map((t) => (
        <div key={t.id} className={`toast ${t.type}`}>
          <span className="t-ic">
            <Icon name={ICON[t.type]} size={14} />
          </span>
          <span>{t.msg}</span>
        </div>
      ))}
    </div>
  );
}
