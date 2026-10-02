import { useCallback, useEffect, useRef, useState } from "react";
import { ipc } from "../lib/ipc";
import type { LogFile } from "../lib/types";
import { useApp } from "../lib/store";

function fmtSize(b: number): string {
  if (b >= 1024 * 1024) return (b / 1024 / 1024).toFixed(1) + " MB";
  return Math.max(1, Math.round(b / 1024)) + " KB";
}

/**
 * 日志查看器：读取 ~/.maclean/logs/maclean_*.log（只显示尾部 512KB），
 * 可切换历史日志文件、刷新、在访达中定位。删除链路的拦截/成功均落盘于此。
 */
export function LogViewer({ open, onClose }: { open: boolean; onClose: () => void }) {
  const { toast } = useApp();
  const [files, setFiles] = useState<LogFile[]>([]);
  const [name, setName] = useState<string>("");
  const [text, setText] = useState<string>("");
  const [loading, setLoading] = useState(false);
  const preRef = useRef<HTMLPreElement>(null);

  const load = useCallback(
    async (target?: string) => {
      setLoading(true);
      try {
        const t = await ipc.logsRead(target);
        setText(t || "（日志为空）");
      } catch (e) {
        setText("读取日志失败：" + String(e));
      } finally {
        setLoading(false);
      }
    },
    []
  );

  useEffect(() => {
    if (!open) return;
    let cancelled = false;
    (async () => {
      try {
        const list = await ipc.logsList();
        if (cancelled) return;
        setFiles(list);
        const first = list[0]?.name ?? "";
        setName(first);
        await load(first || undefined);
      } catch (e) {
        if (!cancelled) toast("warn", "读取日志列表失败：" + String(e));
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [open, load, toast]);

  // 日志文本默认展示最新内容：滚到底部
  useEffect(() => {
    if (preRef.current) preRef.current.scrollTop = preRef.current.scrollHeight;
  }, [text, open]);

  if (!open) return null;

  return (
    <div className="modal show">
      <div className="modal-box" style={{ width: 680 }}>
        <div className="modal-head">
          <div>
            <div className="m-t">日志与诊断</div>
            <div className="m-s">
              扫描 / 删除的拦截与成功记录，位于 ~/.maclean/logs（仅显示最新部分）
            </div>
          </div>
        </div>

        <div className="logbar">
          <select
            className="log-select"
            value={name}
            onChange={(e) => {
              setName(e.target.value);
              void load(e.target.value);
            }}
            disabled={files.length === 0}
          >
            {files.length === 0 && <option value="">暂无日志文件</option>}
            {files.map((f) => (
              <option key={f.name} value={f.name}>
                {f.name} · {fmtSize(f.size_bytes)}
              </option>
            ))}
          </select>
          <button className="btn-secondary" onClick={() => void load(name || undefined)} disabled={loading}>
            刷新
          </button>
          <button
            className="btn-secondary"
            onClick={() =>
              ipc.logsReveal().catch((e) => toast("warn", "无法打开访达：" + String(e)))
            }
          >
            在访达中打开
          </button>
        </div>

        <pre className="log-view" ref={preRef}>
          {loading ? "正在读取…" : text}
        </pre>

        <div className="modal-actions">
          <button className="btn-primary" onClick={onClose}>
            关闭
          </button>
        </div>
      </div>
    </div>
  );
}
