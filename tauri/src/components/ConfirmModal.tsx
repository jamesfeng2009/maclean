import { useState } from "react";
import { useApp } from "../lib/store";
import { Icon } from "./Icon";
import { shortPath } from "../lib/format";

/** 高危操作二次确认弹窗（危险级红色描边 + 路径清单） */
export function ConfirmModal() {
  const { confirmReq, closeConfirm } = useApp();
  const [busy, setBusy] = useState(false);
  if (!confirmReq) return null;

  const doConfirm = async () => {
    setBusy(true);
    try {
      await confirmReq.onConfirm();
    } finally {
      setBusy(false);
      closeConfirm();
    }
  };

  const danger = confirmReq.confirmText?.includes("删除") ||
    confirmReq.confirmText?.includes("清理") ||
    confirmReq.confirmText?.includes("卸载");

  return (
    <div className="modal show" onMouseDown={(e) => !busy && e.target === e.currentTarget && closeConfirm()}>
      <div className={`modal-box${danger ? " danger" : ""}`}>
        <div className="modal-head">
          <span
            className="m-ic"
            style={{
              background: "var(--danger-50)",
              color: "var(--danger)",
            }}
          >
            <Icon name="warning" size={21} />
          </span>
          <div>
            <div className="m-t">{confirmReq.title}</div>
            <div className="m-s">{confirmReq.sub}</div>
          </div>
        </div>

        {confirmReq.warn && (
          <div className="warn-banner">
            <Icon name="warning" size={16} /> {confirmReq.warn}
          </div>
        )}

        {confirmReq.items.length > 0 && (
          <div className="modal-list">
            {confirmReq.items.slice(0, 60).map((p, i) => (
              <div key={i} style={{ padding: "3px 0" }}>
                {shortPath(p)}
              </div>
            ))}
            {confirmReq.items.length > 60 && (
              <div className="muted" style={{ padding: "3px 0" }}>
                … 其余 {confirmReq.items.length - 60} 项
              </div>
            )}
          </div>
        )}

        <div className="modal-actions">
          <button className="btn-secondary" onClick={closeConfirm} disabled={busy}>
            取消
          </button>
          <button
            className={`btn-primary${danger ? " danger" : ""}`}
            onClick={doConfirm}
            disabled={busy}
          >
            {busy ? "执行中…" : confirmReq.confirmText || "确认"}
          </button>
        </div>
      </div>
    </div>
  );
}
