//! App 状态管理
//!
//! 管理 egui GUI 应用的全部状态，包括当前 Tab、扫描结果、选中状态等。

use crate::safety;
use crate::scanner::{self, ScanItem, Scanner};

/// 跨平台 Touch ID 可用性检查（macOS 专属，其他平台返回 false）
fn touch_id_available_cross() -> bool {
    #[cfg(target_os = "macos")]
    {
        crate::touchid::touch_id_available()
    }
    #[cfg(not(target_os = "macos"))]
    {
        false
    }
}

/// 跨平台 sudo Touch ID 启用状态（macOS 专属，其他平台返回 false）
fn touch_id_enabled_cross() -> bool {
    #[cfg(target_os = "macos")]
    {
        crate::touchid::sudo_touch_id_enabled()
    }
    #[cfg(not(target_os = "macos"))]
    {
        false
    }
}

/// Tab 类型
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    /// 概览（聚合推荐清理）
    Overview,
    /// 开发者缓存
    DevCache,
    /// 大文件
    LargeFiles,
    /// App 缓存（仅缓存子目录，安全可删）
    AppCache,
    /// App 数据（Application Support 整目录，高风险）
    AppData,
    /// App 卸载
    AppUninstall,
    /// 系统优化
    SystemOptimize,
    /// APFS 快照
    Apfs,
    /// 设置
    Settings,
}

impl Tab {
    /// 获取 Tab 的标题（直接返回中文 &'static str）
    pub fn title(self) -> &'static str {
        match self {
            Tab::Overview => "概览",
            Tab::DevCache => "开发者缓存",
            Tab::LargeFiles => "大文件",
            Tab::AppCache => "App缓存",
            Tab::AppData => "App数据",
            Tab::AppUninstall => "应用卸载",
            Tab::SystemOptimize => "系统优化",
            Tab::Apfs => "APFS快照",
            Tab::Settings => "设置",
        }
    }

    /// 所有 Tab
    pub fn all() -> [Tab; 9] {
        [
            Tab::Overview,
            Tab::DevCache,
            Tab::LargeFiles,
            Tab::AppCache,
            Tab::AppData,
            Tab::AppUninstall,
            Tab::SystemOptimize,
            Tab::Apfs,
            Tab::Settings,
        ]
    }

    /// 下一个 Tab
    pub fn next(self) -> Self {
        match self {
            Tab::Overview => Tab::DevCache,
            Tab::DevCache => Tab::LargeFiles,
            Tab::LargeFiles => Tab::AppCache,
            Tab::AppCache => Tab::AppData,
            Tab::AppData => Tab::AppUninstall,
            Tab::AppUninstall => Tab::SystemOptimize,
            Tab::SystemOptimize => Tab::Apfs,
            Tab::Apfs => Tab::Settings,
            Tab::Settings => Tab::Overview,
        }
    }

    /// 上一个 Tab
    pub fn prev(self) -> Self {
        match self {
            Tab::Overview => Tab::Settings,
            Tab::DevCache => Tab::Overview,
            Tab::LargeFiles => Tab::DevCache,
            Tab::AppCache => Tab::LargeFiles,
            Tab::AppData => Tab::AppCache,
            Tab::AppUninstall => Tab::AppData,
            Tab::SystemOptimize => Tab::AppUninstall,
            Tab::Apfs => Tab::SystemOptimize,
            Tab::Settings => Tab::Apfs,
        }
    }
}

/// 扫描状态
#[derive(Debug, Clone)]
pub enum ScanState {
    /// 空闲
    Idle,
    /// 扫描中
    Scanning,
    /// 扫描完成
    Done,
}

/// 删除确认状态
#[derive(Debug, Clone)]
pub enum ConfirmState {
    /// 无确认
    None,
    /// 等待确认删除
    Pending,
    /// 正在删除
    Deleting,
    /// 等待用户输入 sudo 密码
    NeedSudoPassword,
    /// 提示用户是否启用 Touch ID
    OfferTouchIdSetup,
    /// 等待用户在 Terminal 中完成 Touch ID 启用（轮询中）
    WaitForTouchIdSetup,
    /// 正在通过 Touch ID 删除（sudo 已配置 pam_tid.so）
    SudoWithTouchId,
}

/// App 主状态
pub struct App {
    /// 当前 Tab
    pub tab: Tab,
    /// 每个 Tab 的扫描结果
    pub results: [Vec<ScanItem>; 9],
    /// 每个 Tab 的扫描状态
    pub scan_states: [ScanState; 9],
    /// 列表选中索引
    pub list_index: usize,
    /// App 卸载 Tab 当前选中的应用分组索引
    pub selected_uninstall_app_index: Option<usize>,
    /// 磁盘总空间（字节）
    pub disk_total: u64,
    /// 磁盘可用空间（字节）
    pub disk_free: u64,
    /// 清理日志
    pub logs: Vec<String>,
    /// 确认状态
    pub confirm: ConfirmState,
    /// 待删除的项索引列表，每项为 (tab_index, item_index)
    /// 支持跨 Tab 删除（如概览一键清理）
    pub pending_delete: Vec<(usize, usize)>,
    /// 是否应该退出
    pub should_quit: bool,
    /// 扫描耗时（毫秒）
    pub scan_time_ms: [u64; 9],
    /// 语言切换（true=英文, false=中文）
    pub lang_en: bool,
    /// 删除进度：已完成的项数
    pub delete_done: usize,
    /// 删除进度：总项数
    pub delete_total: usize,
    /// 扫描进度 (0.0 ~ 1.0)
    pub scan_progress: f32,
    /// 删除结果：成功的路径集合
    pub deleted_paths: Vec<String>,
    /// 删除结果汇总（删除完成后显示）
    pub delete_summary: Option<(usize, usize, usize)>, // (成功, 失败, 跳过)
    /// 删除失败的路径列表（供 sudo 重试）
    pub failed_paths: Vec<(String, String)>, // (path, category)
    /// 是否需要显示权限引导弹窗
    pub show_permission_guide: bool,
    /// 是否显示删除预览（dry-run）
    pub show_preview: bool,
    /// sudo 密码输入框的当前内容
    pub sudo_password_input: String,
    /// 已确认的用户密码（用于 sudo -S）
    pub sudo_password: Option<String>,
    /// 需要 sudo 删除的项（普通删除失败后暂存）
    pub sudo_failed_items: Vec<(String, String)>,
    /// sudo 密码错误提示
    pub sudo_error: Option<String>,
    /// Touch ID 是否可用（缓存检测结果）
    pub touch_id_available: bool,
    /// Touch ID 是否已为 sudo 启用（缓存检测结果）
    pub touch_id_enabled: bool,
    /// 当前是否处于"输入密码以启用 Touch ID"模式
    /// true: 确认密码后先创建 sudo_local，再执行删除
    /// false: 确认密码后直接执行删除
    pub touch_id_setup_mode: bool,
    /// Touch ID 设置/执行中的错误提示
    pub touch_id_error: Option<String>,
    /// Touch ID 启用等待开始时间（用于超时检测）
    pub touch_id_wait_start: Option<std::time::Instant>,
    /// 关联文件明细：key = ScanItem.path，value = [(路径, 大小, 标签)]
    /// 用于 App卸载 tab 展开显示每个关联文件的大小和路径
    pub associated_details: std::collections::HashMap<String, Vec<(String, u64, String)>>,
    /// 已展开的项路径集合（用于展开/收起状态）
    pub expanded_items: std::collections::HashSet<String>,
    /// 磁盘分析器：当前浏览的目录路径
    pub disk_analyzer_path: Option<std::path::PathBuf>,
    /// 磁盘分析器：导航历史栈（用于返回上一级）
    pub disk_analyzer_history: Vec<std::path::PathBuf>,
    /// sudo 会话是否活跃（通过 keepalive 保活）
    pub sudo_session_active: bool,
    /// 过滤/搜索输入框内容
    pub filter_query: String,
    /// 过滤输入框是否获得焦点
    pub filter_active: bool,
    /// 磁盘监控：上次检查时间（用于 5 秒间隔轮询）
    pub disk_last_check: std::time::Instant,
    /// 当前扫描路径（显示在扫描进度 UI）
    pub scan_current_path: String,
    /// 当前分类过滤（按 category 前缀过滤）
    pub filter_category: Option<String>,
    /// App卸载 Tab 已展开的应用分组名集合
    pub expanded_app_groups: std::collections::HashSet<String>,
    /// App卸载 Tab 当前按推荐等级过滤（Safe / Caution / Advanced）
    pub app_uninstall_recommend_filter: Option<crate::scanner::Recommend>,
    /// 是否显示残留清理弹窗（卸载后检测到残留时弹出）
    pub show_residual_dialog: bool,
    /// 启动时显示菜单栏图标
    pub settings_menubar_icon: bool,
    /// 自动保持 sudo 会话
    pub settings_keep_sudo: bool,
    /// 扫描结果本地缓存
    pub settings_scan_cache: bool,
    /// 删除前二次确认（Advanced 项目）
    pub settings_confirm_advanced: bool,
    /// 合盖时禁止删除（macOS only）
    pub settings_prevent_lid_close: bool,
    /// 菜单栏 HUD 是否展开
    pub hud_open: bool,
    /// 上次点击托盘图标的位置（逻辑像素），用于 HUD 窗口定位
    pub last_hud_click_pos: Option<(f32, f32)>,
    /// 残留项选中状态（与残留列表一一对应，true=选中清理）
    pub residual_selected: Vec<bool>,
    /// 残留清理中（正在执行清理操作）
    pub residual_cleaning: bool,
    /// Windows: 卸载后检测到的残留信息
    #[cfg(target_os = "windows")]
    pub uninstall_residual: Option<scanner::windows_apps::UninstallResidual>,
}

