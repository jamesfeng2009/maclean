import { useEffect, useState } from "react";
import { useApp } from "../lib/store";
import { Icon } from "./Icon";
import { shortPath } from "../lib/format";

/**
 * 高危操作二次确认弹窗（危险级红色描边 + 路径清单）。
 *
 * 防误触设计（删除/卸载不可逆，必须由用户一次"独立、有意"的点击触发）：
 * 1. 打开保护期：弹窗出现后的短暂窗口内「确认」按钮处于禁用态，
 *    吞掉来自触发按钮的点击穿透 / 双击第二击 / 合成事件，避免弹窗刚渲染就被确认。
 * 2. 确认与取消按钮均显式 type="button"，不使用任何表单默认提交语义。
 * 3. 不把 Enter 绑定到「确认」：回车不会触发删除；Esc 仅用于取消。
 */
const CONFIRM_GUARD_MS = 700;

export function ConfirmModal() {
  const { confirmReq, closeConfirm, cleanLogs } = useApp();
  const [busy, setBusy] = useState(false);
  const [toggleOn, setToggleOn] = useState(false);
  const [guarded, setGuarded] = useState(false);
  // 删除计时：大目录（build/dist 等）删除可能持续数十秒，给出已用时间避免“卡死”错觉
  const [elapsed, setElapsed] = useState(0);

  // 每次弹出新的确认框：勾选项恢复默认值，并进入"打开保护期"
  useEffect(() => {
    if (!confirmReq) return;
    setToggleOn(!!confirmReq.confirmToggle?.defaultOn);
    setBusy(false);
    setGuarded(true);
    setElapsed(0);
    const t = setTimeout(() => setGuarded(false), CONFIRM_GUARD_MS);
    return () => clearTimeout(t);
  }, [confirmReq]);

  // 执行中每秒计时
  useEffect(() => {
    if (!busy) return;
    setElapsed(0);
    const t = setInterval(() => setElapsed((e) => e + 1), 1000);
    return () => clearInterval(t);
  }, [busy]);

  // Esc 关闭；刻意不监听 Enter（回车不得触发确认）
  useEffect(() => {
    if (!confirmReq) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape" && !busy) closeConfirm();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [confirmReq, busy, closeConfirm]);

  if (!confirmReq) return null;

  const doConfirm = async () => {
    // 保护期内 / 执行中一律忽略，杜绝穿透与重复提交
    if (guarded || busy) return;
    setBusy(true);
    try {
      await confirmReq.onConfirm(confirmReq.confirmToggle ? toggleOn : undefined);
    } finally {
      setBusy(false);
      closeConfirm();
    }
  };

  const danger = confirmReq.confirmText?.includes("删除") ||
    confirmReq.confirmText?.includes("清理") ||
    confirmReq.confirmText?.includes("卸载");

  return (
    <div
      className="modal show"
      role="dialog"
      aria-modal="true"
      onMouseDown={(e) => !busy && !guarded && e.target === e.currentTarget && closeConfirm()}
    >
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

        {confirmReq.confirmToggle && (
          <label className="confirm-toggle">
            <input
              type="checkbox"
              checked={toggleOn}
              disabled={busy}
              onChange={(e) => setToggleOn(e.target.checked)}
            />
            <span>{confirmReq.confirmToggle.label}</span>
          </label>
        )}

        {/* 执行中：实时显示当前删除步骤（大目录删除可能持续数十秒）+ 已用时间 */}
        {busy && (
          <div className="modal-exec">
            <div className="modal-exec-line">
              {cleanLogs.length > 0 ? (
                <>
                  <Icon name="trash" size={13} />
                  <span>{cleanLogs[cleanLogs.length - 1].line}</span>
                </>
              ) : (
                <>
                  <Icon name="zap" size={13} />
                  <span>正在准备删除…</span>
                </>
              )}
            </div>
            <div className="modal-exec-timer">已用 {elapsed}s</div>
          </div>
        )}

        <div className="modal-actions">
          <button type="button" className="btn-secondary" onClick={closeConfirm} disabled={busy}>
            取消
          </button>
          <button
            type="button"
            className={`btn-primary${danger ? " danger" : ""}`}
            onClick={doConfirm}
            disabled={busy || guarded}
            title={guarded ? "请稍候确认，防止误触" : undefined}
          >
            {busy ? `执行中… ${elapsed}s` : guarded ? "请确认…" : confirmReq.confirmText || "确认"}
          </button>
        </div>
      </div>
    </div>
  );
}
