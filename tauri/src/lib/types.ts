// 与 tauri/src-tauri/src/commands.rs 的 DTO 逐字段对齐。

/** core scanner::Recommend 等级 */
export type Recommend = "Safe" | "CacheOnly" | "Caution" | "Advanced";

/** core scanner::ScanItem（serde 原样输出） */
export interface ScanItem {
  path: string;
  size_bytes: number;
  category: string;
  selected: boolean;
  deletable: boolean;
  undeletable_reason: string;
  recommend: Recommend;
  description: string;
  /** 聚合删除的真实路径（重复文件副本等）；为空表示单项 path */
  batch_paths: string[];
}

export interface DiskInfo {
  total_bytes: number;
  free_bytes: number;
  used_bytes: number;
  usage_pct: number;
}

export interface StartupItem {
  label: string;
  plist: string;
  scope: string;
  enabled: boolean;
}

/** clean_preview 入参（回传扫描结果中的项） */
export interface CleanItemReq {
  path: string;
  category: string;
  batch_paths: string[];
  size_bytes: number;
  recommend: string;
}

export interface PreviewItem {
  path: string;
  category: string;
  size_bytes: number;
  allowed: boolean;
  reason: string;
  recommend: string;
}

export interface CleanReport {
  deleted: number;
  intercepted: number;
  skipped: number;
  need_password: number;
  backup_id: string | null;
  restorable: number;
  total: number;
  cancelled: boolean;
}

/** core config::AppConfig 的前端子集（只暴露 UI 用到的键） */
export interface AppSettings {
  lang_en: boolean;
  settings_menubar_icon: boolean;
  settings_keep_sudo: boolean;
  settings_scan_cache: boolean;
  settings_scan_all_disks: boolean;
  settings_prefer_official_uninstaller: boolean;
  settings_show_protected_items: boolean;
  schedule_enabled: boolean;
  schedule_interval_days: number;
  schedule_last_run: number;
  settings_confirm_advanced: boolean;
  settings_prevent_lid_close: boolean;
  [k: string]: unknown;
}

export type ScanScope =
  | "dev_cache"
  | "app_cache"
  | "app_data"
  | "large"
  | "dup"
  | "apps"
  | "all"
  | "full";

/**
 * 可被页面复用、按模块缓存的扫描结果键。
 * 全量体检（scope=full）会依次产出 all / large / dup / apps 四个模块。
 */
export type ResultScope = "all" | "large" | "dup" | "apps";

export interface ScanProgress {
  stage: string;
  label: string;
  pct: number;
}

/** 全量体检中单个模块扫描完成的事件载荷（事件名 scan-module） */
export interface ScanModuleEvent {
  scope: ResultScope;
  items: ScanItem[];
}
