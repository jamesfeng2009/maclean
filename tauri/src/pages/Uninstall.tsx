import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { ipc } from "../lib/ipc";
import { fmt, shortPath } from "../lib/format";
import type { AppInventorySize, InstalledApp } from "../lib/types";
import { useApp } from "../lib/store";
import { Icon } from "../components/Icon";
import { Empty, PageHeader } from "../components/ui";

const TINTS = ["#4F46E5", "#10B981", "#3B82F6", "#F59E0B", "#8B5CF6", "#EC4899", "#14B8A6", "#F97316"];
function tint(name: string) {
  let h = 0;
  for (let i = 0; i < name.length; i++) h = (h * 31 + name.charCodeAt(i)) >>> 0;
  return TINTS[h % TINTS.length];
}

const PWA_LABEL: Record<string, string> = {
  chrome: "Chrome PWA",
  edge: "Edge PWA",
  brave: "Brave PWA",
  chromium: "Chromium PWA",
  safari: "Safari Web App",
};

const PROTECTED_TEXT: Record<string, string> = {
  critical: "系统关键应用",
  official: "需官方卸载器",
  data: "含用户数据",
};

// 模块级缓存：体积统计是重 IO（冷跑数十秒），切走再回本页时直接复用，不重跑；
// 仅「刷新列表」或卸载后才更新。体积是否已就绪与清单一并缓存。
let cachedList: InstalledApp[] | null = null;
let cachedSizesReady = false;

