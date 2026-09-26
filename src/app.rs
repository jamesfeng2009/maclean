//! App 状态管理
//!
//! 管理 egui GUI 应用的全部状态，包括当前 Tab、扫描结果、选中状态等。

use crate::safety;
use crate::scanner::{self, ScanItem};

/// 跨平台 Touch ID 可用性检查（macOS 专属，其他平台返回 false）
/// 删除队列的唯一过滤器：只放行 `deletable`
///
/// 抽成自由函数是因为 `App` 需要 eframe 上下文才能构造，测不了；而这正是
/// "保护有没有真的接上"的判定点，必须有测试。两个 prepare_delete* 出口都走它，
/// 避免再次出现"一个出口过滤、另一个不过滤"的规则漂移。
fn filter_deletable(
    results: &[Vec<crate::scanner::ScanItem>],
    items: &[(usize, usize)],
) -> Vec<(usize, usize)> {
    items
        .iter()
        .filter(|(tab_idx, item_idx)| {
            results
                .get(*tab_idx)
                .and_then(|v| v.get(*item_idx))
                .map(|item| item.deletable)
                // 索引越界 = 结果已过期（扫描后列表被换掉）。宁可不删。
                .unwrap_or(false)
        })
        .copied()
        .collect()
}

/// 把「扫描全部磁盘」设置推给 Windows 扫描器
///
/// 非 Windows 上是 no-op：该设置只影响 Program Files 的盘符枚举，macOS 没有
/// 多盘符概念。抽成函数是为了让"设置 → 扫描器"这条线只存在一个地方，
/// 不要在每个调用点各写一遍 cfg。
fn apply_scan_all_disks(enabled: bool) {
    #[cfg(target_os = "windows")]
    {
        crate::scanner::windows_apps::set_scan_all_disks(enabled);
        if enabled {
            let disks: Vec<String> = crate::scanner::windows_apps::list_available_disks()
                .iter()
                .map(|c| format!("{}:", c))
                .collect();
            crate::logger::info(&format!(
                "多磁盘扫描已开启，检测到盘符: {}",
                disks.join(" ")
            ));
        }
    }
    let _ = enabled;
}

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
    /// 自定义规则（声明式规则扫描）
    CustomRules,
    /// 重复文件（保留一份，其余进废纸篓）
    DuplicateFiles,
    /// 设置
    Settings,
}

// 2026-09-18 删除了 `Tab::title` 与 `Tab::prev`：
// - `title` 硬编码返回中文，而 UI 走 `ui::tab_title(tab, app)`（经 app.t() 中英切换）。
//   留着它，就总有一天被人顺手用上，"切成英文 Tab 标题还是中文"就回来了。
// - `prev` 与 `prev_tab()` 一并零引用（Tab 切换只往前，没有反向快捷键）。

impl Tab {
    /// 所有 Tab
    pub fn all() -> [Tab; 11] {
        [
            Tab::Overview,
            Tab::DevCache,
            Tab::LargeFiles,
            Tab::AppCache,
            Tab::AppData,
            Tab::AppUninstall,
            Tab::SystemOptimize,
            Tab::Apfs,
            Tab::CustomRules,
            Tab::DuplicateFiles,
            Tab::Settings,
        ]
    }

    /// 该 Tab 在当前平台是否有内容
    ///
    /// APFS 快照是 macOS 专有（`tmutil` / `xcrun simctl`）。之前 `Tab::all()`
    /// 不按平台过滤，Windows 上照样显示这个页签，扫描分发又直接返回空结果，
    /// 用户看到的是"扫过了、什么都没有"，而不是"这个平台没这功能"。
    ///
    /// 索引**不能**因为过滤而前移：`results` 是 `[Vec<ScanItem>; 9]`，按
    /// Tab 在 `all()` 里的下标寻址。所以这里只做"显示/跳过"，不改 `all()`。
    pub fn is_supported(self) -> bool {
        match self {
            Tab::Apfs => cfg!(target_os = "macos"),
            _ => true,
        }
    }

    /// 参与扫描的 Tab 及其在 `all()` 中的下标（跳过 Overview，按平台过滤）
    pub fn scannable() -> Vec<(Tab, usize)> {
        Tab::all()
            .iter()
            .enumerate()
            .skip(1) // Overview 不单独扫描
            .filter(|(_, tab)| tab.is_supported())
            .map(|(idx, tab)| (*tab, idx))
            .collect()
    }

    /// 下一个 Tab（自动跳过当前平台不支持的）
    pub fn next(self) -> Self {
        let raw_next = |t: Tab| match t {
            Tab::Overview => Tab::DevCache,
            Tab::DevCache => Tab::LargeFiles,
            Tab::LargeFiles => Tab::AppCache,
            Tab::AppCache => Tab::AppData,
            Tab::AppData => Tab::AppUninstall,
            Tab::AppUninstall => Tab::SystemOptimize,
            Tab::SystemOptimize => Tab::Apfs,
            Tab::Apfs => Tab::CustomRules,
            Tab::CustomRules => Tab::DuplicateFiles,
            Tab::DuplicateFiles => Tab::Settings,
            Tab::Settings => Tab::Overview,
        };
        let mut t = raw_next(self);
        // 上限是 Tab 总数：最坏情况绕一圈也必然停；写死上限防止将来
        // is_supported 改动时变成死循环。
        for _ in 0..Tab::all().len() {
            if t.is_supported() {
                break;
            }
            t = raw_next(t);
        }
        t
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
    /// 正在通过 Touch ID 删除（sudo 已配置 pam_tid.so）
    ///
    /// 从 `OfferTouchIdSetup` 点「启用」会先转到 `NeedSudoPassword`，
    /// 用密码调 `touchid::enable_touch_id_with_password` 写 sudo_local，
    /// 成功后再回到这个状态走 Touch ID 删除 —— 这条路径是通的。
    ///
    /// 2026-09-19 删掉了 `WaitForTouchIdSetup`（早期"开 Terminal 让用户自己授权、
    /// 应用轮询等待"那套设计）。它从未被置位，而密码路径已经能完成同样的事，
    /// 留着只会让人以为启用流程坏了。
    SudoWithTouchId,
}

/// App 主状态
pub struct App {
    /// 当前 Tab
    pub tab: Tab,
    /// 每个 Tab 的扫描结果
    pub results: [Vec<ScanItem>; 11],
    /// 每个 Tab 的扫描状态
    pub scan_states: [ScanState; 11],
    /// 列表选中索引
    pub list_index: usize,
    // 2026-09-18 删除了 `selected_uninstall_app_index`：零引用，卸载 Tab 的
    // 选中态实际记在 `list_index` 上。
    /// 磁盘总空间（字节）
    pub disk_total: u64,
    /// 磁盘可用空间（字节）
    pub disk_free: u64,
    /// 清理日志
    pub logs: Vec<String>,
    /// 系统优化 Tab：待用户确认的高风险任务索引（None=无待确认项）
    ///
    /// P1：用户点击 Advanced 级维护任务（如修复权限/校验启动盘）时不直接执行，
    /// 先弹确认框，确认后才进入执行。
    pub pending_optimize_task: Option<usize>,
    /// 确认状态
    pub confirm: ConfirmState,
    /// 待删除的项索引列表，每项为 (tab_index, item_index)
    /// 支持跨 Tab 删除（如概览一键清理）
    pub pending_delete: Vec<(usize, usize)>,
    /// 上一次 `prepare_delete*` 被保护规则挡掉的项数
    ///
    /// 只用于提示，不参与任何判定。UI 正常路径下应为 0（复选框已按 deletable
    /// 禁用），非 0 说明有程序化入口或某处 UI 漏判 —— 值得显式告诉用户，
    /// 而不是静默少删几项让他以为是清理没生效。
    pub protection_blocked: usize,
    /// 最近一次删除写下的备份清单：(清单 id, 可还原项数, 总项数)
    ///
    /// M-2：macOS 之前没有任何删除前的回滚手段。这里不假装能"系统级还原"
    /// —— 只有走废纸篓的项才真的搬得回来，永久删除的缓存字节已经没了。
    /// 存下 restorable 与 total 两个数，是为了让 UI 能把这句话说准确。
    pub last_backup: Option<(String, usize, usize)>,
    // 2026-09-18 删除了 `should_quit`：零引用。退出走 `App::quit()` /
    // `std::process::exit`，没人轮询这个标志。
    /// 扫描耗时（毫秒）
    pub scan_time_ms: [u64; 11],
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
    /// 关联文件明细：key = ScanItem.path，value = [(路径, 大小, 标签)]
    /// 用于 App卸载 tab 展开显示每个关联文件的大小和路径
    pub associated_details: std::collections::HashMap<String, Vec<(String, u64, String)>>,
    /// 已展开的项路径集合（用于展开/收起状态）
    pub expanded_items: std::collections::HashSet<String>,
    /// 磁盘分析器：当前浏览的目录路径
    pub disk_analyzer_path: Option<std::path::PathBuf>,
    /// 磁盘分析器：导航历史栈（用于返回上一级）
    pub disk_analyzer_history: Vec<std::path::PathBuf>,
    /// 磁盘分析视图：false=列表，true=矩形树图（P3）
    pub disk_view_mode: bool,
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
    /// 用户配置（启动时从 config.json 加载）
    pub user_config: crate::config::AppConfig,
    /// App卸载 Tab 已展开的应用分组名集合
    pub expanded_app_groups: std::collections::HashSet<String>,
    /// App卸载 Tab 当前按推荐等级过滤（Safe / Caution / Advanced）
    pub app_uninstall_recommend_filter: Option<crate::scanner::Recommend>,
    /// 是否显示残留清理弹窗（卸载后检测到残留时弹出）
    // 这四个字段只在 ui 的 `#[cfg(target_os = "windows")]` 分支里读写
    // （残留清理弹窗 / 还原点结果）。在 macOS 上编译时那段代码整块不存在，
    // dead_code 会误报 —— 按平台压制，不要删（删了 Windows 侧功能就没了）。
    #[cfg_attr(not(target_os = "windows"), allow(dead_code))]
    pub show_residual_dialog: bool,
    /// 启动时显示菜单栏图标
    pub settings_menubar_icon: bool,
    /// 自动保持 sudo 会话
    pub settings_keep_sudo: bool,
    /// 扫描结果本地缓存
    pub settings_scan_cache: bool,
    /// 扫描全部磁盘（仅 Windows 生效）
    ///
    /// 影响 Program Files 目录扫描：默认只扫 C 盘，开启后同时扫 D/E/... 盘符。
    /// 注册表扫描不受影响（UninstallString 本来就含完整路径）。
    pub settings_scan_all_disks: bool,
    /// 卸载应用时优先调用官方卸载器（仅 macOS 生效，M-1）
    ///
    /// 开启后，删除 .app 包时若发现厂商自带的卸载器，就交给它处理，
    /// 而不是自己 remove_dir_all —— 那些卸载器还要清 launchd 任务、
    /// pkgutil 收据、系统扩展授权，删目录做不到。
    pub settings_prefer_official_uninstaller: bool,
    /// 本次删除将交给官方卸载器处理的应用：(应用路径, 卸载器路径)
    ///
    /// 在 confirm_delete 时算一次并缓存，确认弹窗据此告知用户；
    /// 逐帧重算会反复读目录，没必要。
    pub official_handoffs: Vec<(String, String)>,
    /// 定时清理开关（C-3）
    pub schedule_enabled: bool,
    /// 定时清理间隔（天），取值经 `scheduler::normalize_interval_days` 归一
    pub schedule_interval_days: u32,
    /// 上次定时清理执行的 Unix 秒，0 = 从未执行
    pub schedule_last_run: u64,
    /// 注册/注销系统定时任务的结果（设置页显示）
    ///
    /// 存的是命令的**实际返回**，不是"已开启"的固定文案 —— 定时任务很可能
    /// 因为权限或策略注册失败，如实显示才不会给用户虚假的安全感。
    pub schedule_result: Option<String>,
    /// 删除前二次确认（Advanced 项目）
    pub settings_confirm_advanced: bool,
    /// 合盖时禁止删除（macOS only）
    pub settings_prevent_lid_close: bool,
    /// 操作前自动创建系统还原点（Windows only）
    pub settings_auto_restore_point: bool,
    /// 深色模式（持久化在 config.json 的 dark_mode）
    pub dark_mode: bool,
    /// "还原上次修改"操作的执行结果（设置页显示）
    #[cfg_attr(not(target_os = "windows"), allow(dead_code))]
    pub last_restore_result: Option<String>,
    /// "配置导入/导出"操作的执行结果（设置页显示）
    pub config_manage_result: Option<String>,
    /// 菜单栏 HUD 是否展开
    pub hud_open: bool,
    /// 上次点击托盘图标的位置（逻辑像素），用于 HUD 窗口定位
    pub last_hud_click_pos: Option<(f32, f32)>,
    /// 残留项选中状态（与残留列表一一对应，true=选中清理）
    #[cfg_attr(not(target_os = "windows"), allow(dead_code))]
    pub residual_selected: Vec<bool>,
    /// 残留清理中（正在执行清理操作）
    #[cfg_attr(not(target_os = "windows"), allow(dead_code))]
    pub residual_cleaning: bool,
    /// License 激活状态（启动时加载）
    pub license_status: crate::license::LicenseStatus,
    /// License 输入框内容
    pub license_input: String,
    /// License 激活错误提示
    pub license_error: Option<String>,
    /// 是否显示"额度用尽需激活"弹窗
    pub show_license_dialog: bool,
    /// 检测到的新版本（有更新时填充）
    pub update_available: Option<crate::updater::UpdateInfo>,
    /// 更新检查后台线程的结果通道
    pub update_rx: Option<std::sync::mpsc::Receiver<crate::updater::UpdateInfo>>,
    /// 用户已忽略当前版本的更新提示（本次运行内不再显示）
    pub update_dismissed: bool,
    /// C-2：是否正在后台下载更新包
    pub update_downloading: bool,
    /// C-2：下载/安装结果（横幅显示，成功与失败都显示原文）
    pub update_result: Option<String>,
    /// C-2：下载线程的返回通道
    pub update_dl_rx: Option<std::sync::mpsc::Receiver<Result<String, String>>>,
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
            Tab::CustomRules => None, // 每次全扫，不缓存
            Tab::DuplicateFiles => Some("dup_files"),
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
        let mut user_config = crate::config::load_config();

