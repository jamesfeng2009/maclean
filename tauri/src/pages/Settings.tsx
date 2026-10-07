import { useEffect, useState } from "react";
import { ipc } from "../lib/ipc";
import type { AppSettings } from "../lib/types";
import { useApp } from "../lib/store";
import { PageHeader } from "../components/ui";
import { LogViewer } from "../components/LogViewer";

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
  const { langEn, setLangEn, theme, setTheme, toast } = useApp();
  const [cfg, setCfg] = useState<AppSettings | null>(null);
  const [showLog, setShowLog] = useState(false);
  // 登录时启动（LaunchAgent plist 是权威状态，进入设置页时实时读取）
  const [launchAtLogin, setLaunchAtLogin] = useState<boolean | null>(null);
  const [launchBusy, setLaunchBusy] = useState(false);

  useEffect(() => {
    ipc.settingsGet().then(setCfg).catch((e) => toast("warn", "读取设置失败：" + e));
    ipc.launchAtLoginGet().then(setLaunchAtLogin).catch(() => setLaunchAtLogin(false));
  }, [toast]);

  if (!cfg) return <div className="empty">正在加载设置…</div>;

  const patch = (p: Partial<AppSettings>) => {
    setCfg((c) => (c ? { ...c, ...p } : c));
    ipc.settingsSet(p).catch((e) => toast("warn", "保存设置失败：" + e));
  };

  const toggleLaunch = (v: boolean) => {
    setLaunchBusy(true);
    setLaunchAtLogin(v);
    ipc
      .launchAtLoginSet(v)
      .then(() => toast("success", v ? "已开启：下次登录自动启动 maclean" : "已关闭登录时启动"))
      .catch((e) => {
        setLaunchAtLogin(!v);
        toast("warn", "设置登录启动失败：" + e);
      })
      .finally(() => setLaunchBusy(false));
  };

  const toggleMenubar = (v: boolean) => {
    patch({ settings_menubar_icon: v });
    ipc
      .menuBarSet(v)
      .then(() => toast("success", v ? "已显示菜单栏图标" : "已隐藏菜单栏图标"))
      .catch((e) => toast("warn", "切换菜单栏图标失败：" + e));
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
        <Row title="主题" desc="使用浅色、深色或跟随系统的外观">
          <div className="seg">
            <button
              className={theme === "system" ? "on" : ""}
              onClick={() => setTheme("system")}
            >
              Follow system
            </button>
            <button
              className={theme === "light" ? "on" : ""}
              onClick={() => setTheme("light")}
            >
              Light
            </button>
            <button
              className={theme === "dark" ? "on" : ""}
              onClick={() => setTheme("dark")}
            >
              Dark
            </button>
          </div>
        </Row>
        <Row title="登录时启动" desc="登录系统后自动启动 maclean（写入 LaunchAgent，下次登录生效）">
          <Switch on={!!launchAtLogin} disabled={launchBusy} onChange={toggleLaunch} />
        </Row>
        <Row title="在菜单栏显示" desc="在 macOS 菜单栏常驻图标，悬停可见磁盘用量，点击唤起主窗口">
          <Switch
            on={!!cfg.settings_menubar_icon}
            onChange={toggleMenubar}
          />
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
          title="删除方式"
          desc="智能分层：安全/缓存垃圾永久删除、立即释放空间，注意/高级项移入废纸篓可恢复；一律进废纸篓：所有项都可从废纸篓恢复，更稳妥"
        >
          <div className="seg">
            <button
              className={cfg.settings_delete_strategy !== "trash" ? "on" : ""}
              onClick={() => patch({ settings_delete_strategy: "smart" })}
            >
              智能分层
            </button>
            <button
              className={cfg.settings_delete_strategy === "trash" ? "on" : ""}
              onClick={() => patch({ settings_delete_strategy: "trash" })}
            >
              一律进废纸篓
            </button>
          </div>
        </Row>
        <Row
          title="优先使用官方卸载器"
          desc="卸载应用时优先调用其自带卸载程序，避免残留与授权损坏"
        >
          <Switch
            on={!!cfg.settings_prefer_official_uninstaller}
            onChange={(v) => patch({ settings_prefer_official_uninstaller: v })}
          />
        </Row>
        <Row
          title="保持提权会话"
          desc="清理含注意/高级项时，先弹一次系统授权（Touch ID/密码）并后台保活 sudo 票据，整轮删除免重复弹窗；密码仅内存保留、不落盘"
        >
          <Switch
            on={!!cfg.settings_keep_sudo}
            onChange={(v) => patch({ settings_keep_sudo: v })}
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

      <div className="set-group">
        <div className="sgh">日志与诊断</div>
        <Row
          title="查看运行日志"
          desc="扫描、删除的安全拦截与成功记录都写入 ~/.maclean/logs；遇到误拦或异常时可据此排查"
        >
          <button className="btn-secondary" onClick={() => setShowLog(true)}>
            查看日志
          </button>
        </Row>
      </div>

      <div className="sub" style={{ textAlign: "center", padding: "8px 0 4px" }}>
        maclean Tauri 骨架 · 所有删除均经 maclean-core safety 闸门
      </div>

      <LogViewer open={showLog} onClose={() => setShowLog(false)} />
    </div>
  );
}
