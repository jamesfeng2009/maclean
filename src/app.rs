//! App 状态管理
//!
//! 管理 TUI 应用的全部状态，包括当前 Tab、扫描结果、选中状态等。

use std::path::Path;

use crate::scanner::{self, ScanItem, Scanner};
use crate::safety;
use rust_i18n::t;

/// Tab 类型
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    /// 开发者缓存
    DevCache,
    /// 大文件
    LargeFiles,
    /// App 缓存
    AppCache,
    /// APFS 快照
    Apfs,
}

impl Tab {
    /// 获取 Tab 的标题（国际化）
    pub fn title(self) -> String {
        match self {
            Tab::DevCache => t!("tab_dev_cache").to_string(),
            Tab::LargeFiles => t!("tab_large_files").to_string(),
            Tab::AppCache => t!("tab_app_cache").to_string(),
            Tab::Apfs => t!("tab_apfs").to_string(),
        }
    }

    /// 所有 Tab
    pub fn all() -> [Tab; 4] {
        [Tab::DevCache, Tab::LargeFiles, Tab::AppCache, Tab::Apfs]
    }

    /// 下一个 Tab
    pub fn next(self) -> Self {
        match self {
            Tab::DevCache => Tab::LargeFiles,
            Tab::LargeFiles => Tab::AppCache,
            Tab::AppCache => Tab::Apfs,
            Tab::Apfs => Tab::DevCache,
        }
    }