        // 首次启动（还没有 config.json）：主题跟随系统，之后以配置为准
        if !crate::config::config_exists() {
            user_config.dark_mode = crate::platform::system_prefers_dark();
            crate::config::save_config(&user_config);
        }

        // 后台线程检查更新（阻塞网络调用，结果经 channel 回传，失败静默）
        let (update_tx, update_rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            if let Some(info) = crate::updater::check_latest() {
                let _ = update_tx.send(info);
            }
        });

        let app = Self {
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
                ScanState::Idle,
                ScanState::Idle,
            ],
            list_index: 0,
            disk_total,
            disk_free,
            logs: Vec::new(),
            pending_optimize_task: None,
            confirm: ConfirmState::None,
            pending_delete: Vec::new(),
            protection_blocked: 0,
            last_backup: None,
            scan_time_ms: [0; 11],
            lang_en: user_config.lang_en,
            settings_menubar_icon: user_config.settings_menubar_icon,
            settings_keep_sudo: user_config.settings_keep_sudo,
            settings_scan_cache: user_config.settings_scan_cache,
            settings_scan_all_disks: user_config.settings_scan_all_disks,
            settings_prefer_official_uninstaller: user_config.settings_prefer_official_uninstaller,
            official_handoffs: Vec::new(),
            schedule_enabled: user_config.schedule_enabled,
            schedule_interval_days: crate::scheduler::normalize_interval_days(
                user_config.schedule_interval_days,
            ),
            schedule_last_run: user_config.schedule_last_run,
            schedule_result: None,
            settings_confirm_advanced: user_config.settings_confirm_advanced,
            settings_prevent_lid_close: user_config.settings_prevent_lid_close,
            settings_auto_restore_point: user_config.settings_auto_restore_point,
            dark_mode: user_config.dark_mode,
            last_restore_result: None,
            config_manage_result: None,
            user_config,
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
            show_permission_guide: !Self::check_full_disk_access(),
            show_preview: false,
            sudo_password_input: String::new(),
            sudo_password: None,
            sudo_failed_items: Vec::new(),
            sudo_error: None,
            touch_id_available: touch_id_available_cross(),
            touch_id_enabled: touch_id_enabled_cross(),
            touch_id_setup_mode: false,
            touch_id_error: None,
            associated_details: std::collections::HashMap::new(),
            expanded_items: std::collections::HashSet::new(),
            disk_view_mode: false,
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
            license_status: crate::license::load_status(),
            license_input: String::new(),
            license_error: None,
            show_license_dialog: false,
            update_available: None,
            update_downloading: false,
            update_result: None,
            update_dl_rx: None,
            update_rx: Some(update_rx),
            update_dismissed: false,
            #[cfg(target_os = "windows")]
            uninstall_residual: None,
        };

        // 启动即把持久化的设置推给扫描器：静态量默认是"仅 C 盘"，
        // 不推的话用户上一次开的开关在这次启动里是失效的。
        apply_scan_all_disks(app.settings_scan_all_disks);
        app
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
            Tab::CustomRules => 8,
            Tab::DuplicateFiles => 9,
            Tab::Settings => 10,
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

    // 2026-09-18 删除了 `prev_tab()`：随 `Tab::prev` 一起零引用。

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
                let prefix_ok = cat_prefix
                    .as_ref()
                    .is_none_or(|prefix| item.category.to_lowercase().starts_with(prefix));
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

    // 2026-09-18 删除了 `current_scan_state()`：与 `any_scanning()` 的语义
    // 冲突正是 P1-8 的根因（概览页按钮扫描中不置灰）。约定见 `any_scanning`
    // 上方注释：按钮态用 `tab_scanning(idx)`，全局态只能用 `any_scanning()`。

    /// 当前 Tab 是否在扫描中
    ///
    /// 与 `any_scanning()` 的区别要明确：
    /// - 判断"本 Tab 的按钮能不能点"用 `tab_scanning(idx)`
    /// - 判断"全局有没有扫描在跑"（概览页、底部删除栏）必须用 `any_scanning()`
    ///
    /// 之前 UI 里混用 `current_scan_state()`，而 `start_scan_all` 不置
    /// `scan_states[0]`（Overview），导致概览页「扫描」按钮扫描中永不置灰，
    /// 可以并发拉起多轮全量扫描。
    pub fn any_scanning(&self) -> bool {
        self.scan_states
            .iter()
            .any(|s| matches!(s, ScanState::Scanning))
    }

    /// 该 Tab 是否可以扫描（概览与设置页没有可扫内容）
    pub fn is_non_scannable_tab(&self) -> bool {
        matches!(self.tab, Tab::Overview | Tab::Settings)
    }

    /// 进入「扫描全部」前的状态复位
    ///
    /// 必须与单 Tab 的 `start_scan` 一样清空 results：`PartialItems` 走 extend，
    /// 不清空的话二次「扫描全部」时列表是旧+新叠加，概览统计翻倍；
    /// 若某 Tab 扫描 panic 没发 Done，重复项还会固化下来。
    ///
    /// 单独抽出来是因为 `start_scan_all` 会真的拉起后台线程，没法单测。
    pub fn reset_for_full_scan(&mut self) {
        for tab_idx in 1..self.results.len() {
            self.scan_states[tab_idx] = ScanState::Scanning;
            self.results[tab_idx].clear();
        }
        self.scan_progress = 0.0;
    }

    /// 指定 Tab 是否在扫描中
    pub fn tab_scanning(&self, tab_idx: usize) -> bool {
        self.scan_states
            .get(tab_idx)
            .map(|s| matches!(s, ScanState::Scanning))
            .unwrap_or(false)
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
            .unwrap_or_else(scanner::home_dir)
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

    // 2026-09-18 删除了 `cache_only_count` / `cache_only_size`：零引用，
    // 概览页的「仅缓存」统计走的是别的聚合路径。

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

    // 2026-09-18 删除了 `App::scan_current()`：实时扫描由 ui 侧的
    // `start_scan` 起线程、经 ScanMessage::Done 回传后直接落库，这个方法
    // 从未被调用。它里面唯一有价值的两个动作 —— 扫描后的安全预检
    // （precheck_deletability）和卸载 Tab 的关联明细（populate_associated_details）
    // —— 已挪到 ui/mod.rs 的真实落库点，此前在生产路径上从未执行过。

    /// 扫描后预检查可删除性
    ///
    /// 对当前 Tab 的每个 ScanItem 运行安全检查（safety::check_path_safety_with_category）。
    /// - Danger → 标记 deletable=false，写明原因，UI 灰色不可选
    /// - Warning → 保持 deletable=true，但在描述中追加警告
    /// - Safe → 不变
    ///
    /// 这样在扫描阶段就屏蔽掉确定无法删除的项，避免用户选中后删除失败。
    pub(crate) fn precheck_deletability(&mut self, idx: usize) {
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
    pub(crate) fn populate_associated_details(&mut self, idx: usize) {
        self.associated_details.clear();
        let items = self.results[idx].clone();
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
                        sub_dirs.sort_by_key(|a| std::cmp::Reverse(a.1));
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
            details.sort_by_key(|a| std::cmp::Reverse(a.1));
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

    /// 取出 `pending_delete` 指向的条目（可能跨 Tab）
    ///
    /// 确认框的一切统计都必须走这里，而不是 `selected_count()` /
    /// `selected_total_size()` —— 后者只统计**当前 Tab**，而跨 Tab 删除
    /// （概览一键清理、概览底部删除栏）时当前 Tab 是 Overview，其 results 恒为空，
    /// 会显示「0 项 / 0 B」却在点确认后真实删除 N 个文件（历史 bug）。
    pub fn pending_items(&self) -> Vec<&crate::scanner::ScanItem> {
        self.pending_delete
            .iter()
            .filter_map(|(tab_idx, item_idx)| {
                self.results.get(*tab_idx).and_then(|v| v.get(*item_idx))
            })
            .collect()
    }

    /// `pending_delete` 的条目数（跨 Tab 安全）
    pub fn pending_count(&self) -> usize {
        self.pending_items().len()
    }

    /// `pending_delete` 的总大小（跨 Tab 安全）
    pub fn pending_total_size(&self) -> u64 {
        self.pending_items().iter().map(|i| i.size_bytes).sum()
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
        let selected: Vec<(usize, usize)> = self.results[idx]
            .iter()
            .enumerate()
            .filter(|(_, item)| item.selected)
            .map(|(i, _)| (idx, i))
            .collect();

        if selected.is_empty() {
            return;
        }

        // 闸门：只放行 deletable。之前这里只过滤 selected，而
        // prepare_delete_cross_tab 才过滤 deletable —— 两个出口规则不一致，
        // 等于保护判定（Critical / RequiresOfficialUninstaller / DataProtected）
        // 在最常用那条路径上根本没接。UI 的复选框大多按 deletable 禁用了，
        // 但那不是闸门：程序化入口（one_click_clean 无条件置 selected = true）
        // 或任何一处 UI 漏判，受保护项就会直接进入删除队列。
        let deletable = filter_deletable(&self.results, &selected);
        self.protection_blocked = selected.len() - deletable.len();
        let selected = deletable;

        if selected.is_empty() {
            return;
        }

        if !self.quota_gate_for(&selected) {
            return;
        }

        self.pending_delete = selected;
        self.confirm = ConfirmState::Pending;
    }

    /// 准备跨 Tab 删除（概览一键清理使用）
    pub fn prepare_delete_cross_tab(&mut self, items: Vec<(usize, usize)>) {
        let before = items.len();
        let selected: Vec<(usize, usize)> = filter_deletable(&self.results, &items);
        self.protection_blocked = before - selected.len();

        if selected.is_empty() {
            return;
        }

        if !self.quota_gate_for(&selected) {
            return;
        }

        self.pending_delete = selected;
        self.confirm = ConfirmState::Pending;
    }

    /// 免费额度闸门：返回 true 表示放行，false 表示拦截并弹激活窗
    fn quota_gate_for(&mut self, selected: &[(usize, usize)]) -> bool {
        // 开发者模式无限制
        if crate::license::is_dev_mode() {
            return true;
        }
        // 已激活用户无限制
        if matches!(
            self.license_status,
            crate::license::LicenseStatus::Activated { .. }
        ) {
            return true;
        }

        let plan_bytes: u64 = selected
            .iter()
            .filter_map(|(tab_idx, item_idx)| {
                self.results.get(*tab_idx).and_then(|v| v.get(*item_idx))
            })
            .map(|item| item.size_bytes)
            .sum();

        if crate::license::check_quota_allow(plan_bytes).is_none() {
            return true;
        }

        // 额度不足，弹激活窗口
        self.show_license_dialog = true;
        false
    }

    /// 轮询后台更新检查结果（主循环每帧调用，非阻塞）
    pub fn poll_update(&mut self) {
        if let Some(rx) = &self.update_rx {
            if let Ok(info) = rx.try_recv() {
                self.update_available = Some(info);
                self.update_rx = None;
            }
        }
        self.poll_update_download();
    }

    /// 收取更新下载线程的结果（C-2）
    ///
    /// 通道断开也要收尾：线程 panic 后若一直卡在 downloading=true，
    /// 横幅上的按钮会永久变成禁用的"下载中"。
    pub fn poll_update_download(&mut self) {
        if !self.update_downloading {
            return;
        }
        let mut finished: Option<Result<String, String>> = None;
        let mut disconnected = false;
        if let Some(rx) = &self.update_dl_rx {
            match rx.try_recv() {
                Ok(r) => finished = Some(r),
                Err(std::sync::mpsc::TryRecvError::Empty) => {}
                Err(std::sync::mpsc::TryRecvError::Disconnected) => disconnected = true,
            }
        }
        if let Some(r) = finished {
            self.update_downloading = false;
            self.update_dl_rx = None;
            self.update_result = Some(r.unwrap_or_else(|e| e));
        } else if disconnected {
            self.update_downloading = false;
            self.update_dl_rx = None;
            self.update_result = Some(self.t("update_download_aborted").to_string());
        }
    }

    /// 开始下载更新包（C-2）
    ///
    /// 返回 false 表示没有匹配本机的产物，调用方应退回"打开 Release 页面"。
    pub fn start_update_download(&mut self) -> bool {
        if self.update_downloading {
            return true;
        }
        let Some(info) = self.update_available.clone() else {
            return false;
        };
        if info.asset.is_none() {
            return false;
        }
        let (tx, rx) = std::sync::mpsc::channel();
        self.update_dl_rx = Some(rx);
        self.update_downloading = true;
        self.update_result = None;
        std::thread::spawn(move || {
            let r = crate::updater::download_and_open(&info);
            let _ = tx.send(r);
        });
        true
    }

    /// 尝试激活 License（UI 调用）
    pub fn try_activate_license(&mut self) {
        let key = self.license_input.trim().to_string();
        if key.is_empty() {
            self.license_error = Some(if self.lang_en {
                "Please enter your license key".to_string()
            } else {
                "请输入 License Key".to_string()
            });
            return;
        }
        match crate::license::activate(&key) {
            Ok(payload) => {
                self.license_status = crate::license::LicenseStatus::Activated {
                    email: payload.email,
                    plan: payload.plan,
                };
                self.license_error = None;
                self.license_input.clear();
                self.show_license_dialog = false;
            }
            Err(e) => {
                self.license_error = Some(e);
            }
        }
    }

    /// 注册 / 注销系统级定时任务（C-3）
    ///
    /// 结果存的是命令的**实际返回**，不是"已开启"的固定文案：
    /// launchd / schtasks 都可能因权限或策略失败，如实显示才不会给用户
    /// 虚假的安全感 —— 以为设好了，其实一次都没跑过。
    pub fn apply_schedule(&mut self) {
        if !crate::license::current_tier().allows_scheduled_cleanup() {
            self.schedule_result = Some(self.t("schedule_pro_only").to_string());
            return;
        }
        self.schedule_interval_days =
            crate::scheduler::normalize_interval_days(self.schedule_interval_days);
        let text = if self.schedule_enabled {
            match crate::scheduler::install(self.schedule_interval_days) {
                Ok(_) => self.t("schedule_installed").to_string(),
                Err(e) => self.tf("schedule_install_failed", &[&e]),
            }
        } else {
            let (prog, args) = crate::scheduler::uninstall_command();
            match std::process::Command::new(prog).args(args).output() {
                Ok(o) if o.status.success() => self.t("schedule_removed").to_string(),
                Ok(o) => self.tf(
                    "schedule_remove_failed",
                    &[String::from_utf8_lossy(&o.stderr).trim()],
                ),
                Err(e) => self.tf("schedule_remove_failed", &[&e.to_string()]),
            }
        };
        self.schedule_result = Some(text);
        self.save_settings();
    }

    /// 确认删除 - 收集待删除项，返回 (path, category, batch_paths, use_trash, size_bytes) 供后台线程使用
    /// use_trash: true 表示移至废纸篓（可恢复），false 表示永久删除
    /// size_bytes: 扫描时算出的大小，随删除任务带到备份清单里。
    ///   这里刻意不做"删前重算"—— 一个大目录重算一遍要走全树，而清单只是
    ///   事后追溯用的，扫描值够用；真要看准确数字，用户看删除结果摘要。
    pub fn confirm_delete(&mut self) -> Vec<(String, String, Vec<String>, bool, u64)> {
        self.confirm = ConfirmState::Deleting;

        // 收集要删除的路径和类别（跳过不可删除的项）
        // 策略：Safe 级别永久删除（缓存自动重建），Caution/Advanced 移至废纸篓（可恢复）
        let to_delete: Vec<(String, String, Vec<String>, bool, u64)> = self
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
                    item.size_bytes,
                )
            })
            .collect();

        // M-1：算出哪些 .app 要交给官方卸载器，确认弹窗据此如实告知用户。
        // 这里算一次缓存起来，避免在弹窗逐帧渲染时反复读目录。
        // 设置关掉时一律不走这条路 —— 用户明确选了"只删目录"。
        self.official_handoffs = if self.settings_prefer_official_uninstaller {
            to_delete
                .iter()
                .filter(|(p, _, _, _, _)| p.ends_with(".app"))
                .filter_map(|(p, _, _, _, _)| {
                    let name = crate::scanner::official_uninstaller::app_display_name(p);
                    crate::scanner::official_uninstaller::find_official_uninstaller(p, &name)
                        .map(|u| (p.clone(), u.path))
                })
                .collect()
        } else {
            Vec::new()
        };

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
        let mut freed_bytes: u64 = 0;

        for (tab_idx, item_idx) in &self.pending_delete {
            affected_tabs.insert(*tab_idx);
            if let Some(item) = self
                .results
                .get_mut(*tab_idx)
                .and_then(|v| v.get_mut(*item_idx))
            {
                item.selected = false;
                if deleted.contains(&item.path) {
                    freed_bytes = freed_bytes.saturating_add(item.size_bytes);
                    // 标记为已删除（稍后统一移除）
                    item.path = String::new();
                }
            }
        }

        // 免费版记录额度消耗（已激活用户无需记录）
        if freed_bytes > 0 && matches!(self.license_status, crate::license::LicenseStatus::Free) {
            crate::license::quota_add(freed_bytes);
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

    // 2026-09-18 删除了 `App::quit()`：它只置一个零引用的 `should_quit`
    // 标志，退出实际由 ui 侧的 `std::process::exit` / 视口关闭完成。

    /// 切换中英文语言
    pub fn toggle_lang(&mut self) {
        self.lang_en = !self.lang_en;
        self.user_config.lang_en = self.lang_en;
        crate::config::save_config(&self.user_config);
    }

    /// 保存当前设置到 user_config 并写盘
    pub fn save_settings(&mut self) {
        self.user_config.settings_menubar_icon = self.settings_menubar_icon;
        self.user_config.settings_keep_sudo = self.settings_keep_sudo;
        self.user_config.settings_scan_cache = self.settings_scan_cache;
        self.user_config.settings_scan_all_disks = self.settings_scan_all_disks;
        self.user_config.settings_prefer_official_uninstaller =
            self.settings_prefer_official_uninstaller;
        self.user_config.schedule_enabled = self.schedule_enabled;
        self.user_config.schedule_interval_days = self.schedule_interval_days;
        self.user_config.schedule_last_run = self.schedule_last_run;
        self.user_config.settings_confirm_advanced = self.settings_confirm_advanced;
        self.user_config.settings_prevent_lid_close = self.settings_prevent_lid_close;
        self.user_config.settings_auto_restore_point = self.settings_auto_restore_point;
        self.user_config.dark_mode = self.dark_mode;
        crate::config::save_config(&self.user_config);
        // 设置必须立刻推给扫描器，否则要等下次启动才生效
        apply_scan_all_disks(self.settings_scan_all_disks);
    }

    /// 当前配色模式
    pub fn theme_mode(&self) -> crate::theme::Mode {
        crate::theme::Mode::from_dark_flag(self.dark_mode)
    }

    /// 切换深色模式并落盘
    pub fn toggle_dark_mode(&mut self) {
        self.dark_mode = !self.dark_mode;
        self.user_config.dark_mode = self.dark_mode;
        crate::config::save_config(&self.user_config);
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
                "tab_custom_rules" => "Custom Rules",
                "tab_dup_files" => "Duplicate Files",
                "disk_view_list" => "List",
                "disk_view_tree" => "Treemap",
                "tab_settings" => "Settings",
                "settings_appearance" => "Appearance",
                "setting_dark_mode" => "Dark mode",
                "setting_dark_mode_desc" => "Switch the whole UI between light and dark",
                "settings_general" => "General",
                "settings_safety" => "Safety",
                "settings_language" => "Language",
                "setting_menubar_icon" => "Show menu bar icon on launch",
                "setting_menubar_icon_desc" => "Display disk usage indicator in the macOS menu bar",
                "setting_keep_sudo" => "Keep sudo session alive",
                "setting_keep_sudo_desc" => "Avoid repeated administrator password prompts",
                "setting_scan_cache" => "Cache scan results locally",
                "setting_scan_cache_desc" => "Avoid re-scanning within 7 days for faster startup",
                "setting_scan_all_disks" => "Scan all disks",
                "setting_scan_all_disks_desc" => {
                    "C: only by default. Enabling also scans Program Files on D:/E:/etc."
                },
                // M-1
                "setting_official_uninstaller" => "Use the app's own uninstaller when available",
                "setting_official_uninstaller_desc" => {
                    "Deleting a .app bundle leaves launchd jobs and pkgutil receipts behind. The vendor uninstaller cleans those up."
                },
                "confirm_official_uninstaller_hint" => {
                    "Handed to the app's own uninstaller (opens a separate window)"
                },
                "log_official_uninstaller" => "Handed to official uninstaller",
                // C-1 · 付费分级
                "tier_free" => "Free",
                "tier_pro" => "Pro",
                "tier_capability_unlimited" => "Unlimited cleanup quota",
                "tier_capability_scheduled" => "Scheduled cleanup",
                "tier_pro_includes" => "Pro adds: {}",
                // C-3 · 定时清理
                "settings_schedule" => "Scheduled cleanup",
                "setting_schedule_enable" => "Run cleanup automatically",
                "setting_schedule_enable_desc" => {
                    "Registers a system job, so cleanup runs even when the app is closed"
                },
                "setting_schedule_interval" => "Interval",
                "schedule_every_day" => "Daily",
                "schedule_every_week" => "Weekly",
                "schedule_every_month" => "Monthly",
                "schedule_last_run" => "Last run: {}",
                "schedule_never_run" => "Never run yet",
                "schedule_pro_only" => "Scheduled cleanup is a Pro feature",
                "schedule_installed" => "Scheduled job registered",
                "schedule_install_failed" => "Could not register the job: {}",
                "schedule_removed" => "Scheduled job removed",
                "schedule_remove_failed" => "Could not remove the job: {}",
                // C-2 · 自动更新
                "update_download_aborted" => "Update download stopped unexpectedly",
                "setting_confirm_advanced" => "Double-check before deleting Advanced items",
                "setting_confirm_advanced_desc" => "Advanced items require manual confirmation",
                "setting_prevent_lid_close" => "Prevent deletion while lid is closed",
                "setting_prevent_lid_close_desc" => "Detect MacBook clamshell state to avoid accidental deletion",
                "setting_auto_restore_point" => "Auto-create restore point before operations",
                "setting_auto_restore_point_desc" => "Create a system restore point before cleanup/optimization (Windows)",
                "setting_restore_last" => "Restore last registry modification",
                "setting_restore_last_desc" => "Re-import the most recent registry backup made before optimization. Restores old values only; newly added keys must be removed manually",
                "restore_last_btn" => "Restore",
                "restore_last_none" => "No registry backup found",
                "restore_last_success" => "✅ Restored: {0}",
                "restore_last_failed" => "⚠️ Restore failed (administrator may be required)",
                "restore_last_backup_info" => "Last backup: {0} ({1}, {2} keys)",
                "restore_point_created" => "✅ System restore point created",
                "restore_point_skipped" => "⏭️ Recent restore point exists, skipped",
                "restore_point_failed" => "⚠️ Failed to create restore point (administrator required)",
                "settings_config_mgmt" => "Config Management",
                "config_dir_label" => "Config directory",
                "config_export_btn" => "Export Config",
                "config_import_btn" => "Import Config",
                "config_open_btn" => "Open Folder",
                "config_export_success" => "✅ Config exported to: {0}",
                "config_export_failed" => "⚠️ Export failed: {0}",
                "config_import_success" => "✅ Config imported ({0}). Restart to fully apply",
                "config_import_failed" => "⚠️ Import failed: {0}",
                "config_nothing_to_export" => "⚠️ Nothing to export yet",
                "config_no_config_found" => "⚠️ No config.json / apps.json found in the folder",
                "config_apps_hint" => "Place a custom apps.json here to override the built-in bloatware list",
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
                // 状态页（设计稿 5.7：未扫描 与 扫描完成为空 是两种不同情绪）
                "empty_never_scanned" => "Not scanned yet",
                "empty_never_scanned_desc" => {
                    "Scanning only reads cache sizes — nothing is modified or deleted"
                }
                "empty_all_clean" => "This machine is clean",
                "empty_all_clean_desc" => "No cleanable items found",
                "empty_view_log" => "View scan log",
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
                "confirm_protection_skipped" => "Protected items skipped (system / security / app data)",
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
                // 系统维护（P1，对标 MangoDisk system_maintenance）
                "optimize_icon_cache_rebuild" => "Rebuild Icon Cache",
                "optimize_finder_service_restart" => "Restart Finder Service",
                "optimize_audio_service_restart" => "Restart Audio Service",
                "optimize_legacy_overrides_clean" => "Clean Legacy Overrides",
                "optimize_user_permissions_repair" => "Repair User Permissions",
                "optimize_startup_disk_verify" => "Verify Startup Disk",
                "opt_icon_cache_success" => "Icon cache rebuilt",
                "opt_icon_cache_partial" => "Icon cache cleared (Finder refresh pending)",
                "opt_icon_cache_fail" => "Failed to rebuild icon cache",
                "opt_finder_restart_success" => "Finder restarted",
                "opt_finder_restart_fail" => "Failed to restart Finder",
                "opt_audio_restart_success" => "Audio service restarted",
                "opt_audio_restart_fail" => "Audio service restart requires administrator privileges; please retry after authorization",
                "opt_legacy_overrides_success" => "Legacy overrides cleared (backup kept)",
                "opt_legacy_overrides_no_backup" => "Legacy overrides cleared (backup failed)",
                "opt_legacy_overrides_empty" => "No legacy overrides found",
                "opt_legacy_overrides_fail" => "Failed to clear legacy overrides",
                "opt_permissions_repair_success" => "User permissions repaired",
                "opt_permissions_repair_fail" => "Failed to repair user permissions",
                "opt_startup_disk_success" => "Startup disk verified, no issues found",
                "opt_startup_disk_fail" => "Startup disk verification failed or found issues",
                "optimize_confirm_title" => "High-risk maintenance task",
                "optimize_confirm_body" => "This task changes user-level settings and may take a while. Continue?",
                "optimize_confirm_yes" => "Continue",
                "optimize_confirm_no" => "Cancel",
                // Windows 优化任务
                "optimize_win_dns_flush" => "Flush DNS Cache",
                "optimize_win_temp_cleanup" => "Clean Temp Files",
                "optimize_win_disable_telemetry" => "Disable Telemetry",
                "optimize_win_disable_copilot" => "Disable Copilot",
                "optimize_win_disable_suggestions" => "Disable Ads & Suggestions",
                "optimize_win_startup_audit" => "Audit Startup Items",
                "optimize_win_disable_fast_startup" => "Disable Fast Startup",
                "optimize_win_restore_point" => "Create Restore Point",
                "optimize_win_restart_explorer" => "Restart Explorer",
                "optimize_win_trim_drives" => "Optimize Drives (TRIM/Defrag)",
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
                // Windows 优化任务结果
                "opt_win_dns_success" => "✅ DNS cache flushed",
                "opt_win_dns_fail" => "⚠️ DNS flush failed. Run in CMD: ipconfig /flushdns",
                "opt_win_temp_success" => "✅ Cleaned {0} of temp files",
                "opt_win_temp_fail" => "⚠️ Temp file cleanup failed",
                "opt_win_telemetry_success" => "✅ Telemetry disabled (registry policy set)",
                "opt_win_telemetry_fail" => "⚠️ Failed to disable telemetry. Run as Administrator",
                "opt_win_copilot_success" => "✅ Copilot disabled (registry policy set)",
                "opt_win_copilot_fail" => "⚠️ Failed to disable Copilot. Run as Administrator",
                "opt_win_suggestions_success" => "✅ Ads & suggestions disabled",
                "opt_win_suggestions_fail" => "⚠️ Failed to disable suggestions. Run as Administrator",
                "opt_win_startup_opened" => "✅ Opened Task Manager > Startup",
                "opt_win_startup_fail" => "⚠️ Failed to open Task Manager",
                "opt_win_faststartup_success" => "✅ Fast startup disabled",
                "opt_win_faststartup_fail" => "⚠️ Failed to disable fast startup. Run as Administrator",
                "opt_win_restore_success" => "✅ System restore point created",
                "opt_win_restore_fail" => "⚠️ Failed to create restore point. Run as Administrator",
                "opt_win_explorer_success" => "✅ Explorer restarted",
                "opt_win_explorer_fail" => "⚠️ Failed to restart Explorer",
                "opt_win_trim_success" => "✅ Drive optimization started",
                "opt_win_trim_fail" => "⚠️ Drive optimization failed. Run as Administrator",
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
                // M-2：{} = 可还原项数 / 总项数。两个数字都给，是因为只说
                // "已备份"会让人以为全都能还原 —— 永久删除的字节回不来。
                "summary_backup" => "Deletion log saved ({} of {} items can be restored from Trash)",
                "summary_backup_none" => "Deletion log saved — none of these items are recoverable (they were permanently deleted)",
                "summary_solution_title" => "Tips: Failure reasons and solutions",
                "summary_sip_tip" => "SIP/System protection: Paths like /Library/Developer/CoreSimulator are protected by macOS SIP and cannot be deleted with any privilege. Such paths are marked as undeletable during scanning — there is no need to disable SIP.",
                "summary_perm_tip" => "Permission denied: Some files are owned by root or inside protected directories. Click \"Retry deletion (admin authorization)\" below and the app requests authorization automatically — no manual commands needed.",
                "summary_solution_1" => "1. Click \"Retry deletion (admin authorization)\" below: the app authorizes and retries automatically",
                "summary_solution_2" => "2. SIP-protected system paths cannot be deleted — the app never removes macOS system files",
                "summary_solution_3" => "3. Items still failing: uncheck them in the tab and continue, or check whether the file is currently in use",
                "summary_open_settings" => "Open System Settings",
                "summary_fail_list" => "Failed list:",
                "summary_copy_paths" => "Copy Paths",
                "summary_copy_sudo" => "Copy sudo Command",
                "summary_retry_delete" => "Retry deletion (admin authorization)",
                "log_protected_root_rejected" => "Protected system root — deletion refused: {}",
                "summary_retry_hint" => "maclean will retry with administrator privileges automatically — no manual commands needed.",
                "fail_reason_perm" => "Permission denied",
                "fail_reason_sip" => "Protected by SIP/system",
                "fail_reason_inuse" => "In use by another process",
                "fail_reason_immutable" => "File is locked (immutable flag)",
                "fail_reason_other" => "Other reason",
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
                "log_trash_failed" => "Could not move to Trash (grant Automation permission in System Settings), kept as-is: {}",
                "log_sudo_rejected" => "Blocked by safety check before sudo deletion: {} — {}",
                "log_sudo_symlink_rejected" => "Blocked before sudo deletion (path became a symlink): {}",
                "log_sudo_unsafe_path" => "Blocked before sudo deletion (path contains unsafe characters): {}",
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
                "tab_custom_rules" => "自定义规则",
                "tab_dup_files" => "重复文件",
                "disk_view_list" => "列表",
                "disk_view_tree" => "树图",
                "tab_settings" => "设置",
                "settings_appearance" => "外观",
                "setting_dark_mode" => "深色模式",
                "setting_dark_mode_desc" => "切换整套界面的浅色 / 深色配色",
                "settings_general" => "通用",
                "settings_safety" => "安全",
                "settings_language" => "语言",
                "setting_menubar_icon" => "启动时显示菜单栏图标",
                "setting_menubar_icon_desc" => "在 macOS 菜单栏常驻磁盘用量指示器",
                "setting_keep_sudo" => "自动保持 sudo 会话",
                "setting_keep_sudo_desc" => "避免重复输入管理员密码",
                "setting_scan_cache" => "扫描结果本地缓存",
                "setting_scan_cache_desc" => "7 天内避免重复扫描，加速启动",
                "setting_scan_all_disks" => "扫描全部磁盘",
                "setting_scan_all_disks_desc" => "默认只扫 C 盘；开启后同时扫描 D/E 等盘符的 Program Files",
                // M-1
                "setting_official_uninstaller" => "优先使用应用自带的官方卸载器",
                "setting_official_uninstaller_desc" => "直接删 .app 目录会留下 launchd 任务与 pkgutil 收据，官方卸载器会一并清掉",
                "confirm_official_uninstaller_hint" => "交由应用自带官方卸载器处理（会弹出独立窗口）",
                "log_official_uninstaller" => "已交由官方卸载器",
                // C-1 · 付费分级
                "tier_free" => "免费版",
                "tier_pro" => "Pro 版",
                "tier_capability_unlimited" => "清理额度无上限",
                "tier_capability_scheduled" => "定时清理",
                "tier_pro_includes" => "Pro 版增加：{}",
                // C-3 · 定时清理
                "settings_schedule" => "定时清理",
                "setting_schedule_enable" => "自动执行清理",
                "setting_schedule_enable_desc" => "注册系统级任务，应用未运行时也会按时清理",
                "setting_schedule_interval" => "执行间隔",
                "schedule_every_day" => "每天",
                "schedule_every_week" => "每周",
                "schedule_every_month" => "每月",
                "schedule_last_run" => "上次执行：{}",
                "schedule_never_run" => "尚未执行过",
                "schedule_pro_only" => "定时清理为 Pro 版功能",
                "schedule_installed" => "已注册定时任务",
                "schedule_install_failed" => "注册定时任务失败：{}",
                "schedule_removed" => "已移除定时任务",
                "schedule_remove_failed" => "移除定时任务失败：{}",
                "setting_confirm_advanced" => "删除前二次确认",
                "setting_confirm_advanced_desc" => "Advanced 项目必须手动确认",
                "setting_prevent_lid_close" => "合盖时禁止删除",
                "setting_prevent_lid_close_desc" => "检测 MacBook 合盖状态，防止误触",
                "setting_auto_restore_point" => "操作前自动创建系统还原点",
                "setting_auto_restore_point_desc" => "清理/优化前自动备份系统状态（Windows）",
                "setting_restore_last" => "还原上次注册表修改",
                "setting_restore_last_desc" => "重新导入优化前自动备份的注册表文件。仅恢复被覆盖的旧值，新增的键值需手动删除",
                "restore_last_btn" => "还原",
                "restore_last_none" => "未找到注册表备份",
                "restore_last_success" => "✅ 已还原: {0}",
                "restore_last_failed" => "⚠️ 还原失败（可能需要管理员权限）",
                "restore_last_backup_info" => "最近备份: {0}（{1}，{2} 个键）",
                "restore_point_created" => "✅ 系统还原点已创建",
                "restore_point_skipped" => "⏭️ 近期已有还原点，跳过创建",
                "restore_point_failed" => "⚠️ 还原点创建失败（需管理员权限）",
                "settings_config_mgmt" => "配置管理",
                "config_dir_label" => "配置目录",
                "config_export_btn" => "导出配置",
                "config_import_btn" => "导入配置",
                "config_open_btn" => "打开文件夹",
                "config_export_success" => "✅ 配置已导出到: {0}",
                "config_export_failed" => "⚠️ 导出失败: {0}",
                "config_import_success" => "✅ 配置已导入（{0}），重启后完全生效",
                "config_import_failed" => "⚠️ 导入失败: {0}",
                "config_nothing_to_export" => "⚠️ 暂无可导出的配置",
                "config_no_config_found" => "⚠️ 该文件夹中未找到 config.json / apps.json",
                "config_apps_hint" => "将自定义 apps.json 放入配置目录可覆盖内置应用列表",
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
                // 状态页（设计稿 5.7：未扫描 与 扫描完成为空 是两种不同情绪）
                "empty_never_scanned" => "还没有扫描过",
                "empty_never_scanned_desc" => "扫描只会读取缓存目录大小，不会修改或删除任何文件",
                "empty_all_clean" => "这台机器很干净",
                "empty_all_clean_desc" => "未发现可清理的项目",
                "empty_view_log" => "查看扫描日志",
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
                "confirm_protection_skipped" => "已跳过受保护项（系统 / 安全软件 / 应用数据）",
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
                // 系统维护（P1，对标 MangoDisk system_maintenance）
                "optimize_icon_cache_rebuild" => "图标缓存重建",
                "optimize_finder_service_restart" => "重启 Finder 服务",
                "optimize_audio_service_restart" => "重启音频服务",
                "optimize_legacy_overrides_clean" => "清理旧版打开方式覆盖",
                "optimize_user_permissions_repair" => "修复用户目录权限",
                "optimize_startup_disk_verify" => "校验启动盘",
                "opt_icon_cache_success" => "图标缓存已重建",
                "opt_icon_cache_partial" => "图标缓存已清除（Finder 待刷新）",
                "opt_icon_cache_fail" => "图标缓存重建失败",
                "opt_finder_restart_success" => "Finder 已重启",
                "opt_finder_restart_fail" => "Finder 重启失败",
                "opt_audio_restart_success" => "音频服务已重启",
                "opt_audio_restart_fail" => "音频服务重启需要管理员权限，授权后重试即可",
                "opt_legacy_overrides_success" => "旧版打开方式覆盖已清理（已备份）",
                "opt_legacy_overrides_no_backup" => "旧版打开方式覆盖已清理（备份失败）",
                "opt_legacy_overrides_empty" => "未发现旧版打开方式覆盖",
                "opt_legacy_overrides_fail" => "旧版打开方式覆盖清理失败",
                "opt_permissions_repair_success" => "用户目录权限已修复",
                "opt_permissions_repair_fail" => "用户目录权限修复失败",
                "opt_startup_disk_success" => "启动盘校验完成，未发现问题",
                "opt_startup_disk_fail" => "启动盘校验失败或发现问题",
                "optimize_confirm_title" => "高风险维护任务",
                "optimize_confirm_body" => "该任务会修改用户级设置，可能需要一段时间。是否继续？",
                "optimize_confirm_yes" => "继续",
                "optimize_confirm_no" => "取消",
                // Windows 优化任务
                "optimize_win_dns_flush" => "DNS 缓存刷新",
                "optimize_win_temp_cleanup" => "临时文件清理",
                "optimize_win_disable_telemetry" => "关闭遥测",
                "optimize_win_disable_copilot" => "禁用 Copilot",
                "optimize_win_disable_suggestions" => "关闭广告与建议",
                "optimize_win_startup_audit" => "启动项审计",
                "optimize_win_disable_fast_startup" => "关闭快速启动",
                "optimize_win_restore_point" => "创建系统还原点",
                "optimize_win_restart_explorer" => "重启资源管理器",
                "optimize_win_trim_drives" => "驱动器优化 (TRIM/碎片整理)",
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
                // Windows 优化任务结果
                "opt_win_dns_success" => "✅ DNS 缓存已刷新",
                "opt_win_dns_fail" => "⚠️ DNS 刷新失败。请在 CMD 执行：ipconfig /flushdns",
                "opt_win_temp_success" => "✅ 已清理 {0} 临时文件",
                "opt_win_temp_fail" => "⚠️ 临时文件清理失败",
                "opt_win_telemetry_success" => "✅ 遥测已关闭（注册表策略已设置）",
                "opt_win_telemetry_fail" => "⚠️ 关闭遥测失败，请以管理员身份运行",
                "opt_win_copilot_success" => "✅ Copilot 已禁用（注册表策略已设置）",
                "opt_win_copilot_fail" => "⚠️ 禁用 Copilot 失败，请以管理员身份运行",
                "opt_win_suggestions_success" => "✅ 广告与建议已关闭",
                "opt_win_suggestions_fail" => "⚠️ 关闭建议失败，请以管理员身份运行",
                "opt_win_startup_opened" => "✅ 已打开任务管理器 > 启动",
                "opt_win_startup_fail" => "⚠️ 无法打开任务管理器",
                "opt_win_faststartup_success" => "✅ 快速启动已关闭",
                "opt_win_faststartup_fail" => "⚠️ 关闭快速启动失败，请以管理员身份运行",
                "opt_win_restore_success" => "✅ 系统还原点已创建",
                "opt_win_restore_fail" => "⚠️ 创建还原点失败，请以管理员身份运行",
                "opt_win_explorer_success" => "✅ 资源管理器已重启",
                "opt_win_explorer_fail" => "⚠️ 重启资源管理器失败",
                "opt_win_trim_success" => "✅ 驱动器优化已启动",
                "opt_win_trim_fail" => "⚠️ 驱动器优化失败，请以管理员身份运行",
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
                "summary_sip_tip" => "SIP/系统保护: /Library/Developer/CoreSimulator 等路径受 macOS SIP 保护，任何权限都无法删除。此类路径已在扫描时标记为不可删除，无需关闭 SIP。",
                "summary_perm_tip" => "权限不足: 部分文件由 root 拥有或位于受保护目录。点击下方「重试删除（管理员授权）」，应用会自动请求授权后完成删除，无需手动执行命令。",
                "summary_solution_1" => "1. 点击下方「重试删除（管理员授权）」：应用自动授权并重试删除失败项",
                "summary_solution_2" => "2. 系统保护路径（SIP）无法删除属正常现象：应用不会移除 macOS 系统文件",
                "summary_solution_3" => "3. 重试后仍无法删除的项目：可在对应 Tab 取消勾选后继续使用，或检查文件是否被占用",
                "summary_open_settings" => "打开系统设置",
                "summary_fail_list" => "失败列表:",
                "summary_copy_paths" => "复制路径",
                "summary_copy_sudo" => "复制 sudo 命令",
                "summary_retry_delete" => "重试删除（管理员授权）",
                "log_protected_root_rejected" => "受保护的系统根目录，拒绝删除：{}",
                "summary_retry_hint" => "应用会自动请求管理员授权重试删除，无需手动执行命令。",
                "fail_reason_perm" => "权限不足",
                "fail_reason_sip" => "受 SIP/系统保护",
                "fail_reason_inuse" => "文件正被其他进程占用",
                "fail_reason_immutable" => "文件被锁定（immutable 标志）",
                "fail_reason_other" => "其他原因",
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
                "log_trash_failed" => "无法移入废纸篓（请在系统设置中授予「自动化」权限），已保留原文件：{}",
                "log_sudo_rejected" => "sudo 删除前被安全校验拦截：{} — {}",
                "log_sudo_symlink_rejected" => "sudo 删除前被拦截（路径已变为符号链接）：{}",
                "log_sudo_unsafe_path" => "sudo 删除前被拦截（路径含不安全字符）：{}",
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

    /// 格式化多语言文本（支持 {0}, {1}, {2} 与无索引 {} 两种占位符）
    ///
    /// 用于需要动态参数的 UI 文本，例如 "成功删除 {} 项" 或 "{0} items, total {1}"。
    /// - 无索引 `{}`：按出现顺序依次替换为 args 中的值（第 1 个 `{}` 用 args[0]，以此类推）
    /// - 带索引 `{i}`：替换为 args[i]
    ///
    /// 两种占位符可混用；args 不足时多余的占位符保持原样。
    pub fn tf(&self, key: &str, args: &[&str]) -> String {
        Self::tf_lang(self.lang_en, key, args)
    }

    /// 静态版本，供后台线程等无法访问 `&App` 的地方使用
    pub fn tf_lang(lang_en: bool, key: &str, args: &[&str]) -> String {
        let mut s = Self::t_lang(lang_en, key).to_string();
        // 无索引占位符 `{}`：按出现顺序消费 args。逐个替换而不是整体 replace，
        // 避免模板中出现多个 `{}` 时全部被替换成第一个参数。
        for arg in args {
            if let Some(pos) = s.find("{}") {
                s.replace_range(pos..pos + 2, arg);
            } else {
                break;
            }
        }
        // 带索引占位符 `{i}`：按索引替换
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scanner::{Recommend, ScanItem};

    fn item(deletable: bool) -> ScanItem {
        ScanItem {
            path: "/tmp/x".to_string(),
            size_bytes: 1024,
            category: "test".to_string(),
            selected: true,
            deletable,
            undeletable_reason: String::new(),
            recommend: Recommend::Safe,
            description: String::new(),
            batch_paths: Vec::new(),
        }
    }

    /// 回归：无索引 `{}` 与带索引 `{0}` 两种占位符都必须被替换。
    /// 历史 bug：tf_lang 只替换 `{i}`，导致 summary_success 等 40+ 条
    /// 文案运行时显示字面 `{}`（"成功删除 {} 项" 里的计数一直没回写）。
    #[test]
    fn tf_lang_replaces_both_placeholder_styles() {
        // 无索引：单个参数
        assert_eq!(
            App::tf_lang(true, "summary_success", &["42"]),
            "Successfully deleted 42 items"
        );
        // 无索引：多个参数按出现顺序消费
        assert_eq!(
            App::tf_lang(true, "summary_backup", &["3", "5"]),
            "Deletion log saved (3 of 5 items can be restored from Trash)"
        );
        // 带索引：按索引替换
        assert_eq!(
            App::tf_lang(true, "items_total_size", &["12", "34 MB"]),
            "12 items, total 34 MB"
        );
        // 混合场景不存在于现有文案，但保证两种占位符互不干扰：
        // 无索引占位符只消费 args[0]，带索引按编号消费
        assert_eq!(
            App::tf_lang(true, "finish_summary", &["7", "2"]),
            "Cleanup completed: 7 succeeded, 2 failed"
        );
    }

    // -----------------------------------------------------------------
    //  Tab 平台可用性（W-3）
    // -----------------------------------------------------------------

    #[test]
    fn apfs_tab_exists_only_on_macos() {
        assert_eq!(Tab::Apfs.is_supported(), cfg!(target_os = "macos"));
        // 其它 Tab 必须两平台都可用，否则 Windows 会多出第二个空页签
        for t in [
            Tab::Overview,
            Tab::DevCache,
            Tab::LargeFiles,
            Tab::AppCache,
            Tab::AppData,
            Tab::AppUninstall,
            Tab::SystemOptimize,
            Tab::Settings,
        ] {
            assert!(t.is_supported(), "{:?} 不应被平台过滤", t);
        }
    }

    // -----------------------------------------------------------------
    //  M-2：删除备份清单
    // -----------------------------------------------------------------

    #[test]
    fn confirm_delete_carries_size_bytes_into_the_delete_batch() {
        // 清单里的"删了多大"完全依赖这个字段。少带它，清单只能写 0，
        // 用户事后看清单会以为自己什么都没删。
        let mut app = App::new();
        let mut it = item(true);
        it.size_bytes = 42 * 1024 * 1024;
        app.results[1].push(it);
        app.pending_delete.push((1, 0));
        let batch = app.confirm_delete();
        assert_eq!(batch.len(), 1);
        assert_eq!(batch[0].4, 42 * 1024 * 1024);
        // 新一轮删除必须清掉上一轮清单，否则摘要会拿旧清单说这次的事
        assert!(app.last_backup.is_none());
    }

    #[test]
    fn delete_batch_stays_a_five_field_tuple() {
        // 钉住签名：(path, category, batch_paths, use_trash, size_bytes)。
        // 有人改回 4 元组，编译能过，但清单从此拿不到大小 —— 静默退化，
        // 只能靠源码断言拦住。
        assert!(include_str!("app.rs").contains(
            "pub fn confirm_delete(&mut self) -> Vec<(String, String, Vec<String>, bool, u64)>"
        ));
    }

    #[test]
    fn delete_pipeline_writes_a_manifest_and_reports_it_back() {
        // 反向验证的对象是"接线"本身：backup::record 存在不等于被调用。
        // 拆掉 start_delete 里的 record / BackupRecorded 任一段，这条就红。
        let ops = include_str!("ops/mod.rs");
        assert!(
            ops.contains("crate::backup::record("),
            "删除链路必须写备份清单"
        );
        assert!(
            ops.contains("DeleteMessage::BackupRecorded"),
            "清单 id 必须回传给 UI"
        );
        assert!(
            include_str!("ui/mod.rs").contains("Ok(DeleteMessage::BackupRecorded"),
            "UI 必须消费清单消息，否则写了也没人看得到"
        );
    }

    #[test]
    fn tab_indices_stay_aligned_with_results() {
        // results 是 [Vec<ScanItem>; 10]，按 Tab::all() 的下标寻址。
        // 平台过滤只能"跳过显示"，绝不能改变 all() 的长度或顺序，
        // 否则会删错 Tab 的数据。
        assert_eq!(Tab::all().len(), 11);
        // results 是定长数组，长度写死在类型里；用源码钉住两者必须一致。
        // 一旦有人往 Tab 加变体却忘了扩 results，下标就会静默错位。
        assert!(
            include_str!("app.rs").contains("pub results: [Vec<ScanItem>; 11],"),
            "results 数组长度变了，必须同步 Tab::all()"
        );
    }

    #[test]
    fn next_tab_never_lands_on_an_unsupported_tab() {
        let mut t = Tab::Overview;
        for _ in 0..Tab::all().len() {
            t = t.next();
            assert!(t.is_supported(), "next() 落在了不支持的 Tab: {:?}", t);
        }
        // 绕一圈必须回到起点：证明跳过逻辑没有把环走死
        assert_eq!(t, Tab::Overview);
    }

    #[test]
    fn scannable_tabs_exclude_unsupported_and_overview() {
        let scannable = Tab::scannable();
        assert!(scannable.iter().all(|(t, _)| t.is_supported()));
        assert!(!scannable.iter().any(|(t, _)| *t == Tab::Overview));
        // all() 11 个里跳过 Overview，剩 10 个；非 macOS 再去掉 Apfs → 9 个
        let expected = if cfg!(target_os = "macos") { 10 } else { 9 };
        assert_eq!(scannable.len(), expected, "实际: {:?}", scannable);
        // 下标必须是 all() 里的真实下标，不能是过滤后重排的序号
        for (t, idx) in &scannable {
            assert_eq!(Tab::all()[*idx], *t);
        }
    }

    #[test]
    fn sidebar_skips_unsupported_tabs() {
        // 源码级：侧栏必须过滤，否则 Windows 上会渲染一个永远为空的页签。
        let src = include_str!("ui/mod.rs");
        let region = src[src.find("导航项列表").expect("找不到侧栏")..]
            .split("\n                }\n")
            .next()
            .unwrap();
        assert!(region.contains("is_supported()"), "侧栏没有按平台过滤 Tab");
    }

    // -----------------------------------------------------------------
    //  多磁盘扫描设置（W-2）
    // -----------------------------------------------------------------

    #[test]
    fn scan_all_disks_defaults_to_c_drive_only() {
        // 默认只扫 C 盘。开全盘会更慢，也会把其它系统盘的 Program Files
        // 一起算进来 —— 属于"用户明确要求才开"的开关。
        assert!(!crate::config::AppConfig::default().settings_scan_all_disks);
    }

    #[test]
    fn scan_all_disks_setting_reaches_the_scanner() {
        // 之前 set_scan_all_disks / list_available_disks 就是"写了但没人调"：
        // 扫描器内部确实读 should_scan_all_disks()，可没有任何地方去 set，
        // 于是静态量永远是 false。光在 UI 上加开关不够，必须真的推过去。
        let src = include_str!("app.rs");
        let new_body = src[src.find("pub fn new() -> Self {").expect("App::new")..]
            .split("\n    /// 获取当前 Tab 索引")
            .next()
            .unwrap();
        assert!(
            new_body.contains("apply_scan_all_disks("),
            "App::new 没有把设置推给扫描器，上次的选择会失效"
        );
        let save_body = src[src.find("pub fn save_settings(").expect("save_settings")..]
            .split("\n    pub fn theme_mode(")
            .next()
            .unwrap();
        assert!(
            save_body.contains("apply_scan_all_disks("),
            "save_settings 没有把设置推给扫描器，要等重启才生效"
        );
    }

    #[test]
    fn scan_all_disks_change_is_written_back_to_disk() {
        // 设置页的"变更自动保存"靠对比两个快照元组（入口 + 离开设置页时）。
        // 两处都得带上新字段，漏一处开关就写不回盘。
        let src = include_str!("ui/mod.rs");
        // 1 处渲染 + 2 处快照元组
        assert_eq!(
            src.matches("app.settings_scan_all_disks,").count(),
            3,
            "设置项或快照元组漏了 settings_scan_all_disks"
        );
    }

    #[test]
    fn applying_scan_all_disks_outside_windows_is_a_noop() {
        // 非 Windows 上该设置无意义，但调用不得 panic（cfg 分支里已处理）
        apply_scan_all_disks(true);
        apply_scan_all_disks(false);
    }

    // -----------------------------------------------------------------
    //  M-1 · 官方卸载器（macOS）
    // -----------------------------------------------------------------

    /// 造一个自带官方卸载器的 .app 包
    fn make_app_bundle_with_uninstaller(tag: &str) -> std::path::PathBuf {
        let base = std::env::temp_dir().join(format!("maclean-{}-{}", tag, std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let app_dir = base.join("Foo.app");
        let helpers = app_dir.join("Contents").join("Helpers");
        std::fs::create_dir_all(&helpers).unwrap();
        let exe = helpers.join("uninstall_foo.sh");
        std::fs::write(&exe, "#!/bin/sh\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        base
    }

    #[test]
    fn confirm_delete_registers_apps_with_an_official_uninstaller() {
        // 真实文件树：证明"识别 → 确认弹窗"整条链路通了，而不只是纯函数对。
        // 这条断掉的话，用户点了确认才被弹一个陌生窗口 —— 必须提前告知。
        let base = make_app_bundle_with_uninstaller("m1-on");
        let mut app = App::new();
        app.settings_prefer_official_uninstaller = true;
        let mut it = item(true);
        it.path = base.join("Foo.app").to_string_lossy().to_string();
        app.results[1].push(it);
        app.pending_delete.push((1, 0));
        let _ = app.confirm_delete();
        assert_eq!(
            app.official_handoffs.len(),
            1,
            "识别到官方卸载器就必须登记，否则确认弹窗不会告知用户"
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn turning_the_setting_off_disables_the_handoff() {
        // 反向验证：开关关掉必须真的不走这条路，不能只改 UI 文案。
        let base = make_app_bundle_with_uninstaller("m1-off");
        let mut app = App::new();
        app.settings_prefer_official_uninstaller = false;
        let mut it = item(true);
        it.path = base.join("Foo.app").to_string_lossy().to_string();
        app.results[1].push(it);
        app.pending_delete.push((1, 0));
        let _ = app.confirm_delete();
        assert!(app.official_handoffs.is_empty());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn official_uninstaller_preference_reaches_the_delete_thread() {
        // 设置项到位 ≠ 删除线程收到。拆掉任一个调用点，这条就红。
        let ops = include_str!("ops/mod.rs");
        assert!(
            ops.contains("prefer_official_uninstaller: bool,"),
            "start_delete 没有接收该偏好"
        );
        assert!(
            ops.contains("if prefer_official_uninstaller && path.ends_with(\".app\")"),
            "删除线程没有真的去查官方卸载器"
        );
        // 5 处 = 2 个删除出口 + 1 处设置开关渲染 + 2 处变更快照元组。
        // 两个删除出口都必须传：漏一个，那条路径上的设置就是死的。
        assert_eq!(
            include_str!("ui/mod.rs")
                .matches("app.settings_prefer_official_uninstaller,")
                .count(),
            5,
            "删除出口 / 设置项 / 快照元组漏了 settings_prefer_official_uninstaller"
        );
    }

    // -----------------------------------------------------------------
    //  C-3 · 定时清理
    // -----------------------------------------------------------------

    #[test]
    fn scheduled_cleanup_defaults_to_off() {
        // 默认关：没人设过的东西不该自己动用户的文件
        let cfg = crate::config::AppConfig::default();
        assert!(!cfg.schedule_enabled);
        assert_eq!(cfg.schedule_interval_days, 7);
        assert_eq!(cfg.schedule_last_run, 0);
    }

    #[test]
    fn scheduling_is_gated_by_the_plan_tier() {
        // C-1 定义了能力，C-3 必须真的去查它 —— 否则分级又只是一个标签。
        // 只看非测试部分：否则这条用例自己的字面量会被 include_str 抓到，
        // 变成永真的自我指涉断言。
        let body = include_str!("app.rs")
            .split("\n#[cfg(test)]")
            .next()
            .unwrap_or("");
        assert!(
            body.contains("current_tier().allows_scheduled_cleanup()"),
            "定时清理没有按等级判定，C-1 的能力形同虚设"
        );
    }

    // -----------------------------------------------------------------
    //  C-2 · 自动更新
    // -----------------------------------------------------------------

    #[test]
    fn update_download_refuses_to_start_without_a_matching_asset() {
        // 没有匹配本机的产物时必须返回 false，让 UI 退回"打开 Release 页面"。
        // 若这里也返回 true，用户会点到一个永远不动的"下载中"按钮。
        let mut app = App::new();
        assert!(!app.start_update_download()); // update_available 为空
        app.update_available = Some(crate::updater::UpdateInfo {
            version: "9.9.9".to_string(),
            url: "https://example.com".to_string(),
            notes: String::new(),
            assets: Vec::new(),
            asset: None,
        });
        assert!(!app.start_update_download());
        assert!(!app.update_downloading);
    }

    #[test]
    fn a_dead_update_thread_does_not_freeze_the_banner() {
        // 反向验证：通道断开必须收尾。线程 panic 后若 downloading 一直是 true，
        // 横幅按钮会永久禁用 —— 用户再也无法重试。
        let mut app = App::new();
        let (tx, rx) = std::sync::mpsc::channel::<Result<String, String>>();
        app.update_dl_rx = Some(rx);
        app.update_downloading = true;
        drop(tx); // 发送端释放 == 线程已死
        app.poll_update_download();
        assert!(!app.update_downloading);
        assert!(app.update_result.is_some());
    }

    #[test]
    fn the_update_banner_really_downloads_instead_of_just_opening_a_page() {
        // 钉住源码：以前横幅只有 open_url 一条路，"检查更新"名不副实。
        let ui = include_str!("ui/mod.rs");
        assert!(
            ui.contains("app.start_update_download()"),
            "更新横幅没有接下载入口，C-2 等于没做"
        );
        // 没有产物时要能退回跳网页，不能点了没反应。
        // 只断言"两者都出现且相邻"，不钉具体缩进 —— 钉缩进会被单纯的格式
        // 调整打断，那是噪音不是回归。
        let at = ui.find("app.start_update_download()").expect("无下载入口");
        // 按字符截取而不是按字节：源码里有中文注释，按字节切会踩到
        // 多字节字符边界直接 panic
        let tail: String = ui[at..].chars().take(200).collect();
        assert!(
            tail.contains("open_url(&info.url)"),
            "缺少无产物时退回打开 Release 页面的分支"
        );
    }

    #[test]
    fn delete_gate_drops_protected_items() {
        // 这是核心断言：受保护项即使 selected = true 也不得进入删除队列。
        // UI 复选框大多按 deletable 禁用，但那不是闸门 —— 程序化入口
        // （one_click_clean 无条件置 selected = true）不经过复选框。
        let results = vec![vec![item(true), item(false), item(true)]];
        let out = filter_deletable(&results, &[(0, 0), (0, 1), (0, 2)]);
        assert_eq!(out, vec![(0, 0), (0, 2)]);
    }

    #[test]
    fn delete_gate_rejects_stale_indices() {
        // 扫描结果被换掉后，旧的 (tab, item) 索引可能越界或指向另一项。
        // 宁可不删，也不能删到重新扫描后同一位置上的别的东西。
        let results = vec![vec![item(true)]];
        assert!(filter_deletable(&results, &[(0, 9)]).is_empty());
        assert!(filter_deletable(&results, &[(7, 0)]).is_empty());
    }

    #[test]
    fn delete_gate_allows_everything_when_unprotected() {
        let results = vec![vec![item(true), item(true)]];
        assert_eq!(
            filter_deletable(&results, &[(0, 0), (0, 1)]),
            vec![(0, 0), (0, 1)]
        );
    }

    #[test]
    fn both_delete_entrypoints_share_one_gate() {
        // 两个出口曾经规则不一致（prepare_delete 不过滤 deletable，
        // prepare_delete_cross_tab 才过滤）。用源码钉住：现在都必须调用
        // filter_deletable，否则保护在最常用那条路径上就是没接。
        let src = include_str!("app.rs");
        for entry in ["fn prepare_delete(", "fn prepare_delete_cross_tab("] {
            let body = src[src.find(entry).expect(entry)..]
                .split("\n    pub fn ")
                .next()
                .unwrap();
            assert!(
                body.contains("filter_deletable("),
                "{} 没有走统一闸门，规则会与另一个出口漂移",
                entry
            );
        }
    }

    /// 回归：sudo 阶段启动前清空 failed_paths，避免阶段1失败计数重复计入最终弹窗
    /// （用户场景：阶段1失败 59 项 → sudo 重试成功 30、再失败 29 → 弹窗必须显示 29 而非 88）
    #[test]
    fn sudo_retry_does_not_double_count_failures() {
        let mut app = App::new();
        app.confirm = ConfirmState::None;
        app.pending_delete.clear();

        // 模拟阶段1：59 项失败日志 → failed_paths = 59
        for i in 0..59 {
            app.receive_delete_log(
                format!("✗ fail {}", i),
                format!("/tmp/f{}", i),
                "test".to_string(),
                false,
            );
        }
        assert_eq!(app.failed_paths.len(), 59);

        // sudo 阶段启动点：先清空阶段1失败计数（与 src/ui/mod.rs 中
        // start_sudo_delete / start_sudo_delete_touchid 调用前的 clear 对齐）
        app.failed_paths.clear();

        // 模拟 sudo 阶段：成功 30、再失败 29
        for i in 0..30 {
            app.receive_delete_log(
                format!("✓ ok {}", i),
                format!("/tmp/s{}", i),
                "test".to_string(),
                true,
            );
        }
        for i in 0..29 {
            app.receive_delete_log(
                format!("✗ still fail {}", i),
                format!("/tmp/r{}", i),
                "test".to_string(),
                false,
            );
        }

        // 收尾统计：成功 = 阶段1成功(0) + sudo 成功(30)；失败 = 仅 sudo 再失败(29)
        app.finish_delete();
        let (ok, fail, _) = app.delete_summary.expect("应有汇总");
        assert_eq!(ok, 30, "成功计数应含 sudo 阶段成功的项");
        assert_eq!(fail, 29, "失败计数不得重复计入阶段1的 59 项");
        assert_eq!(app.failed_paths.len(), 29, "failed_paths 只保留真实失败");
    }
}
