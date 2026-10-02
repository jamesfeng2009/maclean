import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type {
  AppSettings,
  CleanItemReq,
  CleanReport,
  DiskInfo,
  PreviewItem,
  ScanItem,
  ScanModuleEvent,
  ScanProgress,
  ScanScope,
  StartupItem,
} from "./types";

/**
 * 唯一的后端能力入口。所有函数都是白名单 command 的薄封装，
 * 页面组件不直接调 invoke，便于集中审计与 mock。
 */
export const ipc = {
  diskInfo: () => invoke<DiskInfo>("disk_info"),

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
  cleanExecute: (items: CleanItemReq[], langEn: boolean) =>
    invoke<CleanReport>("clean_execute", { items, langEn }),

  settingsGet: () => invoke<AppSettings>("settings_get"),
  settingsSet: (patch: Partial<AppSettings>) =>
    invoke<void>("settings_set", { patch }),
};

export type { UnlistenFn };