impl App {
    /// 启动时尝试从本地缓存加载当前 Tab 的扫描结果
    ///
    /// 仅在 settings_scan_cache 开启且缓存有效时执行，避免每次启动都重新扫描。
    pub fn load_current_tab_cache(&mut self) {
        if !self.settings_scan_cache {
            crate::logger::info("缓存设置已关闭，跳过缓存加载");
            return;
        }
        let cache_name = match self.tab {
            Tab::Overview | Tab::Settings => None,
            Tab::DevCache => Some("dev_cache"),
            Tab::LargeFiles => Some("large_files"),
            Tab::AppCache => Some("app_cache"),
            Tab::AppData => Some("app_data"),
            Tab::AppUninstall => Some("app_uninstall"),
            Tab::SystemOptimize => None,
            Tab::Apfs => Some("apfs"),
        };
        crate::logger::info(&format!(
            "尝试加载当前 Tab({:?}) 缓存: {:?}",
            self.tab, cache_name
        ));
        if let Some(name) = cache_name {
            match scanner::cache::load_cache(name) {
                Some(cached) => {
                    crate::logger::info(&format!(
                        "缓存加载成功: {} 项, 总大小 {}",
                        cached.items.len(),
                        crate::scanner::format_size(cached.total_size)
                    ));
                    let mut items = cached.items;
                    for item in &mut items {
                        let (deletable, reason) = scanner::check_deletable(&item.path);
                        item.deletable = deletable && item.deletable;
                        if !item.deletable && !reason.is_empty() {
                            item.undeletable_reason = reason;
                        }
                    }
                    let idx = self.tab_index();
                    self.results[idx] = items;
                    self.scan_states[idx] = ScanState::Done;
                    self.scan_time_ms[idx] = cached.scan_time_ms;
                }
                None => {
                    crate::logger::info("缓存加载失败或不存在");
                }
            }
        }
    }

    /// 创建新 App 状态
    pub fn new() -> Self {
        let (disk_total, disk_free) = get_disk_info();

        Self {
            tab: Tab::Overview,
            results: [
                Vec::new(),
                Vec::new(),
                Vec::new(),
                Vec::new(),
                Vec::new(),
                Vec::new(),
                Vec::new(),
                Vec::new(),
                Vec::new(),
            ],
            scan_states: [
                ScanState::Idle,
                ScanState::Idle,
                ScanState::Idle,
                ScanState::Idle,
                ScanState::Idle,
                ScanState::Idle,
                ScanState::Idle,
                ScanState::Idle,
                ScanState::Idle,
            ],
            list_index: 0,
            selected_uninstall_app_index: None,
            disk_total,
            disk_free,
            logs: Vec::new(),
            confirm: ConfirmState::None,
            pending_delete: Vec::new(),
            should_quit: false,
            scan_time_ms: [0; 9],
            lang_en: false, // 默认中文
            settings_menubar_icon: true,
            settings_keep_sudo: true,
            settings_scan_cache: true,
            settings_confirm_advanced: true,
            settings_prevent_lid_close: true,
            hud_open: false,
            scan_current_path: String::new(),
            filter_category: None,
            expanded_app_groups: std::collections::HashSet::new(),
            app_uninstall_recommend_filter: None,
            delete_done: 0,
            delete_total: 0,
            scan_progress: 0.0,
            deleted_paths: Vec::new(),
            delete_summary: None,
            failed_paths: Vec::new(),
            show_permission_guide: Self::check_full_disk_access() == false,
            show_preview: false,
            sudo_password_input: String::new(),
            sudo_password: None,
            sudo_failed_items: Vec::new(),
            sudo_error: None,
            touch_id_available: touch_id_available_cross(),
            touch_id_enabled: touch_id_enabled_cross(),
            touch_id_setup_mode: false,
            touch_id_error: None,
            touch_id_wait_start: None,
            associated_details: std::collections::HashMap::new(),
            expanded_items: std::collections::HashSet::new(),
            disk_analyzer_path: None,
            disk_analyzer_history: Vec::new(),
            sudo_session_active: false,
            filter_query: String::new(),
            filter_active: false,
            disk_last_check: std::time::Instant::now(),
            show_residual_dialog: false,
            last_hud_click_pos: None,
            residual_selected: Vec::new(),
            residual_cleaning: false,
            #[cfg(target_os = "windows")]
            uninstall_residual: None,
        }
    }

    /// 获取当前 Tab 索引
    pub fn tab_index(&self) -> usize {
        match self.tab {
            Tab::Overview => 0,
            Tab::DevCache => 1,
            Tab::LargeFiles => 2,
            Tab::AppCache => 3,
            Tab::AppData => 4,
            Tab::AppUninstall => 5,
            Tab::SystemOptimize => 6,
            Tab::Apfs => 7,
            Tab::Settings => 8,
        }
    }

    /// 切换到下一个 Tab
    pub fn next_tab(&mut self) {
        if !matches!(self.confirm, ConfirmState::None) {
            return;
        }
        self.tab = self.tab.next();
        self.list_index = 0;
    }

    /// 切换到上一个 Tab
    pub fn prev_tab(&mut self) {
        if !matches!(self.confirm, ConfirmState::None) {
            return;
        }
        self.tab = self.tab.prev();
        self.list_index = 0;
    }

    /// 获取当前 Tab 的扫描结果
    pub fn current_items(&self) -> &Vec<ScanItem> {
        &self.results[self.tab_index()]
    }

    /// 获取经过过滤后的当前 Tab 扫描项索引列表
    ///
    /// 当 `filter_query` 非空时，只返回路径、类别或描述中包含查询字符串
    /// （大小写不敏感）的项的索引。查询为空时返回所有项的索引。
    /// 当 `filter_category` 存在时，只返回 category 前缀匹配的项。
    pub fn filtered_indices(&self) -> Vec<usize> {
        let idx = self.tab_index();
        let items = &self.results[idx];
        let query = self.filter_query.trim().to_lowercase();
        let cat_prefix = self
            .filter_category
            .as_deref()
            .map(|s| s.trim().to_lowercase());
        items
            .iter()
            .enumerate()
            .filter(|(_, item)| {
                let prefix_ok = cat_prefix.as_ref().map_or(true, |prefix| {
                    item.category.to_lowercase().starts_with(prefix)
                });
                if !prefix_ok {
                    return false;
                }
                if query.is_empty() {
                    return true;
                }
                item.path.to_lowercase().contains(&query)
                    || item.category.to_lowercase().contains(&query)
                    || item.description.to_lowercase().contains(&query)
            })
            .map(|(i, _)| i)
            .collect()
    }

    /// 清除过滤条件
    pub fn clear_filter(&mut self) {
        self.filter_query.clear();
        self.filter_active = false;
        self.filter_category = None;
    }

    /// 磁盘监控：轮询检查磁盘空间（每 5 秒调用一次）
    ///
    /// 在 `update()` 中调用，自动节流避免频繁 statvfs。
    pub fn poll_disk_space(&mut self) {
        if self.disk_last_check.elapsed().as_secs() < 5 {
            return;
        }
        let (total, free) = get_disk_info();
        if total > 0 {
            self.disk_total = total;
            self.disk_free = free;
        }
        self.disk_last_check = std::time::Instant::now();
    }

    /// 磁盘告警等级
    ///
    /// 返回 (等级, 颜色, 剩余百分比)
    /// - 0: 正常 (>20%)
    /// - 1: 注意 (10-20%)
    /// - 2: 警告 (5-10%)
    /// - 3: 危险 (<5%)
    pub fn disk_alert_level(&self) -> (u8, egui::Color32, f64) {
        if self.disk_total == 0 {
            return (0, egui::Color32::from_rgb(100, 200, 100), 0.0);
        }
        let pct = self.disk_free as f64 / self.disk_total as f64 * 100.0;
        if pct < 5.0 {
            (3, egui::Color32::from_rgb(220, 50, 50), pct)
        } else if pct < 10.0 {
            (2, egui::Color32::from_rgb(230, 140, 30), pct)
        } else if pct < 20.0 {
            (1, egui::Color32::from_rgb(220, 200, 50), pct)
        } else {
            (0, egui::Color32::from_rgb(100, 200, 100), pct)
        }
    }

    /// 获取当前 Tab 的扫描状态
    pub fn current_scan_state(&self) -> &ScanState {
        &self.scan_states[self.tab_index()]
    }

    /// 列表向上移动
    pub fn move_up(&mut self) {
        if self.list_index > 0 {
            self.list_index -= 1;
        }
    }

    /// 列表向下移动
    pub fn move_down(&mut self) {
        let len = self.current_items().len();
        if len > 0 && self.list_index < len - 1 {
            self.list_index += 1;
        }
    }

    /// 切换当前选中项的勾选状态
    pub fn toggle_select(&mut self) {
        let idx = self.tab_index();
        let i = self.list_index;
        if i < self.results[idx].len() && self.results[idx][i].deletable {
            self.results[idx][i].selected = !self.results[idx][i].selected;
        }
    }

    /// 全选当前 Tab 中可删除的项
    pub fn select_all(&mut self) {
        let idx = self.tab_index();
        for item in &mut self.results[idx] {
            if item.deletable {
                item.selected = true;
            }
        }
    }

    /// 智能选择：只选中推荐清理（Safe / CacheOnly）的项
    pub fn select_safe_only(&mut self) {
        let idx = self.tab_index();
        for item in &mut self.results[idx] {
            item.selected = item.deletable && item.recommend.default_selected();
        }
    }

