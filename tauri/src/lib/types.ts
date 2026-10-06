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
  /** true=移入废纸篓，false=永久删除；风险项以后端强制进废纸篓为准 */
  use_trash?: boolean;
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
  /** 删除方式：smart=安全项永久删/风险项进废纸篓；trash=一律进废纸篓 */
  settings_delete_strategy?: "smart" | "trash";
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

/** 全量体检中单个模块的状态事件（事件名 scan-module） */
export interface ScanModuleEvent {
  scope: ResultScope;
  /** start=模块开始扫描（items 为空）；done=模块完成并带回结果。缺省按 done 处理 */
  status?: "start" | "done";
  items: ScanItem[];
  /** done 时为 true 表示该模块因磁盘繁忙 / 不可达目录只拿到部分结果（可能偏小） */
  partial?: boolean;
}

/**
 * 部分结果事件（事件名 scan-partial）。
 *
 * 高磁盘负载下，某些扫描模块 / 分段在预算内没能完整跑完：后端不再整体归零，而是返回
 * 已统计到的部分，并用本事件告知前端哪些模块「不完整」，由 UI 提示用户空闲后重扫补齐。
 */
export interface ScanPartialEvent {
  /** 未完整完成的模块中文名列表（已去重保序） */
  scopes: string[];
}

/** 删除过程中的单条日志（事件名 clean-log） */
export interface CleanLogEntry {
  line: string;
  path: string;
  ok: boolean;
}

/** 删除进度（事件名 clean-progress） */
export interface CleanProgress {
  done: number;
  total: number;
  pct: number;
  deleted: number;
  intercepted: number;
  skipped: number;
  path: string;
  ok: boolean;
}

/** 日志目录中的一个日志文件（logs_list） */
export interface LogFile {
  name: string;
  size_bytes: number;
}

/** IM 只读占用分析中的一个分类（im_breakdown） */
export interface ImPart {
  key: string;
  label: string;
  size_bytes: number;
}

/** IM Documents 的只读占用构成（im_breakdown，核心 im_data::ImBreakdown） */
export interface ImBreakdown {
  app_name: string;
  storage_hint: string;
  root: string;
  total_bytes: number;
  parts: ImPart[];
}

/** 已安装应用清单条目（apps_inventory，core scanner::uninstall::InstalledApp） */
export interface InstalledApp {
  name: string;
  path: string;
  bundle_id: string;
  version: string;
  /** .app 包本体大小 */
  app_size: number;
  /** 关联数据（Containers / Application Support / Preferences 等） */
  data_size: number;
  /** 关联缓存（Caches / Logs / HTTPStorages 等） */
  cache_size: number;
  /** 是否为浏览器安装的 PWA / Web App */
  is_pwa: boolean;
  /** PWA 来源：chrome / edge / brave / chromium / safari（非 PWA 为空串） */
  pwa_kind: string;
  /** 是否允许一键卸载 */
  deletable: boolean;
  undeletable_reason: string;
  /** 保护级别：none / critical / official / data */
  protection: string;
  has_official_uninstaller: boolean;
  /** 应用真实图标（PNG data URL）；提取失败时缺省，前端回退首字母色块 */
  icon?: string | null;
  /** 前端私有：该应用的体积是否已流式回填（不参与后端数据） */
  _sized?: boolean;
  /** 前端私有：磁盘繁忙、预算内未算完精确体积（不参与后端数据） */
  _skipped?: boolean;
}

/** 应用体积补算条目（apps_sizes，与轻量清单按 path 对齐） */
export interface AppInventorySize {
  path: string;
  app_size: number;
  data_size: number;
  cache_size: number;
}

/** 应用一键卸载结果（app_uninstall，core ops::UninstallAppReport） */
export interface UninstallAppReport {
  /** blocked（拒绝）/ delegated（已交官方卸载器）/ done（已卸载） */
  status: "blocked" | "delegated" | "done";
  message: string;
  deleted: number;
  intercepted: number;
  skipped: number;
  restorable: number;
  total: number;
  backup_id: string | null;
  freed_bytes: number;
  /** 有关联项因 TCC 系统保护（完全磁盘访问 / App 管理）或应用运行中而删除失败，需引导授权 */
  needs_full_disk_access?: boolean;
}