    /// 上一个 Tab
    pub fn prev(self) -> Self {
        match self {
            Tab::DevCache => Tab::Apfs,
            Tab::LargeFiles => Tab::DevCache,
            Tab::AppCache => Tab::LargeFiles,
            Tab::Apfs => Tab::AppCache,
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
}

/// App 主状态
pub struct App {
    /// 当前 Tab
    pub tab: Tab,
    /// 每个 Tab 的扫描结果
    pub results: [Vec<ScanItem>; 4],
    /// 每个 Tab 的扫描状态
    pub scan_states: [ScanState; 4],
    /// 列表选中索引
    pub list_index: usize,
    /// 列表滚动偏移
    pub scroll_offset: usize,
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
    pub scan_time_ms: [u64; 4],
}

impl App {
    /// 创建新 App 状态
    pub fn new() -> Self {
        let (disk_total, disk_free) = get_disk_info();

        Self {
            tab: Tab::DevCache,
            results: [Vec::new(), Vec::new(), Vec::new(), Vec::new()],
            scan_states: [
                ScanState::Idle,
                ScanState::Idle,
                ScanState::Idle,
                ScanState::Idle,
            ],
            list_index: 0,
            scroll_offset: 0,
            disk_total,
            disk_free,
            logs: Vec::new(),
            confirm: ConfirmState::None,
            pending_delete: Vec::new(),
            should_quit: false,
            scan_time_ms: [0; 4],
        }
    }

    /// 获取当前 Tab 索引
    fn tab_index(&self) -> usize {
        match self.tab {
            Tab::DevCache => 0,
            Tab::LargeFiles => 1,
            Tab::AppCache => 2,
            Tab::Apfs => 3,
        }
    }

    /// 切换到下一个 Tab
    pub fn next_tab(&mut self) {
        if !matches!(self.confirm, ConfirmState::None) {
            return;
        }
        self.tab = self.tab.next();
        self.list_index = 0;
        self.scroll_offset = 0;
    }

    /// 切换到上一个 Tab
    pub fn prev_tab(&mut self) {
        if !matches!(self.confirm, ConfirmState::None) {
            return;
        }
        self.tab = self.tab.prev();
        self.list_index = 0;
        self.scroll_offset = 0;
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
            self.adjust_scroll();
        }
    }

    /// 列表向下移动
    pub fn move_down(&mut self) {
        let len = self.current_items().len();
        if len > 0 && self.list_index < len - 1 {
            self.list_index += 1;
            self.adjust_scroll();
        }
    }

    /// 调整滚动偏移
    fn adjust_scroll(&mut self) {
        // 每页显示 15 行
        let page_size = 15;
        if self.list_index < self.scroll_offset {
            self.scroll_offset = self.list_index;
        } else if self.list_index >= self.scroll_offset + page_size {
            self.scroll_offset = self.list_index - page_size + 1;
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

    /// 取消全选
    pub fn deselect_all(&mut self) {
        let idx = self.tab_index();
        for item in &mut self.results[idx] {
            item.selected = false;
        }
    }

    /// 执行扫描（当前 Tab）
    pub fn scan_current(&mut self) {
        let idx = self.tab_index();
        self.scan_states[idx] = ScanState::Scanning;

        let result = match self.tab {
            Tab::DevCache => scanner::dev_cache::DevCacheScanner::new().scan(),
            Tab::LargeFiles => scanner::large_files::LargeFileScanner::new().scan(),
            Tab::AppCache => scanner::app_cache::AppCacheScanner::new().scan(),
            Tab::Apfs => scanner::apfs::ApfsScanner::new().scan(),
        };

        self.scan_time_ms[idx] = result.scan_time_ms;
        self.results[idx] = result.items;
        self.scan_states[idx] = ScanState::Done;
        self.list_index = 0;
        self.scroll_offset = 0;

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
        let selected: Vec<usize> = self.current_items()
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

    /// 确认删除
    pub fn confirm_delete(&mut self) {
        self.confirm = ConfirmState::Deleting;
        let tab = self.tab;
        let idx = self.tab_index();

        let mut deleted_count = 0u64;
        let mut failed_count = 0u64;
        let mut blocked_count = 0u64;
        let mut freed_bytes = 0u64;

        // 收集要删除的路径（倒序删除，避免索引变化问题）
        let to_delete: Vec<(usize, String, String)> = self.pending_delete
            .iter()
            .rev()
            .map(|&i| {
                let item = &self.results[idx][i];
                (i, item.path.clone(), item.category.clone())
            })
            .collect();

        for (i, path, category) in &to_delete {
            // ================================================================
            //  安全校验: 删除前对每个路径做最终安全检查
            // ================================================================
            match safety::check_path_safety(path) {
                safety::SafetyCheck::Danger(reason) => {
                    blocked_count += 1;
                    self.logs.push(t!("log_blocked", path = path, reason = reason).to_string());
                    safety::log_deletion(path, category, false, Some(&reason));
                    continue;
                }
                safety::SafetyCheck::Warning(reason) => {
                    blocked_count += 1;
                    self.logs.push(t!("log_skipped", path = path, reason = reason).to_string());
                    safety::log_deletion(path, category, false, Some(&reason));
                    continue;
                }
                safety::SafetyCheck::Safe => {
                    // 通过安全检查，继续执行删除
                }
            }

            // APFS 快照特殊处理
            if tab == Tab::Apfs {
                if category == "APFS快照" {
                    match scanner::apfs::delete_snapshot(path) {
                        Ok(_) => {
                            deleted_count += 1;
                            self.logs.push(t!("log_deleted_snapshot", path = path).to_string());
                            safety::log_deletion(path, category, true, None);
                        }
                        Err(e) => {
                            failed_count += 1;
                            self.logs.push(t!("log_delete_snapshot_fail", path = path, err = e).to_string());
                            safety::log_deletion(path, category, false, Some(&e));
                        }
                    }
                } else if category == "模拟器运行时" {
                    match scanner::apfs::delete_simulator_runtime(path) {
                        Ok(_) => {
                            deleted_count += 1;
                            self.logs.push(t!("log_deleted_runtime", path = path).to_string());
                            safety::log_deletion(path, category, true, None);
                        }
                        Err(e) => {
                            failed_count += 1;
                            self.logs.push(t!("log_delete_runtime_fail", path = path, err = e).to_string());
                            safety::log_deletion(path, category, false, Some(&e));
                        }
                    }
                }
                continue;
            }

            // 普通文件/目录删除
            let p = Path::new(path.as_str());
            let size = self.results[idx][*i].size_bytes;

            // 二次校验：删除前再次确认路径存在且不是符号链接
            if !p.exists() && !p.symlink_metadata().is_ok() {
                failed_count += 1;
                self.logs.push(t!("log_path_not_exist", path = path).to_string());
                safety::log_deletion(path, category, false, Some("路径不存在"));
                continue;
            }

            // 拒绝删除符号链接（防止链接攻击）
            if let Ok(meta) = p.symlink_metadata() {
                if meta.file_type().is_symlink() {
                    blocked_count += 1;
                    self.logs.push(t!("log_symlink_blocked", path = path).to_string());
                    safety::log_deletion(path, category, false, Some("符号链接拒绝删除"));
                    continue;
                }
            }

            let result = if p.is_dir() {
                std::fs::remove_dir_all(p)
            } else {
                std::fs::remove_file(p)
            };

            match result {
                Ok(_) => {
                    deleted_count += 1;
                    freed_bytes += size;
                    self.logs.push(t!("log_deleted", cat = category, path = path).to_string());
                    safety::log_deletion(path, category, true, None);
                }
                Err(e) => {
                    failed_count += 1;
                    self.logs.push(t!("log_delete_fail", cat = category, path = path, err = e.to_string()).to_string());
                    safety::log_deletion(path, category, false, Some(&e.to_string()));
                }
            }
        }

        // 从结果列表中移除已删除的项
        for (i, _, _) in &to_delete {
            self.results[idx].remove(*i);
        }

        self.logs.push(t!(
            "log_summary",
            deleted = deleted_count,
            blocked = blocked_count,
            failed = failed_count,
            freed = scanner::format_size(freed_bytes)
        ).to_string());

        // 刷新磁盘信息
        let (total, free) = get_disk_info();
        self.disk_total = total;
        self.disk_free = free;

        self.pending_delete.clear();
        self.confirm = ConfirmState::None;
        self.list_index = 0;
        self.scroll_offset = 0;
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
}

/// 获取磁盘信息（总量、可用）
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
                if let (Ok(total_kb), Ok(free_kb)) = (
                    parts[1].parse::<u64>(),
                    parts[3].parse::<u64>(),
                ) {
                    return (total_kb * 1024, free_kb * 1024);
                }
            }
        }
    }

    (0, 0)
}