    /// 取消全选
    pub fn deselect_all(&mut self) {
        let idx = self.tab_index();
        for item in &mut self.results[idx] {
            item.selected = false;
        }
    }

    /// 磁盘分析器：进入子目录
    ///
    /// 将当前路径压入历史栈，然后切换到新路径。
    pub fn disk_analyzer_enter(&mut self, path: std::path::PathBuf) {
        if let Some(current) = &self.disk_analyzer_path {
            self.disk_analyzer_history.push(current.clone());
        }
        self.disk_analyzer_path = Some(path);
    }

    /// 磁盘分析器：返回上一级
    ///
    /// 从历史栈弹出一个路径。如果栈为空，返回到主目录。
    pub fn disk_analyzer_back(&mut self) -> Option<std::path::PathBuf> {
        let prev = self.disk_analyzer_history.pop();
        if prev.is_some() {
            self.disk_analyzer_path = prev.clone();
        } else {
            self.disk_analyzer_path = None; // 回到主目录
        }
        prev
    }

    /// 磁盘分析器：获取当前浏览路径
    ///
    /// 如果为 None，表示在主目录（home）。
    pub fn disk_analyzer_current_path(&self) -> std::path::PathBuf {
        self.disk_analyzer_path
            .clone()
            .unwrap_or_else(|| scanner::home_dir())
    }

    /// 统计当前 Tab 中推荐清理（Safe + CacheOnly）的项数
    pub fn safe_count(&self) -> usize {
        self.current_items()
            .iter()
            .filter(|i| i.deletable && i.recommend.default_selected())
            .count()
    }

    /// 统计当前 Tab 中推荐清理（Safe + CacheOnly）的总大小
    pub fn safe_size(&self) -> u64 {
        self.current_items()
            .iter()
            .filter(|i| i.deletable && i.recommend.default_selected())
            .map(|i| i.size_bytes)
            .sum()
    }

    /// 统计当前 Tab 中仅缓存（CacheOnly）的项数
    pub fn cache_only_count(&self) -> usize {
        self.current_items()
            .iter()
            .filter(|i| i.deletable && i.recommend == crate::scanner::Recommend::CacheOnly)
            .count()
    }

    /// 统计当前 Tab 中仅缓存（CacheOnly）的总大小
    pub fn cache_only_size(&self) -> u64 {
        self.current_items()
            .iter()
            .filter(|i| i.deletable && i.recommend == crate::scanner::Recommend::CacheOnly)
            .map(|i| i.size_bytes)
            .sum()
    }

    /// 统计当前 Tab 中需谨慎（Caution）的项数
    pub fn caution_count(&self) -> usize {
        self.current_items()
            .iter()
            .filter(|i| i.deletable && i.recommend == crate::scanner::Recommend::Caution)
            .count()
    }

    /// 统计当前 Tab 中需谨慎（Caution）的总大小
    pub fn caution_size(&self) -> u64 {
        self.current_items()
            .iter()
            .filter(|i| i.deletable && i.recommend == crate::scanner::Recommend::Caution)
            .map(|i| i.size_bytes)
            .sum()
    }

    /// 统计当前 Tab 中需确认（Advanced）的数量
    pub fn advanced_count(&self) -> usize {
        self.current_items()
            .iter()
            .filter(|i| i.deletable && i.recommend == crate::scanner::Recommend::Advanced)
            .count()
    }

    /// 统计当前 Tab 中需确认（Advanced）的总大小
    pub fn advanced_size(&self) -> u64 {
        self.current_items()
            .iter()
            .filter(|i| i.deletable && i.recommend == crate::scanner::Recommend::Advanced)
            .map(|i| i.size_bytes)
            .sum()
    }

    /// 执行扫描（当前 Tab）
    pub fn scan_current(&mut self) {
        let idx = self.tab_index();
        self.scan_states[idx] = ScanState::Scanning;

        let result = match self.tab {
            Tab::Overview | Tab::Settings => scanner::ScanResult {
                items: Vec::new(),
                total_size: 0,
                scan_time_ms: 0,
            },
            Tab::DevCache => scanner::dev_cache::DevCacheScanner::new().scan(),
            Tab::LargeFiles => scanner::large_files::LargeFileScanner::new().scan(),
            #[cfg(target_os = "macos")]
            Tab::AppCache => scanner::app_cache::AppCacheScanner::new().scan(),
            #[cfg(target_os = "macos")]
            Tab::AppData => scanner::app_data::AppDataScanner::new().scan(),
            #[cfg(target_os = "macos")]
            Tab::AppUninstall => scanner::uninstall::UninstallScanner::new().scan(),
            #[cfg(target_os = "macos")]
            Tab::SystemOptimize => scanner::optimize::OptimizeScanner::new().scan(),
            #[cfg(target_os = "macos")]
            Tab::Apfs => scanner::apfs::ApfsScanner::new().scan(),
            // Windows/Linux: 这些 Tab 返回空结果
            #[cfg(not(target_os = "macos"))]
            Tab::AppCache => {
                #[cfg(target_os = "windows")]
                {
                    scanner::windows_apps::WindowsAppCacheScanner::new().scan()
                }
                #[cfg(not(target_os = "windows"))]
                {
                    scanner::ScanResult {
                        items: Vec::new(),
                        total_size: 0,
                        scan_time_ms: 0,
                    }
                }
            }
            #[cfg(not(target_os = "macos"))]
            Tab::AppData => {
                #[cfg(target_os = "windows")]
                {
                    scanner::windows_apps::WindowsAppDataScanner::new().scan()
                }
                #[cfg(not(target_os = "windows"))]
                {
                    scanner::ScanResult {
                        items: Vec::new(),
                        total_size: 0,
                        scan_time_ms: 0,
                    }
                }
            }
            #[cfg(not(target_os = "macos"))]
            Tab::AppUninstall => {
                #[cfg(target_os = "windows")]
                {
                    scanner::windows_apps::WindowsUninstallScanner::new().scan()
                }
                #[cfg(not(target_os = "windows"))]
                {
                    scanner::ScanResult {
                        items: Vec::new(),
                        total_size: 0,
                        scan_time_ms: 0,
                    }
                }
            }
            #[cfg(not(target_os = "macos"))]
            Tab::SystemOptimize | Tab::Apfs => scanner::ScanResult {
                items: Vec::new(),
                total_size: 0,
                scan_time_ms: 0,
            },
        };

        let item_count = result.items.len();
        let total_size = result.total_size;
        let scan_time = result.scan_time_ms;
        self.scan_time_ms[idx] = result.scan_time_ms;
        self.results[idx] = result.items;
        crate::logger::info(&format!(
            "扫描完成 [{}]: {} 项, {}, 耗时 {}ms",
            idx,
            item_count,
            crate::scanner::format_size(total_size),
            scan_time
        ));
        self.scan_states[idx] = ScanState::Done;
        self.list_index = 0;

        // 扫描后预检查可删除性：对每项运行安全检查，
        // 不可删除的标记 deletable=false，UI 会灰色显示且不可选中
        self.precheck_deletability();

        // App卸载 tab：计算关联文件明细，供 UI 展开
        if self.tab == Tab::AppUninstall {
            self.populate_associated_details();
        }

        // 刷新磁盘信息
        let (total, free) = get_disk_info();
        self.disk_total = total;
        self.disk_free = free;
    }

    /// 扫描后预检查可删除性
    ///
    /// 对当前 Tab 的每个 ScanItem 运行安全检查（safety::check_path_safety_with_category）。
    /// - Danger → 标记 deletable=false，写明原因，UI 灰色不可选
    /// - Warning → 保持 deletable=true，但在描述中追加警告
    /// - Safe → 不变
    ///
    /// 这样在扫描阶段就屏蔽掉确定无法删除的项，避免用户选中后删除失败。
    fn precheck_deletability(&mut self) {
        let idx = self.tab_index();
        let items = &mut self.results[idx];
        for item in items.iter_mut() {
            if !item.deletable {
                continue; // 已被扫描器标记为不可删除
            }
            match safety::check_path_safety_with_category(&item.path, &item.category) {
                safety::SafetyCheck::Safe => {}
                safety::SafetyCheck::Warning(msg) => {
                    // 保持可删除，但追加警告到描述
                    if !item.description.contains(&msg) {
                        item.description = format!("{} ⚠️ {}", item.description, msg);
                    }
                }
                safety::SafetyCheck::Danger(msg) => {
                    item.deletable = false;
                    item.undeletable_reason = msg;
                }
            }
        }
    }

    /// 为 App卸载 tab 的每个项计算关联文件明细
    ///
    /// 遍历 batch_paths 中每个路径，计算大小并生成标签。
    /// 对于大于 100MB 的目录，进一步枚举其直接子目录并作为子明细展示，
    /// 让用户清楚看到空间被什么占用（如 Documents、Caches 等）。
    fn populate_associated_details(&mut self) {
        self.associated_details.clear();
        let items = self.results[self.tab_index()].clone();
        for item in &items {
            if item.batch_paths.is_empty() {
                continue;
            }
            let mut details: Vec<(String, u64, String)> = Vec::new();
            for bp in &item.batch_paths {
                let p = std::path::Path::new(bp);
                let size = if p.is_dir() {
                    scanner::dir_size(p)
                } else if p.is_file() {
                    p.symlink_metadata().map(|m| m.len()).unwrap_or(0)
                } else {
                    0
                };
                let label = classify_associated_path(bp, self.lang_en);
                details.push((bp.clone(), size, label));

                // 对于大于 100MB 的目录，枚举直接子目录展示子明细
                if p.is_dir() && size > 100 * 1024 * 1024 {
                    if let Ok(entries) = std::fs::read_dir(p) {
                        let mut sub_dirs: Vec<(String, u64)> = Vec::new();
                        for entry in entries.filter_map(|e| e.ok()) {
                            let sp = entry.path();
                            if !sp.is_dir() {
                                continue;
                            }
                            let ss = scanner::dir_size(&sp);
                            if ss > 10 * 1024 * 1024 {
                                // > 10MB 的子目录才展示
                                sub_dirs.push((sp.to_string_lossy().to_string(), ss));
                            }
                        }
                        // 按大小降序，最多展示 8 个子目录
                        sub_dirs.sort_by(|a, b| b.1.cmp(&a.1));
                        for (sp, ss) in sub_dirs.into_iter().take(8) {
                            let sub_label = format!(
                                "  ├─ {}",
                                std::path::Path::new(&sp)
                                    .file_name()
                                    .and_then(|n| n.to_str())
                                    .unwrap_or("?")
                            );
                            details.push((sp, ss, sub_label));
                        }
                    }
                }
            }
            // 按大小降序排列
            details.sort_by(|a, b| b.1.cmp(&a.1));
            self.associated_details.insert(item.path.clone(), details);
        }
    }

