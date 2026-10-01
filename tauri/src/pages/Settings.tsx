import { useEffect, useState } from "react";
import { ipc } from "../lib/ipc";
import type { AppSettings } from "../lib/types";
import { useApp } from "../lib/store";
import { PageHeader } from "../components/ui";

function Switch({
  on,
  disabled,
  onChange,
}: {
  on: boolean;
  disabled?: boolean;
  onChange: (v: boolean) => void;
}) {
  return (
    <button
      className={`switch${on ? " on" : ""}`}
      disabled={disabled}
      onClick={() => onChange(!on)}
    />
  );
}

function Row({
  title,
  desc,
  children,
}: {
  title: string;
  desc: string;
  children: React.ReactNode;
}) {
  return (
    <div className="set-row">
      <div className="meta">
        <div className="t">{title}</div>
        <div className="d">{desc}</div>
      </div>
      {children}
    </div>
  );
}

export function Settings() {
  const { langEn, setLangEn, toast } = useApp();
  const [cfg, setCfg] = useState<AppSettings | null>(null);

  useEffect(() => {
    ipc.settingsGet().then(setCfg).catch((e) => toast("warn", "读取设置失败：" + e));
  }, [toast]);

  if (!cfg) return <div className="empty">正在加载设置…</div>;

  const patch = (p: Partial<AppSettings>) => {
    setCfg((c) => (c ? { ...c, ...p } : c));
    ipc.settingsSet(p).catch((e) => toast("warn", "保存设置失败：" + e));
  };

  return (
    <div>
      <PageHeader title="设置" sub="偏好会写入 ~/.maclean/config.json，与桌面版共享" />

      <div className="set-group">
        <div className="sgh">通用</div>
        <Row title="界面语言" desc="切换后立即生效（与桌面版共用同一配置）">
          <div className="seg">
            <button className={!langEn ? "on" : ""} onClick={() => setLangEn(false)}>
              中文
            </button>
            <button className={langEn ? "on" : ""} onClick={() => setLangEn(true)}>
              English
            </button>
          </div>
        </Row>
        <Row title="删除前确认高级风险项" desc="清理包含「注意/高级」等级项目时，强制弹出二次确认">
          <Switch
            on={!!cfg.settings_confirm_advanced}
            onChange={(v) => patch({ settings_confirm_advanced: v })}
          />
        </Row>
        <Row
          title="显示受保护项目"
          desc="在列表中显示系统关键目录（始终不可勾选删除，仅作可见性）"
        >
          <Switch
            on={!!cfg.settings_show_protected_items}
            onChange={(v) => patch({ settings_show_protected_items: v })}
          />
        </Row>
      </div>

      <div className="set-group">
        <div className="sgh">清理与卸载</div>
        <Row
          title="优先使用官方卸载器"
          desc="卸载应用时优先调用其自带卸载程序，避免残留与授权损坏"
        >
          <Switch
            on={!!cfg.settings_prefer_official_uninstaller}
            onChange={(v) => patch({ settings_prefer_official_uninstaller: v })}
          />
        </Row>
      </div>

      <div className="set-group">
        <div className="sgh">定时清理</div>
        <Row title="启用定时清理" desc="到达间隔后由调度器在后台执行安全项清理">
          <Switch
            on={!!cfg.schedule_enabled}
            onChange={(v) => patch({ schedule_enabled: v })}
          />
        </Row>
        <Row title="清理间隔" desc="两次自动清理之间的最小间隔">
          <div className="seg">
            {[7, 14, 30].map((d) => (
              <button
                key={d}
                className={cfg.schedule_interval_days === d ? "on" : ""}
                onClick={() => patch({ schedule_interval_days: d })}
                disabled={!cfg.schedule_enabled}
              >
                {d} 天
              </button>
            ))}
          </div>
        </Row>
      </div>

      <div className="sub" style={{ textAlign: "center", padding: "8px 0 4px" }}>
        maclean Tauri 骨架 · 所有删除均经 maclean-core safety 闸门
      </div>
    </div>
  );
}
