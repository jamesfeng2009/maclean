import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type {
  AppSettings,
  AppInventorySize,
  CacheChild,
  CleanItemReq,
  CleanLogEntry,
  CleanProgress,
  CleanReport,
  DiskInfo,
  ImBreakdown,
  InstalledApp,
  LogFile,
  PreviewItem,
  ScanItem,
  ScanModuleEvent,
  ScanProgress,
  ScanScope,
  StartupItem,
  UninstallAppReport,
} from "./types";

/**
 * 唯一的后端能力入口。所有函数都是白名单 command 的薄封装，
 * 页面组件不直接调 invoke，便于集中审计与 mock。
 */
export const ipc = {
  diskInfo: () => invoke<DiskInfo>("disk_info"),
  /** 在访达中打开废纸篓（仅打开，清空由用户在 Finder 内确认；不需要完全磁盘访问） */
  revealTrash: () => invoke<void>("reveal_trash"),

  /** 运行扫描（只读），进度经 onProgress 回调推送；全量体检时每个模块结果经 onModule 推送 */
  scan: async (
    scope: ScanScope,
    onProgress?: (p: ScanProgress) => void,
    onModule?: (m: ScanModuleEvent) => void
  ): Promise<ScanItem[]> => {
    let unlistenP: UnlistenFn | undefined;
    let unlistenM: UnlistenFn | undefined;
    if (onProgress) {
      unlistenP = await listen<ScanProgress>("scan-progress", (e) =>
        onProgress(e.payload)
      );
    }
    if (onModule) {
      unlistenM = await listen<ScanModuleEvent>("scan-module", (e) =>
        onModule(e.payload)
      );
    }
    try {
      return await invoke<ScanItem[]>("scan", { scope });
    } finally {
      unlistenP?.();
      unlistenM?.();
    }
  },

  startups: () => invoke<StartupItem[]>("startups_list"),

  /** 登录时启动（macOS LaunchAgent plist 存在性） */
  launchAtLoginGet: () => invoke<boolean>("launch_at_login_get"),
  launchAtLoginSet: (enabled: boolean) => invoke<void>("launch_at_login_set", { enabled }),
  /** 菜单栏托盘开关 */
  menuBarStatus: () => invoke<boolean>("menu_bar_status"),
  menuBarSet: (show: boolean) => invoke<void>("menu_bar_set", { show }),
  startupSetEnabled: (
    label: string,
    plist: string,
    scope: string,
    enabled: boolean
  ) =>
    invoke<string>("startup_set_enabled", { label, plist, scope, enabled }),

  optimizeList: () => invoke<ScanItem[]>("optimize_list"),
  optimizeRun: (path: string, langEn: boolean) =>
    invoke<string>("optimize_run", { path, langEn }),

  cleanPreview: (items: CleanItemReq[]) =>
    invoke<PreviewItem[]>("clean_preview", { items }),
  /**
   * 执行清理；删除全程经 clean-log / clean-progress 事件推送，
   * 用于全局进度遮罩与实时日志。
   */
  cleanExecute: async (
    items: CleanItemReq[],
    langEn: boolean,
    onLog?: (l: CleanLogEntry) => void,
    onProgress?: (p: CleanProgress) => void
  ): Promise<CleanReport> => {
    let unL: UnlistenFn | undefined;
    let unP: UnlistenFn | undefined;
    if (onLog) unL = await listen<CleanLogEntry>("clean-log", (e) => onLog(e.payload));
    if (onProgress)
      unP = await listen<CleanProgress>("clean-progress", (e) => onProgress(e.payload));
    try {
      return await invoke<CleanReport>("clean_execute", { items, langEn });
    } finally {
      unL?.();
      unP?.();
    }
  },

  settingsGet: () => invoke<AppSettings>("settings_get"),
  settingsSet: (patch: Partial<AppSettings>) =>
    invoke<void>("settings_set", { patch }),

  logsList: () => invoke<LogFile[]>("logs_list"),
  logsRead: (name?: string) => invoke<string>("logs_read", { name: name ?? null }),
  logsReveal: () => invoke<void>("logs_reveal"),

  /** 按 bundle id 打开本机应用（引导去微信/QQ 内清理，不触碰数据） */
  appOpen: (bundleId: string) => invoke<void>("app_open", { bundleId }),
  /** 在访达中定位受保护 IM 的 Documents 目录（仅白名单路径，交 Finder 打开） */
  revealPath: (path: string) => invoke<void>("reveal_path", { path }),
  /** 对 IM 的 Documents 根做只读占用分析 */
  imBreakdown: (path: string) => invoke<ImBreakdown>("im_breakdown", { path }),
  /** 缓存展开明细：点击展开后按需返回某缓存目录的直接子项（体积/文件数/最后修改时间） */
  cacheChildren: (path: string) => invoke<CacheChild[]>("cache_children", { path }),

  /** 已安装应用轻量清单（元数据 + 真实图标，不含体积，打开页面秒回） */
  appsInventory: () => invoke<InstalledApp[]>("apps_inventory"),
  /** 后台补算应用本体 / 数据 / 缓存体积（与清单按 path 对齐） */
  appsSizes: (paths: string[]) =>
    invoke<AppInventorySize[]>("apps_sizes", { paths }),
  /** 应用一键卸载（本体 + 关联数据 + 缓存，全部过安全闸门进废纸篓） */
  appUninstall: (appPath: string, langEn: boolean) =>
    invoke<UninstallAppReport>("app_uninstall", { appPath, langEn }),

  /** 打开「系统设置 → 隐私与安全性 → 完全磁盘访问」引导授权（卸载遇 TCC 拒绝时） */
  openFdaSettings: () => invoke<void>("open_full_disk_access_settings"),

  /** 打开「系统设置 → 隐私与安全性 → App 管理」引导授权（卸载 root 安装的 .app 失败时） */
  openAppManagementSettings: () => invoke<void>("open_app_management_settings"),
};

export type { UnlistenFn };