    /// 计算当前 Tab 选中项的总大小
    pub fn selected_total_size(&self) -> u64 {
        self.current_items()
            .iter()
            .filter(|i| i.selected)
            .map(|i| i.size_bytes)
            .sum()
    }

    /// 获取当前 Tab 选中项的数量
    pub fn selected_count(&self) -> usize {
        self.current_items().iter().filter(|i| i.selected).count()
    }

    /// 统计所有 Tab 中可安全释放的总大小（Safe / CacheOnly）
    pub fn total_releasable_size(&self) -> u64 {
        self.results
            .iter()
            .flat_map(|v| v.iter())
            .filter(|i| i.deletable && i.recommend.default_selected())
            .map(|i| i.size_bytes)
            .sum()
    }

    /// 准备删除选中的项（进入确认状态）
    /// 只收集当前 Tab 的选中项。
    pub fn prepare_delete(&mut self) {
        let idx = self.tab_index();
        let selected: Vec<(usize, usize)> = self
            .results[idx]
            .iter()
            .enumerate()
            .filter(|(_, item)| item.selected)
            .map(|(i, _)| (idx, i))
            .collect();

        if selected.is_empty() {
            return;
        }

        self.pending_delete = selected;
        self.confirm = ConfirmState::Pending;
    }

    /// 准备跨 Tab 删除（概览一键清理使用）
    pub fn prepare_delete_cross_tab(&mut self, items: Vec<(usize, usize)>) {
        let selected: Vec<(usize, usize)> = items
            .into_iter()
            .filter(|(tab_idx, item_idx)| {
                self.results
                    .get(*tab_idx)
                    .and_then(|v| v.get(*item_idx))
                    .map(|item| item.deletable)
                    .unwrap_or(false)
            })
            .collect();

        if selected.is_empty() {
            return;
        }

        self.pending_delete = selected;
        self.confirm = ConfirmState::Pending;
    }

    /// 确认删除 - 收集待删除项，返回 (path, category, batch_paths, use_trash) 供后台线程使用
    /// use_trash: true 表示移至废纸篓（可恢复），false 表示永久删除
    pub fn confirm_delete(&mut self) -> Vec<(String, String, Vec<String>, bool)> {
        self.confirm = ConfirmState::Deleting;

        // 收集要删除的路径和类别（跳过不可删除的项）
        // 策略：Safe 级别永久删除（缓存自动重建），Caution/Advanced 移至废纸篓（可恢复）
        let to_delete: Vec<(String, String, Vec<String>, bool)> = self
            .pending_delete
            .iter()
            .rev()
            .filter(|(tab_idx, item_idx)| {
                self.results
                    .get(*tab_idx)
                    .and_then(|v| v.get(*item_idx))
                    .map(|item| item.deletable)
                    .unwrap_or(false)
            })
            .map(|(tab_idx, item_idx)| {
                let item = &self.results[*tab_idx][*item_idx];
                let use_trash = match item.recommend {
                    crate::scanner::Recommend::Safe => false, // 缓存类：永久删除
                    crate::scanner::Recommend::CacheOnly => false, // 应用缓存/日志：永久删除
                    crate::scanner::Recommend::Caution => false, // 系统缓存：永久删除（root 属主无法移到用户废纸篓）
                    crate::scanner::Recommend::Advanced => true, // 大文件/高级项：移至废纸篓
                };
                (
                    item.path.clone(),
                    item.category.clone(),
                    item.batch_paths.clone(),
                    use_trash,
                )
            })
            .collect();

        self.delete_total = to_delete.len();
        self.delete_done = 0;
        self.logs.clear();
        self.deleted_paths.clear();
        self.delete_summary = None;
        self.failed_paths.clear();
        to_delete
    }

    /// 接收一条删除日志并更新进度
    pub fn receive_delete_log(
        &mut self,
        log: String,
        path: String,
        category: String,
        success: bool,
    ) {
        if success {
            self.deleted_paths.push(path);
        } else {
            self.failed_paths.push((path, category));
        }
        self.logs.push(log);
        self.delete_done += 1;
    }

    /// 删除完成后的收尾工作
    /// 支持跨 Tab：根据 pending_delete 中记录的 (tab_idx, item_idx) 处理。
    pub fn finish_delete(&mut self) {
        // 统计成功/失败
        let success_count = self.deleted_paths.len();
        let failed_count = self.failed_paths.len();

        // 按 Tab 分组处理：移除成功删除的项，取消该 Tab 的选中状态
        let mut affected_tabs: std::collections::HashSet<usize> = std::collections::HashSet::new();
        let deleted = self.deleted_paths.clone();

        for (tab_idx, item_idx) in &self.pending_delete {
            affected_tabs.insert(*tab_idx);
            if let Some(item) = self.results.get_mut(*tab_idx).and_then(|v| v.get_mut(*item_idx)) {
                item.selected = false;
                if deleted.contains(&item.path) {
                    // 标记为已删除（稍后统一移除）
                    item.path = String::new();
                }
            }
        }

        let affected_tabs_clone = affected_tabs.clone();

        // 从各 Tab 中移除路径为空的项
        for tab_idx in affected_tabs {
            self.results[tab_idx].retain(|item| !item.path.is_empty());
        }

        // 失效受影响 Tab 的扫描缓存
        for tab_idx in affected_tabs_clone {
            let cache_name = match Tab::all().get(tab_idx) {
                Some(Tab::DevCache) => Some("dev_cache"),
                Some(Tab::LargeFiles) => Some("large_files"),
                Some(Tab::AppCache) => Some("app_cache"),
                Some(Tab::AppData) => Some("app_data"),
                Some(Tab::AppUninstall) => Some("app_uninstall"),
                Some(Tab::Apfs) => Some("apfs"),
                _ => None,
            };
            if let Some(name) = cache_name {
                scanner::cache::invalidate_cache(name);
            }
        }

        // 生成汇总
        self.delete_summary = Some((success_count, failed_count, 0));

        self.logs.push(format!(
            "✅ {}",
            self.tf(
                "finish_summary",
                &[&success_count.to_string(), &failed_count.to_string()]
            )
        ));

        // 刷新磁盘信息
        let (total, free) = get_disk_info();
        self.disk_total = total;
        self.disk_free = free;

        self.pending_delete.clear();
        self.confirm = ConfirmState::None;
        self.list_index = 0;
    }

    /// 关闭删除汇总弹窗
    pub fn dismiss_summary(&mut self) {
        self.delete_summary = None;
    }

    /// 检测是否有完全磁盘访问权限
    /// 通过尝试读取多个 TCC 保护目录来判断
    pub fn check_full_disk_access() -> bool {
        let home = std::env::var("HOME").unwrap_or_default();

        // 尝试多个 TCC 保护路径，任一可读即说明有 FDA
        let test_paths = [
            format!("{}/Library/Metadata", home),
            format!("{}/Library/Safari", home),
            format!("{}/Library/Containers/com.apple.mail", home),
            format!("{}/Library/Mail", home),
            format!("{}/Library/Messages", home),
        ];

        for path in &test_paths {
            let p = std::path::Path::new(path);
            if p.exists() {
                // 尝试 read_dir，如果能读取说明有 FDA
                if std::fs::read_dir(p).is_ok() {
                    return true;
                }
            }
        }

        false
    }

    /// 关闭权限引导弹窗
    pub fn dismiss_permission_guide(&mut self) {
        self.show_permission_guide = false;
    }

    /// 取消删除
    pub fn cancel_delete(&mut self) {
        self.pending_delete.clear();
        self.confirm = ConfirmState::None;
    }

    /// 退出
    pub fn quit(&mut self) {
        self.should_quit = true;
    }

    /// 切换中英文语言
    pub fn toggle_lang(&mut self) {
        self.lang_en = !self.lang_en;
    }