export function Uninstall() {
  const { confirm, toast, langEn, results, removePaths } = useApp();
  // 首帧即带上模块缓存，切回本页不必先看一次空状态
  const [apps, setApps] = useState<InstalledApp[] | null>(cachedList);
  const [sizesReady, setSizesReady] = useState(cachedSizesReady);
  const [loading, setLoading] = useState(false);
  /** 正在执行一键卸载的应用路径集合 */
  const [busy, setBusy] = useState<Set<string>>(new Set());
  const [sel, setSel] = useState<Set<string>>(new Set());
  /** 真实图标加载失败的应用路径（回退为首字母色块） */
  const [iconErr, setIconErr] = useState<Set<string>>(new Set());
  /** 卸载遇系统保护（TCC）或应用运行中时，记录应用名与需要的授权类型，显示分步引导条 */
  const [fdaTip, setFdaTip] = useState<{ name: string; fda: boolean; am: boolean } | null>(null);
  /** 加载代次，刷新时使上一次进行中的体积补算结果作废，避免竞态覆盖 */
  const genRef = useRef(0);
  /** 当前体积事件监听取消函数 */
  const unlistenRef = useRef<UnlistenFn | null>(null);

  /** 单条体积流式回填：更新 state 与模块缓存 */
  const applySize = useCallback((s: AppInventorySize) => {
    setApps((prev) => {
      if (!prev) return prev;
      const next = prev.map((a) =>
        a.path === s.path
          ? { ...a, app_size: s.app_size, data_size: s.data_size, cache_size: s.cache_size, _sized: true, _skipped: false }
          : a
      );
      cachedList = next;
      return next;
    });
  }, []);

  /** 后台补算体积并流式合并（不阻塞首屏交互） */
  const loadSizes = useCallback(
    async (list: InstalledApp[], gen: number) => {
      // 清掉上一次的事件监听，避免刷新后旧批次串入
      unlistenRef.current?.();
      unlistenRef.current = null;

      let un: UnlistenFn = () => {};
      try {
        un = await listen<AppInventorySize>("app-size", (e) => {
          // 每算完一个应用就先显示它（小应用秒出，不被大目录拖住）
          if (gen === genRef.current) applySize(e.payload);
        });
      } catch {
        // 监听不可用时退化为等待整批结果
      }
      if (gen !== genRef.current) {
        un();
        return;
      }
      unlistenRef.current = un;

      try {
        const all = await ipc.appsSizes(list.map((a) => a.path));
        if (gen !== genRef.current) return;
        // 最终整批：命中的填入精确体积；预算内仍未算完的标注「繁忙跳过」
        const by = new Map(all.map((s) => [s.path, s]));
        setApps((prev) => {
          if (!prev) return prev;
          const merged = prev.map((a) => {
            const s = by.get(a.path);
            return s
              ? { ...a, app_size: s.app_size, data_size: s.data_size, cache_size: s.cache_size, _sized: true, _skipped: false }
              : { ...a, _skipped: true };
          });
          cachedList = merged;
          return merged;
        });
        cachedSizesReady = true;
        setSizesReady(true);
      } catch (e) {
        if (gen === genRef.current) toast("warn", "应用占用统计失败：" + String(e));
      } finally {
        if (unlistenRef.current === un) unlistenRef.current = null;
        un();
      }
    },
    [applySize, toast]
  );

  // 卸载页面时关闭事件监听
  useEffect(() => () => unlistenRef.current?.(), []);

  const load = useCallback(
    async (force = false) => {
      // 命中模块缓存：直接还原；若体积此前没算完则续算
      if (!force && cachedList) {
        setApps(cachedList);
        setSizesReady(cachedSizesReady);
        if (!cachedSizesReady) {
          const g = ++genRef.current;
          void loadSizes(cachedList, g);
        }
        return;
      }

      const gen = ++genRef.current;
      setLoading(true);
      try {
        // 轻量清单：元数据 + 真实图标，无体积，秒回
        const list = await ipc.appsInventory();
        if (gen !== genRef.current) return;
        cachedList = list;
        cachedSizesReady = false;
        setApps(list);
        setSizesReady(false);
        setLoading(false);
        // 首屏已可交互，体积在后台补，页面不再因整盘统计而卡死
        void loadSizes(list, gen);
      } catch (e) {
        if (gen === genRef.current) {
          toast("warn", "读取应用清单失败：" + String(e));
          setLoading(false);
        }
      }
    },
    [loadSizes, toast]
  );

  useEffect(() => {
    void load(false);
  }, [load]);

  const selected = useMemo(
    () => apps?.filter((a) => sel.has(a.path) && a.deletable) ?? [],
    [apps, sel]
  );
  const total = selected.reduce((s, a) => s + a.app_size + a.data_size + a.cache_size, 0);

  const toggle = (p: string) =>
    setSel((s) => {
      const n = new Set(s);
      if (n.has(p)) n.delete(p);
      else n.add(p);
      return n;
    });

  /** 卸载单个应用；成功后从清单与智能清理缓存中移除 */
  const uninstallOne = useCallback(
    async (app: InstalledApp) => {
      setBusy((b) => new Set(b).add(app.path));
      try {
        const rep = await ipc.appUninstall(app.path, langEn);
        if (rep.status === "done") {
          const suffix = rep.restorable > 0 ? `，可还原 ${rep.restorable} 项` : "";
          toast("success", rep.message + suffix);
          // 从本页清单（含模块缓存）移除
          setApps((prev) => {
            const next = prev?.filter((a) => a.path !== app.path) ?? prev;
            cachedList = next;
            return next;
          });
          setSel((s) => {
            const n = new Set(s);
            n.delete(app.path);
            return n;
          });
          // 同步清理全局「已安装应用」缓存里该应用的扫描项（本体/数据/缓存/残留）
          const removed = new Set<string>();
          for (const it of results.apps) {
            if (
              it.path === app.path ||
              it.category === app.name ||
              it.category.startsWith(app.name + " ")
            ) {
              removed.add(it.path);
            }
          }
          if (removed.size > 0) removePaths("apps", removed);
        } else if (rep.status === "delegated") {
          toast("info", rep.message);
        } else {
          toast("warn", rep.message);
        }
        // 有残留因系统保护（TCC）或应用运行中删不掉：拉起授权引导条
        if (rep.needs_full_disk_access || rep.needs_app_management) {
          setFdaTip({
            name: app.name,
            fda: !!rep.needs_full_disk_access,
            am: !!rep.needs_app_management,
          });
        }
      } catch (e) {
        toast("warn", "卸载失败：" + String(e));
      } finally {
        setBusy((b) => {
          const n = new Set(b);
          n.delete(app.path);
          return n;
        });
      }
    },
    [langEn, results.apps, removePaths, toast]
  );

  const confirmOne = (app: InstalledApp) => {
    const release = app.app_size + app.data_size + app.cache_size;
    confirm({
      title: `卸载 ${app.name}？`,
      sub: app.is_pwa
        ? "将移除该网页应用的启动图标（.app 移入废纸篓，可还原）。浏览器内的应用注册会在下次打开浏览器时自动清除，不影响书签与浏览数据。"
        : "将删除应用本体与关联数据（移入废纸篓，可还原）。",
      warn: "应用数据删除后可能丢失登录状态与本地配置，请确认已备份或不再需要。",
      items: [app.path],
      confirmText:
        sizesReady && !app._skipped ? `一键卸载（释放 ${fmt(release)}）` : "一键卸载",
      onConfirm: () => void uninstallOne(app),
    });
  };

  const runSelected = () => {
    if (selected.length === 0) return;
    const anySkipped = selected.some((a) => a._skipped);
    confirm({
      title: `确认卸载 ${selected.length} 个应用？`,
      sub: "将逐个卸载所选应用，删除本体与关联数据（移入废纸篓，可还原）。",
      warn: "应用数据删除后可能丢失登录状态与本地配置，请确认已备份或不再需要。",
      items: selected.map((a) =>
        sizesReady
          ? a._skipped
            ? `${a.name} — 占用未精确统计（磁盘繁忙）`
            : `${a.name} — ${fmt(a.app_size + a.data_size + a.cache_size)}`
          : `${a.name} — 占用统计中…`
      ),
      confirmText:
        sizesReady && !anySkipped
          ? `卸载 ${selected.length} 个应用（${fmt(total)}）`
          : `卸载 ${selected.length} 个应用`,
      onConfirm: async () => {
        for (const app of selected) {
          await uninstallOne(app);
        }
      },
    });
  };

  return (
    <div>
      <PageHeader
        title="应用卸载"
        sub="每应用一键卸载：本体、关联数据与缓存一并移入废纸篓；Chrome / Edge / Arc 等浏览器安装的网页应用（PWA）同样支持"
        action={
          <button
            className="btn-secondary"
            onClick={() => void load(true)}
            disabled={loading}
            style={{ height: 34 }}
          >
            <Icon name="refresh" size={14} /> {loading ? "读取中…" : "刷新列表"}
          </button>
        }
      />

      {fdaTip && (
        <div
          style={{
            display: "flex",
            alignItems: "flex-start",
            gap: 10,
            margin: "2px 0 12px",
            padding: "10px 12px",
            borderRadius: 12,
            background: "rgba(245,158,11,.12)",
            border: "1px solid rgba(245,158,11,.38)",
            color: "var(--text-2)",
          }}
        >
          <Icon name="shield" size={16} />
          <div style={{ flex: 1, fontSize: 12.5, lineHeight: 1.6 }}>
            {langEn ? (
              <>
                Some data of <b>{fdaTip.name}</b> couldn&apos;t be removed because macOS protects
                those locations. Please follow the steps below and uninstall again:
                <ol style={{ margin: "4px 0 0 18px", padding: 0 }}>
                  {fdaTip.fda && (
                    <li>
                      Grant maclean <b>Full Disk Access</b> (protects the app&apos;s container data).
                    </li>
                  )}
                  {fdaTip.am && (
                    <li>
                      Grant maclean <b>App Management</b> (deleting a root-installed app); the
                      system may still ask for Touch ID / password once — please confirm it.
                    </li>
                  )}
                  <li>Quit the app if it is running, then uninstall again.</li>
                </ol>
              </>
            ) : (
              <>
                「{fdaTip.name}」的部分数据因 macOS 系统保护未能删除。请按下面步骤操作后重新卸载：
                <ol style={{ margin: "4px 0 0 18px", padding: 0 }}>
                  {fdaTip.fda && (
                    <li>
                      在「系统设置 → 隐私与安全性 → <b>完全磁盘访问</b>」中为 maclean 打开开关
                      （保护的是该应用的容器数据）。
                    </li>
                  )}
                  {fdaTip.am && (
                    <li>
                      在「系统设置 → 隐私与安全性 → <b>App 管理（应用管理）</b>」中为 maclean
                      打开开关（删除 root 安装的应用本体时需要）；系统可能还会弹一次
                      Touch ID / 密码授权，<b>请在弹窗里确认，不要取消</b>。
                    </li>
                  )}
                  <li>若该应用正在运行，先退出它，再重新卸载。</li>
                </ol>
              </>
            )}
          </div>
          <div style={{ display: "flex", flexDirection: "column", gap: 6, flex: "none" }}>
            {fdaTip.fda && (
              <button
                className="btn-primary"
                style={{ height: 30, fontSize: 12, whiteSpace: "nowrap" }}
                onClick={() =>
                  void ipc
                    .openFdaSettings()
                    .catch((e) => toast("warn", "打开系统设置失败：" + String(e)))
                }
              >
                <Icon name="settings" size={13} />{" "}
                {langEn ? "Full Disk Access" : "打开：完全磁盘访问"}
              </button>
            )}
            {fdaTip.am && (
              <button
                className="btn-primary"
                style={{ height: 30, fontSize: 12, whiteSpace: "nowrap" }}
                onClick={() =>
                  void ipc
                    .openAppManagementSettings()
                    .catch((e) => toast("warn", "打开系统设置失败：" + String(e)))
                }
              >
                <Icon name="settings" size={13} /> {langEn ? "App Management" : "打开：App 管理"}
              </button>
            )}
          </div>
          <button
            title={langEn ? "Dismiss" : "关闭"}
            onClick={() => setFdaTip(null)}
            style={{
              flex: "none",
              border: "none",
              background: "transparent",
              color: "var(--text-3)",
              fontSize: 16,
              lineHeight: 1,
              cursor: "pointer",
              padding: "2px 2px",
            }}
          >
            ×
          </button>
        </div>
      )}

      {apps === null || apps.length === 0 ? (
        <Empty
          icon="uninstall"
          text={
            loading
              ? "正在读取已安装应用…"
              : apps === null
                ? "尚未读取应用清单"
                : "未发现可卸载的应用"
          }
          action={
            !loading && apps !== null ? (
              <button className="btn-primary" onClick={() => void load(true)}>
                <Icon name="zap" size={15} /> 重新读取
              </button>
            ) : undefined
          }
        />
      ) : (
        <>
          {!sizesReady && (
            <div
              style={{
                fontSize: 12,
                color: "var(--text-3)",
                margin: "2px 0 10px",
                display: "flex",
                alignItems: "center",
                gap: 6,
              }}
            >
              <Icon name="refresh" size={13} /> 正在后台统计各应用占用，期间可正常浏览与卸载…
            </div>
          )}
          <div className="app-grid">
            {apps.map((app) => {
              const on = sel.has(app.path);
              const release = app.app_size + app.data_size + app.cache_size;
              const dataSize = app.data_size + app.cache_size;
              const ready = sizesReady || !!app._sized;
              const skipped = !!app._skipped;
              const showRealIcon = !!app.icon && !iconErr.has(app.path);
              return (
                <div
                  key={app.path}
                  className={`app-card${on ? " sel" : ""}${!app.deletable ? " protected" : ""}`}
                  style={!app.deletable ? { opacity: 0.6 } : undefined}
                  onClick={() => app.deletable && toggle(app.path)}
                >
                  <div className="apl">
                    {showRealIcon ? (
                      <img
                        className="ap-ico ap-ico-img"
                        src={app.icon as string}
                        alt=""
                        draggable={false}
                        onError={() =>
                          setIconErr((s) => {
                            const n = new Set(s);
                            n.add(app.path);
                            return n;
                          })
                        }
                      />
                    ) : (
                      <span className="ap-ico" style={{ background: tint(app.name) }}>
                        {app.name.slice(0, 1)}
                      </span>
                    )}
                    <div style={{ minWidth: 0, flex: 1 }}>
                      <div
                        className="ap-nm"
                        style={{
                          whiteSpace: "nowrap",
                          overflow: "hidden",
                          textOverflow: "ellipsis",
                          display: "flex",
                          alignItems: "center",
                          gap: 6,
                        }}
                      >
                        <span style={{ overflow: "hidden", textOverflow: "ellipsis" }}>
                          {app.name}
                        </span>
                        {app.is_pwa && (
                          <span className="ap-pwa">{PWA_LABEL[app.pwa_kind] ?? "PWA"}</span>
                        )}
                      </div>
                      <div className="ap-v">
                        {app.version ? `v${app.version}` : shortPath(app.path)}
                      </div>
                    </div>
                  </div>
                  <div className="ap-meta">
                    <b>
                      {ready && !skipped ? (
                        fmt(app.app_size)
                      ) : (
                        <span className="ap-dim">—</span>
                      )}
                    </b>
                    {!ready ? (
                      <span className="ap-dim">统计中…</span>
                    ) : skipped ? (
                      <span className="ap-dim" title="磁盘繁忙，未在预算内完成精确统计；不影响卸载">
                        繁忙跳过
                      </span>
                    ) : dataSize > 0 ? (
                      <span>数据 {fmt(dataSize)}</span>
                    ) : (
                      <span style={{ color: "var(--text-3)" }}>无关联数据</span>
                    )}
                  </div>
                  <div className="ap-foot">
                    {app.deletable ? (
                      <span
                        style={{
                          fontSize: 11,
                          color: "var(--text-3)",
                          overflow: "hidden",
                          textOverflow: "ellipsis",
                          whiteSpace: "nowrap",
                        }}
                      >
                        {app.has_official_uninstaller ? "含官方卸载器" : "移入废纸篓"}
                        {ready && !skipped && dataSize > 0 ? ` · 共 ${fmt(release)}` : ""}
                      </span>
                    ) : (
                      <span className="ap-protected" title={app.undeletable_reason}>
                        {PROTECTED_TEXT[app.protection] ?? "受保护"}
                      </span>
                    )}
                    <button
                      className="ap-uninstall"
                      disabled={!app.deletable || busy.has(app.path)}
                      onClick={(e) => {
                        e.stopPropagation();
                        confirmOne(app);
                      }}
                    >
                      {busy.has(app.path) ? "卸载中…" : "一键卸载"}
                    </button>
                  </div>
                </div>
              );
            })}
          </div>
        </>
      )}

      {selected.length > 0 && (
        <div className="summary-bar">
          <span className="sel">
            已选 <b>{selected.length}</b> 个应用 · 释放{" "}
            <b>{sizesReady ? fmt(total) : "…"}</b>
          </span>
          <div className="grow" />
          <button className="btn-primary danger" onClick={runSelected} disabled={busy.size > 0}>
            <Icon name="trash" size={16} /> {busy.size > 0 ? "正在卸载…" : "卸载选中"}
          </button>
        </div>
      )}
    </div>
  );
}
