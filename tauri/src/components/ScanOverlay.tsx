import { useApp, FULL_SEQUENCE, FULL_MODULE_LABEL } from "../lib/store";
import { Icon } from "./Icon";

/**
 * 后台扫描进度（非阻断）：
 * - 顶部一条细进度条，不拦截任何点击；
 * - 右下角浮动卡展示全量体检的各模块完成情况，扫描进行中用户仍可自由切换页面，
 *   哪个模块先完成，对应页面立刻可看。
 */
export function ScanOverlay() {
  const { scanning, scanPct, scanLabel, fullRunning, runningScopes, singleScope, ready } =
    useApp();

  return (
    <>
      <div className={`scanprog${scanning ? " show" : ""}`}>
        <i style={{ width: scanPct + "%" }} />
      </div>

      {scanning && (
        <div className="scancard">
          <div className="sc-head">
            <span className="sc-spin">
              <Icon name="broom" size={16} />
            </span>
            <div className="grow">
              <div className="sc-title">
                {fullRunning
                  ? runningScopes.length > 1
                    ? `全盘体检中 · ${runningScopes.length} 个模块并行`
                    : "全盘体检中"
                  : `正在重新扫描${singleScope ? "·" + FULL_MODULE_LABEL[singleScope] : ""}`}
              </div>
              <div className="sc-sub">只读扫描，不会删除任何文件 · 可继续浏览其它页面</div>
            </div>
            <span className="sc-pct">{Math.round(scanPct)}%</span>
          </div>

          {fullRunning ? (
            <div className="sc-mods">
              {FULL_SEQUENCE.map((s) => {
                const done = ready[s];
                const run = runningScopes.includes(s);
                return (
                  <div key={s} className={`sc-mod ${done ? "done" : run ? "run" : "pending"}`}>
                    <span className="sc-ic">
                      {done ? (
                        <Icon name="check" size={12} />
                      ) : run ? (
                        <span className="sc-dot" />
                      ) : (
                        <span className="sc-dim" />
                      )}
                    </span>
                    <span className="grow">{FULL_MODULE_LABEL[s]}</span>
                    <span className="sc-state">{done ? "完成" : run ? "扫描中" : "等待"}</span>
                  </div>
                );
              })}
            </div>
          ) : (
            <div className="sc-current">{scanLabel}</div>
          )}
        </div>
      )}
    </>
  );
}