    /// 根据当前语言返回 UI 文本
    ///
    /// 简单的 key-value 映射，不依赖外部 i18n 库。
    /// `lang_en == true` 时返回英文，否则返回中文。
    pub fn t(&self, key: &str) -> &'static str {
        Self::t_lang(self.lang_en, key)
    }

    /// 静态版本，供后台线程等无法访问 `&App` 的地方使用
    pub fn t_lang(lang_en: bool, key: &str) -> &'static str {
        if lang_en {
            match key {
                // Tab 标题
                "tab_overview" => "Overview",
                "tab_dev_cache" => "Dev Cache",
                "tab_large_files" => "Large Files",
                "tab_app_cache" => "App Cache",
                "tab_app_data" => "App Data",
                "tab_app_uninstall" => "Uninstall",
                "tab_system_optimize" => "Optimize",
                "tab_apfs" => "APFS Snapshots",
                "tab_settings" => "Settings",
                "settings_general" => "General",
                "settings_safety" => "Safety",
                "settings_language" => "Language",
                "setting_menubar_icon" => "Show menu bar icon on launch",
                "setting_menubar_icon_desc" => "Display disk usage indicator in the macOS menu bar",
                "setting_keep_sudo" => "Keep sudo session alive",
                "setting_keep_sudo_desc" => "Avoid repeated administrator password prompts",
                "setting_scan_cache" => "Cache scan results locally",
                "setting_scan_cache_desc" => "Avoid re-scanning within 7 days for faster startup",
                "setting_confirm_advanced" => "Double-check before deleting Advanced items",
                "setting_confirm_advanced_desc" => "Advanced items require manual confirmation",
                "setting_prevent_lid_close" => "Prevent deletion while lid is closed",
                "setting_prevent_lid_close_desc" => "Detect MacBook clamshell state to avoid accidental deletion",
                "setting_language" => "Interface language",
                "setting_language_current" => "Current: Simplified Chinese",
                "switch_to_english" => "Switch to English",
                "switch_to_chinese" => "Switch to 中文",
                // 按钮
                "scan" => "Scan",
                "delete" => "Delete",
                "select_all" => "Select All",
                "deselect_all" => "Deselect All",
                "select_safe" => "Clean",
                "expand_all" => "Expand All",
                "confirm_delete" => "Confirm Delete",
                "cancel" => "Cancel",
                // 磁盘信息
                "disk_used" => "Used",
                "disk_free" => "Free",
                "disk_total" => "Total",
                // 磁盘告警
                "disk_alert_notice" => "Disk space low: {1} GB free ({0}%), keep an eye on it",
                "disk_alert_warning" => "Disk space warning: {1} GB free ({0}%), cleanup recommended",
                "disk_alert_critical" => "Disk critically low! {1} GB free ({0}%), cleanup now",
                "disk_alert_clean_now" => "Clean Now",
                // 扫描状态
                "scanning" => "Scanning",
                "scanning_hint" => "Scanning disk for cleanable files, please wait...",
                "press_r_to_scan" => "Click Scan to start",
                "items_found" => "found",
                "items_selected" => "selected",
                "items" => "items",
                "total" => "total",
                "all_items" => "All",
                "window_title" => "Maclean - macOS Disk Cleaner",
                "back" => "Back",
                "home" => "Home",
                "delete_selected" => "Delete Selected",
                "delete_selected_count" => "Delete Selected ({0} items)",
                "items_total_size" => "{0} items, total {1}",
                "selected_count_size" => "{0} items selected, {1}",
                "found_items_total" => "{0} found items, total {1}",
                "logs" => "Logs",
                "no_large_files" => "No files larger than 1MB in this directory",
                "rescan" => "Rescan",
                "analyzing" => "Analyzing {0}...",
                "home_dir_label" => "Home",
                "safe_clean" => "safe to clean",
                "cache_only_clean" => "cache only",
                "caution_clean" => "Needs Caution",
                "confirm_clean" => "Needs Confirm",
                // 列表
                "category" => "Category",
                "size" => "Size",
                "path" => "Path",
                "unknown" => "unknown",
                "no_items_hint" => "No items yet - click Scan to find cleanable files",
                "click_to_start" => "Click to start",
                "app_subitems_detail" => "Sub-items: {0}",
                "select_app_from_list" => "Please select an app from the left",
                // 概览
                "overview_releasable" => "Releasable Space",
                "overview_recommendation" => "Recommended Cleanup",
                // App 卸载
                "app_uninstall_subtitle" => "{0} apps, {1} releasable",
                "app_uninstall_search_placeholder" => "Search app name...",
                "app_list_title" => "App List",
                "subitem_detail_title" => "Sub-item Details",
                "current_selected" => "Current selected: {0}",
                "overview_recommendation_hint" => "Safe items from all categories, sorted by size",
                "overview_empty_title" => "Everything looks clean",
                "overview_empty_hint" => "Click the Scan button at the top right to find cleanable files",
                "scan_all" => "Scan All",
                "one_click_clean" => "Clean",
                "one_click_recommended_clean" => "Select Recommended",
                "app_list_title_with_count" => "App List ({0})",
                "selected_apps_partial" => "Selected items from {0} apps · {1}",
                "delete_with_size" => "🗑 Delete {0}",
                "all" => "All",
                "collapse_all" => "Collapse All",
                "expand_n_groups" => "Expand {0} groups",
                "collapse_n_groups" => "Collapse {0} groups",
                "badge_undeletable" => "🔒 Undeletable",
                // 过滤/搜索
                "filter" => "Filter",
                "filter_placeholder" => "Filter by path, category, or description... (/ to focus, Esc to clear)",
                "filter_results" => "{0} of {1} items matched",
                "no_match" => "No items match the filter",
                // 删除确认
                "about_to_delete" => "About to delete",
                "irreversible" => "This operation is irreversible!",
                "confirm_subtitle" => "Deleted files cannot be recovered. Please confirm.",
                "confirm_selected_items" => "Selected items",
                "confirm_releasable" => "Releasable space",
                "confirm_safe" => "Safe",
                "confirm_caution" => "Caution",
                "confirm_advanced" => "Advanced",
                "confirm_admin_required" => "Administrator privileges",
                "confirm_admin_yes" => "Required (contains root/SIP items)",
                "confirm_admin_no" => "Not required",
                "cleaning" => "Cleaning",
                "cleaning_in_progress" => "Cleaning in progress",
                "cleaning_log" => "Latest logs:",
                "progress_background_run" => "Run in background",
                "progress_authorize" => "Authorize and continue",
                "deleting_subtitle" => "Completed {0} / {1} items",
                // 系统优化
                "optimize_click_to_scan" => "Click Scan to view available optimization tasks",
                "optimize_safe_hint" => "Optimization tasks are safe and will not affect system stability",
                "optimize_dns_cache_flush" => "Flush DNS Cache",
                "optimize_quicklook_rebuild" => "Rebuild QuickLook Thumbnails",
                "optimize_launchservices_rebuild" => "Rebuild LaunchServices",
                "optimize_saved_state_cleanup" => "Clean Saved States",
                "optimize_gatekeeper_cleanup" => "Clean Gatekeeper Records",
                "optimize_memory_pressure_release" => "Release Memory Pressure",
                "optimize_spotlight_reindex" => "Rebuild Spotlight Index",
                "optimize_login_items_audit" => "Audit Login Items",
                "optimize_logs" => "📋 Optimize Logs",
                // 优化任务执行结果
                "opt_dns_success" => "✅ DNS cache flushed",
                "opt_dns_fail" => "⚠️ DNS flush requires admin privileges. Run in Terminal: sudo dscacheutil -flushcache && sudo killall -HUP mDNSResponder",
                "opt_quicklook_success" => "✅ QuickLook thumbnail cache rebuilt",
                "opt_quicklook_fail" => "⚠️ QuickLook cache rebuild failed",
                "opt_launchservices_success" => "✅ LaunchServices database rebuilt",
                "opt_launchservices_fail" => "⚠️ LaunchServices rebuild failed",
                "opt_saved_state_success" => "✅ Cleaned {0} old saved application states",
                "opt_gatekeeper_success" => "✅ Gatekeeper records cleaned",
                "opt_gatekeeper_fail" => "⚠️ Gatekeeper cleanup failed",
                "opt_gatekeeper_empty" => "✅ Gatekeeper records already empty",
                "opt_memory_success" => "✅ Inactive memory released",
                "opt_memory_fail" => "⚠️ Memory release requires admin privileges. Run in Terminal: sudo purge",
                "opt_spotlight_success" => "✅ Spotlight index rebuild started (may take a few minutes)",
                "opt_spotlight_fail" => "⚠️ Spotlight reindex requires admin privileges. Run in Terminal: sudo mdutil -E /",
                "opt_login_items_opened" => "✅ Opened System Settings > Login Items",
                "opt_login_items_fail" => "⚠️ Failed to open Login Items settings",
                "opt_unknown" => "⚠️ Unknown optimization task: {0}",
                // 权限引导
                "permission_title" => "Permission Settings",
                "permission_headline" => "Grant Full Disk Access",
                "permission_desc" => "Maclean needs Full Disk Access permission to delete developer cache files.",
                "permission_sub_desc" => "Some cache files are created by root with macOS security attributes; without this permission they cannot be deleted.",
                "permission_steps_title" => "Please follow these steps:",
                "permission_step1" => "Click the \"Open System Settings\" button below",
                "permission_step2" => "Find Maclean in the Full Disk Access list",
                "permission_step3" => "If missing, click the + button to add Maclean.app",
                "permission_step4" => "Make sure the switch next to Maclean is turned on",
                "permission_step5" => "Restart Maclean to use all deletion features",
                "permission_open_settings" => "Open System Settings",
                "permission_install_app" => "Install to Applications",
                "permission_later" => "Later",
                "permission_hint" => "Hint: Restart Maclean after authorization to use all deletion features",
                // 删除确认
                "confirm_fda_warning" => "Full Disk Access not granted",
                "confirm_fda_sub_warning" => "Some files may not be deleted; authorization is recommended",
                "confirm_grant" => "Go to Settings",
                "confirm_preview" => "Preview items to delete",
                "confirm_preview_summary" => "{} items total, cache permanently deleted, large files moved to Trash",
                // sudo 密码弹窗
                "sudo_title_setup" => "Enable Touch ID",
                "sudo_title_delete" => "Administrator Password Required",
                "sudo_desc_setup" => "Enter your administrator password once to create /etc/pam.d/sudo_local.\nAfter enabling, you can use Touch ID to authenticate future deletions.",
                "sudo_desc_delete" => "{} items need administrator privileges to continue deletion.",
                "sudo_password_hint" => "Enter administrator password...",
                "sudo_password_note" => "Password is only used for this sudo authorization and will not be saved to Keychain.",
                "sudo_confirm_setup" => "Enable",
                "sudo_confirm_delete" => "Confirm Delete",
                "sudo_cancelled_log" => "Authorization cancelled by user",
                // Touch ID 启用提示
                "touchid_setup_title" => "Enable Touch ID",
                "touchid_setup_headline" => "Use Touch ID instead of password",
                "touchid_setup_desc" => "{} items need administrator privileges to delete.",
                "touchid_setup_detail" => "Enabling will create the /etc/pam.d/sudo_local config file (Apple recommended),\nso all future admin operations can use Touch ID instead of a password.\nThis is a one-time setup and persists after macOS updates.",
                "touchid_enable" => "Enable Touch ID",
                "touchid_use_password" => "Use Password",
                // Touch ID 等待
                "touchid_wait_title" => "Waiting for Touch ID Setup",
                "touchid_wait_headline" => "Please enter your password in the system dialog",
                "touchid_wait_desc" => "A password dialog will appear. Enter your administrator password\nto create the /etc/pam.d/sudo_local configuration file.\nDeletion will continue automatically after completion.",
                "touchid_wait_time" => "Waited {} seconds (timeout 120s)",
                "touchid_wait_cancel" => "Cancel, use password instead",
                // Touch ID 删除中
                "touchid_verify_title" => "Touch ID Verification",
                "touchid_verify_headline" => "Please authenticate with Touch ID",
                "touchid_verify_desc" => "Deleting {} items that require administrator privileges...",
                "touchid_verify_hint" => "A Touch ID dialog will appear; touch the fingerprint sensor.",
                "touchid_verify_log" => "Recent logs:",
                "touchid_cancelled" => "Touch ID authorization cancelled",
                "touchid_clamshell_error" => "Screen is closed, Touch ID is unavailable, please enter password",
                "touchid_timeout_error" => "Operation timed out: Touch ID setup not detected, please retry or use password",
                // 删除中
                "deleting_sudo_phase" => "Deleting with administrator privileges...",
                // 删除完成汇总
                "summary_title" => "Cleanup Result",
                "summary_success" => "Successfully deleted {} items",
                "summary_fail" => "Failed to delete {} items",
                "summary_fail_hint" => "Some files could not be deleted due to permissions or system protection. See logs above.",
                "summary_solution_title" => "Tips: Failure reasons and solutions",
                "summary_sip_tip" => "SIP/System protection: System paths like /Library/Developer/CoreSimulator cannot be deleted even with sudo; disable SIP or use Apple official tools.",
                "summary_perm_tip" => "Permission denied: Directories like node_modules may contain root-owned files. Click \"Copy sudo command\" to run manually in Terminal.",
                "summary_solution_1" => "1. Copy sudo command to Terminal (recommended)",
                "summary_solution_2" => "2. Disable SIP: Restart → hold Cmd+R → Terminal → csrutil disable → Restart",
                "summary_solution_3" => "3. Use project tools: cd project dir && npm run clean / npx rimraf .next",
                "summary_open_settings" => "Open System Settings",
                "summary_fail_list" => "Failed list:",
                "summary_copy_paths" => "Copy Paths",
                "summary_copy_sudo" => "Copy sudo Command",
                "summary_free_space" => "Available space: {}",
                "summary_ok" => "OK",
                "finish_summary" => "Cleanup completed: {} succeeded, {} failed",
                // 后台日志
                "log_intercepted" => "Blocked: {} - {}",
                "log_skipped" => "Skipped: {} - {}",
                "log_deleted" => "Deleted [{}] {} (success {} / fail {})",
                "log_deleted_sudo" => "Deleted [{}] {} (admin privileges)",
                "log_deleted_touchid" => "Deleted [{}] {} (Touch ID)",
                "log_delete_failed" => "Delete failed: {} - {}",
                "log_snapshot_deleted" => "Deleted snapshot: {}",
                "log_runtime_deleted" => "Deleted runtime: {}",
                "log_action_trashed" => "Moved to Trash",
                "log_action_deleted" => "Deleted",
                "log_xcrun_failed" => "xcrun deletion failed, will try sudo: {}",
                "log_skip_running" => "Skipped [{}] Xcode/Simulator is running",
                "log_docker_failed" => "Docker cleanup failed: {}",
                "log_path_not_exist" => "Path does not exist: {}",
                "already_cleaned" => "already cleaned",
                "log_symlink_rejected" => "Refuse to delete symlink: {}",
                "log_need_sudo" => "{} items need administrator privileges",
                "log_sudo_phase" => "Deleting with administrator privileges...",
                "log_password_wrong" => "Administrator password incorrect, please re-enter",
                "log_exit_code" => "sudo exit code {}",
                "log_no_sim_runtimes" => "No installed simulator runtimes found",
                "log_mount_in_use" => "Simulator runtime is mounted in use, skipping",
                "log_cannot_get_mount" => "Cannot get mount point info, skipping for safety",
                "log_sim_deleted" => "Deleted {} simulator runtime(s) via xcrun simctl{}",
                "log_sim_failed_suffix" => ", {} failed",
                "log_docker_not_running" => "Docker daemon is not running, please start Docker Desktop",
                "log_xcrun_done" => "xcrun completed: {}",
                "log_touchid_verifying" => "Touch ID verifying, please authenticate...",
                "log_wait_touchid" => "Waiting for Touch ID authorization to execute xcrun...",
                "log_touchid_prepare_xcrun" => "Preparing xcrun deletion: {}",
                "log_touchid_xcrun_deleted" => "Deleted {} simulator runtime images via xcrun simctl",
                "log_touchid_runtime_deleted_path" => "Deleted simulator runtime image via xcrun simctl: {}",
                "log_sudo_execute" => "Preparing sudo deletion for {} residual items...",
                "log_sudo_done2" => "sudo deletion completed, parsing results...",
                "log_no_sudo_needed" => "No sudo needed, all completed via xcrun",
                "log_touchid_cancel" => "Touch ID cancelled",
                "log_sip_protected" => "SIP protection, cannot delete: {}",
                "log_still_exists" => "Still exists after admin deletion",
                "log_sudo_failed" => "Failed to start sudo: {} - {}",
                "log_cannot_start_sudo" => "Cannot start sudo: {} - {}",
                "log_docker_done" => "Docker cleanup completed, reclaimed space: {}",
                "log_menu_event" => "Menu bar event: {}",
                "log_quickclean_start" => "QuickClean: {} safe items selected, start deletion",
                "log_quickclean_none" => "QuickClean: no deletable safe items",
                "log_keepalive_failed" => "sudo keepalive startup failed: {}",
                "log_unknown" => "Unknown",
                "log_cancelled_auth" => "Authorization cancelled: {}",
                "log_cancel_reason" => "Authorization cancelled",
                "log_sip_reason" => "SIP protection or system restriction",
                // 关联文件标签
                "assoc_app" => "App Bundle",
                "assoc_container" => "App Container",
                "assoc_group" => "Shared Container",
                "assoc_cookie" => "Cookies",
                "assoc_webkit" => "WebKit Data",
                "assoc_script" => "App Scripts",
                "assoc_metadata" => "Metadata",
                "assoc_cache" => "Cache",
                "assoc_app_data" => "App Data",
                "assoc_prefs" => "Preferences",
                "assoc_logs" => "Logs",
                "assoc_saved_state" => "Saved State",
                "assoc_http_storage" => "HTTP Storage",
                "assoc_other" => "Other",
                _ => "",
            }
        } else {
            match key {
                // Tab 标题
                "tab_overview" => "概览",
                "tab_dev_cache" => "开发者缓存",
                "tab_large_files" => "大文件",
                "tab_app_cache" => "App缓存",
                "tab_app_data" => "App数据",
                "tab_app_uninstall" => "应用卸载",
                "tab_system_optimize" => "系统优化",
                "tab_apfs" => "APFS快照",
                "tab_settings" => "设置",
                "settings_general" => "通用",
                "settings_safety" => "安全",
                "settings_language" => "语言",
                "setting_menubar_icon" => "启动时显示菜单栏图标",
                "setting_menubar_icon_desc" => "在 macOS 菜单栏常驻磁盘用量指示器",
                "setting_keep_sudo" => "自动保持 sudo 会话",
                "setting_keep_sudo_desc" => "避免重复输入管理员密码",
                "setting_scan_cache" => "扫描结果本地缓存",
                "setting_scan_cache_desc" => "7 天内避免重复扫描，加速启动",
                "setting_confirm_advanced" => "删除前二次确认",
                "setting_confirm_advanced_desc" => "Advanced 项目必须手动确认",
                "setting_prevent_lid_close" => "合盖时禁止删除",
                "setting_prevent_lid_close_desc" => "检测 MacBook 合盖状态，防止误触",
                "setting_language" => "界面语言",
                "setting_language_current" => "当前：简体中文",
                "switch_to_english" => "切换 English",
                "switch_to_chinese" => "切换 中文",
                // 按钮
                "scan" => "扫描",
                "delete" => "删除",
                "select_all" => "全选",
                "deselect_all" => "取消全选",
                "select_safe" => "一键选择清理",
                "expand_all" => "展开全部",
                "confirm_delete" => "确认删除",
                "cancel" => "取消",
                // 磁盘信息
                "disk_used" => "已用",
                "disk_free" => "可用",
                "disk_total" => "总量",
                // 磁盘告警
                "disk_alert_notice" => "磁盘空间注意：剩余 {1} GB ({0}%)，建议关注",
                "disk_alert_warning" => "磁盘空间警告：剩余 {1} GB ({0}%)，建议立即清理",
                "disk_alert_critical" => "磁盘空间严重不足！剩余 {1} GB ({0}%)，请立即清理",
                "disk_alert_clean_now" => "立即清理",
                // 扫描状态
                "scanning" => "扫描中",
                "scanning_hint" => "正在扫描磁盘上的可清理文件，请稍候...",
                "press_r_to_scan" => "点击「扫描」开始",
                "items_found" => "找到",
                "items_selected" => "已选",
                "items" => "个子项",
                "total" => "共计",
                "all_items" => "全部",
                "window_title" => "Maclean - macOS 磁盘清理",
                "back" => "返回",
                "home" => "主目录",
                "delete_selected" => "删除选中",
                "delete_selected_count" => "删除选中 ({0}项)",
                "items_total_size" => "{0} 项, 总计 {1}",
                "selected_count_size" => "已选 {0} 项, {1}",
                "found_items_total" => "{0} 找到项, 共计 {1}",
                "logs" => "日志",
                "safe_clean" => "可安全清理",
                "cache_only_clean" => "仅缓存可清",
                "caution_clean" => "需谨慎处理",
                "confirm_clean" => "需确认删除",
                // 列表
                "category" => "类别",
                "size" => "大小",
                "path" => "路径",
                "unknown" => "未知",
                "no_items_hint" => "暂无数据 - 点击「扫描」查找可清理文件",
                "click_to_start" => "点击开始",
                "app_subitems_detail" => "子项详情：{0}",
                "select_app_from_list" => "请从左侧选择一个应用",
                // 概览
                "overview_releasable" => "可释放空间",
                "overview_recommendation" => "推荐清理",
                // App 卸载
                "app_uninstall_subtitle" => "{0} 个应用 · 共 {1} 可释放",
                "app_uninstall_search_placeholder" => "搜索应用名称...",
                "app_list_title" => "应用列表",
                "subitem_detail_title" => "子项详情",
                "current_selected" => "当前选中：{0}",
                "overview_recommendation_hint" => "聚合所有分类中的安全项，按大小排序",
                "overview_empty_title" => "看起来一切整洁",
                "overview_empty_hint" => "点击右上角「扫描」查找可清理文件",
                "scan_all" => "扫描全部",
                "one_click_clean" => "一键清理",
                "one_click_recommended_clean" => "一键选推荐清理",
                "app_list_title_with_count" => "应用列表（{0}）",
                "selected_apps_partial" => "已选中 {0} 个应用的部分项目 · 可释放 {1}",
                "delete_with_size" => "🗑 删除 {0}",
                "all" => "全部",
                "collapse_all" => "收起全部",
                "expand_n_groups" => "展开 {0} 个应用分组",
                "collapse_n_groups" => "收起 {0} 个应用分组",
                "badge_undeletable" => "🔒 不可删除",
                // 过滤/搜索
                "filter" => "过滤",
                "filter_placeholder" => "按路径、类别或描述过滤...（/ 聚焦，Esc 清除）",
                "filter_results" => "匹配 {0} / {1} 项",
                "no_match" => "没有匹配过滤条件的项",
                // 删除确认
                "about_to_delete" => "即将删除",
                "irreversible" => "此操作不可逆！",
                "confirm_subtitle" => "删除后文件将不可恢复，请确认。",
                "confirm_selected_items" => "选中项目",
                "confirm_releasable" => "预计释放",
                "confirm_safe" => "Safe",
                "confirm_caution" => "Caution",
                "confirm_advanced" => "Advanced",
                "confirm_admin_required" => "管理员权限",
                "confirm_admin_yes" => "需要（含 root/SIP 项目）",
                "confirm_admin_no" => "不需要",
                "cleaning" => "清理中",
                "cleaning_in_progress" => "正在执行清理",
                "cleaning_log" => "最新日志：",
                "progress_background_run" => "后台运行",
                "progress_authorize" => "授权并继续",
                "deleting_subtitle" => "已完成 {0} / {1} 项",
                // 系统优化
                "optimize_click_to_scan" => "点击扫描查看可用的优化任务",
                "optimize_safe_hint" => "优化任务安全可执行，不会影响系统稳定性",
                "optimize_dns_cache_flush" => "DNS 缓存刷新",
                "optimize_quicklook_rebuild" => "QuickLook 缩略图重建",
                "optimize_launchservices_rebuild" => "LaunchServices 重建",
                "optimize_saved_state_cleanup" => "Saved State 清理",
                "optimize_gatekeeper_cleanup" => "Gatekeeper 下载清理",
                "optimize_memory_pressure_release" => "内存压力释放",
                "optimize_spotlight_reindex" => "Spotlight 索引重建",
                "optimize_login_items_audit" => "登录项审计",
                "optimize_logs" => "📋 优化日志",
                // 优化任务执行结果
                "opt_dns_success" => "✅ DNS 缓存已刷新",
                "opt_dns_fail" => "⚠️ DNS 缓存刷新需要管理员权限。请在终端执行：sudo dscacheutil -flushcache && sudo killall -HUP mDNSResponder",
                "opt_quicklook_success" => "✅ QuickLook 缩略图缓存已重建",
                "opt_quicklook_fail" => "⚠️ QuickLook 缓存重建失败",
                "opt_launchservices_success" => "✅ LaunchServices 数据库已重建",
                "opt_launchservices_fail" => "⚠️ LaunchServices 重建失败",
                "opt_saved_state_success" => "✅ 已清理 {0} 个旧的应用保存状态",
                "opt_gatekeeper_success" => "✅ Gatekeeper 下载记录已清理",
                "opt_gatekeeper_fail" => "⚠️ Gatekeeper 清理失败",
                "opt_gatekeeper_empty" => "✅ Gatekeeper 下载记录已为空",
                "opt_memory_success" => "✅ 已释放非活跃内存",
                "opt_memory_fail" => "⚠️ 内存释放需要管理员权限。请在终端执行：sudo purge",
                "opt_spotlight_success" => "✅ Spotlight 索引重建已启动（可能需要几分钟）",
                "opt_spotlight_fail" => "⚠️ Spotlight 重建需要管理员权限。请在终端执行：sudo mdutil -E /",
                "opt_login_items_opened" => "✅ 已打开系统设置 > 登录项",
                "opt_login_items_fail" => "⚠️ 无法打开登录项设置",
                "opt_unknown" => "⚠️ 未知的优化任务: {0}",
                // 权限引导
                "permission_title" => "权限设置",
                "permission_headline" => "授权完全磁盘访问",
                "permission_desc" => "Maclean 需要完全磁盘访问权限才能删除开发者缓存文件。",
                "permission_sub_desc" => "部分缓存文件由 root 创建且带有 macOS 安全属性，没有此权限将无法删除。",
                "permission_steps_title" => "请按以下步骤操作：",
                "permission_step1" => "点击下方「打开系统设置」按钮",
                "permission_step2" => "在「完全磁盘访问」列表中找到 Maclean",
                "permission_step3" => "如果没有，点击 + 号添加 Maclean.app",
                "permission_step4" => "确保 Maclean 旁边的开关已打开",
                "permission_step5" => "重启 Maclean 后即可正常删除",
                "permission_open_settings" => "打开系统设置",
                "permission_install_app" => "安装到应用程序",
                "permission_later" => "稍后再说",
                "permission_hint" => "提示: 授权后重启 Maclean 即可正常使用所有删除功能",
                // 删除确认
                "confirm_fda_warning" => "未授予完全磁盘访问权限",
                "confirm_fda_sub_warning" => "部分文件可能无法删除，建议先授权",
                "confirm_grant" => "去授权",
                "confirm_preview" => "预览删除项",
                "confirm_preview_summary" => "共 {} 项, 缓存类永久删除, 大文件移至废纸篓",
                // sudo 密码弹窗
                "sudo_title_setup" => "启用 Touch ID",
                "sudo_title_delete" => "需要管理员权限",
                "sudo_desc_setup" => "首次启用 Touch ID 需要输入一次管理员密码，以创建 /etc/pam.d/sudo_local。\n启用后，后续删除操作可使用 Touch ID 验证。",
                "sudo_desc_delete" => "{} 项文件因权限不足需要输入管理员密码继续删除。",
                "sudo_password_hint" => "请输入管理员密码...",
                "sudo_password_note" => "密码仅用于本次 sudo 授权，不会保存到钥匙串。",
                "sudo_confirm_setup" => "确认启用",
                "sudo_confirm_delete" => "确认删除",
                "sudo_cancelled_log" => "用户取消密码授权",
                // Touch ID 启用提示
                "touchid_setup_title" => "启用 Touch ID",
                "touchid_setup_headline" => "使用 Touch ID 代替密码",
                "touchid_setup_desc" => "{} 项文件需要管理员权限删除。",
                "touchid_setup_detail" => "启用后会创建 /etc/pam.d/sudo_local 配置文件（macOS 官方推荐方式），\n之后所有管理员操作都可以用 Touch ID 验证，无需输入密码。\n这是一次性操作，系统更新后依然有效。",
                "touchid_enable" => "启用 Touch ID",
                "touchid_use_password" => "用密码代替",
                // Touch ID 等待
                "touchid_wait_title" => "等待 Touch ID 启用",
                "touchid_wait_headline" => "请在系统弹窗中输入密码",
                "touchid_wait_desc" => "系统会弹出密码对话框，请输入管理员密码\n以创建 /etc/pam.d/sudo_local 配置文件。\n完成后会自动继续删除操作。",
                "touchid_wait_time" => "已等待 {} 秒（超时 120 秒）",
                "touchid_wait_cancel" => "取消，用密码代替",
                // Touch ID 删除中
                "touchid_verify_title" => "Touch ID 验证",
                "touchid_verify_headline" => "请在 Touch ID 传感器上验证指纹",
                "touchid_verify_desc" => "正在删除 {} 项需要管理员权限的文件...",
                "touchid_verify_hint" => "系统会弹出 Touch ID 对话框，请触碰指纹传感器",
                "touchid_verify_log" => "最近日志:",
                "touchid_cancelled" => "已取消 Touch ID 授权",
                "touchid_clamshell_error" => "屏幕已合上，Touch ID 不可用，请输入密码",
                "touchid_timeout_error" => "操作超时：未检测到 Touch ID 启用，请重试或使用密码",
                // 删除中
                "deleting_sudo_phase" => "正在使用管理员权限删除...",
                // 删除完成汇总
                "summary_title" => "清理结果",
                "summary_success" => "成功删除 {} 项",
                "summary_fail" => "删除失败 {} 项",
                "summary_fail_hint" => "部分文件因权限或系统保护无法删除，详见上方日志。",
                "summary_solution_title" => "提示: 失败原因及解决方案",
                "summary_sip_tip" => "SIP/系统保护: /Library/Developer/CoreSimulator 等系统路径即使 sudo 也无法删除，需关闭 SIP 或使用 Apple 官方工具。",
                "summary_perm_tip" => "权限不足: node_modules 等目录内部可能存在 root 拥有的文件，可点击「复制 sudo 命令」在终端手动执行。",
                "summary_solution_1" => "1. 复制 sudo 命令到终端执行（推荐）",
                "summary_solution_2" => "2. 关闭 SIP: 重启→按住 Cmd+R→终端→csrutil disable→重启",
                "summary_solution_3" => "3. 用项目工具删除: cd 项目目录 && npm run clean / npx rimraf .next",
                "summary_open_settings" => "打开系统设置",
                "summary_fail_list" => "失败列表:",
                "summary_copy_paths" => "复制路径",
                "summary_copy_sudo" => "复制 sudo 命令",
                "summary_free_space" => "当前可用空间: {}",
                "summary_ok" => "确定",
                "finish_summary" => "清理完成: 成功 {} 项, 失败 {} 项",
                // 后台日志
                "log_intercepted" => "已拦截: {} - {}",
                "log_skipped" => "已跳过: {} - {}",
                "log_deleted" => "已删除 [{}] {} (成功 {} / 失败 {})",
                "log_deleted_sudo" => "已删除 [{}] {} (管理员权限)",
                "log_deleted_touchid" => "已删除 [{}] {} (Touch ID)",
                "log_delete_failed" => "删除失败: {} - {}",
                "log_snapshot_deleted" => "已删除快照: {}",
                "log_runtime_deleted" => "已删除运行时: {}",
                "log_action_trashed" => "已移至废纸篓",
                "log_action_deleted" => "已删除",
                "log_xcrun_failed" => "xcrun 删除失败，将尝试 sudo: {}",
                "log_skip_running" => "跳过 [{}] Xcode/Simulator 正在运行",
                "log_docker_failed" => "Docker 清理失败: {}",
                "log_path_not_exist" => "路径不存在: {}",
                "already_cleaned" => "已清理",
                "log_symlink_rejected" => "拒绝删除符号链接: {}",
                "log_need_sudo" => "{} 项需要管理员权限",
                "log_sudo_phase" => "正在使用管理员权限删除...",
                "log_password_wrong" => "管理员密码错误，请重新输入",
                "log_exit_code" => "sudo 退出码 {}",
                "log_no_sim_runtimes" => "没有找到已安装的模拟器运行时",
                "log_mount_in_use" => "模拟器运行时正在被挂载使用，跳过删除",
                "log_cannot_get_mount" => "无法获取挂载点信息，为安全起见跳过删除",
                "log_sim_deleted" => "已通过 xcrun simctl 删除 {} 个模拟器运行时{}",
                "log_sim_failed_suffix" => "，{} 个失败",
                "log_docker_not_running" => "Docker daemon 未运行，请先启动 Docker Desktop",
                "log_xcrun_done" => "xcrun 执行完成: {}",
                "log_touchid_verifying" => "Touch ID 验证中，请在传感器上验证指纹...",
                "log_wait_touchid" => "等待 Touch ID 授权执行 xcrun...",
                "log_touchid_prepare_xcrun" => "准备通过 xcrun 删除: {}",
                "log_touchid_xcrun_deleted" => "已通过 xcrun simctl 删除 {} 个模拟器运行时镜像",
                "log_touchid_runtime_deleted_path" => "已通过 xcrun simctl 删除模拟器运行时镜像: {}",
                "log_sudo_execute" => "准备 sudo 删除 {} 项残留文件...",
                "log_sudo_done2" => "sudo 删除执行完成，正在解析结果...",
                "log_no_sudo_needed" => "无需 sudo 删除，全部通过 xcrun 完成",
                "log_touchid_cancel" => "Touch ID 取消",
                "log_sip_protected" => "SIP保护无法删除: {}",
                "log_still_exists" => "管理员权限删除后仍存在",
                "log_sudo_failed" => "无法启动 sudo: {} - {}",
                "log_cannot_start_sudo" => "无法启动 sudo: {} - {}",
                "log_docker_done" => "Docker 清理完成，释放空间: {}",
                "log_menu_event" => "菜单栏事件: {}",
                "log_quickclean_start" => "一键清理：自动选择 {} 个安全项，开始删除",
                "log_quickclean_none" => "一键清理：没有可删除的安全项",
                "log_keepalive_failed" => "sudo keepalive 启动失败: {}",
                "log_unknown" => "未知",
                "log_cancelled_auth" => "已取消授权: {}",
                "log_cancel_reason" => "用户取消授权",
                "log_sip_reason" => "SIP保护或系统限制",
                // 关联文件标签
                "assoc_app" => "应用本体",
                "assoc_container" => "应用容器",
                "assoc_group" => "共享容器",
                "assoc_cookie" => "Cookie",
                "assoc_webkit" => "WebKit数据",
                "assoc_script" => "应用脚本",
                "assoc_metadata" => "元数据",
                "assoc_cache" => "缓存",
                "assoc_app_data" => "应用数据",
                "assoc_prefs" => "偏好设置",
                "assoc_logs" => "日志",
                "assoc_saved_state" => "窗口状态",
                "assoc_http_storage" => "网络存储",
                "assoc_other" => "其他",
                _ => "",
            }
        }
    }

    /// 格式化多语言文本（支持 {0}, {1}, {2} 占位符）
    ///
    /// 用于需要动态参数的 UI 文本，例如 "{} 项, 总计 {}" 在英文中应为
    /// "{} items, total {}"。占位符按顺序替换为 args 中的值。
    pub fn tf(&self, key: &str, args: &[&str]) -> String {
        Self::tf_lang(self.lang_en, key, args)
    }

    /// 静态版本，供后台线程等无法访问 `&App` 的地方使用
    pub fn tf_lang(lang_en: bool, key: &str, args: &[&str]) -> String {
        let mut s = Self::t_lang(lang_en, key).to_string();
        for (i, arg) in args.iter().enumerate() {
            s = s.replace(&format!("{{{}}}", i), arg);
        }
        s
    }
}

