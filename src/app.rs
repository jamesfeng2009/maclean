//! App 状态管理
//!
//! 管理 egui GUI 应用的全部状态，包括当前 Tab、扫描结果、选中状态等。

use std::path::Path;

use crate::scanner::{self, ScanItem, Scanner};
use crate::safety;

/// Tab 类型
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
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
}

impl Tab {
    /// 获取 Tab 的标题（直接返回中文 &'static str）
    pub fn title(self) -> &'static str {
        match self {
            Tab::DevCache => "开发者缓存",
            Tab::LargeFiles => "大文件",
            Tab::AppCache => "App缓存",
            Tab::AppData => "App数据",
            Tab::AppUninstall => "App卸载",
            Tab::SystemOptimize => "系统优化",
            Tab::Apfs => "APFS快照",
        }
    }

    /// 所有 Tab
    pub fn all() -> [Tab; 7] {
        [Tab::DevCache, Tab::LargeFiles, Tab::AppCache, Tab::AppData, Tab::AppUninstall, Tab::SystemOptimize, Tab::Apfs]
    }

    /// 下一个 Tab
    pub fn next(self) -> Self {
        match self {
            Tab::DevCache => Tab::LargeFiles,
            Tab::LargeFiles => Tab::AppCache,
            Tab::AppCache => Tab::AppData,
            Tab::AppData => Tab::AppUninstall,
            Tab::AppUninstall => Tab::SystemOptimize,
            Tab::SystemOptimize => Tab::Apfs,
            Tab::Apfs => Tab::DevCache,
        }
    }

    /// 上一个 Tab
    pub fn prev(self) -> Self {
        match self {
            Tab::DevCache => Tab::Apfs,
            Tab::LargeFiles => Tab::DevCache,
            Tab::AppCache => Tab::LargeFiles,
            Tab::AppData => Tab::AppCache,
            Tab::AppUninstall => Tab::AppData,
            Tab::SystemOptimize => Tab::AppUninstall,
            Tab::Apfs => Tab::SystemOptimize,
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
    pub results: [Vec<ScanItem>; 7],
    /// 每个 Tab 的扫描状态
    pub scan_states: [ScanState; 7],
    /// 列表选中索引
    pub list_index: usize,
    /// 磁盘总空间（字节）
    pub disk_total: u64,
    /// 磁盘可用空间（字节）
    pub disk_free: u64,
    /// 清理日志
    pub logs: Vec<String>,
    /// 确认状态
    pub confirm: ConfirmState,
    /// 待删除的项索引列表
    pub pending_delete: Vec<usize>,
    /// 是否应该退出
    pub should_quit: bool,
    /// 扫描耗时（毫秒）
    pub scan_time_ms: [u64; 7],
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
    /// Touch ID 设置/执行中的错误提示
    pub touch_id_error: Option<String>,
    /// Touch ID 启用等待开始时间（用于超时检测）
    pub touch_id_wait_start: Option<std::time::Instant>,
}

impl App {
    /// 创建新 App 状态
    pub fn new() -> Self {
        let (disk_total, disk_free) = get_disk_info();

        Self {
            tab: Tab::DevCache,
            results: [Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new()],
            scan_states: [
                ScanState::Idle,
                ScanState::Idle,
                ScanState::Idle,
                ScanState::Idle,
                ScanState::Idle,
                ScanState::Idle,
                ScanState::Idle,
            ],
            list_index: 0,
            disk_total,
            disk_free,
            logs: Vec::new(),
            confirm: ConfirmState::None,
            pending_delete: Vec::new(),
            should_quit: false,
            scan_time_ms: [0; 7],
            lang_en: false, // 默认中文
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
            touch_id_available: crate::touchid::touch_id_available(),
            touch_id_enabled: crate::touchid::sudo_touch_id_enabled(),
            touch_id_error: None,
            touch_id_wait_start: None,
        }
    }

    /// 获取当前 Tab 索引
    pub fn tab_index(&self) -> usize {
        match self.tab {
            Tab::DevCache => 0,
            Tab::LargeFiles => 1,
            Tab::AppCache => 2,
            Tab::AppData => 3,
            Tab::AppUninstall => 4,
            Tab::SystemOptimize => 5,
            Tab::Apfs => 6,
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

    /// 智能选择：只选中推荐清理（Safe）的项
    pub fn select_safe_only(&mut self) {
        let idx = self.tab_index();
        for item in &mut self.results[idx] {
            item.selected = item.deletable && item.recommend == crate::scanner::Recommend::Safe;
        }
    }

    /// 取消全选
    pub fn deselect_all(&mut self) {
        let idx = self.tab_index();
        for item in &mut self.results[idx] {
            item.selected = false;
        }
    }

    /// 统计当前 Tab 中推荐清理（Safe）的项数
    pub fn safe_count(&self) -> usize {
        self.current_items()
            .iter()
            .filter(|i| i.deletable && i.recommend == crate::scanner::Recommend::Safe)
            .count()
    }

    /// 统计当前 Tab 中推荐清理（Safe）的总大小
    pub fn safe_size(&self) -> u64 {
        self.current_items()
            .iter()
            .filter(|i| i.deletable && i.recommend == crate::scanner::Recommend::Safe)
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
            Tab::DevCache => scanner::dev_cache::DevCacheScanner::new().scan(),
            Tab::LargeFiles => scanner::large_files::LargeFileScanner::new().scan(),
            Tab::AppCache => scanner::app_cache::AppCacheScanner::new().scan(),
            Tab::AppData => scanner::app_data::AppDataScanner::new().scan(),
            Tab::AppUninstall => scanner::uninstall::UninstallScanner::new().scan(),
            Tab::SystemOptimize => scanner::optimize::OptimizeScanner::new().scan(),
            Tab::Apfs => scanner::apfs::ApfsScanner::new().scan(),
        };

        self.scan_time_ms[idx] = result.scan_time_ms;
        self.results[idx] = result.items;
        self.scan_states[idx] = ScanState::Done;
        self.list_index = 0;

        // 刷新磁盘信息
        let (total, free) = get_disk_info();
        self.disk_total = total;
        self.disk_free = free;
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

    /// 准备删除选中的项（进入确认状态）
    pub fn prepare_delete(&mut self) {
        let selected: Vec<usize> = self
            .current_items()
            .iter()
            .enumerate()
            .filter(|(_, item)| item.selected)
            .map(|(i, _)| i)
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
        let idx = self.tab_index();

        // 收集要删除的路径和类别（跳过不可删除的项）
        // 策略：Safe 级别永久删除（缓存自动重建），Caution/Advanced 移至废纸篓（可恢复）
        let to_delete: Vec<(String, String, Vec<String>, bool)> = self
            .pending_delete
            .iter()
            .rev()
            .filter(|&&i| self.results[idx][i].deletable)
            .map(|&i| {
                let item = &self.results[idx][i];
                let use_trash = match item.recommend {
                    crate::scanner::Recommend::Safe => false,      // 缓存类：永久删除
                    crate::scanner::Recommend::Caution => false,   // 系统缓存：永久删除（root 属主无法移到用户废纸篓）
                    crate::scanner::Recommend::Advanced => true,   // 大文件/高级项：移至废纸篓
                };
                (item.path.clone(), item.category.clone(), item.batch_paths.clone(), use_trash)
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
    pub fn receive_delete_log(&mut self, log: String, path: String, category: String, success: bool) {
        if success {
            self.deleted_paths.push(path);
        } else {
            self.failed_paths.push((path, category));
        }
        self.logs.push(log);
        self.delete_done += 1;
    }

    /// 删除完成后的收尾工作
    pub fn finish_delete(&mut self) {
        let idx = self.tab_index();

        // 统计成功/失败
        let success_count = self.deleted_paths.len();
        let failed_count = self.failed_paths.len();

        // 只移除成功删除的项（通过路径匹配），保留失败的项让用户看到
        let deleted = self.deleted_paths.clone();
        self.results[idx].retain(|item| !deleted.contains(&item.path));

        // 生成汇总
        self.delete_summary = Some((success_count, failed_count, 0));

        self.logs.push(format!(
            "✅ 清理完成: 成功 {} 项, 失败 {} 项",
            success_count, failed_count
        ));

        // 刷新磁盘信息
        let (total, free) = get_disk_info();
        self.disk_total = total;
        self.disk_free = free;

        // 取消所有选中状态（失败的项保留在列表但取消选中）
        for item in &mut self.results[idx] {
            item.selected = false;
        }

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
        if self.lang_en {
            match key {
                // Tab 标题
                "tab_dev_cache" => "Dev Cache",
                "tab_large_files" => "Large Files",
                "tab_app_cache" => "App Cache",
                "tab_app_data" => "App Data",
                "tab_app_uninstall" => "Uninstall",
                "tab_system_optimize" => "Optimize",
                "tab_apfs" => "APFS Snapshots",
                // 按钮
                "scan" => "Scan",
                "delete" => "Delete",
                "select_all" => "Select All",
                "deselect_all" => "Deselect All",
                "select_safe" => "Select Safe Only",
                "confirm_delete" => "Confirm Delete",
                "cancel" => "Cancel",
                // 磁盘信息
                "disk_used" => "Used",
                "disk_free" => "Free",
                "disk_total" => "Total",
                // 扫描状态
                "scanning" => "Scanning",
                "scanning_hint" => "Scanning disk for cleanable files, please wait...",
                "press_r_to_scan" => "Click Scan to start",
                "items_found" => "found",
                "items_selected" => "selected",
                "items" => "items",
                "total" => "total",
                "safe_clean" => "safe to clean",
                "caution_clean" => "need caution",
                "confirm_clean" => "need confirm",
                // 列表
                "category" => "Category",
                "size" => "Size",
                "path" => "Path",
                "unknown" => "unknown",
                "no_items_hint" => "No items yet - click Scan to find cleanable files",
                "click_to_start" => "Click to start",
                // 删除确认
                "about_to_delete" => "About to delete",
                "irreversible" => "This operation is irreversible!",
                "cleaning" => "Cleaning",
                "cleaning_in_progress" => "Cleaning in progress",
                "cleaning_log" => "Latest logs:",
                _ => "",
            }
        } else {
            match key {
                // Tab 标题
                "tab_dev_cache" => "开发者缓存",
                "tab_large_files" => "大文件",
                "tab_app_cache" => "App缓存",
                "tab_app_data" => "App数据",
                "tab_app_uninstall" => "App卸载",
                "tab_system_optimize" => "系统优化",
                "tab_apfs" => "APFS快照",
                // 按钮
                "scan" => "扫描",
                "delete" => "删除",
                "select_all" => "全选",
                "deselect_all" => "取消全选",
                "select_safe" => "一键选推荐清理",
                "confirm_delete" => "确认删除",
                "cancel" => "取消",
                // 磁盘信息
                "disk_used" => "已用",
                "disk_free" => "可用",
                "disk_total" => "总量",
                // 扫描状态
                "scanning" => "扫描中",
                "scanning_hint" => "正在扫描磁盘上的可清理文件，请稍候...",
                "press_r_to_scan" => "点击「扫描」开始",
                "items_found" => "找到",
                "items_selected" => "已选",
                "items" => "项",
                "total" => "共计",
                "safe_clean" => "可安全清理",
                "caution_clean" => "需谨慎确认",
                "confirm_clean" => "需确认",
                // 列表
                "category" => "类别",
                "size" => "大小",
                "path" => "路径",
                "unknown" => "未知",
                "no_items_hint" => "暂无数据 - 点击「扫描」查找可清理文件",
                "click_to_start" => "点击开始",
                // 删除确认
                "about_to_delete" => "即将删除",
                "irreversible" => "此操作不可逆！",
                "cleaning" => "清理中",
                "cleaning_in_progress" => "正在执行清理",
                "cleaning_log" => "最新日志：",
                _ => "",
            }
        }
    }
}

/// 获取磁盘信息（总量、可用）
///
/// 执行 `df -k /` 命令，解析输出获取磁盘总量和可用空间（字节）。
fn get_disk_info() -> (u64, u64) {
    let output = std::process::Command::new("df")
        .arg("-k")
        .arg("/")
        .output();

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
