import { useEffect, useRef } from "react";
import { useApp } from "../lib/store";
import { shortPath } from "../lib/format";

/**
 * 全局清理进度遮罩：任意页面触发 executeClean 时出现。
 * - 删除必须有明确反馈，避免用户以为“点了没反应”；
 * - 遮罩拦截一切底层操作，防止删除途中重复点击 / 切页；
 * - 实时滚动 clean-log，成功与拦截分色；结束由调用方 toast 汇总。
 */
export function CleanProgress() {
  const { cleaning, cleanProgress, cleanLogs } = useApp();
  const logRef = useRef<HTMLDivElement>(null);

  // 新日志到达时自动滚到底部
  useEffect(() => {
    const el = logRef.current;
    if (el) el.scrollTop = el.scrollHeight;
  }, [cleanLogs]);

  if (!cleaning) return null;

  const pct = cleanProgress?.pct ?? 0;
  const done = cleanProgress?.done ?? 0;
  const total = cleanProgress?.total ?? 0;
  const deleted = cleanProgress?.deleted ?? 0;
  const intercepted = cleanProgress?.intercepted ?? 0;
  const skipped = cleanProgress?.skipped ?? 0;
  const current = cleanProgress?.path ?? "";
  const recent = cleanLogs.slice(-8);

  return (
    <div className="cleanprog-mask">
      <div className="cleanprog-card" role="dialog" aria-label="正在清理">
        <div className="cleanprog-head">
          <span className="cleanprog-spin" aria-hidden />
          <div>
            <div className="cleanprog-title">正在清理，请稍候…</div>
            <div className="cleanprog-sub">
              {total > 0 ? `已处理 ${done} / ${total} 项` : "正在准备删除任务…"}
            </div>
          </div>
          <div className="cleanprog-pct">{Math.round(pct)}%</div>
        </div>

        <div className="cleanprog-track">
          <div className="cleanprog-fill" style={{ width: `${pct}%` }} />
        </div>

        <div className="cleanprog-stats">
          <span className="cp-stat ok">已移入废纸篓 {deleted}</span>
          <span className="cp-stat warn">已安全拦截 {intercepted}</span>
          {skipped > 0 && <span className="cp-stat skip">跳过 {skipped}</span>}
        </div>

        {current ? (
          <div className="cleanprog-current" title={current}>
            {shortPath(current)}
          </div>
        ) : null}

        <div className="cleanprog-logs" ref={logRef}>
          {recent.length === 0 ? (
            <div className="cleanprog-empty">等待删除引擎回传进度…</div>
          ) : (
            recent.map((l, i) => (
              <div key={i} className={l.ok ? "cp-log ok" : "cp-log fail"}>
                <span className="cp-log-mark">{l.ok ? "✓" : "!"}</span>
                <span className="cp-log-line">{l.line}</span>
              </div>
            ))
          )}
        </div>

        <div className="cleanprog-foot">文件会先移入废纸篓，误删可恢复；请勿退出应用。</div>
      </div>
    </div>
  );
}