/// 获取磁盘信息（总量、可用）
///
/// 执行 `df -k /` 命令，解析输出获取磁盘总量和可用空间（字节）。
fn get_disk_info() -> (u64, u64) {
    let output = std::process::Command::new("df").arg("-k").arg("/").output();

    if let Ok(output) = output {
        let stdout = String::from_utf8_lossy(&output.stdout);
        for line in stdout.lines().skip(1) {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() >= 4 {
                if let (Ok(total_kb), Ok(free_kb)) =
                    (parts[1].parse::<u64>(), parts[3].parse::<u64>())
                {
                    return (total_kb * 1024, free_kb * 1024);
                }
            }
        }
    }

    (0, 0)
}

/// 根据路径分类关联文件的标签
///
/// 将 ~/Library 下的路径映射为用户友好的标签：
/// - Containers → 应用容器
/// - Caches → 缓存
/// - Application Support → 应用数据
/// - Preferences → 偏好设置
/// - Logs → 日志
/// - .app → 应用本体
fn classify_associated_path(path: &str, lang_en: bool) -> String {
    let key = if path.ends_with(".app") {
        "assoc_app"
    } else if path.contains("/Containers/") {
        "assoc_container"
    } else if path.contains("/Group Containers/") {
        "assoc_group"
    } else if path.contains("/Cookies/") {
        "assoc_cookie"
    } else if path.contains("/WebKit/") {
        "assoc_webkit"
    } else if path.contains("/Application Scripts/") {
        "assoc_script"
    } else if path.contains("/Metadata/") {
        "assoc_metadata"
    } else if path.contains("/Caches/") {
        "assoc_cache"
    } else if path.contains("/Application Support/") {
        "assoc_app_data"
    } else if path.contains("/Preferences/") {
        "assoc_prefs"
    } else if path.contains("/Logs/") {
        "assoc_logs"
    } else if path.contains("/Saved Application State/") {
        "assoc_saved_state"
    } else if path.contains("/HTTPStorages/") {
        "assoc_http_storage"
    } else {
        "assoc_other"
    };
    App::t_lang(lang_en, key).to_string()
}
