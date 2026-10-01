import { useApp } from "../lib/store";
import { Icon } from "./Icon";

export function ScanOverlay() {
  const { scanning, scanPct, scanLabel } = useApp();
  return (
    <div className={`scanoverlay${scanning ? " show" : ""}`}>
      <div className="scanbox scanning">
        <div className="row" style={{ gap: 12 }}>
          <span
            className="dot"
            style={{
              width: 38,
              height: 38,
              borderRadius: 11,
              background: "var(--brand-50)",
              color: "var(--brand)",
              display: "flex",
              alignItems: "center",
              justifyContent: "center",
            }}
          >
            <Icon name="broom" size={19} />
          </span>
          <div>
            <div className="h3">正在扫描磁盘</div>
            <div className="sub">只读扫描，不会删除任何文件</div>
          </div>
        </div>
        <div className="pbar">
          <i style={{ width: scanPct + "%" }} />
        </div>
        <div
          className="row"
          style={{ justifyContent: "space-between", alignItems: "baseline" }}
        >
          <span className="pnum">{Math.round(scanPct)}%</span>
          <span className="pcurrent">{scanLabel}</span>
        </div>
      </div>
    </div>
  );
}
