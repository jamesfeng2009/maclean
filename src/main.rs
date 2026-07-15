//! maclean - macOS 磁盘清理 GUI 工具
//!
//! 使用 egui 构建，专注开发者缓存与深度清理。

#[cfg(target_os = "macos")]
mod aewp;
mod app;
mod app_protection;
mod cli;
mod i18n;
mod logger;
mod menubar;
mod platform;
mod safety;
mod scanner;
#[cfg(target_os = "macos")]
mod sudo_keepalive;
#[cfg(target_os = "macos")]
mod touchid;

use std::sync::mpsc;

use eframe::egui;

use app::{App, ConfirmState, ScanState, Tab};
use scanner::{format_size, Recommend, ScanItem, Scanner};

// =========================================================================
//  颜色常量（浅色 macOS 风格）
// =========================================================================

// 品牌色系
const BRAND: egui::Color32 = egui::Color32::from_rgb(75, 63, 227); // #4B3FE3
const BRAND_SOFT: egui::Color32 = egui::Color32::from_rgb(242, 247, 255); // #F2F7FF
const BRAND_TEXT: egui::Color32 = egui::Color32::from_rgb(26, 23, 89); // #1A1759
                                                                       // 表面色
const SURFACE: egui::Color32 = egui::Color32::from_rgb(247, 247, 248); // #F7F7F8
const SURFACE_ELEVATED: egui::Color32 = egui::Color32::from_rgb(255, 255, 255); // #FFFFFF
const SURFACE_MUTED: egui::Color32 = egui::Color32::from_rgb(239, 239, 242); // #EFEFF2
const SURFACE_SIDEBAR: egui::Color32 = egui::Color32::from_rgb(242, 242, 245); // #F2F2F5
                                                                               // 文本色
const TEXT_PRIMARY: egui::Color32 = egui::Color32::from_rgb(23, 23, 23); // #171717
const TEXT_SECONDARY: egui::Color32 = egui::Color32::from_rgb(82, 82, 91); // #52525B
const TEXT_TERTIARY: egui::Color32 = egui::Color32::from_rgb(113, 113, 122); // #71717A
                                                                             // 边框
const BORDER_LIGHT: egui::Color32 = egui::Color32::from_rgba_premultiplied(23, 23, 23, 26); // ~0.1 alpha
const BORDER_STRONG: egui::Color32 = egui::Color32::from_rgba_premultiplied(23, 23, 23, 46); // ~0.18 alpha
                                                                                             // 状态色
const SAFE_COLOR: egui::Color32 = egui::Color32::from_rgb(29, 201, 129); // #1DC981
const CAUTION_COLOR: egui::Color32 = egui::Color32::from_rgb(239, 170, 23); // #EFAA17
const ADVANCED_COLOR: egui::Color32 = egui::Color32::from_rgb(232, 70, 58); // #E8463A
const DANGER_COLOR: egui::Color32 = egui::Color32::from_rgb(232, 70, 58); // #E8463A

/// 选中项卡片背景色（浅绿）
const SELECTED_CARD_BG: egui::Color32 = egui::Color32::from_rgb(240, 255, 244); // #F0FFF4

/// 列表区背景色（极浅灰，内容少时避免刺眼空白）
const LIST_BG: egui::Color32 = egui::Color32::from_rgb(250, 250, 250); // #FAFAFA

/// 后台扫描消息
enum ScanMessage {
    /// 扫描进度更新
    Progress(f32),
    /// 增量结果（扫描中部分项）— (items, tab_index)
    PartialItems(Vec<ScanItem>, u64),
    /// 当前扫描路径
    CurrentPath(String),
    /// 单个 Tab 扫描完成
    Done(Vec<ScanItem>, u64, u64), // (items, scan_time_ms, tab_index)
    /// 全部扫描完成（用于批量扫描）
    AllDone,
}

/// 后台删除消息
enum DeleteMessage {
    /// 单项删除结果（日志, 路径, 类别, 是否成功）
    Log(String, String, String, bool),
    /// 进度信息（不计入成功/失败统计）
    Info(String),
    /// 普通删除完成，部分项需要管理员权限
    NeedPassword(Vec<(String, String)>),
    /// 全部删除完成
    Done,
    /// Windows: 卸载后检测到残留，弹出残留清理弹窗
    #[cfg(target_os = "windows")]
    ResidualFound(scanner::windows_apps::UninstallResidual),
    /// Windows: 残留清理完成
    #[cfg(target_os = "windows")]
    ResidualCleaned(usize, usize, usize), // (注册表, 环境变量, 文件)
}

/// 写入扫描日志（用于追踪扫描进度，崩溃时定位问题）
pub fn log_scan_step(msg: &str) {
    logger::info(msg);
}

fn main() -> eframe::Result {
    // 初始化日志系统（CLI 和 GUI 模式都需要）
    logger::init();

    // CLI 模式：有子命令时执行并退出，无子命令时启动 GUI
    if cli::run_cli() {
        logger::info("CLI 模式执行完毕，退出");
        return Ok(());
    }

    // GUI 模式
    logger::info("GUI 模式启动");

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([960.0, 680.0])
            .with_min_inner_size([760.0, 540.0])
            .with_title("Maclean"),
        ..Default::default()
    };

    eframe::run_simple_native("Maclean", options, move |ctx, _frame| {
        static mut APP: Option<App> = None;
        static mut SCAN_RX: Option<mpsc::Receiver<ScanMessage>> = None;
        static mut DELETE_RX: Option<mpsc::Receiver<DeleteMessage>> = None;
        static mut MENUBAR: Option<menubar::MenuBarHud> = None;
        static mut NEEDS_INIT: bool = true;
        static mut LAST_DISK_UPDATE: f64 = 0.0;
        // QuickClean 标志：扫描完成后自动选择 Safe 项并删除
        static mut AUTO_CLEAN_AFTER_SCAN: bool = false;

        /// 渲染菜单栏 HUD 悬浮窗口
        fn render_hud_window(ctx: &egui::Context, app: &mut App) {
            if !app.hud_open {
                return;
            }

            let frame = egui::Frame::none()
                .fill(SURFACE_ELEVATED)
                .stroke(egui::Stroke::new(1.0, BORDER_LIGHT))
                .rounding(egui::Rounding::same(12.0))
                .inner_margin(egui::Margin::same(12.0));

            let mut action_to_run: Option<HudAction> = None;

            egui::Window::new("maclean_hud")
                .title_bar(false)
                .collapsible(false)
                .resizable(false)
                .movable(false)
                .fixed_pos([700.0, 32.0])
                .default_width(220.0)
                .frame(frame)
                .show(ctx, |ui| {
                    ui.set_min_width(196.0);

                    // Header
                    ui.horizontal(|ui| {
                        ui.colored_label(
                            TEXT_PRIMARY,
                            egui::RichText::new("maclean").size(14.0).strong(),
                        );
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            let size_str = format_size(app.total_releasable_size());
                            ui.colored_label(
                                BRAND,
                                egui::RichText::new(format!("可释放 {}", size_str))
                                    .size(13.0)
                                    .strong()
                                    .monospace(),
                            );
                        });
                    });

                    ui.add_space(8.0);
                    ui.separator();
                    ui.add_space(4.0);

                    // 绘制可点击的 HUD 动作行
                    let row = |ui: &mut egui::Ui, icon: &str, text: &str| -> bool {
                        let label = format!("{} {}", icon, text);
                        let galley = ui.painter().layout_no_wrap(
                            label.clone(),
                            egui::FontId::new(13.0, egui::FontFamily::Proportional),
                            TEXT_PRIMARY,
                        );
                        let padding = egui::vec2(8.0, 8.0);
                        let desired_size = galley.size() + padding * 2.0;
                        let (rect, response) =
                            ui.allocate_exact_size(desired_size, egui::Sense::click());
                        if ui.is_rect_visible(rect) {
                            if response.hovered() {
                                ui.painter().rect_filled(
                                    rect,
                                    egui::Rounding::same(6.0),
                                    SURFACE_MUTED,
                                );
                            }
                            ui.painter().galley(
                                egui::pos2(rect.min.x + padding.x, rect.min.y + padding.y),
                                galley,
                                TEXT_PRIMARY,
                            );
                        }
                        response.clicked()
                    };

                    if row(ui, "🚀", "快速扫描") {
                        action_to_run = Some(HudAction::QuickScan);
                    }
                    if row(ui, "🗑", "清理 Safe 项目") {
                        action_to_run = Some(HudAction::QuickClean);
                    }
                    if row(ui, "📦", "打开主窗口") {
                        action_to_run = Some(HudAction::ShowWindow);
                    }
                    if row(ui, "⚙️", "设置") {
                        action_to_run = Some(HudAction::Settings);
                    }
                    if row(ui, "⏏", "退出") {
                        action_to_run = Some(HudAction::Quit);
                    }
                });

            // 在窗口绘制结束后再修改状态，避免即时模式借用问题
            match action_to_run {
                Some(HudAction::QuickScan) => {
                    app.tab = Tab::Overview;
                    app.hud_open = false;
                    ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                    ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
                    ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                    unsafe {
                        AUTO_CLEAN_AFTER_SCAN = false;
                        if !app
                            .scan_states
                            .iter()
                            .any(|s| matches!(s, ScanState::Scanning))
                        {
                            start_scan_all(app, &mut SCAN_RX);
                        }
                    }
                }
                Some(HudAction::QuickClean) => {
                    app.hud_open = false;
                    ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                    ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
                    ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                    app.tab = Tab::DevCache;
                    unsafe {
                        if !matches!(app.current_scan_state(), ScanState::Scanning) {
                            AUTO_CLEAN_AFTER_SCAN = true;
                            start_scan(app, &mut SCAN_RX);
                        }
                    }
                }
                Some(HudAction::ShowWindow) => {
                    app.hud_open = false;
                    ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                    ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
                    ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                }
                Some(HudAction::Settings) => {
                    app.tab = Tab::Settings;
                    app.hud_open = false;
                    ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                    ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
                    ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                }
                Some(HudAction::Quit) => {
                    std::process::exit(0);
                }
                None => {}
            }
        }

        #[derive(Debug, Clone, Copy)]
        enum HudAction {
            QuickScan,
            QuickClean,
            ShowWindow,
            Settings,
            Quit,
        }

        unsafe {
            if NEEDS_INIT {
                APP = Some(App::new());
                NEEDS_INIT = false;
                setup_fonts(ctx);

                // 设置浅色主题
                let mut visuals = egui::Visuals::light();
                visuals.window_rounding = egui::Rounding::same(10.0);
                visuals.widgets.noninteractive.rounding = egui::Rounding::same(8.0);
                visuals.widgets.hovered.rounding = egui::Rounding::same(8.0);
                visuals.widgets.active.rounding = egui::Rounding::same(8.0);
                ctx.set_visuals(visuals);

                // 初始化菜单栏 HUD
                {
                    MENUBAR = Some(menubar::MenuBarHud::new());
                    if let Some(ref mut mb) = MENUBAR {
                        mb.init();
                    }
                }
            }

            // 轮询菜单栏事件
            let mut hud_toggle = false;
            if let Some(ref mb) = MENUBAR {
                let actions = mb.poll_events();
                if !actions.is_empty() {
                    let lang_en = APP.as_ref().map(|a| a.lang_en).unwrap_or(false);
                    log_scan_step(&App::tf_lang(
                        lang_en,
                        "log_menu_event",
                        &[&format!("{:?}", actions)],
                    ));
                }
                for action in actions {
                    match action {
                        menubar::TrayAction::ShowWindow => {
                            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                            ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                        }
                        menubar::TrayAction::QuickScan => {
                            // 快速扫描：只扫描不删除
                            AUTO_CLEAN_AFTER_SCAN = false;
                            if let Some(app) = &mut APP {
                                if !matches!(app.current_scan_state(), ScanState::Scanning) {
                                    ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                                    start_scan(app, &mut SCAN_RX);
                                }
                            }
                        }
                        menubar::TrayAction::QuickClean => {
                            // 一键清理：扫描 + 自动删除所有 Safe 项
                            // 1. 显示并聚焦主窗口
                            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                            ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
                            ctx.send_viewport_cmd(egui::ViewportCommand::Focus);

                            // 2. 切换到开发者缓存 Tab 并开始扫描
                            if let Some(app) = &mut APP {
                                app.tab = Tab::DevCache;
                                let state = app.current_scan_state().clone();
                                if !matches!(state, ScanState::Scanning) {
                                    // 设置标志：扫描完成后自动选择 Safe 项并删除
                                    AUTO_CLEAN_AFTER_SCAN = true;
                                    start_scan(app, &mut SCAN_RX);
                                }
                            }
                        }
                        menubar::TrayAction::Quit => {
                            std::process::exit(0);
                        }
                    }
                }

                // 点击托盘图标：展开/收起 HUD
                if mb.poll_click() {
                    hud_toggle = true;
                }
            }
            if hud_toggle {
                if let Some(app) = &mut APP {
                    app.hud_open = !app.hud_open;
                }
            }

            // 定期更新托盘显示的可释放空间（每 60 秒，或数值变化时由 update_releasable 内部节流）
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs_f64())
                .unwrap_or(0.0);
            if now - LAST_DISK_UPDATE > 60.0 || LAST_DISK_UPDATE == 0.0 {
                LAST_DISK_UPDATE = now;
                if let Some(app) = &APP {
                    let releasable = app.total_releasable_size();
                    let lang_en = app.lang_en;
                    if let Some(ref mut mb) = MENUBAR {
                        mb.update_releasable(releasable, lang_en);
                    }
                }
            }

            // 检查后台扫描结果
            if let Some(rx) = &SCAN_RX {
                loop {
                    match rx.try_recv() {
                        Ok(ScanMessage::Progress(p)) => {
                            if let Some(app) = &mut APP {
                                app.scan_progress = p;
                            }
                        }
                        Ok(ScanMessage::PartialItems(items, tab_idx)) => {
                            // 增量结果：扫描中已发现的部分项，直接追加到当前 Tab 的结果列表
                            if let Some(app) = &mut APP {
                                let idx = tab_idx as usize;
                                app.results[idx].extend(items.clone());
                                // 报告最后一条路径
                                if let Some(last) = items.last() {
                                    let path = if last.path.is_empty() {
                                        last.category.clone()
                                    } else {
                                        last.path.clone()
                                    };
                                    app.scan_current_path = path;
                                }
                            }
                        }
                        Ok(ScanMessage::CurrentPath(path)) => {
                            if let Some(app) = &mut APP {
                                app.scan_current_path = path;
                            }
                        }
                        Ok(ScanMessage::Done(items, time_ms, tab_idx)) => {
                            if let Some(app) = &mut APP {
                                app.results[tab_idx as usize] = items;
                                app.scan_states[tab_idx as usize] = ScanState::Done;
                                app.scan_time_ms[tab_idx as usize] = time_ms;
                                app.scan_progress = 1.0;
                                let (total, free) = get_disk_info();
                                app.disk_total = total;
                                app.disk_free = free;
                            }
                            // 单个 Tab 扫描完成后不 break，继续接收 AllDone 或更多 Done
                        }
                        Ok(ScanMessage::AllDone) => {
                            if let Some(app) = &mut APP {
                                app.scan_progress = 1.0;
                                let (total, free) = get_disk_info();
                                app.disk_total = total;
                                app.disk_free = free;

                                // 一键清理模式：自动选择 Safe 项并删除
                                if AUTO_CLEAN_AFTER_SCAN {
                                    AUTO_CLEAN_AFTER_SCAN = false;
                                    app.select_safe_only();
                                    let selected_count = app.selected_count();
                                    if selected_count > 0 {
                                        log_scan_step(&app.tf(
                                            "log_quickclean_start",
                                            &[&selected_count.to_string()],
                                        ));
                                        app.prepare_delete();
                                        let to_delete = app.confirm_delete();
                                        start_delete(to_delete, app.lang_en, &mut DELETE_RX);
                                    } else {
                                        log_scan_step(app.t("log_quickclean_none"));
                                    }
                                }
                            }
                            SCAN_RX = None;
                            break;
                        }
                        Err(_) => break,
                    }
                }
            }

            // 检查后台删除进度
            if let Some(rx) = &DELETE_RX {
                loop {
                    match rx.try_recv() {
                        Ok(DeleteMessage::Log(log, path, category, success)) => {
                            if let Some(app) = &mut APP {
                                app.receive_delete_log(log, path, category, success);
                            }
                        }
                        Ok(DeleteMessage::Info(info)) => {
                            if let Some(app) = &mut APP {
                                app.logs.push(info);
                            }
                        }
                        Ok(DeleteMessage::NeedPassword(items)) => {
                            if let Some(app) = &mut APP {
                                app.sudo_failed_items = items;
                                app.sudo_password_input.clear();
                                app.sudo_password = None;
                                app.sudo_error = None;
                                app.touch_id_error = None;
                                // 刷新 Touch ID 状态 (macOS 专属)
                                #[cfg(target_os = "macos")]
                                {
                                    app.touch_id_available = touchid::touch_id_available();
                                    app.touch_id_enabled = touchid::sudo_touch_id_enabled();
                                }

                                // 合盖检测：Touch ID 在合盖时不可用，回退到密码输入 (macOS 专属)
                                #[cfg(target_os = "macos")]
                                let clamshell_closed = safety::is_clamshell_closed();
                                #[cfg(not(target_os = "macos"))]
                                let clamshell_closed = false;
                                if clamshell_closed {
                                    app.touch_id_error =
                                        Some(app.t("touchid_clamshell_error").to_string());
                                    app.touch_id_available = false;
                                }

                                if app.touch_id_enabled && !clamshell_closed {
                                    // Touch ID 已启用：直接用 sudo（Touch ID 自动触发）(macOS 专属)
                                    #[cfg(target_os = "macos")]
                                    {
                                        app.confirm = ConfirmState::SudoWithTouchId;
                                        let items = app.sudo_failed_items.clone();
                                        app.delete_done = 0;
                                        app.delete_total = items.len();
                                        start_sudo_delete_touchid(
                                            items,
                                            app.lang_en,
                                            &mut DELETE_RX,
                                        );
                                    }
                                    #[cfg(not(target_os = "macos"))]
                                    {
                                        app.confirm = ConfirmState::NeedSudoPassword;
                                    }
                                } else if app.touch_id_available && !clamshell_closed {
                                    // Touch ID 可用但未启用：提示用户是否启用 (macOS 专属)
                                    #[cfg(target_os = "macos")]
                                    {
                                        app.confirm = ConfirmState::OfferTouchIdSetup;
                                    }
                                    #[cfg(not(target_os = "macos"))]
                                    {
                                        app.confirm = ConfirmState::NeedSudoPassword;
                                    }
                                } else {
                                    // 无 Touch ID 或合盖：走密码输入流程
                                    app.confirm = ConfirmState::NeedSudoPassword;
                                }
                            }
                            DELETE_RX = None;
                            break;
                        }
                        Ok(DeleteMessage::Done) => {
                            if let Some(app) = &mut APP {
                                app.sudo_password = None;
                                app.sudo_password_input.clear();
                                // 刷新 sudo 会话状态（keepalive 可能仍活跃）(macOS 专属)
                                #[cfg(target_os = "macos")]
                                {
                                    app.sudo_session_active = sudo_keepalive::is_sudo_active();
                                }
                                app.finish_delete();
                            }
                            DELETE_RX = None;
                            break;
                        }
                        #[cfg(target_os = "windows")]
                        Ok(DeleteMessage::ResidualFound(residual)) => {
                            if let Some(app) = &mut APP {
                                // 初始化选中状态：所有可删除项默认选中
                                let total = residual.registry.len()
                                    + residual.env_vars.len()
                                    + residual.filesystem.len();
                                let mut selected = Vec::with_capacity(total);
                                for r in &residual.registry {
                                    selected.push(r.deletable);
                                }
                                for e in &residual.env_vars {
                                    selected.push(e.deletable);
                                }
                                for f in &residual.filesystem {
                                    selected.push(f.deletable);
                                }
                                app.residual_selected = selected;
                                app.uninstall_residual = Some(residual);
                                app.show_residual_dialog = true;
                            }
                        }
                        #[cfg(target_os = "windows")]
                        Ok(DeleteMessage::ResidualCleaned(reg_c, env_c, fs_c)) => {
                            if let Some(app) = &mut APP {
                                logger::info(&format!(
                                    "残留清理完成: 注册表 {} 项, 环境变量 {} 项, 文件 {} 项",
                                    reg_c, env_c, fs_c
                                ));
                                app.logs.push(format!(
                                    "✓ 残留清理完成: 注册表 {} 项, 环境变量 {} 项, 文件 {} 项",
                                    reg_c, env_c, fs_c
                                ));
                                app.residual_cleaning = false;
                                app.show_residual_dialog = false;
                                app.uninstall_residual = None;
                                app.residual_selected.clear();
                            }
                        }
                        Err(_) => break,
                    }
                }
            }

            if let Some(app) = &mut APP {
                // 磁盘监控：每 5 秒轮询磁盘空间
                app.poll_disk_space();

                // 请求重绘以保持告警 UI 实时更新
                if app.disk_alert_level().0 >= 2 {
                    ctx.request_repaint();
                }

                render_gui(ctx, app, &mut SCAN_RX, &mut DELETE_RX);
                render_hud_window(ctx, app);
            }
        }
    })
}

/// 加载系统中文字体（跨平台）
fn setup_fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();

    // 按平台选择字体路径
    #[cfg(target_os = "macos")]
    let font_paths = [
        "/System/Library/Fonts/PingFang.ttc",
        "/System/Library/Fonts/STHeiti Medium.ttc",
        "/Library/Fonts/Arial Unicode.ttf",
    ];

    #[cfg(target_os = "windows")]
    let font_paths = [
        "C:\\Windows\\Fonts\\msyh.ttc",   // 微软雅黑
        "C:\\Windows\\Fonts\\msyhbd.ttc", // 微软雅黑粗体
        "C:\\Windows\\Fonts\\simhei.ttf", // 黑体
        "C:\\Windows\\Fonts\\simsun.ttc", // 宋体
    ];

    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let font_paths = [
        "/usr/share/fonts/truetype/noto/NotoSansCJK-Regular.ttc",
        "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
        "/usr/share/fonts/truetype/wqy/wqy-microhei.ttc",
    ];

    for path in &font_paths {
        if let Ok(font_data) = std::fs::read(path) {
            fonts.font_data.insert(
                "CJK".to_owned(),
                egui::FontData::from_owned(font_data.into()),
            );
            fonts
                .families
                .entry(egui::FontFamily::Proportional)
                .or_default()
                .insert(0, "CJK".to_owned());
            fonts
                .families
                .entry(egui::FontFamily::Monospace)
                .or_default()
                .push("CJK".to_owned());
            break;
        }
    }

    ctx.set_fonts(fonts);
}

/// 获取磁盘信息 (macOS 实现)
#[cfg(target_os = "macos")]
fn get_disk_info_macos() -> (u64, u64) {
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

/// 获取磁盘信息 (Windows 实现)
#[cfg(target_os = "windows")]
fn get_disk_info_windows() -> (u64, u64) {
    // Windows: 用 fsutil 或 wmic 获取磁盘信息
    // 这里用 PowerShell 调用 Get-PSDrive
    let output = std::process::Command::new("powershell")
        .arg("-NoProfile")
        .arg("-NonInteractive")
        .arg("-Command")
        .arg("Get-PSDrive C | Select-Object Used,Free | ConvertTo-Json")
        .output();

    if let Ok(output) = output {
        let stdout = String::from_utf8_lossy(&output.stdout);
        // 解析 JSON: {"Used":123,"Free":456}
        let mut used: u64 = 0;
        let mut free: u64 = 0;
        for line in stdout.lines() {
            let line = line.trim();
            if line.starts_with("\"Used\"") {
                if let Some(val) = line.split(':').nth(1) {
                    let val = val.trim().trim_end_matches(',').trim();
                    used = val.parse::<u64>().unwrap_or(0);
                }
            } else if line.starts_with("\"Free\"") {
                if let Some(val) = line.split(':').nth(1) {
                    let val = val.trim().trim_end_matches(',').trim();
                    free = val.parse::<u64>().unwrap_or(0);
                }
            }
        }
        return (used + free, free);
    }

    (0, 0)
}

/// 获取磁盘信息（跨平台入口）
fn get_disk_info() -> (u64, u64) {
    platform::disk_info()
}

/// 推荐等级标签文字
fn recommend_badge(rec: &Recommend) -> &'static str {
    match rec {
        Recommend::Safe => "🟢",
        Recommend::CacheOnly => "🧹",
        Recommend::Caution => "🟡",
        Recommend::Advanced => "🔴",
    }
}

/// 推荐等级标签文字（含多语言文字）
fn recommend_badge_text(rec: &Recommend, lang_en: bool) -> String {
    let icon = recommend_badge(rec);
    format!("{} {}", icon, i18n::translate_recommend(rec, lang_en))
}

/// 从 category 中提取应用名（用于 App卸载 Tab 分组）
///
/// 例如 "WeChat (卸载)" -> Some("WeChat")，"WeChat 数据" -> Some("WeChat")。
/// 如果 category 不符合已知的子项后缀，返回 None。
fn extract_app_name(category: &str) -> Option<&str> {
    const SUFFIXES: &[&str] = &[" (卸载)", " 数据", " 缓存"];
    for suffix in SUFFIXES {
        if let Some(name) = category.strip_suffix(suffix) {
            return Some(name);
        }
    }
    None
}

/// 将 App 卸载子项 category 转换为中文/英文标题
///
/// "... (卸载)" -> "应用本体" / "App Bundle"
/// "... 数据"   -> "应用数据" / "App Data"
/// "... 缓存"   -> "缓存与日志" / "Cache & Logs"
fn uninstall_child_title(category: &str, lang_en: bool) -> String {
    if category.ends_with(" (卸载)") {
        if lang_en {
            "App Bundle".to_string()
        } else {
            "应用本体".to_string()
        }
    } else if category.ends_with(" 数据") {
        if lang_en {
            "App Data".to_string()
        } else {
            "应用数据".to_string()
        }
    } else if category.ends_with(" 缓存") {
        if lang_en {
            "Cache & Logs".to_string()
        } else {
            "缓存与日志".to_string()
        }
    } else {
        i18n::translate_category(category, lang_en)
    }
}

/// 将 App 卸载 Tab 的 ScanItem 按应用名分组
///
/// 输入为过滤后的原始索引列表，输出为 (应用名, 原始索引列表) 的分组列表。
fn build_uninstall_groups(
    items: &[ScanItem],
    filtered_indices: &[usize],
) -> Vec<(String, Vec<usize>)> {
    let mut groups: Vec<(String, Vec<usize>)> = Vec::new();
    let mut group_map: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for &idx in filtered_indices {
        let item = &items[idx];
        if let Some(app_name) = extract_app_name(&item.category) {
            let gidx = *group_map.entry(app_name.to_string()).or_insert_with(|| {
                groups.push((app_name.to_string(), Vec::new()));
                groups.len() - 1
            });
            groups[gidx].1.push(idx);
        } else {
            groups.push((item.category.clone(), vec![idx]));
        }
    }
    groups
}

/// 渲染 App 卸载 Tab 的左侧应用列表面板
fn render_app_uninstall_list_panel(ui: &mut egui::Ui, app: &mut App) {
    let tab_idx = app.tab_index();
    let items = app.results[tab_idx].clone();
    let filtered_indices = app.filtered_indices();
    let groups = build_uninstall_groups(&items, &filtered_indices);

    // 如果当前选中索引越界，重置为未选中
    if let Some(sel) = app.selected_uninstall_app_index {
        if sel >= groups.len() {
            app.selected_uninstall_app_index = None;
        }
    }

    // 顶部过滤输入框
    ui.horizontal(|ui| {
        ui.add_space(4.0);
        let filter_placeholder = app.t("filter_placeholder").to_string();
        let resp = ui.add(
            egui::TextEdit::singleline(&mut app.filter_query)
                .hint_text(&filter_placeholder)
                .desired_width(ui.available_width() - 8.0)
                .min_size([80.0, 28.0].into()),
        );
        if resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            app.clear_filter();
        }
        app.filter_active = resp.has_focus();
        ui.add_space(4.0);
    });
    ui.add_space(4.0);

    egui::ScrollArea::vertical()
        .auto_shrink([false; 2])
        .show(ui, |ui| {
            ui.add_space(6.0);
            if groups.is_empty() {
                ui.vertical_centered(|ui| {
                    ui.add_space(20.0);
                    ui.colored_label(
                        TEXT_TERTIARY,
                        egui::RichText::new(app.t("no_match")).size(12.0),
                    );
                });
            } else {
                for (group_idx, (app_name, indices)) in groups.iter().enumerate() {
                    let total_size: u64 = indices.iter().map(|&i| items[i].size_bytes).sum();
                    let deletable_indices: Vec<usize> = indices
                        .iter()
                        .copied()
                        .filter(|&i| items[i].deletable)
                        .collect();
                    let all_selected = deletable_indices.iter().all(|&i| items[i].selected);
                    let any_selected = deletable_indices.iter().any(|&i| items[i].selected);
                    let has_deletable = !deletable_indices.is_empty();

                    let is_selected = app.selected_uninstall_app_index == Some(group_idx);
                    let row_bg = if is_selected {
                        BRAND_SOFT
                    } else {
                        SURFACE_ELEVATED
                    };
                    let left_stroke = if is_selected {
                        egui::Stroke::new(3.0, BRAND)
                    } else {
                        egui::Stroke::NONE
                    };

                    let row_frame = egui::Frame::none()
                        .fill(row_bg)
                        .stroke(left_stroke)
                        .inner_margin(egui::Margin::symmetric(10.0, 8.0))
                        .rounding(egui::Rounding::same(8.0));

                    let row_resp = row_frame.show(ui, |ui| {
                        ui.horizontal(|ui| {
                            let cb_resp = render_custom_checkbox(
                                ui,
                                all_selected || any_selected,
                                has_deletable,
                            );
                            if cb_resp.clicked() && has_deletable {
                                let target = !all_selected;
                                for &idx in &deletable_indices {
                                    if app.results[tab_idx][idx].selected != target {
                                        app.results[tab_idx][idx].selected = target;
                                    }
                                }
                            }

                            ui.add_space(8.0);

                            ui.vertical(|ui| {
                                ui.set_min_width(100.0);
                                ui.colored_label(
                                    TEXT_PRIMARY,
                                    egui::RichText::new(app_name.as_str()).size(13.0).strong(),
                                );
                                let size_str = if total_size == 0 {
                                    "—".to_string()
                                } else {
                                    format_size(total_size)
                                };
                                ui.colored_label(
                                    TEXT_TERTIARY,
                                    egui::RichText::new(size_str).size(11.0).monospace(),
                                );
                            });
                        });
                    });

                    if row_resp.response.clicked() {
                        app.selected_uninstall_app_index = Some(group_idx);
                    }

                    ui.add_space(4.0);
                }
            }
            ui.add_space(6.0);
        });
}

/// 渲染 App 卸载 Tab 的右侧子项详情面板
fn render_app_uninstall_details_panel(
    ui: &mut egui::Ui,
    app: &mut App,
    scan_rx: &mut Option<mpsc::Receiver<ScanMessage>>,
) {
    let tab_idx = app.tab_index();
    let items = app.results[tab_idx].clone();
    let filtered_indices = app.filtered_indices();
    let groups = build_uninstall_groups(&items, &filtered_indices);

    if let Some(sel) = app.selected_uninstall_app_index {
        if sel >= groups.len() {
            app.selected_uninstall_app_index = None;
        }
    }

    let available = ui.available_size();
    egui::Frame::none()
        .fill(LIST_BG)
        .rounding(egui::Rounding::same(8.0))
        .show(ui, |ui| {
            ui.set_min_size(available);

            if groups.is_empty() {
                ui.vertical_centered(|ui| {
                    ui.label(
                        egui::RichText::new(app.t("no_items_hint"))
                            .size(14.0)
                            .color(TEXT_TERTIARY),
                    );
                    ui.add_space(8.0);
                    let scan_hint = format!("🔍 {} → {}", app.t("scan"), app.t("click_to_start"));
                    if ui
                        .add(
                            egui::Button::new(
                                egui::RichText::new(&scan_hint)
                                    .size(13.0)
                                    .color(egui::Color32::WHITE),
                            )
                            .min_size([0.0, 28.0].into()),
                        )
                        .clicked()
                    {
                        start_scan(app, scan_rx);
                    }
                });
                return;
            }

            if let Some(sel) = app.selected_uninstall_app_index {
                let (app_name, indices) = &groups[sel];
                ui.horizontal(|ui| {
                    ui.colored_label(
                        TEXT_PRIMARY,
                        egui::RichText::new(app.tf("app_subitems_detail", &[app_name]))
                            .size(14.0)
                            .strong(),
                    );
                });
                ui.add_space(8.0);

                egui::ScrollArea::vertical()
                    .auto_shrink([false; 2])
                    .show(ui, |ui| {
                        for &display_idx in indices {
                            if let Some(toggled) = render_app_uninstall_child_row(
                                ui,
                                &items[display_idx],
                                display_idx,
                                app,
                            ) {
                                if app.results[tab_idx][toggled].deletable {
                                    app.results[tab_idx][toggled].selected =
                                        !app.results[tab_idx][toggled].selected;
                                }
                            }
                        }
                    });
            } else {
                ui.vertical_centered(|ui| {
                    ui.label(
                        egui::RichText::new(app.t("select_app_from_list"))
                            .size(14.0)
                            .color(TEXT_TERTIARY),
                    );
                });
            }
        });
}

/// 渲染 App 卸载 Tab 中的子项行
///
/// 用于父卡片内部的「应用数据 / 缓存与日志 / 应用本体」行。
/// 返回被点击切换选中的原始索引。
fn render_app_uninstall_child_row(
    ui: &mut egui::Ui,
    item: &ScanItem,
    index: usize,
    app: &App,
) -> Option<usize> {
    let mut toggled: Option<usize> = None;
    let locked = !item.deletable;
    let selected = item.selected;

    let child_frame = egui::Frame::none()
        .fill(SURFACE_MUTED)
        .inner_margin(egui::Margin::symmetric(10.0, 6.0))
        .stroke(egui::Stroke::new(1.0, BORDER_LIGHT))
        .rounding(egui::Rounding::same(8.0));

    let row_resp = child_frame.show(ui, |ui| {
        ui.horizontal(|ui| {
            // 自定义复选框
            let cb_response = render_custom_checkbox(ui, selected && !locked, !locked);
            if cb_response.clicked() && !locked {
                toggled = Some(index);
            }

            ui.add_space(8.0);

            // 中间信息区
            ui.vertical(|ui| {
                ui.set_min_width(160.0);

                // 标题行：标题 + 大小
                ui.horizontal(|ui| {
                    let title = uninstall_child_title(&item.category, app.lang_en);
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(title)
                                .size(13.0)
                                .strong()
                                .color(if locked { TEXT_TERTIARY } else { TEXT_PRIMARY }),
                        )
                        .truncate(),
                    );

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let size_str = if item.size_bytes == 0 {
                            "—".to_string()
                        } else {
                            format_size(item.size_bytes)
                        };
                        ui.colored_label(
                            if locked { TEXT_TERTIARY } else { TEXT_PRIMARY },
                            egui::RichText::new(&size_str)
                                .size(13.0)
                                .strong()
                                .monospace(),
                        );
                    });
                });

                // 路径
                let path_display = truncate_path(&item.path, 80);
                ui.colored_label(
                    TEXT_TERTIARY,
                    egui::RichText::new(&path_display).size(11.0).monospace(),
                );

                // 状态徽章 + 描述
                ui.horizontal(|ui| {
                    if locked {
                        render_status_badge(
                            ui,
                            app.t("badge_undeletable"),
                            TEXT_TERTIARY,
                            SURFACE_MUTED,
                        );
                    } else {
                        let (fg, bg, label) = recommend_badge_colors(&item.recommend);
                        render_status_badge(ui, label, fg, bg);
                    }
                    ui.add_space(6.0);
                    let desc = i18n::translate_description(&item.description, app.lang_en);
                    ui.colored_label(TEXT_SECONDARY, egui::RichText::new(&desc).size(11.0));
                });
            });
        });
    });

    // 整行点击切换选中（锁定项除外），但避免与按钮冲突
    if row_resp.response.clicked() && !locked && !row_resp.response.drag_started() {
        toggled = Some(index);
    }

    ui.add_space(4.0);

    toggled
}

/// 从 category 中提取前缀（用于分类过滤标签页）
///
/// 例如 "Docker dangling 镜像" -> "Docker"，"Xcode DerivedData — ProjectA" -> "Xcode"。
fn category_prefix(category: &str) -> String {
    let delimiters: &[char] = &[' ', '—', '-', '/', '(', '（', '·'];
    category
        .split(delimiters)
        .next()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| category.to_string())
}

/// 渲染分类过滤标签
fn render_category_tab(
    ui: &mut egui::Ui,
    label: &str,
    count: Option<usize>,
    active: bool,
) -> egui::Response {
    let text = match count {
        Some(c) => format!("{} ({})", label, c),
        None => label.to_string(),
    };
    let text_color = if active { BRAND } else { TEXT_SECONDARY };
    let bottom_stroke = if active {
        egui::Stroke::new(2.0, BRAND)
    } else {
        egui::Stroke::NONE
    };
    let padding = egui::vec2(14.0, 10.0);

    let galley = ui.painter().layout(
        text,
        egui::FontId::new(13.0, egui::FontFamily::Proportional),
        text_color,
        ui.available_width(),
    );
    let desired_size = egui::vec2(
        galley.size().x + padding.x * 2.0,
        galley.size().y + padding.y + 2.0,
    );
    let (rect, response) = ui.allocate_exact_size(desired_size, egui::Sense::click());

    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        if active {
            painter.line_segment(
                [
                    egui::pos2(rect.min.x, rect.max.y - 1.0),
                    egui::pos2(rect.max.x, rect.max.y - 1.0),
                ],
                bottom_stroke,
            );
        }
        let text_pos = egui::pos2(rect.min.x + padding.x, rect.min.y + padding.y - 2.0);
        painter.galley(text_pos, galley, text_color);
    }
    response
}

/// 状态徽章颜色与背景
fn recommend_badge_colors(rec: &Recommend) -> (egui::Color32, egui::Color32, &'static str) {
    match rec {
        Recommend::Safe | Recommend::CacheOnly => (
            egui::Color32::from_rgb(6, 95, 70),
            egui::Color32::from_rgb(209, 250, 229),
            "Safe",
        ),
        Recommend::Caution => (
            egui::Color32::from_rgb(146, 64, 14),
            egui::Color32::from_rgb(254, 243, 199),
            "Caution",
        ),
        Recommend::Advanced => (
            egui::Color32::from_rgb(153, 27, 27),
            egui::Color32::from_rgb(254, 226, 226),
            "Advanced",
        ),
    }
}

/// 渲染状态徽章
fn render_status_badge(ui: &mut egui::Ui, text: &str, fg: egui::Color32, bg: egui::Color32) {
    let galley = ui.painter().layout(
        text.to_string(),
        egui::FontId::new(11.0, egui::FontFamily::Proportional),
        fg,
        ui.available_width(),
    );
    let padding = egui::vec2(8.0, 3.0);
    let size = galley.size() + padding * 2.0;
    let (rect, _resp) = ui.allocate_exact_size(size, egui::Sense::hover());
    if ui.is_rect_visible(rect) {
        ui.painter()
            .rect_filled(rect, egui::Rounding::same(999.0), bg);
        ui.painter().galley(
            egui::pos2(rect.min.x + padding.x, rect.min.y + padding.y),
            galley,
            fg,
        );
    }
}

/// 渲染自定义复选框
fn render_custom_checkbox(ui: &mut egui::Ui, checked: bool, enabled: bool) -> egui::Response {
    let size = egui::vec2(16.0, 16.0);
    let (rect, response) = ui.allocate_exact_size(
        size,
        if enabled {
            egui::Sense::click()
        } else {
            egui::Sense::hover()
        },
    );
    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        let rounding = egui::Rounding::same(4.0);
        if checked {
            painter.rect_filled(rect, rounding, BRAND);
            // 白色对勾
            let stroke = egui::Stroke::new(2.0, egui::Color32::WHITE);
            painter.line_segment(
                [
                    egui::pos2(rect.min.x + 4.0, rect.center().y),
                    egui::pos2(rect.center().x - 1.0, rect.max.y - 5.0),
                ],
                stroke,
            );
            painter.line_segment(
                [
                    egui::pos2(rect.center().x - 1.0, rect.max.y - 5.0),
                    egui::pos2(rect.max.x - 3.0, rect.min.y + 4.0),
                ],
                stroke,
            );
        } else {
            let stroke_color = if enabled { BORDER_STRONG } else { BORDER_LIGHT };
            painter.rect_filled(rect, rounding, SURFACE_ELEVATED);
            painter.rect_stroke(rect, rounding, egui::Stroke::new(2.0, stroke_color));
        }
    }
    response
}

/// 渲染设置页 Toggle 开关
fn render_settings_toggle(ui: &mut egui::Ui, value: &mut bool, enabled: bool) -> egui::Response {
    let size = egui::vec2(40.0, 22.0);
    let (rect, response) = ui.allocate_exact_size(
        size,
        if enabled {
            egui::Sense::click()
        } else {
            egui::Sense::hover()
        },
    );

    if response.clicked() && enabled {
        *value = !*value;
    }

    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        let rounding = egui::Rounding::same(11.0);
        let on = *value && enabled;
        let bg = if on {
            BRAND
        } else if enabled {
            SURFACE_MUTED
        } else {
            egui::Color32::from_rgb(230, 230, 232)
        };
        let stroke_color = if on { BRAND } else { BORDER_STRONG };
        painter.rect_filled(rect, rounding, bg);
        painter.rect_stroke(rect, rounding, egui::Stroke::new(1.0, stroke_color));

        let knob_radius = 9.0;
        let knob_y = rect.center().y;
        let knob_x = if on {
            rect.max.x - knob_radius - 2.0
        } else {
            rect.min.x + knob_radius + 2.0
        };
        painter.circle_filled(
            egui::pos2(knob_x, knob_y),
            knob_radius,
            egui::Color32::WHITE,
        );
    }

    response
}

/// 渲染单个扫描项行（含关联明细展开）
///
/// 用于主列表和 App卸载 Tab 分组后的子项渲染。
/// 返回 (被点击切换选中的原始索引, 请求切换展开的路径)。
fn render_scan_item_row(
    ui: &mut egui::Ui,
    item: &ScanItem,
    index: usize,
    app: &App,
    is_uninstall_tab: bool,
    associated_details: &std::collections::HashMap<String, Vec<(String, u64, String)>>,
    expanded_items: &std::collections::HashSet<String>,
) -> (Option<usize>, Option<String>) {
    let mut toggled: Option<usize> = None;
    let mut expand_toggle: Option<String> = None;

    let locked = !item.deletable;
    let selected = item.selected;

    // 边框与背景
    let border_color = if selected {
        BRAND
    } else {
        egui::Color32::from_rgb(226, 226, 229)
    };
    let row_bg = if selected {
        BRAND_SOFT
    } else if locked {
        SURFACE_MUTED
    } else {
        SURFACE_ELEVATED
    };

    let row_frame = egui::Frame::none()
        .fill(row_bg)
        .inner_margin(egui::Margin::symmetric(12.0, 8.0))
        .stroke(egui::Stroke::new(1.0, border_color))
        .rounding(egui::Rounding::same(8.0));

    let has_details = is_uninstall_tab
        && !item.batch_paths.is_empty()
        && associated_details.contains_key(&item.path);
    let is_expanded = expanded_items.contains(&item.path);

    let row_resp = row_frame.show(ui, |ui| {
        ui.horizontal(|ui| {
            // 自定义复选框
            let cb_response = render_custom_checkbox(ui, selected && !locked, !locked);
            if cb_response.clicked() && !locked {
                toggled = Some(index);
            }

            ui.add_space(10.0);

            // 中间信息区
            ui.vertical(|ui| {
                ui.set_min_width(180.0);

                // 标题行：title + size
                ui.horizontal(|ui| {
                    // 标题
                    let title_text = if is_uninstall_tab {
                        i18n::translate_category(&item.category, app.lang_en)
                    } else {
                        i18n::translate_category(&item.category, app.lang_en)
                    };
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(title_text)
                                .size(13.0)
                                .strong()
                                .color(if locked { TEXT_TERTIARY } else { TEXT_PRIMARY }),
                        )
                        .truncate(),
                    );

                    // 展开按钮（仅 App卸载 tab 且有明细时显示）
                    if has_details {
                        let expand_icon = if is_expanded { "▼" } else { "▶" };
                        if ui.small_button(expand_icon).clicked() {
                            expand_toggle = Some(item.path.clone());
                        }
                    }

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let size_str = if item.size_bytes == 0 {
                            "—".to_string()
                        } else {
                            format_size(item.size_bytes)
                        };
                        ui.colored_label(
                            if locked { TEXT_TERTIARY } else { TEXT_PRIMARY },
                            egui::RichText::new(&size_str)
                                .size(13.0)
                                .strong()
                                .monospace(),
                        );
                    });
                });

                // 路径
                let path_display = truncate_path(&item.path, 90);
                ui.colored_label(
                    TEXT_TERTIARY,
                    egui::RichText::new(&path_display).size(11.0).monospace(),
                );

                // 状态徽章 + 描述
                ui.horizontal(|ui| {
                    if locked {
                        render_status_badge(
                            ui,
                            app.t("badge_undeletable"),
                            TEXT_TERTIARY,
                            egui::Color32::from_rgb(239, 239, 242),
                        );
                    } else {
                        let (fg, bg, label) = recommend_badge_colors(&item.recommend);
                        render_status_badge(ui, label, fg, bg);
                    }
                    ui.add_space(6.0);
                    let desc = i18n::translate_description(&item.description, app.lang_en);
                    ui.colored_label(TEXT_SECONDARY, egui::RichText::new(&desc).size(12.0));
                });

                // 不可删除原因
                if locked && !item.undeletable_reason.is_empty() {
                    let reason =
                        i18n::translate_undeletable_reason(&item.undeletable_reason, app.lang_en);
                    ui.colored_label(
                        ADVANCED_COLOR,
                        egui::RichText::new(format!("⚠️ {}", reason)).size(11.0),
                    );
                }
            });
        });
    });

    // 整行点击切换选中（锁定项除外），但避免与按钮冲突
    if row_resp.response.clicked() && !locked && !row_resp.response.drag_started() {
        toggled = Some(index);
    }

    // 展开时显示关联文件明细
    if has_details && is_expanded {
        if let Some(details) = associated_details.get(&item.path) {
            for (detail_path, detail_size, detail_label) in details {
                let detail_bg = SURFACE;
                let detail_frame = egui::Frame::none()
                    .fill(detail_bg)
                    .inner_margin(egui::Margin::symmetric(10.0, 4.0))
                    .stroke(egui::Stroke::new(1.0, BORDER_LIGHT))
                    .rounding(egui::Rounding::same(6.0));

                detail_frame.show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.add_space(28.0); // 缩进对齐
                        ui.colored_label(BRAND, egui::RichText::new(detail_label).size(11.0));
                        let sz_str = if *detail_size == 0 {
                            "—".to_string()
                        } else {
                            format_size(*detail_size)
                        };
                        ui.colored_label(
                            TEXT_SECONDARY,
                            egui::RichText::new(&sz_str).size(12.0).monospace(),
                        );
                        ui.add_space(8.0);
                        let dp = truncate_path(detail_path, 55);
                        ui.colored_label(
                            TEXT_TERTIARY,
                            egui::RichText::new(&dp).size(10.0).monospace(),
                        );
                    });
                });
            }
        }
    }

    ui.add_space(4.0);

    (toggled, expand_toggle)
}

/// 渲染 GUI 主界面
fn render_gui(
    ctx: &egui::Context,
    app: &mut App,
    scan_rx: &mut Option<mpsc::Receiver<ScanMessage>>,
    delete_rx: &mut Option<mpsc::Receiver<DeleteMessage>>,
) {
    // 动态更新窗口标题（跟随语言切换）
    ctx.send_viewport_cmd(egui::ViewportCommand::Title(
        app.t("window_title").to_string(),
    ));

    // ========== 磁盘告警横幅（顶部）==========
    let (alert_level, alert_color, free_pct) = app.disk_alert_level();
    if alert_level >= 1 {
        egui::TopBottomPanel::top("disk_alert_banner").show(ctx, |ui| {
            let (icon, msg_key) = match alert_level {
                3 => ("🔴", "disk_alert_critical"),
                2 => ("🟠", "disk_alert_warning"),
                1 => ("🟡", "disk_alert_notice"),
                _ => ("🟢", "disk_alert_notice"),
            };
            let free_gb = app.disk_free as f64 / 1_073_741_824.0;
            let msg = app.tf(
                msg_key,
                &[&format!("{:.1}", free_pct), &format!("{:.1}", free_gb)],
            );

            ui.horizontal(|ui| {
                ui.colored_label(
                    alert_color,
                    egui::RichText::new(format!("{} {}", icon, msg)).size(14.0),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if alert_level >= 2 {
                        if ui.button(app.t("disk_alert_clean_now")).clicked() {
                            // 跳转到开发者缓存 Tab（通常回收空间最大）
                            app.tab = crate::app::Tab::DevCache;
                            app.list_index = 0;
                        }
                    }
                });
            });
        });
    }

    // ========== 底部 Footer ==========
    egui::TopBottomPanel::bottom("footer").show(ctx, |ui| {
        egui::Frame::none()
            .fill(SURFACE_ELEVATED)
            .stroke(egui::Stroke::new(1.0, BORDER_LIGHT))
            .inner_margin(egui::Margin::symmetric(16.0, 8.0))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    let selected_cnt = app.selected_count();
                    let selected_sz = app.selected_total_size();

                    // 左侧：已选中 N 项 · 可释放 XX GB
                    ui.colored_label(
                        TEXT_SECONDARY,
                        egui::RichText::new(format!(
                            "{} {} · {} {}",
                            selected_cnt,
                            app.t("items_selected"),
                            app.t("total"),
                            format_size(selected_sz)
                        ))
                        .size(13.0),
                    );

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let delete_enabled =
                            selected_cnt > 0 && matches!(app.confirm, ConfirmState::None);
                        let delete_btn = ui.add_enabled(
                            delete_enabled,
                            egui::Button::new(
                                egui::RichText::new(format!(
                                    "🗑 {} ({})",
                                    app.t("delete"),
                                    format_size(selected_sz)
                                ))
                                .color(egui::Color32::WHITE)
                                .size(13.0),
                            )
                            .fill(DANGER_COLOR)
                            .rounding(egui::Rounding::same(8.0))
                            .min_size([0.0, 28.0].into()),
                        );
                        if delete_btn.clicked() {
                            app.prepare_delete();
                        }
                    });
                });
            });
    });

    // ========== 左侧导航栏 ==========
    egui::SidePanel::left("sidebar")
        .exact_width(220.0)
        .frame(egui::Frame::side_top_panel(&ctx.style()).fill(SURFACE_SIDEBAR))
        .show(ctx, |ui| {
            ui.set_min_width(200.0);
            ui.vertical(|ui| {
                // --- 品牌区 ---
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    // 品牌图标：紫色圆角方块 + 白色 M
                    let icon_size = egui::vec2(28.0, 28.0);
                    let (icon_rect, _icon_resp) =
                        ui.allocate_exact_size(icon_size, egui::Sense::hover());
                    if ui.is_rect_visible(icon_rect) {
                        let painter = ui.painter();
                        painter.rect_filled(icon_rect, egui::Rounding::same(8.0), BRAND);
                        painter.text(
                            icon_rect.center(),
                            egui::Align2::CENTER_CENTER,
                            "M",
                            egui::FontId::proportional(13.0),
                            egui::Color32::WHITE,
                        );
                    }
                    ui.vertical(|ui| {
                        ui.label(
                            egui::RichText::new("Maclean")
                                .size(15.0)
                                .strong()
                                .color(BRAND_TEXT),
                        );
                        ui.label(
                            egui::RichText::new(format!("v{}", env!("CARGO_PKG_VERSION")))
                                .size(11.0)
                                .color(TEXT_TERTIARY),
                        );
                    });
                });
                ui.add_space(10.0);

                // --- 磁盘用量卡片 ---
                let free_pct = if app.disk_total > 0 {
                    app.disk_free as f32 / app.disk_total as f32 * 100.0
                } else {
                    0.0
                };
                egui::Frame::none()
                    .fill(SURFACE_ELEVATED)
                    .stroke(egui::Stroke::new(1.0, BORDER_LIGHT))
                    .rounding(egui::Rounding::same(8.0))
                    .inner_margin(egui::Margin::same(10.0))
                    .show(ui, |ui| {
                        ui.set_min_width(180.0);
                        ui.colored_label(
                            TEXT_SECONDARY,
                            egui::RichText::new(app.t("disk_free")).size(11.0),
                        );
                        ui.add_space(2.0);
                        ui.horizontal(|ui| {
                            ui.label(
                                egui::RichText::new(format_size(app.disk_free))
                                    .size(16.0)
                                    .strong()
                                    .color(TEXT_PRIMARY),
                            );
                            ui.label(
                                egui::RichText::new(format!("/ {}", format_size(app.disk_total)))
                                    .size(11.0)
                                    .color(TEXT_TERTIARY),
                            );
                        });
                        ui.add_space(6.0);
                        // 细进度条（无文字）
                        let bar_rect = ui.available_rect_before_wrap();
                        let bar_height = 6.0;
                        let bar_rect = egui::Rect::from_min_size(
                            bar_rect.min,
                            egui::vec2(bar_rect.width(), bar_height),
                        );
                        ui.painter().rect_filled(
                            bar_rect,
                            egui::Rounding::same(3.0),
                            SURFACE_MUTED,
                        );
                        let fill_width = bar_rect.width() * (free_pct / 100.0);
                        if fill_width > 0.0 {
                            let fill_rect = egui::Rect::from_min_size(
                                bar_rect.min,
                                egui::vec2(fill_width, bar_height),
                            );
                            ui.painter()
                                .rect_filled(fill_rect, egui::Rounding::same(3.0), BRAND);
                        }
                        ui.allocate_rect(bar_rect, egui::Sense::hover());
                    });
                ui.add_space(10.0);

                // --- 导航项列表 ---
                ui.spacing_mut().item_spacing.y = 2.0;
                let nav_icons = ["🏠", "📦", "📁", "🧹", "📂", "🧩", "⚙️", "💾", "🛠"];
                for (i, tab) in Tab::all().iter().enumerate() {
                    let count = app.results[i].len();
                    let is_selected = *tab == app.tab;
                    let icon = nav_icons[i];
                    let title = tab_title(tab, app);

                    let nav_frame = if is_selected {
                        egui::Frame::none()
                            .fill(BRAND_SOFT)
                            .rounding(egui::Rounding::same(8.0))
                            .inner_margin(egui::Margin::symmetric(8.0, 6.0))
                            .stroke(egui::Stroke::NONE)
                    } else {
                        egui::Frame::none()
                            .inner_margin(egui::Margin::symmetric(8.0, 6.0))
                            .stroke(egui::Stroke::NONE)
                    };

                    let nav_resp = nav_frame.show(ui, |ui| {
                        ui.set_min_width(180.0);
                        ui.horizontal(|ui| {
                            ui.label(egui::RichText::new(icon).size(16.0));
                            ui.label(egui::RichText::new(title).size(13.0).color(if is_selected {
                                BRAND_TEXT
                            } else {
                                TEXT_PRIMARY
                            }));
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    if count > 0 {
                                        ui.label(
                                            egui::RichText::new(count.to_string())
                                                .size(11.0)
                                                .color(if is_selected {
                                                    BRAND
                                                } else {
                                                    TEXT_TERTIARY
                                                })
                                                .background_color(if is_selected {
                                                    BRAND_SOFT
                                                } else {
                                                    SURFACE_MUTED
                                                }),
                                        );
                                    }
                                },
                            );
                        });
                    });

                    // Frame 默认只响应 hover，需要单独分配 click sense 才能点击
                    let nav_click = ui.interact(
                        nav_resp.response.rect,
                        egui::Id::new(("nav_click", i)),
                        egui::Sense::click(),
                    );
                    if nav_click.clicked() && matches!(app.confirm, ConfirmState::None) {
                        app.tab = *tab;
                        app.list_index = 0;
                    }
                }
            });

            // --- 底部：语言切换 + 日志按钮 ---
            ui.with_layout(egui::Layout::bottom_up(egui::Align::LEFT), |ui| {
                ui.add_space(4.0);
                ui.separator();
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    if ui
                        .add(
                            egui::Button::new(if app.lang_en { "中文" } else { "EN" })
                                .min_size([0.0, 28.0].into()),
                        )
                        .clicked()
                    {
                        app.toggle_lang();
                    }
                    if ui
                        .add(
                            egui::Button::new(format!("📋 {}", app.t("logs")))
                                .min_size([0.0, 28.0].into()),
                        )
                        .clicked()
                    {
                        let log_dir = logger::log_dir();
                        // 跨平台打开日志目录
                        #[cfg(target_os = "macos")]
                        let _ = std::process::Command::new("open").arg(&log_dir).spawn();
                        #[cfg(target_os = "windows")]
                        let _ = std::process::Command::new("explorer").arg(&log_dir).spawn();
                        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
                        let _ = std::process::Command::new("xdg-open").arg(&log_dir).spawn();
                    }
                });
                ui.add_space(4.0);
            });
        });

    // ========== App 卸载：左侧应用列表面板 ==========
    // 仅在 App 卸载 Tab 有数据且非扫描中时显示左右分栏
    let show_app_list_panel = app.tab == Tab::AppUninstall
        && !matches!(app.scan_states[app.tab_index()], ScanState::Scanning)
        && !app.results[app.tab_index()].is_empty();
    if show_app_list_panel {
        egui::SidePanel::left("app_list_panel")
            .exact_width(240.0)
            .frame(egui::Frame::side_top_panel(&ctx.style()).fill(LIST_BG))
            .show(ctx, |ui| {
                render_app_uninstall_list_panel(ui, app);
            });
    }

    // ========== 右侧内容区 ==========
    // 减小 CentralPanel 默认内边距，避免顶部和两侧留空过多
    let central_frame =
        egui::Frame::central_panel(&ctx.style()).inner_margin(egui::Margin::symmetric(12.0, 0.0));
    egui::CentralPanel::default()
        .frame(central_frame)
        .show(ctx, |ui| {
            // --- 顶部 Header：当前 Tab 标题 + 副标题 + 扫描/刷新按钮 ---
            egui::Frame::none()
                .inner_margin(egui::Margin::symmetric(16.0, 10.0))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.vertical(|ui| {
                            ui.label(
                                egui::RichText::new(tab_title(&app.tab, app))
                                    .size(16.0)
                                    .strong()
                                    .color(TEXT_PRIMARY),
                            );
                            // 副标题：扫描状态/项目数
                            let tab_idx_h = app.tab_index();
                            let subtitle = match &app.scan_states[tab_idx_h] {
                                ScanState::Idle => app.t("press_r_to_scan").to_string(),
                                ScanState::Scanning => format!("⏳ {}", app.t("scanning")),
                                ScanState::Done => {
                                    let count = app.current_items().len();
                                    let total: u64 =
                                        app.current_items().iter().map(|i| i.size_bytes).sum();
                                    app.tf(
                                        "found_items_total",
                                        &[&count.to_string(), &format_size(total)],
                                    )
                                }
                            };
                            ui.colored_label(
                                TEXT_TERTIARY,
                                egui::RichText::new(subtitle).size(11.0),
                            );
                        });

                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            // 强制刷新按钮（清除缓存后重新扫描）
                            let is_scanning =
                                matches!(app.current_scan_state(), ScanState::Scanning);
                            let refresh_btn = ui.add_enabled(
                                !is_scanning,
                                egui::Button::new("🔄").min_size([0.0, 28.0].into()),
                            );
                            if refresh_btn.clicked() {
                                if app.tab == Tab::Overview {
                                    // 概览页：清空所有 Tab 缓存后扫描全部
                                    scanner::cache::invalidate_all_caches();
                                    start_scan_all(app, scan_rx);
                                } else {
                                    let tab_name = match app.tab {
                                        Tab::Settings => None,
                                        Tab::DevCache => Some("dev_cache"),
                                        Tab::LargeFiles => Some("large_files"),
                                        Tab::AppCache => Some("app_cache"),
                                        Tab::AppData => Some("app_data"),
                                        Tab::AppUninstall => Some("app_uninstall"),
                                        Tab::SystemOptimize => Some("system_optimize"),
                                        Tab::Apfs => Some("apfs"),
                                        Tab::Overview => unreachable!(),
                                    };
                                    if let Some(name) = tab_name {
                                        scanner::cache::invalidate_cache(name);
                                    }
                                    start_scan(app, scan_rx);
                                }
                            }

                            ui.add_space(4.0);

                            // 扫描按钮
                            let scan_button = ui.add_enabled(
                                !is_scanning,
                                egui::Button::new(
                                    egui::RichText::new(if is_scanning {
                                        "⏳..."
                                    } else {
                                        app.t("scan")
                                    })
                                    .color(egui::Color32::WHITE),
                                )
                                .fill(BRAND)
                                .rounding(egui::Rounding::same(8.0))
                                .min_size([0.0, 28.0].into()),
                            );
                            if scan_button.clicked() {
                                if app.tab == Tab::Overview {
                                    start_scan_all(app, scan_rx);
                                } else {
                                    start_scan(app, scan_rx);
                                }
                            }
                        });
                    });
                });

            ui.separator();

            // --- 概览 Tab：聚合推荐清理 ---
            if app.tab == Tab::Overview {
                render_overview_panel(ui, app, scan_rx);
                return;
            }

            // --- 系统优化 Tab：特殊渲染（操作面板而非列表选择）---
            if app.tab == Tab::SystemOptimize {
                render_optimize_panel(ui, app, scan_rx);
                return;
            }

            // --- 设置 Tab：配置面板 ---
            if app.tab == Tab::Settings {
                render_settings_panel(ui, app);
                return;
            }

            // --- 磁盘分析器 Tab：目录钻取式浏览 ---
            if app.tab == Tab::LargeFiles {
                render_disk_analyzer(ui, app, scan_rx);
                return;
            }

            // --- App 卸载 Tab：左右分栏（左侧应用列表已在外部 SidePanel 渲染）---
            if app.tab == Tab::AppUninstall {
                let is_scanning = matches!(app.scan_states[app.tab_index()], ScanState::Scanning);
                let is_empty = app.results[app.tab_index()].is_empty();
                if is_scanning {
                    ui_scanning(ui, app);
                } else if is_empty {
                    let empty_size = ui.available_size();
                    egui::Frame::none()
                        .fill(LIST_BG)
                        .rounding(egui::Rounding::same(8.0))
                        .show(ui, |ui| {
                            ui.set_min_size(empty_size);
                            ui.vertical_centered(|ui| {
                                ui.label(
                                    egui::RichText::new(app.t("no_items_hint"))
                                        .size(14.0)
                                        .color(TEXT_TERTIARY),
                                );
                                ui.add_space(8.0);
                                let scan_hint =
                                    format!("🔍 {} → {}", app.t("scan"), app.t("click_to_start"));
                                if ui
                                    .add(
                                        egui::Button::new(
                                            egui::RichText::new(&scan_hint)
                                                .size(13.0)
                                                .color(egui::Color32::WHITE),
                                        )
                                        .min_size([0.0, 28.0].into()),
                                    )
                                    .clicked()
                                {
                                    start_scan(app, scan_rx);
                                }
                            });
                        });
                } else {
                    render_app_uninstall_details_panel(ui, app, scan_rx);
                }
                return;
            }

            // --- 扫描结果区 ---
            let tab_idx = app.tab_index();
            let items = app.results[tab_idx].clone();
            let is_scanning = matches!(app.scan_states[tab_idx], ScanState::Scanning);

            // --- 分类过滤标签页（Overview / LargeFiles / SystemOptimize / Settings 除外）---
            let show_category_tabs = !matches!(
                app.tab,
                Tab::Overview | Tab::LargeFiles | Tab::SystemOptimize | Tab::Settings
            );
            if show_category_tabs && !is_scanning && !items.is_empty() {
                let mut prefixes: Vec<String> = items
                    .iter()
                    .map(|item| category_prefix(&item.category))
                    .filter(|p| !p.is_empty())
                    .collect::<std::collections::HashSet<_>>()
                    .into_iter()
                    .collect();
                prefixes.sort();

                if !prefixes.is_empty() {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 0.0;
                        let all_count = items.len();
                        let all_active = app.filter_category.is_none();
                        if render_category_tab(ui, app.t("select_all"), Some(all_count), all_active)
                            .clicked()
                        {
                            app.filter_category = None;
                        }
                        for prefix in &prefixes {
                            let count = items
                                .iter()
                                .filter(|i| category_prefix(&i.category) == *prefix)
                                .count();
                            let active = app.filter_category.as_deref() == Some(prefix);
                            if render_category_tab(ui, prefix, Some(count), active).clicked() {
                                app.filter_category = Some(prefix.clone());
                            }
                        }
                    });
                }
            }

            if is_scanning {
                ui_scanning(ui, app);
                return;
            } else if items.is_empty() {
                // 列表区域：占满剩余高度并居中显示空状态
                let list_area_size = ui.available_size();
                egui::Frame::none()
                    .fill(LIST_BG)
                    .rounding(egui::Rounding::same(8.0))
                    .show(ui, |ui| {
                        ui.set_min_size(list_area_size);
                        ui.vertical_centered(|ui| {
                            ui.label(
                                egui::RichText::new(app.t("no_items_hint"))
                                    .size(14.0)
                                    .color(TEXT_TERTIARY),
                            );
                            ui.add_space(8.0);
                            let scan_hint =
                                format!("🔍 {} → {}", app.t("scan"), app.t("click_to_start"));
                            if ui
                                .add(
                                    egui::Button::new(
                                        egui::RichText::new(&scan_hint)
                                            .size(13.0)
                                            .color(egui::Color32::WHITE),
                                    )
                                    .min_size([0.0, 28.0].into()),
                                )
                                .clicked()
                            {
                                start_scan(app, scan_rx);
                            }
                        });
                    });
            } else {
                // ====== 扫描结果汇总卡片 ======
                let safe_cnt = app.safe_count();
                let safe_sz = app.safe_size();
                let caution_cnt = app.caution_count();
                let caution_sz = app.caution_size();
                let advanced_cnt = app.advanced_count();
                let advanced_sz = app.advanced_size();
                let selected_cnt = app.selected_count();
                let selected_sz = app.selected_total_size();

                // Summary Pills：胶囊式汇总卡片
                egui::Frame::none()
                    .inner_margin(egui::Margin::symmetric(16.0, 8.0))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 6.0;

                            // Safe 胶囊
                            egui::Frame::none()
                                .fill(egui::Color32::from_rgb(232, 255, 243))
                                .stroke(egui::Stroke::new(1.0, SAFE_COLOR))
                                .rounding(egui::Rounding::same(14.0))
                                .inner_margin(egui::Margin::symmetric(10.0, 8.0))
                                .show(ui, |ui| {
                                    ui.horizontal(|ui| {
                                        ui.colored_label(
                                            SAFE_COLOR,
                                            egui::RichText::new("●").size(9.0),
                                        );
                                        ui.colored_label(
                                            SAFE_COLOR,
                                            egui::RichText::new(format!(
                                                "{} {} · {}",
                                                safe_cnt,
                                                app.t("safe_clean"),
                                                format_size(safe_sz)
                                            ))
                                            .size(11.0),
                                        );
                                    });
                                });

                            // Caution 胶囊
                            egui::Frame::none()
                                .fill(egui::Color32::from_rgb(255, 247, 230))
                                .stroke(egui::Stroke::new(1.0, CAUTION_COLOR))
                                .rounding(egui::Rounding::same(14.0))
                                .inner_margin(egui::Margin::symmetric(10.0, 8.0))
                                .show(ui, |ui| {
                                    ui.horizontal(|ui| {
                                        ui.colored_label(
                                            CAUTION_COLOR,
                                            egui::RichText::new("●").size(9.0),
                                        );
                                        ui.colored_label(
                                            CAUTION_COLOR,
                                            egui::RichText::new(format!(
                                                "{} {} · {}",
                                                caution_cnt,
                                                app.t("caution_clean"),
                                                format_size(caution_sz)
                                            ))
                                            .size(11.0),
                                        );
                                    });
                                });

                            // Advanced 胶囊（如果有）
                            if advanced_cnt > 0 {
                                egui::Frame::none()
                                    .fill(egui::Color32::from_rgb(255, 233, 230))
                                    .stroke(egui::Stroke::new(1.0, ADVANCED_COLOR))
                                    .rounding(egui::Rounding::same(14.0))
                                    .inner_margin(egui::Margin::symmetric(10.0, 8.0))
                                    .show(ui, |ui| {
                                        ui.horizontal(|ui| {
                                            ui.colored_label(
                                                ADVANCED_COLOR,
                                                egui::RichText::new("●").size(9.0),
                                            );
                                            ui.colored_label(
                                                ADVANCED_COLOR,
                                                egui::RichText::new(format!(
                                                    "{} {} · {}",
                                                    advanced_cnt,
                                                    app.t("confirm_clean"),
                                                    format_size(advanced_sz)
                                                ))
                                                .size(11.0),
                                            );
                                        });
                                    });
                            }

                            // 已选胶囊
                            if selected_cnt > 0 {
                                egui::Frame::none()
                                    .fill(BRAND_SOFT)
                                    .stroke(egui::Stroke::new(1.0, BRAND))
                                    .rounding(egui::Rounding::same(14.0))
                                    .inner_margin(egui::Margin::symmetric(10.0, 8.0))
                                    .show(ui, |ui| {
                                        ui.horizontal(|ui| {
                                            ui.colored_label(
                                                BRAND,
                                                egui::RichText::new("✓").size(9.0),
                                            );
                                            ui.colored_label(
                                                BRAND,
                                                egui::RichText::new(format!(
                                                    "{} {} · {}",
                                                    selected_cnt,
                                                    app.t("items_selected"),
                                                    format_size(selected_sz)
                                                ))
                                                .size(11.0),
                                            );
                                        });
                                    });
                            }
                        });
                    });

                // ====== 操作按钮栏（选择分组 + 搜索）======
                egui::Frame::none()
                    .inner_margin(egui::Margin::symmetric(16.0, 6.0))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 4.0;

                            // 智能选择：只选推荐清理
                            let smart_btn = ui.add(
                                egui::Button::new(
                                    egui::RichText::new(format!("🟢 {}", app.t("select_safe")))
                                        .color(egui::Color32::WHITE),
                                )
                                .fill(SAFE_COLOR)
                                .rounding(egui::Rounding::same(8.0))
                                .min_size([0.0, 28.0].into()),
                            );
                            if smart_btn.clicked() {
                                app.select_safe_only();
                            }

                            if ui
                                .add(
                                    egui::Button::new(app.t("select_all"))
                                        .min_size([0.0, 28.0].into()),
                                )
                                .clicked()
                            {
                                app.select_all();
                            }
                            if ui
                                .add(
                                    egui::Button::new(app.t("deselect_all"))
                                        .min_size([0.0, 28.0].into()),
                                )
                                .clicked()
                            {
                                app.deselect_all();
                            }

                            ui.separator();

                            // 过滤/搜索输入框（/ 键聚焦，Esc 清除过滤）
                            let filter_placeholder = app.t("filter_placeholder").to_string();
                            let resp = ui.add(
                                egui::TextEdit::singleline(&mut app.filter_query)
                                    .hint_text(&filter_placeholder)
                                    .desired_width(280.0)
                                    .min_size([120.0, 28.0].into()),
                            );
                            if resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                                app.clear_filter();
                            }
                            if !app.filter_query.is_empty() {
                                if ui
                                    .add(egui::Button::new("✕").min_size([0.0, 28.0].into()))
                                    .clicked()
                                {
                                    app.clear_filter();
                                }
                            }
                            app.filter_active = resp.has_focus();
                        });
                    });

                // 全局 / 键快捷聚焦过滤输入框
                if ui.input(|i| i.key_pressed(egui::Key::Slash) && !app.filter_active) {
                    // 标记需要聚焦（下一帧通过 request_focus 实现）
                    app.filter_active = true;
                }

                // 显示过滤结果计数
                let total_count = items.len();
                let filtered_indices = app.filtered_indices();
                let filtered_count = filtered_indices.len();
                if !app.filter_query.trim().is_empty() {
                    ui.colored_label(
                        BRAND,
                        egui::RichText::new(app.tf(
                            "filter_results",
                            &[&filtered_count.to_string(), &total_count.to_string()],
                        ))
                        .size(12.0),
                    );
                }

                // 列表区域：占满剩余高度，内容少时保持背景色
                let list_area_size = ui.available_size();
                egui::Frame::none()
                    .fill(LIST_BG)
                    .rounding(egui::Rounding::same(8.0))
                    .show(ui, |ui| {
                        ui.set_min_size(list_area_size);

                        // 过滤后无结果
                        if filtered_count == 0 && !app.filter_query.trim().is_empty() {
                            let no_match_text = app.t("no_match").to_string();
                            ui.vertical_centered(|ui| {
                                ui.label(
                                    egui::RichText::new(&no_match_text)
                                        .size(13.0)
                                        .color(TEXT_TERTIARY),
                                );
                            });
                            return;
                        }

                        // ====== 可滚动列表 ======
                        // 提前克隆关联明细和展开状态，避免借用冲突
                        let associated_details = app.associated_details.clone();
                        let expanded_items = app.expanded_items.clone();

                        egui::ScrollArea::vertical()
                            .auto_shrink([false; 2])
                            .show(ui, |ui| {
                                let mut toggled_indices: Vec<usize> = Vec::new();
                                let mut expand_toggles: std::collections::HashSet<String> =
                                    std::collections::HashSet::new();

                                // ====== 其它 Tab：平铺显示 ======
                                for &display_idx in &filtered_indices {
                                    let (toggled, expand) = render_scan_item_row(
                                        ui,
                                        &items[display_idx],
                                        display_idx,
                                        app,
                                        false,
                                        &associated_details,
                                        &expanded_items,
                                    );
                                    if let Some(idx) = toggled {
                                        toggled_indices.push(idx);
                                    }
                                    if let Some(path) = expand {
                                        expand_toggles.insert(path);
                                    }
                                }

                                // 应用选中变更
                                for idx in toggled_indices {
                                    let items_mut = &mut app.results[tab_idx];
                                    if idx < items_mut.len() && items_mut[idx].deletable {
                                        items_mut[idx].selected = !items_mut[idx].selected;
                                    }
                                }

                                // 应用展开/收起变更
                                for path in expand_toggles {
                                    if !app.expanded_items.insert(path.clone()) {
                                        app.expanded_items.remove(&path);
                                    }
                                }
                            });
                    });
            }
        });

    // ========== 弹窗层（必须在 CentralPanel 闭包外部，确保不被 return 跳过） ==========

    // 权限引导弹窗（首次启动时显示）
    if app.show_permission_guide {
        show_permission_guide_window(ctx, app);
    }

    // 删除确认弹窗
    if matches!(app.confirm, ConfirmState::Pending) {
        show_confirm_window(ctx, app, delete_rx);
    }

    // sudo 密码输入弹窗
    if matches!(app.confirm, ConfirmState::NeedSudoPassword) {
        show_sudo_password_window(ctx, app, delete_rx);
    }

    // Touch ID 启用提示弹窗
    if matches!(app.confirm, ConfirmState::OfferTouchIdSetup) {
        show_touch_id_setup_window(ctx, app, delete_rx);
    }

    // Touch ID 启用等待中（轮询检测 Terminal 中用户是否已完成授权）(macOS 专属)
    #[cfg(target_os = "macos")]
    if matches!(app.confirm, ConfirmState::WaitForTouchIdSetup) {
        show_touch_id_waiting_window(ctx, app);

        // 检查是否已启用成功
        if touchid::sudo_touch_id_enabled() {
            // Touch ID 已启用，开始 sudo 删除
            app.touch_id_enabled = true;
            app.touch_id_wait_start = None;
            app.confirm = ConfirmState::SudoWithTouchId;
            let items = app.sudo_failed_items.clone();
            app.delete_done = 0;
            app.delete_total = items.len();
            start_sudo_delete_touchid(items, app.lang_en, delete_rx);
        } else if let Some(start) = app.touch_id_wait_start {
            // 检查超时（120 秒）
            if start.elapsed().as_secs() > 120 {
                app.touch_id_wait_start = None;
                app.touch_id_error = Some(app.t("touchid_timeout_error").to_string());
                app.confirm = ConfirmState::OfferTouchIdSetup;
            }
        }
    }

    // Touch ID 删除中弹窗
    if matches!(app.confirm, ConfirmState::SudoWithTouchId) {
        show_touch_id_deleting_window(ctx, app);
    }

    // 删除中弹窗
    if matches!(app.confirm, ConfirmState::Deleting) {
        show_deleting_window(ctx, app);
    }

    // 删除完成汇总弹窗
    if let Some((ok, fail, skip)) = app.delete_summary {
        show_summary_window(ctx, app, ok, fail, skip, delete_rx);
    }

    // Windows: 残留清理弹窗（卸载后检测到残留时弹出）
    #[cfg(target_os = "windows")]
    if app.show_residual_dialog {
        show_residual_window(ctx, app, delete_rx);
    }
}

/// 启动后台扫描
fn start_scan(app: &mut App, scan_rx: &mut Option<mpsc::Receiver<ScanMessage>>) {
    let tab = app.tab;
    let tab_idx = app.tab_index();
    app.scan_states[tab_idx] = ScanState::Scanning;
    app.scan_progress = 0.0;
    // 清空上一次扫描结果，为增量显示做准备
    app.results[tab_idx].clear();

    // 磁盘分析器：获取当前浏览路径
    let disk_path = if tab == Tab::LargeFiles {
        Some(app.disk_analyzer_current_path())
    } else {
        None
    };

    let (tx, rx) = mpsc::channel();
    *scan_rx = Some(rx);

    // 进度估算线程：每 200ms 发送进度更新
    // 扫描通常 2-8 秒完成，但大文件扫描可能更久，用渐近曲线估算进度
    let tx_progress = tx.clone();
    std::thread::spawn(move || {
        let start = std::time::Instant::now();
        loop {
            std::thread::sleep(std::time::Duration::from_millis(200));
            let elapsed = start.elapsed().as_secs_f32();
            // 分段渐近曲线：
            //   0-5s: 快速上升到约 90%
            //   5-15s: 缓慢上升到约 99%
            //   15s+: 极缓慢逼近 99.5%，避免长时间卡在同一个百分比
            let progress = if elapsed < 5.0 {
                0.9 * (1.0 - (-elapsed / 2.5).exp())
            } else if elapsed < 15.0 {
                0.9 + 0.09 * (1.0 - (-(elapsed - 5.0) / 10.0).exp())
            } else {
                0.99 + 0.005 * (1.0 - (-(elapsed - 15.0) / 10.0).exp())
            };
            // 如果通道关闭（扫描已完成），退出
            if tx_progress
                .send(ScanMessage::Progress(progress.min(0.995)))
                .is_err()
            {
                break;
            }
        }
    });

    // 实际扫描线程
    std::thread::spawn(move || {
        // 获取 Tab 名称用于缓存
        // 磁盘分析器在非主目录浏览时不使用缓存（路径不同，结果不同）
        let tab_name = match tab {
            Tab::Overview | Tab::Settings => None,
            Tab::DevCache => Some("dev_cache"),
            Tab::LargeFiles => {
                // 仅在主目录时缓存
                if disk_path
                    .as_ref()
                    .map(|p| *p == scanner::home_dir())
                    .unwrap_or(true)
                {
                    Some("large_files")
                } else {
                    None
                }
            }
            Tab::AppCache => Some("app_cache"),
            Tab::AppData => Some("app_data"),
            Tab::AppUninstall => Some("app_uninstall"),
            Tab::SystemOptimize => None, // 不缓存
            Tab::Apfs => Some("apfs"),
        };

        // 尝试从磁盘缓存加载
        if let Some(name) = tab_name {
            if let Some(cached) = scanner::cache::load_cache(name) {
                // 缓存命中：直接使用缓存结果
                let mut items = cached.items;
                // 后处理：重新检测可删除性（路径可能已变化）
                for item in &mut items {
                    let (deletable, reason) = scanner::check_deletable(&item.path);
                    item.deletable = deletable && item.deletable;
                    if !item.deletable && !reason.is_empty() {
                        item.undeletable_reason = reason;
                    }
                }
                // 缓存命中也按批次发送，提供增量显示体验
                send_items_in_batches(&tx, &items, tab_idx as u64);
                let _ = tx.send(ScanMessage::Done(
                    items,
                    cached.scan_time_ms,
                    tab_idx as u64,
                ));
                let _ = tx.send(ScanMessage::AllDone);
                return;
            }
        }

        // 缓存未命中：执行全量扫描
        // 用 catch_unwind 兜底，防止扫描 panic 后 UI 卡死
        let result = std::panic::catch_unwind(|| {
            match tab {
                Tab::Overview | Tab::Settings => scanner::ScanResult {
                    items: Vec::new(),
                    total_size: 0,
                    scan_time_ms: 0,
                },
                Tab::DevCache => scanner::dev_cache::DevCacheScanner::new().scan(),
                Tab::LargeFiles => {
                    // 磁盘分析器：扫描指定目录（默认为主目录）
                    if let Some(ref path) = disk_path {
                        scanner::large_files::scan_directory(path)
                    } else {
                        scanner::large_files::LargeFileScanner::new().scan()
                    }
                }
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
            }
        });

        match result {
            Ok(scan_result) => {
                // 保存到磁盘缓存
                if let Some(name) = tab_name {
                    scanner::cache::save_cache(name, &scan_result);
                }

                // 后处理：检测每个 item 的可删除性
                let mut items = scan_result.items;
                for item in &mut items {
                    let (deletable, reason) = scanner::check_deletable(&item.path);
                    item.deletable = deletable && item.deletable;
                    if !item.deletable && !reason.is_empty() {
                        item.undeletable_reason = reason;
                    }
                }

                // 增量显示：按大小降序排序后分批发送，最后 Done 发送完整列表替换
                send_items_in_batches(&tx, &items, tab_idx as u64);
                let _ = tx.send(ScanMessage::Done(
                    items,
                    scan_result.scan_time_ms,
                    tab_idx as u64,
                ));
            }
            Err(_) => {
                // 扫描 panic，发送空结果让 UI 恢复正常
                let _ = tx.send(ScanMessage::Done(Vec::new(), 0, tab_idx as u64));
            }
        }
        let _ = tx.send(ScanMessage::AllDone);
    });
}

/// 启动所有 Tab 的后台扫描（用于概览页"扫描全部"）
fn start_scan_all(app: &mut App, scan_rx: &mut Option<mpsc::Receiver<ScanMessage>>) {
    // 标记所有 Tab 为扫描中
    for tab_idx in 1..app.results.len() {
        app.scan_states[tab_idx] = ScanState::Scanning;
    }
    app.scan_progress = 0.0;

    let (tx, rx) = mpsc::channel();
    *scan_rx = Some(rx);

    // 进度估算线程
    let tx_progress = tx.clone();
    std::thread::spawn(move || {
        let start = std::time::Instant::now();
        loop {
            std::thread::sleep(std::time::Duration::from_millis(200));
            let elapsed = start.elapsed().as_secs_f32();
            let progress = if elapsed < 10.0 {
                0.9 * (1.0 - (-elapsed / 5.0).exp())
            } else if elapsed < 30.0 {
                0.9 + 0.09 * (1.0 - (-(elapsed - 10.0) / 20.0).exp())
            } else {
                0.99 + 0.005 * (1.0 - (-(elapsed - 30.0) / 20.0).exp())
            };
            if tx_progress
                .send(ScanMessage::Progress(progress.min(0.995)))
                .is_err()
            {
                break;
            }
        }
    });

    std::thread::spawn(move || {
        let tabs_to_scan: Vec<(Tab, u64)> = Tab::all()
            .iter()
            .enumerate()
            .skip(1) // 跳过 Overview
            .map(|(idx, tab)| (*tab, idx as u64))
            .collect();

        for (tab, tab_idx) in tabs_to_scan {
            // 检查缓存
            let cache_name = match tab {
                Tab::Overview | Tab::Settings => None,
                Tab::DevCache => Some("dev_cache"),
                Tab::LargeFiles => Some("large_files"),
                Tab::AppCache => Some("app_cache"),
                Tab::AppData => Some("app_data"),
                Tab::AppUninstall => Some("app_uninstall"),
                Tab::SystemOptimize => None,
                Tab::Apfs => Some("apfs"),
            };

            if let Some(name) = cache_name {
                if let Some(cached) = scanner::cache::load_cache(name) {
                    let mut items = cached.items;
                    for item in &mut items {
                        let (deletable, reason) = scanner::check_deletable(&item.path);
                        item.deletable = deletable && item.deletable;
                        if !item.deletable && !reason.is_empty() {
                            item.undeletable_reason = reason;
                        }
                    }
                    send_items_in_batches(&tx, &items, tab_idx);
                    let _ = tx.send(ScanMessage::Done(items, cached.scan_time_ms, tab_idx));
                    continue;
                }
            }

            let result = std::panic::catch_unwind(|| match tab {
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
            });

            match result {
                Ok(scan_result) => {
                    if let Some(name) = cache_name {
                        scanner::cache::save_cache(name, &scan_result);
                    }
                    let mut items = scan_result.items;
                    for item in &mut items {
                        let (deletable, reason) = scanner::check_deletable(&item.path);
                        item.deletable = deletable && item.deletable;
                        if !item.deletable && !reason.is_empty() {
                            item.undeletable_reason = reason;
                        }
                    }
                    send_items_in_batches(&tx, &items, tab_idx);
                    let _ = tx.send(ScanMessage::Done(items, scan_result.scan_time_ms, tab_idx));
                }
                Err(_) => {
                    let _ = tx.send(ScanMessage::Done(Vec::new(), 0, tab_idx));
                }
            }
        }

        let _ = tx.send(ScanMessage::AllDone);
    });
}

/// 将扫描结果按大小降序分批发送，实现增量显示
///
/// 排序后按每批最多 10 项分割，每批之间 sleep 50ms，
/// 让 UI 有机会渲染已发现的项。发送完毕后调用方再发送 Done
/// （Done 携带完整列表，会覆盖累积的部分项，保证最终结果一致）。
fn send_items_in_batches(tx: &mpsc::Sender<ScanMessage>, items: &[ScanItem], tab_idx: u64) {
    // 按大小降序排序，让用户先看到最大的项
    let mut sorted: Vec<ScanItem> = items.to_vec();
    sorted.sort_by(|a, b| b.size_bytes.cmp(&a.size_bytes));

    const BATCH_SIZE: usize = 10;
    for chunk in sorted.chunks(BATCH_SIZE) {
        // 报告当前扫描路径
        if let Some(last) = chunk.last() {
            let path = if last.path.is_empty() {
                last.category.clone()
            } else {
                last.path.clone()
            };
            let _ = tx.send(ScanMessage::CurrentPath(path));
        }
        let _ = tx.send(ScanMessage::PartialItems(chunk.to_vec(), tab_idx));
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
}

/// 尽力删除：优先用系统命令（对 node_modules 等大目录更快），
/// 失败则尝试解除只读标志后再删，再失败就放弃
fn best_effort_delete(path: &std::path::Path) -> bool {
    let path_str = path.to_string_lossy().to_string();

    #[cfg(target_os = "macos")]
    {
        // macOS: 优先用系统 rm -rf
        if std::process::Command::new("/bin/rm")
            .arg("-rf")
            .arg(&path_str)
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
            && !path.exists()
        {
            return true;
        }

        // rm -rf 失败，尝试解除可能存在的 immutable/只读标志后再删除
        let _ = std::process::Command::new("/usr/bin/chflags")
            .arg("-R")
            .arg("nouchg")
            .arg(&path_str)
            .output();
        let _ = std::process::Command::new("/bin/chmod")
            .arg("-R")
            .arg("u+w")
            .arg(&path_str)
            .output();

        let _ = std::process::Command::new("/bin/rm")
            .arg("-rf")
            .arg(&path_str)
            .output();

        !path.exists() && path.symlink_metadata().is_err()
    }

    #[cfg(target_os = "windows")]
    {
        // Windows: 用 rd /s /q 删除目录，del /f /q 删除文件
        let success = if path.is_dir() {
            std::process::Command::new("cmd")
                .arg("/C")
                .arg("rd")
                .arg("/S")
                .arg("/Q")
                .arg(&path_str)
                .status()
                .map(|s| s.success())
                .unwrap_or(false)
        } else {
            std::process::Command::new("cmd")
                .arg("/C")
                .arg("del")
                .arg("/F")
                .arg("/Q")
                .arg(&path_str)
                .status()
                .map(|s| s.success())
                .unwrap_or(false)
        };

        if success {
            return true;
        }

        // fallback: Rust API
        if path.is_dir() {
            std::fs::remove_dir_all(path).is_ok()
        } else {
            std::fs::remove_file(path).is_ok()
        }
    }

    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        if path.is_dir() {
            std::fs::remove_dir_all(path).is_ok()
        } else {
            std::fs::remove_file(path).is_ok()
        }
    }
}

/// 获取系统所有挂载点 (macOS 专属)
#[cfg(target_os = "macos")]
fn get_mount_points() -> Vec<String> {
    let output = std::process::Command::new("/sbin/mount").output();
    match output {
        Ok(o) if o.status.success() => {
            let stdout = String::from_utf8_lossy(&o.stdout);
            stdout
                .lines()
                .filter_map(|line| {
                    let parts: Vec<&str> = line.split_whitespace().collect();
                    if parts.len() >= 3 {
                        Some(parts[2].to_string())
                    } else {
                        None
                    }
                })
                .collect()
        }
        _ => Vec::new(),
    }
}

/// 检查路径是否被挂载使用（macOS 专属）
#[cfg(target_os = "macos")]
fn is_path_mounted(path: &str, mount_points: &[String]) -> bool {
    for mp in mount_points {
        if mp == path || mp.starts_with(&format!("{}/", path)) {
            return true;
        }
    }
    false
}

/// 移动文件/目录到废纸篓（可恢复）
/// 用于 Caution/Advanced 级别的文件，给用户后悔的机会
/// 移动文件到废纸篓（跨平台，委托给 platform 模块）
fn move_to_trash(path: &str) -> bool {
    platform::move_to_trash(path)
}

/// 从文本中提取所有 UUID（格式: xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx）
fn extract_all_uuids(text: &str) -> Vec<String> {
    let mut uuids = Vec::new();
    let chars: Vec<char> = text.chars().collect();
    let len = chars.len();
    let mut i = 0;
    while i + 36 <= len {
        // 检查 UUID 格式: 8-4-4-4-12
        if chars[i..i + 8].iter().all(|c| c.is_ascii_hexdigit())
            && chars[i + 8] == '-'
            && chars[i + 9..i + 13].iter().all(|c| c.is_ascii_hexdigit())
            && chars[i + 13] == '-'
            && chars[i + 14..i + 18].iter().all(|c| c.is_ascii_hexdigit())
            && chars[i + 18] == '-'
            && chars[i + 19..i + 23].iter().all(|c| c.is_ascii_hexdigit())
            && chars[i + 23] == '-'
            && chars[i + 24..i + 36].iter().all(|c| c.is_ascii_hexdigit())
        {
            let uuid: String = chars[i..i + 36].iter().collect();
            if !uuids.contains(&uuid) {
                uuids.push(uuid);
            }
            i += 36;
        } else {
            i += 1;
        }
    }
    uuids
}

/// 安全删除模拟器运行时镜像（/Library/Developer/CoreSimulator/Volumes）
///
/// 安全检查层：
/// 1. 进程检测：Xcode/Simulator/CoreSimulatorService 运行时拒绝删除
/// 2. 挂载点检测：正在挂载使用的运行时跳过，只删 UNUSED 的
/// 3. 使用 `xcrun simctl runtime delete <uuid>` 安全删除
/// 4. 如果 xcrun 失败，返回 Err 让调用方 fallback 到 sudo rm -rf（仅 UNUSED 项）
#[cfg(target_os = "macos")]
fn delete_simulator_volumes(path: &str, lang_en: bool) -> Result<String, String> {
    // 1. 进程检测：模拟器运行中时拒绝删除
    if safety::is_simulator_running() {
        return Err(App::t_lang(lang_en, "log_skip_running")
            .replace("[{}]", "")
            .replace("Xcode/Simulator", "Xcode/Simulator")
            .trim()
            .to_string());
    }

    // 2. 挂载点检测：如果路径被挂载使用，跳过
    let mount_points = get_mount_points();
    if mount_points.is_empty() {
        // mount 命令失败，无法确认安全，拒绝删除
        return Err(App::t_lang(lang_en, "log_cannot_get_mount").to_string());
    }
    if is_path_mounted(path, &mount_points) {
        return Err(App::t_lang(lang_en, "log_mount_in_use").to_string());
    }

    // 3. 列出所有运行时
    let list_output = std::process::Command::new("xcrun")
        .args(["simctl", "runtime", "list"])
        .output()
        .map_err(|e| format!("{}: {}", App::t_lang(lang_en, "log_unknown"), e))?;

    if !list_output.status.success() {
        let stderr = String::from_utf8_lossy(&list_output.stderr);
        return Err(format!("xcrun simctl runtime list: {}", stderr.trim()));
    }

    let stdout = String::from_utf8_lossy(&list_output.stdout);

    // 4. 提取所有 UUID（格式: xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx）
    let uuids: Vec<String> = extract_all_uuids(&stdout);

    if uuids.is_empty() {
        return Err(App::t_lang(lang_en, "log_no_sim_runtimes").to_string());
    }

    // 3. 逐个删除运行时
    let mut success_count = 0;
    let mut fail_msgs: Vec<String> = Vec::new();

    for uuid in &uuids {
        let del_output = std::process::Command::new("xcrun")
            .args(["simctl", "runtime", "delete", uuid])
            .output();

        match del_output {
            Ok(o) if o.status.success() => {
                success_count += 1;
            }
            Ok(o) => {
                let stderr = String::from_utf8_lossy(&o.stderr);
                // 如果是 "not found" 或 "already deleted" 类错误，算成功
                if stderr.contains("not found") || stderr.contains("No such") {
                    success_count += 1;
                } else {
                    fail_msgs.push(format!("{}: {}", uuid, stderr.trim()));
                }
            }
            Err(e) => {
                fail_msgs.push(format!("{}: {}", uuid, e));
            }
        }
    }

    if success_count > 0 {
        let suffix = if !fail_msgs.is_empty() {
            App::tf_lang(
                lang_en,
                "log_sim_failed_suffix",
                &[&fail_msgs.len().to_string()],
            )
        } else {
            String::new()
        };
        let msg = App::tf_lang(
            lang_en,
            "log_sim_deleted",
            &[&success_count.to_string(), &suffix],
        );
        Ok(msg)
    } else {
        Err(format!(
            "{}: {}",
            App::t_lang(lang_en, "log_unknown"),
            fail_msgs.join("; ")
        ))
    }
}

/// 执行 Docker 系统清理
///
/// 运行 `docker system prune -a --volumes -f` 清理:
/// - 所有已停止的容器
/// - 所有未被容器使用的网络
/// - 所有未被容器引用的镜像（dangling + unused）
/// - 所有未被容器使用的卷
/// - 所有构建缓存
fn run_docker_prune(lang_en: bool) -> Result<String, String> {
    // 先检查 Docker 是否运行
    let info_check = std::process::Command::new("docker")
        .arg("info")
        .output()
        .map_err(|e| format!("{}: {}", App::t_lang(lang_en, "log_unknown"), e))?;

    if !info_check.status.success() {
        return Err(App::t_lang(lang_en, "log_docker_not_running").to_string());
    }

    // 执行 prune（-f 跳过交互确认，-a 删除所有未使用镜像，--volumes 删除卷）
    let output = std::process::Command::new("docker")
        .args(["system", "prune", "-a", "--volumes", "-f"])
        .output()
        .map_err(|e| format!("{}: {}", App::t_lang(lang_en, "log_unknown"), e))?;

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();

    if !output.status.success() {
        return Err(format!("docker prune: {}", stderr.trim()));
    }

    // 解析输出中的释放空间
    // 输出包含: "Total reclaimed space: 1.2GB"
    let reclaimed = stdout
        .lines()
        .find(|line| line.contains("Total reclaimed space"))
        .map(|line| line.split(':').nth(1).unwrap_or("").trim().to_string())
        .unwrap_or_else(|| App::t_lang(lang_en, "log_unknown").to_string());

    Ok(App::tf_lang(lang_en, "log_docker_done", &[&reclaimed]))
}

/// 启动后台删除线程（两阶段自动删除）
/// 阶段1: 普通删除（多线程并行 rm -rf）
/// 阶段2: 对失败项自动 sudo 批量删除（后台并发，只弹一次密码框）
fn start_delete(
    to_delete: Vec<(String, String, Vec<String>, bool)>,
    lang_en: bool,
    delete_rx: &mut Option<mpsc::Receiver<DeleteMessage>>,
) {
    let (tx, rx) = mpsc::channel();
    *delete_rx = Some(rx);

    std::thread::spawn(move || {
        let failed_items: std::sync::Mutex<Vec<(String, String)>> =
            std::sync::Mutex::new(Vec::new());

        logger::info(&format!("删除任务开始: {} 项", to_delete.len()));

        // ========== 阶段1: 普通删除（多线程并行） ==========
        let worker_count = std::cmp::min(4, to_delete.len().max(1));
        let idx = std::sync::atomic::AtomicUsize::new(0);

        std::thread::scope(|s| {
            for _ in 0..worker_count {
                s.spawn(|| {
                    loop {
                        let i = idx.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        if i >= to_delete.len() { break; }
                        let (path, category, batch_paths, use_trash) = &to_delete[i];
                        let path = path.to_string();
                        let category = category.to_string();
                        let batch_paths = batch_paths.clone();
                        let use_trash = *use_trash;

                        // Windows 应用卸载特殊处理（干净卸载：卸载程序 + 扫描残留，不自动清理）
                        #[cfg(target_os = "windows")]
                        if path.starts_with("uwp:") || path.starts_with("uninstall:") {
                            logger::info(&format!("开始卸载应用: {}", path));

                            // 查找应用信息以支持干净卸载
                            let key_name = if path.starts_with("uninstall:") {
                                &path[10..]
                            } else {
                                ""
                            };
                            let app_info = scanner::windows_apps::find_app_by_key(key_name);
                            let app_name = app_info.as_ref().map(|a| a.name.as_str()).unwrap_or("");
                            let install_path = app_info.as_ref().and_then(|a| a.install_location.as_deref());

                            // 执行干净卸载（只卸载 + 扫描残留，不自动清理）
                            // 用户选择权：残留信息返回给用户，由用户决定是否清理
                            let (success, msg, residual) = scanner::windows_apps::clean_uninstall(
                                &path,
                                app_name,
                                install_path,
                            );

                            if success {
                                logger::info(&format!("卸载成功: {}", msg));
                            } else {
                                logger::error(&format!("卸载失败: {}", msg));
                            }

                            // 记录残留扫描详情（不自动清理，等待用户确认）
                            if !residual.is_empty() {
                                let fs_size: u64 = residual.filesystem.iter().map(|f| f.size).sum();
                                logger::info(&format!(
                                    "残留扫描结果: 注册表 {} 项 (可删 {}), 环境变量 {} 项 (可删 {}), 文件 {} 项 (可删 {}, 共 {}) — 等待用户选择",
                                    residual.registry.len(),
                                    residual.registry.iter().filter(|r| r.deletable).count(),
                                    residual.env_vars.len(),
                                    residual.env_vars.iter().filter(|e| e.deletable).count(),
                                    residual.filesystem.len(),
                                    residual.filesystem.iter().filter(|f| f.deletable).count(),
                                    format_size(fs_size),
                                ));
                                // 发送残留信息到 UI，弹出残留清理弹窗让用户选择
                                let _ = tx.send(DeleteMessage::ResidualFound(residual));
                            }

                            let _ = tx.send(DeleteMessage::Log(
                                if success { format!("✓ {}", msg) } else { format!("✗ {}", msg) },
                                path.clone(), category.clone(), success));
                            safety::log_deletion(&path, &category, success, None);
                            if !success {
                                failed_items.lock().unwrap().push((path.clone(), category.clone()));
                            }
                            continue;
                        }

                        // 批量删除模式（如 __pycache__）：逐个安全删除
                        if !batch_paths.is_empty() {
                            let mut success_count = 0;
                            let mut fail_count = 0;
                            for bp in &batch_paths {
                                match safety::check_path_safety_with_category(bp, &category) {
                                    safety::SafetyCheck::Danger(reason) => {
                                        fail_count += 1;
                                        let _ = tx.send(DeleteMessage::Log(
                                            format!("⛔ {}", App::tf_lang(lang_en, "log_intercepted", &[bp, &reason])), bp.clone(), category.clone(), false));
                                        safety::log_deletion(bp, &category, false, Some(&reason));
                                        continue;
                                    }
                                    safety::SafetyCheck::Warning(reason) => {
                                        fail_count += 1;
                                        let _ = tx.send(DeleteMessage::Log(
                                            format!("⚠️ {}", App::tf_lang(lang_en, "log_skipped", &[bp, &reason])), bp.clone(), category.clone(), false));
                                        safety::log_deletion(bp, &category, false, Some(&reason));
                                        continue;
                                    }
                                    safety::SafetyCheck::Safe => {}
                                }
                                let p = std::path::Path::new(bp.as_str());
                                if best_effort_delete(p) {
                                    success_count += 1;
                                } else {
                                    fail_count += 1;
                                    failed_items.lock().unwrap().push((bp.clone(), category.clone()));
                                }
                            }
                            let _ = tx.send(DeleteMessage::Log(
                                format!("✓ {}", App::tf_lang(lang_en, "log_deleted", &[&category, &path, &success_count.to_string(), &fail_count.to_string()])),
                                path.clone(), category.clone(), fail_count == 0));
                            safety::log_deletion(&path, &category, fail_count == 0, None);
                            continue;
                        }

                        // 安全校验
                        match safety::check_path_safety_with_category(&path, &category) {
                            safety::SafetyCheck::Danger(reason) => {
                                let _ = tx.send(DeleteMessage::Log(
                                    format!("⛔ {}", App::tf_lang(lang_en, "log_intercepted", &[&path, &reason])), path.clone(), category.clone(), false));
                                safety::log_deletion(&path, &category, false, Some(&reason));
                                continue;
                            }
                            safety::SafetyCheck::Warning(reason) => {
                                let _ = tx.send(DeleteMessage::Log(
                                    format!("⚠️ {}", App::tf_lang(lang_en, "log_skipped", &[&path, &reason])), path.clone(), category.clone(), false));
                                safety::log_deletion(&path, &category, false, Some(&reason));
                                continue;
                            }
                            safety::SafetyCheck::Safe => {}
                        }

                        // APFS 快照特殊处理 (macOS 专属)
                        if cfg!(target_os = "macos") && category == "APFS快照" {
                            #[cfg(target_os = "macos")]
                            {
                                match scanner::apfs::delete_snapshot(&path) {
                                    Ok(_) => {
                                        let _ = tx.send(DeleteMessage::Log(
                                            format!("✓ {}", App::tf_lang(lang_en, "log_snapshot_deleted", &[&path])), path.clone(), category.clone(), true));
                                        safety::log_deletion(&path, &category, true, None);
                                    }
                                    Err(e) => {
                                        let _ = tx.send(DeleteMessage::Log(
                                            format!("✗ {}", App::tf_lang(lang_en, "log_delete_failed", &[&path, &e])), path.clone(), category.clone(), false));
                                        safety::log_deletion(&path, &category, false, Some(&e));
                                    }
                                }
                            }
                            continue;
                        }

                        if cfg!(target_os = "macos") && category == "模拟器运行时" {
                            #[cfg(target_os = "macos")]
                            {
                                match scanner::apfs::delete_simulator_runtime(&path) {
                                    Ok(_) => {
                                        let _ = tx.send(DeleteMessage::Log(
                                            format!("✓ {}", App::tf_lang(lang_en, "log_runtime_deleted", &[&path])), path.clone(), category.clone(), true));
                                        safety::log_deletion(&path, &category, true, None);
                                    }
                                    Err(e) => {
                                        let _ = tx.send(DeleteMessage::Log(
                                            format!("✗ {}", App::tf_lang(lang_en, "log_delete_failed", &[&path, &e])), path.clone(), category.clone(), false));
                                        safety::log_deletion(&path, &category, false, Some(&e));
                                    }
                                }
                            }
                            continue;
                        }

                        // 模拟器镜像/Cryptex — 通过 xcrun simctl runtime delete 安全删除 (macOS 专属)
                        if cfg!(target_os = "macos") && (category == "模拟器镜像" || category == "模拟器Cryptex") {
                            #[cfg(target_os = "macos")]
                            {
                                match delete_simulator_volumes(&path, lang_en) {
                                    Ok(msg) => {
                                        let _ = tx.send(DeleteMessage::Log(
                                            format!("✓ {}", msg), path.clone(), category.clone(), true));
                                        safety::log_deletion(&path, &category, true, None);
                                    }
                                    Err(e) => {
                                        // xcrun 失败，加入 sudo 重试列表
                                        failed_items.lock().unwrap().push((path.clone(), category.clone()));
                                        let _ = tx.send(DeleteMessage::Info(
                                            format!("🔄 {}", App::tf_lang(lang_en, "log_xcrun_failed", &[&e])),
                                        ));
                                    }
                                }
                            }
                            continue;
                        }

                        // 模拟器缓存 — 进程检测后删除 (macOS 专属)
                        if cfg!(target_os = "macos") && category == "模拟器缓存" {
                            #[cfg(target_os = "macos")]
                            {
                                if safety::is_simulator_running() {
                                    let _ = tx.send(DeleteMessage::Log(
                                        format!("⏭️ {}", App::tf_lang(lang_en, "log_skip_running", &[&path])), path.clone(), category.clone(), false));
                                    safety::log_deletion(&path, &category, false, Some(&App::t_lang(lang_en, "log_skip_running").replace("[{}]", "").trim().to_string()));
                                    continue;
                                }
                            }
                            // 走普通删除流程（会自动 fallback 到 sudo）
                        }

                        // Docker 清理 — 通过 docker system prune 命令清理
                        if category == "Docker清理" {
                            match run_docker_prune(lang_en) {
                                Ok(msg) => {
                                    let _ = tx.send(DeleteMessage::Log(
                                        format!("✓ {}", msg), path.clone(), category.clone(), true));
                                    safety::log_deletion(&path, &category, true, None);
                                }
                                Err(e) => {
                                    let _ = tx.send(DeleteMessage::Log(
                                        format!("✗ {}", App::tf_lang(lang_en, "log_docker_failed", &[&e])), path.clone(), category.clone(), false));
                                    safety::log_deletion(&path, &category, false, Some(&e));
                                }
                            }
                            continue;
                        }

                        // 普通文件/目录删除 - 尽力删除模式
                        let p = std::path::Path::new(path.as_str());

                        if !p.exists() && !p.symlink_metadata().is_ok() {
                            // 路径已不存在：视为删除成功（幂等性）。
                            // 用户想要的结果就是该路径消失，现在目标已经达成，
                            // 无需因缓存过期或外部已删除而报错。
                            let _ = tx.send(DeleteMessage::Log(
                                format!("✓ {} ({})", App::tf_lang(lang_en, "log_path_not_exist", &[&path]),
                                    App::t_lang(lang_en, "already_cleaned")), path.clone(), category.clone(), true));
                            safety::log_deletion(&path, &category, true, None);
                            continue;
                        }

                        // 拒绝删除符号链接
                        if let Ok(meta) = p.symlink_metadata() {
                            if meta.file_type().is_symlink() {
                                let _ = tx.send(DeleteMessage::Log(
                                    format!("⛔ {}", App::tf_lang(lang_en, "log_symlink_rejected", &[&path])), path.clone(), category.clone(), false));
                                safety::log_deletion(&path, &category, false, Some(&App::t_lang(lang_en, "log_symlink_rejected").replace("{}", "").trim().to_string()));
                                continue;
                            }
                        }

                        // 尽力删除：废纸篓模式或永久删除
                        let deleted_ok = if use_trash {
                            // 移至废纸篓（可恢复）
                            if move_to_trash(&path) {
                                true
                            } else {
                                // 废纸篓失败，尝试永久删除
                                best_effort_delete(p)
                            }
                        } else {
                            // 永久删除
                            best_effort_delete(p)
                        };

                        if deleted_ok {
                            let action = App::t_lang(lang_en, if use_trash { "log_action_trashed" } else { "log_action_deleted" });
                            let _ = tx.send(DeleteMessage::Log(
                                format!("✓ {} [{}] {}", action, category, path), path.clone(), category.clone(), true));
                            safety::log_deletion(&path, &category, true, None);
                        } else {
                            // 普通删除失败，加入待 sudo 列表
                            failed_items.lock().unwrap().push((path, category));
                        }
                    }
                });
            }
        });

        let failed_items: Vec<(String, String)> = failed_items.into_inner().unwrap();

        logger::info(&format!("阶段1删除完成, 失败 {} 项", failed_items.len()));

        // 普通删除完成后，若还有失败项，通知 GUI 弹出 egui 内置密码输入框
        if !failed_items.is_empty() {
            let _ = tx.send(DeleteMessage::Info(format!(
                "🔐 {}",
                App::tf_lang(lang_en, "log_need_sudo", &[&failed_items.len().to_string()])
            )));
            let _ = tx.send(DeleteMessage::NeedPassword(failed_items));
            return;
        }

        let _ = tx.send(DeleteMessage::Done);
    });
}

/// 启动后台 sudo 删除线程
/// 使用 egui 内置输入框收集到的密码，通过 sudo -S 的 stdin 传入，
/// 避免调用 System Events / osascript 触发钥匙串弹窗。
fn start_sudo_delete(
    failed_items: Vec<(String, String)>,
    password: String,
    lang_en: bool,
    delete_rx: &mut Option<mpsc::Receiver<DeleteMessage>>,
) {
    if failed_items.is_empty() {
        return;
    }

    let (tx, rx) = mpsc::channel();
    *delete_rx = Some(rx);

    std::thread::spawn(move || {
        let _ = tx.send(DeleteMessage::Info(format!(
            "🔐 {}",
            App::t_lang(lang_en, "log_sudo_phase")
        )));

        let sudo_debug_log = std::env::temp_dir().join("maclean_sudo_debug.log");
        let mut debug_entries: Vec<String> = Vec::new();
        debug_entries.push(format!(
            "[sudo phase] started, {} items",
            failed_items.len()
        ));
        for (p, c) in &failed_items {
            debug_entries.push(format!("  item: [{}] {}", c, p));
        }

        // ========== 预处理：模拟器镜像通过 sudo xcrun simctl runtime delete 删除 ==========
        let mut remaining_items: Vec<(String, String)> = Vec::new();
        for (path, category) in &failed_items {
            if category == "模拟器镜像" || category == "模拟器Cryptex" {
                // 用 sudo xcrun simctl runtime delete 删除所有运行时
                let script = r#"#!/bin/bash
set +e
uuids=$(xcrun simctl runtime list 2>/dev/null | grep -oE '[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}' | sort -u)
count=0
fail=0
for uuid in $uuids; do
    xcrun simctl runtime delete "$uuid" 2>/dev/null
    rc=$?
    if [ $rc -eq 0 ]; then
        count=$((count + 1))
    else
        fail=$((fail + 1))
    fi
done
echo "xcrun_deleted:$count"
echo "xcrun_failed:$fail"
exit 0
"#;
                let xcrun_script = std::env::temp_dir().join("maclean_xcrun_delete.sh");
                let _ = std::fs::write(&xcrun_script, script);
                let _ = std::process::Command::new("/bin/chmod")
                    .arg("+x")
                    .arg(&xcrun_script)
                    .output();

                let xcrun_result = std::process::Command::new("/usr/bin/sudo")
                    .arg("-S")
                    .arg("/bin/bash")
                    .arg(&xcrun_script)
                    .stdin(std::process::Stdio::piped())
                    .stdout(std::process::Stdio::piped())
                    .stderr(std::process::Stdio::piped())
                    .spawn()
                    .and_then(|mut child| {
                        if let Some(mut stdin) = child.stdin.take() {
                            use std::io::Write;
                            let _ = writeln!(stdin, "{}", password);
                        }
                        child.wait_with_output()
                    });

                let mut xcrun_success = false;
                match &xcrun_result {
                    Ok(output) => {
                        let stdout = String::from_utf8_lossy(&output.stdout);
                        let stderr = String::from_utf8_lossy(&output.stderr);
                        debug_entries.push(format!("xcrun sudo stdout: {}", stdout));
                        debug_entries.push(format!("xcrun sudo stderr: {}", stderr));

                        // 检查是否密码错误
                        if stderr.contains("incorrect password") || stderr.contains("3 incorrect") {
                            let _ = tx.send(DeleteMessage::Info(format!(
                                "🔒 {}",
                                App::t_lang(lang_en, "log_password_wrong")
                            )));
                            let _ = tx.send(DeleteMessage::NeedPassword(failed_items.clone()));
                            let _ = std::fs::write(&sudo_debug_log, debug_entries.join("\n"));
                            let _ = std::fs::remove_file(&xcrun_script);
                            return;
                        }

                        // 检查 Volumes 目录是否已清空
                        let p = std::path::Path::new(path);
                        if !p.exists()
                            || std::fs::read_dir(p)
                                .map(|mut d| d.next().is_none())
                                .unwrap_or(true)
                        {
                            xcrun_success = true;
                            let _ = tx.send(DeleteMessage::Log(
                                format!(
                                    "✓ {}",
                                    App::tf_lang(
                                        lang_en,
                                        "log_touchid_runtime_deleted_path",
                                        &[path]
                                    )
                                ),
                                path.clone(),
                                category.clone(),
                                true,
                            ));
                            safety::log_deletion(path, category, true, None);
                        }
                    }
                    Err(e) => {
                        debug_entries.push(format!("xcrun sudo error: {}", e));
                    }
                }

                let _ = std::fs::remove_file(&xcrun_script);

                if !xcrun_success {
                    // xcrun 删除后仍有残留，用 sudo rm -rf 清理
                    remaining_items.push((path.clone(), category.clone()));
                }
            } else {
                remaining_items.push((path.clone(), category.clone()));
            }
        }

        let failed_items = remaining_items;

        if failed_items.is_empty() {
            let _ = std::fs::write(&sudo_debug_log, debug_entries.join("\n"));
            let _ = tx.send(DeleteMessage::Done);
            return;
        }

        // 写临时删除脚本：所有目录后台并行删除
        let tmp_script = std::env::temp_dir().join("maclean_sudo_delete.sh");
        let current_user = std::env::var("USER")
            .or_else(|_| std::env::var("LOGNAME"))
            .unwrap_or_else(|_| "root".to_string());
        let mut script_content = String::from("#!/bin/bash\nset +e\n");
        script_content.push_str("workdir=$(/usr/bin/mktemp -d)\n");
        script_content.push_str("trap \"/bin/rm -rf \\\"$workdir\\\"\" EXIT\n\n");
        script_content.push_str("process_one() {\n");
        script_content.push_str("  local idx=\"$1\"\n");
        script_content.push_str("  local path=\"$2\"\n");
        script_content.push_str("  local out=\"$workdir/${idx}.out\"\n");
        script_content.push_str("  echo \">MACLEAN_BEGIN:$path\" > \"$out\"\n");
        script_content.push_str("  /usr/bin/chflags -R nouchg \"$path\" 2>/dev/null\n");
        script_content.push_str("  /usr/sbin/chown -R '");
        script_content.push_str(&current_user.replace("'", "'\\''"));
        script_content.push_str(":staff' \"$path\" 2>/dev/null\n");
        script_content.push_str("  /bin/chmod -R u+w \"$path\" 2>/dev/null\n");
        script_content.push_str("  /bin/rm -rf \"$path\" 2>&1 >> \"$out\"\n");
        script_content.push_str("  echo \">MACLEAN_EXIT:$path:$?\" >> \"$out\"\n");
        script_content.push_str("}\n\n");

        for (i, (path, _)) in failed_items.iter().enumerate() {
            let escaped = path.replace("'", "'\\''");
            script_content.push_str(&format!("process_one {} '{}' &\n", i, escaped));
        }
        script_content.push_str("\nwait\n");
        script_content
            .push_str("for f in \"$workdir\"/*.out; do [ -f \"$f\" ] && /bin/cat \"$f\"; done\n");
        script_content.push_str("exit 0\n");
        let _ = std::fs::write(&tmp_script, &script_content);
        let _ = std::process::Command::new("/bin/chmod")
            .arg("+x")
            .arg(&tmp_script)
            .output();

        debug_entries.push(format!("delete script: {}", tmp_script.display()));
        debug_entries.push("--- delete script content ---".to_string());
        debug_entries.push(script_content.clone());

        // 使用 sudo -S，通过 stdin 传入密码，避免钥匙串弹窗
        let sudo_result = std::process::Command::new("/usr/bin/sudo")
            .arg("-S")
            .arg("/bin/bash")
            .arg(&tmp_script)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .and_then(|mut child| {
                if let Some(mut stdin) = child.stdin.take() {
                    use std::io::Write;
                    let _ = writeln!(stdin, "{}", password);
                }
                child.wait_with_output()
            });

        // 解析脚本输出，按路径收集错误信息
        let mut rm_stderr: std::collections::HashMap<String, String> =
            std::collections::HashMap::new();
        match &sudo_result {
            Ok(output) => {
                let stdout = String::from_utf8_lossy(&output.stdout);
                let stderr = String::from_utf8_lossy(&output.stderr);
                debug_entries.push(format!("sudo exit code: {:?}", output.status.code()));
                debug_entries.push(format!("sudo stdout:\n{}", stdout));
                debug_entries.push(format!("sudo stderr:\n{}", stderr));
                let mut current_path = String::new();
                for line in stdout.lines() {
                    if let Some(p) = line.strip_prefix(">MACLEAN_BEGIN:") {
                        current_path = p.to_string();
                    } else if let Some(_rest) = line.strip_prefix(">MACLEAN_EXIT:") {
                        current_path.clear();
                    } else if !line.is_empty() && !current_path.is_empty() {
                        rm_stderr
                            .entry(current_path.clone())
                            .or_default()
                            .push_str(line);
                        rm_stderr
                            .entry(current_path.clone())
                            .or_default()
                            .push('\n');
                    }
                }
            }
            Err(e) => {
                debug_entries.push(format!("sudo spawn error: {}", e));
            }
        }

        // 写入调试日志
        let _ = std::fs::write(&sudo_debug_log, debug_entries.join("\n"));

        // 逐项验证删除结果
        match sudo_result {
            Ok(output) => {
                let stderr_all = String::from_utf8_lossy(&output.stderr);
                let sudo_failed = !output.status.success();
                let password_error = stderr_all.contains("sudo: 3 incorrect password attempts")
                    || stderr_all.contains("incorrect password");
                let user_cancelled = !password_error
                    && (stderr_all.contains("sudo: a password is required")
                        || stderr_all.contains("User canceled"));

                if password_error {
                    let _ = tx.send(DeleteMessage::Info(format!(
                        "🔒 {}",
                        App::t_lang(lang_en, "log_password_wrong")
                    )));
                    // 密码错误时 sudo 不会执行任何删除，全部项都需要重试
                    let _ = tx.send(DeleteMessage::NeedPassword(failed_items));
                    let _ = std::fs::remove_file(&tmp_script);
                    return;
                }

                for (path, category) in &failed_items {
                    let p = std::path::Path::new(path.as_str());
                    if !p.exists() && p.symlink_metadata().is_err() {
                        let _ = tx.send(DeleteMessage::Log(
                            format!(
                                "✓ {}",
                                App::tf_lang(lang_en, "log_deleted_sudo", &[category, path])
                            ),
                            path.clone(),
                            category.clone(),
                            true,
                        ));
                        safety::log_deletion(path, category, true, None);
                        continue;
                    }

                    // 仍在：判断是 SIP 保护还是普通权限/占用问题
                    let err_text = rm_stderr
                        .get(path)
                        .map(|s| s.as_str())
                        .unwrap_or(&stderr_all);
                    let is_sip = err_text.contains("Operation not permitted")
                        || std::process::Command::new("/usr/bin/xattr")
                            .arg(path)
                            .output()
                            .map(|o| {
                                String::from_utf8_lossy(&o.stdout).contains("com.apple.provenance")
                            })
                            .unwrap_or(false)
                        || path.starts_with("/Library/Developer/CoreSimulator/Caches");

                    if is_sip {
                        let _ = tx.send(DeleteMessage::Log(
                            format!("🔒 {}", App::tf_lang(lang_en, "log_sip_protected", &[path])),
                            path.clone(),
                            category.clone(),
                            false,
                        ));
                        safety::log_deletion(
                            path,
                            category,
                            false,
                            Some(App::t_lang(lang_en, "log_sip_reason")),
                        );
                    } else if user_cancelled {
                        let _ = tx.send(DeleteMessage::Log(
                            format!("✗ {}", App::tf_lang(lang_en, "log_cancelled_auth", &[path])),
                            path.clone(),
                            category.clone(),
                            false,
                        ));
                        safety::log_deletion(
                            path,
                            category,
                            false,
                            Some(App::t_lang(lang_en, "log_cancel_reason")),
                        );
                    } else if sudo_failed {
                        let detail = if stderr_all.is_empty() {
                            App::tf_lang(
                                lang_en,
                                "log_exit_code",
                                &[&output.status.code().unwrap_or(-1).to_string()],
                            )
                        } else {
                            stderr_all.trim().to_string()
                        };
                        let _ = tx.send(DeleteMessage::Log(
                            format!(
                                "✗ {}",
                                App::tf_lang(lang_en, "log_delete_failed", &[path, &detail])
                            ),
                            path.clone(),
                            category.clone(),
                            false,
                        ));
                        safety::log_deletion(path, category, false, Some(&detail));
                    } else {
                        let detail = if err_text.is_empty() {
                            App::t_lang(lang_en, "log_still_exists").to_string()
                        } else {
                            err_text.trim().to_string()
                        };
                        let _ = tx.send(DeleteMessage::Log(
                            format!(
                                "✗ {}",
                                App::tf_lang(lang_en, "log_delete_failed", &[path, &detail])
                            ),
                            path.clone(),
                            category.clone(),
                            false,
                        ));
                        safety::log_deletion(path, category, false, Some(&detail));
                    }
                }
            }
            Err(e) => {
                for (path, category) in &failed_items {
                    let _ = tx.send(DeleteMessage::Log(
                        format!(
                            "✗ {}",
                            App::tf_lang(lang_en, "log_cannot_start_sudo", &[path, &e.to_string()])
                        ),
                        path.clone(),
                        category.clone(),
                        false,
                    ));
                    safety::log_deletion(path, category, false, Some(&e.to_string()));
                }
            }
        }

        // 清理临时脚本
        let _ = std::fs::remove_file(&tmp_script);

        // 如果全部失败项仍在且不是因为密码错误，正常结束
        let _ = tx.send(DeleteMessage::Done);
    });
}

/// 权限引导弹窗
fn show_permission_guide_window(ctx: &egui::Context, app: &mut App) {
    egui::Window::new(app.t("permission_title"))
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ctx, |ui| {
            ui.set_min_width(480.0);
            ui.set_max_width(520.0);
            ui.add_space(10.0);
            ui.vertical(|ui| {
                // 标题
                ui.horizontal(|ui| {
                    ui.colored_label(
                        BRAND,
                        egui::RichText::new("🔐").size(28.0),
                    );
                    ui.label(egui::RichText::new(app.t("permission_headline")).size(18.0).strong());
                });

                ui.add_space(8.0);
                ui.separator();
                ui.add_space(8.0);

                // 说明
                ui.colored_label(
                    TEXT_PRIMARY,
                    egui::RichText::new(app.t("permission_desc")).size(13.0),
                );
                ui.add_space(3.0);
                ui.colored_label(
                    TEXT_SECONDARY,
                    egui::RichText::new(app.t("permission_sub_desc")).size(12.0),
                );

                ui.add_space(10.0);

                // 步骤
                ui.colored_label(
                    SAFE_COLOR,
                    egui::RichText::new(app.t("permission_steps_title")).size(13.0).strong(),
                );
                ui.add_space(5.0);

                let steps = [
                    app.t("permission_step1"),
                    app.t("permission_step2"),
                    app.t("permission_step3"),
                    app.t("permission_step4"),
                    app.t("permission_step5"),
                ];
                for (i, step) in steps.iter().enumerate() {
                    ui.horizontal(|ui| {
                        ui.colored_label(
                            SAFE_COLOR,
                            egui::RichText::new(format!("{}. ", i + 1)).size(13.0),
                        );
                        ui.colored_label(
                            TEXT_PRIMARY,
                            egui::RichText::new(*step).size(13.0),
                        );
                    });
                }

                ui.add_space(12.0);

                // 按钮
                ui.horizontal(|ui| {
                    let btn = ui.add(
                        egui::Button::new(
                            egui::RichText::new(format!("⚙️ {}", app.t("permission_open_settings")))
                                .color(egui::Color32::WHITE)
                                .size(14.0)
                        )
                        .fill(egui::Color32::from_rgb(0, 122, 255))
                    );
                    if btn.clicked() {
                        let _ = std::process::Command::new("open")
                            .arg("x-apple.systempreferences:com.apple.preference.security?Privacy_AllFiles")
                            .spawn();
                    }

                    ui.add_space(10.0);

                    // 安装到 /Applications 按钮
                    let install_btn = ui.add(
                        egui::Button::new(
                            egui::RichText::new(format!("📦 {}", app.t("permission_install_app")))
                                .color(egui::Color32::WHITE)
                                .size(14.0)
                        )
                        .fill(SAFE_COLOR)
                    );
                    if install_btn.clicked() {
                        // 获取当前应用路径
                        if let Ok(exe_path) = std::env::current_exe() {
                            if let Some(app_path) = exe_path.ancestors().nth(2) {
                                let dest = "/Applications/maclean.app";
                                let src = app_path.to_string_lossy().to_string();
                                // 用 osascript 执行复制（需要管理员权限写入 /Applications）
                                let script = format!(
                                    "do shell script \"cp -R '{}' '{}'\" with administrator privileges",
                                    src.replace("'", "'\\''"),
                                    dest
                                );
                                let _ = std::process::Command::new("osascript")
                                    .arg("-e")
                                    .arg(&script)
                                    .output();
                            }
                        }
                    }

                    ui.add_space(10.0);

                    if ui.button(egui::RichText::new(app.t("permission_later")).size(14.0)).clicked() {
                        app.dismiss_permission_guide();
                    }
                });

                ui.add_space(5.0);
                ui.colored_label(
                    TEXT_TERTIARY,
                    egui::RichText::new(app.t("permission_hint")).size(11.0),
                );
            });
        });
}

/// 删除确认弹窗（设计稿 4.4 样式）
fn show_confirm_window(
    ctx: &egui::Context,
    app: &mut App,
    delete_rx: &mut Option<mpsc::Receiver<DeleteMessage>>,
) {
    let count = app.selected_count();
    let size = app.selected_total_size();
    let idx = app.tab_index();

    let mut safe_cnt = 0usize;
    let mut safe_sz = 0u64;
    let mut caution_cnt = 0usize;
    let mut caution_sz = 0u64;
    let mut advanced_cnt = 0usize;
    let mut advanced_sz = 0u64;
    let mut needs_admin = false;

    for item in &app.results[idx] {
        if !item.selected || !item.deletable {
            continue;
        }
        match item.recommend {
            crate::scanner::Recommend::Safe | crate::scanner::Recommend::CacheOnly => {
                safe_cnt += 1;
                safe_sz += item.size_bytes;
            }
            crate::scanner::Recommend::Caution => {
                caution_cnt += 1;
                caution_sz += item.size_bytes;
            }
            crate::scanner::Recommend::Advanced => {
                advanced_cnt += 1;
                advanced_sz += item.size_bytes;
            }
        }
        if item.path.starts_with("/Library")
            || item.path.contains("CoreSimulator")
            || item.category.contains("模拟器")
            || item.category.contains("系统")
        {
            needs_admin = true;
        }
    }
    if !App::check_full_disk_access() {
        needs_admin = true;
    }

    egui::Window::new(app.t("confirm_delete"))
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ctx, |ui| {
            ui.set_min_width(420.0);
            ui.set_max_width(480.0);

            // Header
            ui.vertical(|ui| {
                ui.label(
                    egui::RichText::new(format!("🗑 {}", app.t("confirm_delete")))
                        .size(15.0)
                        .strong()
                        .color(TEXT_PRIMARY),
                );
                ui.colored_label(
                    TEXT_SECONDARY,
                    egui::RichText::new(app.t("confirm_subtitle")).size(12.0),
                );
            });

            ui.add_space(12.0);

            // Body rows
            egui::Frame::none()
                .fill(SURFACE_MUTED)
                .stroke(egui::Stroke::NONE)
                .rounding(egui::Rounding::same(8.0))
                .inner_margin(egui::Margin::same(12.0))
                .show(ui, |ui| {
                    render_confirm_row(
                        ui,
                        app.t("confirm_selected_items"),
                        &format!("{} {}", count, app.t("items")),
                        TEXT_PRIMARY,
                    );
                    render_confirm_row(ui, app.t("confirm_releasable"), &format_size(size), BRAND);
                    if safe_cnt > 0 {
                        render_confirm_row(
                            ui,
                            app.t("confirm_safe"),
                            &format!("{} {} · {}", safe_cnt, app.t("items"), format_size(safe_sz)),
                            SAFE_COLOR,
                        );
                    }
                    if caution_cnt > 0 {
                        render_confirm_row(
                            ui,
                            app.t("confirm_caution"),
                            &format!(
                                "{} {} · {}",
                                caution_cnt,
                                app.t("items"),
                                format_size(caution_sz)
                            ),
                            CAUTION_COLOR,
                        );
                    }
                    if advanced_cnt > 0 {
                        render_confirm_row(
                            ui,
                            app.t("confirm_advanced"),
                            &format!(
                                "{} {} · {}",
                                advanced_cnt,
                                app.t("items"),
                                format_size(advanced_sz)
                            ),
                            ADVANCED_COLOR,
                        );
                    }
                    render_confirm_row(
                        ui,
                        app.t("confirm_admin_required"),
                        if needs_admin {
                            app.t("confirm_admin_yes")
                        } else {
                            app.t("confirm_admin_no")
                        },
                        TEXT_PRIMARY,
                    );
                });

            ui.add_space(8.0);

            // 预览折叠
            if ui
                .button(egui::RichText::new(format!("🔍 {}", app.t("confirm_preview"))).size(12.0))
                .clicked()
            {
                app.show_preview = !app.show_preview;
            }
            if app.show_preview {
                ui.add_space(4.0);
                egui::Frame::none()
                    .stroke(egui::Stroke::new(1.0, BORDER_LIGHT))
                    .rounding(egui::Rounding::same(6.0))
                    .inner_margin(egui::Margin::same(8.0))
                    .show(ui, |ui| {
                        ui.set_max_height(150.0);
                        egui::ScrollArea::vertical().show(ui, |ui| {
                            let items: Vec<_> = app.results[idx]
                                .iter()
                                .filter(|item| item.selected && item.deletable)
                                .collect();
                            for item in &items {
                                ui.horizontal(|ui| {
                                    let (fg, _, badge) = recommend_badge_colors(&item.recommend);
                                    render_status_badge(ui, badge, fg, SURFACE_MUTED);
                                    ui.colored_label(
                                        TEXT_PRIMARY,
                                        egui::RichText::new(format_size(item.size_bytes))
                                            .size(12.0)
                                            .monospace(),
                                    );
                                    let cat = i18n::translate_category(&item.category, app.lang_en);
                                    ui.colored_label(
                                        TEXT_SECONDARY,
                                        egui::RichText::new(&cat).size(12.0),
                                    );
                                    ui.colored_label(
                                        TEXT_TERTIARY,
                                        egui::RichText::new(truncate_path(&item.path, 50))
                                            .size(11.0)
                                            .monospace(),
                                    );
                                });
                            }
                            ui.add_space(3.0);
                            let summary_text = App::tf_lang(
                                app.lang_en,
                                "confirm_preview_summary",
                                &[&items.len().to_string()],
                            );
                            ui.colored_label(
                                TEXT_TERTIARY,
                                egui::RichText::new(summary_text).size(11.0),
                            );
                        });
                    });
            }

            ui.add_space(12.0);

            // Footer
            ui.horizontal(|ui| {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let danger_btn = ui.add(
                        egui::Button::new(
                            egui::RichText::new(format!("🗑 {}", app.t("confirm_delete")))
                                .color(egui::Color32::WHITE)
                                .size(13.0),
                        )
                        .fill(DANGER_COLOR)
                        .rounding(egui::Rounding::same(8.0)),
                    );
                    if danger_btn.clicked() {
                        app.show_preview = false;
                        let to_delete = app.confirm_delete();
                        start_delete(to_delete, app.lang_en, delete_rx);
                    }

                    ui.add_space(8.0);

                    if ui
                        .add(
                            egui::Button::new(egui::RichText::new(app.t("cancel")).size(13.0))
                                .fill(SURFACE_ELEVATED)
                                .stroke(egui::Stroke::new(1.0, BORDER_LIGHT))
                                .rounding(egui::Rounding::same(8.0)),
                        )
                        .clicked()
                    {
                        app.show_preview = false;
                        app.cancel_delete();
                    }
                });
            });
        });
}

/// 确认弹窗中的单行 Key-Value
fn render_confirm_row(ui: &mut egui::Ui, label: &str, value: &str, value_color: egui::Color32) {
    ui.horizontal(|ui| {
        ui.colored_label(TEXT_SECONDARY, egui::RichText::new(label).size(13.0));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.colored_label(value_color, egui::RichText::new(value).size(13.0).strong());
        });
    });
    ui.add_space(2.0);
    let available = ui.available_rect_before_wrap();
    let sep_y = available.min.y;
    let sep_rect = egui::Rect::from_min_max(
        egui::pos2(available.min.x, sep_y),
        egui::pos2(available.max.x, sep_y + 1.0),
    );
    ui.painter()
        .rect_filled(sep_rect, egui::Rounding::ZERO, BORDER_LIGHT);
    ui.add_space(4.0);
}

/// sudo 密码输入弹窗（egui 内置输入框）
///
/// 两种模式：
/// - 正常删除：输入密码后直接 sudo -S 删除 failed_items
/// - Touch ID 启用模式（touch_id_setup_mode=true）：输入密码后先创建 sudo_local，
///   启用成功再继续删除（后续 sudo 会触发 Touch ID）
fn show_sudo_password_window(
    ctx: &egui::Context,
    app: &mut App,
    delete_rx: &mut Option<mpsc::Receiver<DeleteMessage>>,
) {
    let is_setup_mode = app.touch_id_setup_mode;

    let window_title = if is_setup_mode {
        app.t("sudo_title_setup")
    } else {
        app.t("sudo_title_delete")
    };
    let headline = if is_setup_mode {
        app.t("sudo_title_setup")
    } else {
        app.t("sudo_title_delete")
    };
    let emoji = if is_setup_mode { "👆" } else { "🔐" };
    let description = if is_setup_mode {
        app.t("sudo_desc_setup").to_string()
    } else {
        App::tf_lang(
            app.lang_en,
            "sudo_desc_delete",
            &[&app.sudo_failed_items.len().to_string()],
        )
    };
    let button_text = if is_setup_mode {
        app.t("sudo_confirm_setup")
    } else {
        app.t("sudo_confirm_delete")
    };

    egui::Window::new(window_title)
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ctx, |ui| {
            ui.set_min_width(420.0);
            ui.set_max_width(480.0);
            ui.add_space(10.0);

            ui.vertical(|ui| {
                ui.horizontal(|ui| {
                    ui.colored_label(CAUTION_COLOR, egui::RichText::new(emoji).size(28.0));
                    ui.label(egui::RichText::new(headline).size(18.0).strong());
                });

                ui.add_space(8.0);
                ui.colored_label(TEXT_PRIMARY, egui::RichText::new(description).size(13.0));
                if !is_setup_mode {
                    ui.add_space(2.0);
                    ui.colored_label(
                        TEXT_SECONDARY,
                        egui::RichText::new(app.t("sudo_password_note")).size(12.0),
                    );
                }

                if let Some(ref err) = app.sudo_error {
                    ui.add_space(8.0);
                    ui.colored_label(ADVANCED_COLOR, egui::RichText::new(err).size(13.0).strong());
                }

                ui.add_space(12.0);

                // 密码输入框
                let password_hint = App::t_lang(app.lang_en, "sudo_password_hint");
                ui.add(
                    egui::TextEdit::singleline(&mut app.sudo_password_input)
                        .password(true)
                        .hint_text(password_hint)
                        .desired_width(360.0),
                );

                ui.add_space(15.0);

                ui.horizontal(|ui| {
                    let confirm_enabled = !app.sudo_password_input.is_empty();
                    if ui
                        .add_enabled(
                            confirm_enabled,
                            egui::Button::new(
                                egui::RichText::new(button_text)
                                    .color(egui::Color32::WHITE)
                                    .size(14.0),
                            )
                            .fill(egui::Color32::from_rgb(0, 122, 255)),
                        )
                        .clicked()
                    {
                        let password = app.sudo_password_input.clone();
                        app.sudo_password = Some(password.clone());
                        app.sudo_error = None;

                        // 启动 sudo keepalive 会话（保活票据，避免重复弹密码框）(macOS 专属)
                        #[cfg(target_os = "macos")]
                        match sudo_keepalive::start_sudo_session(&password) {
                            Ok(_) => {
                                app.sudo_session_active = true;
                            }
                            Err(e) => {
                                // keepalive 启动失败不阻断流程，仅记录
                                log_scan_step(&app.tf("log_keepalive_failed", &[&e]));
                            }
                        }

                        if is_setup_mode {
                            // 启用 Touch ID 模式：先创建 sudo_local (macOS 专属)
                            #[cfg(target_os = "macos")]
                            match touchid::enable_touch_id_with_password(&password) {
                                Ok(true) => {
                                    app.touch_id_enabled = true;
                                    app.touch_id_setup_mode = false;
                                    app.touch_id_error = None;
                                    app.confirm = ConfirmState::SudoWithTouchId;
                                    let items = std::mem::take(&mut app.sudo_failed_items);
                                    app.delete_done = 0;
                                    app.delete_total = items.len();
                                    start_sudo_delete_touchid(items, app.lang_en, delete_rx);
                                }
                                Ok(false) => {
                                    // 已启用，直接走 Touch ID 删除
                                    app.touch_id_enabled = true;
                                    app.touch_id_setup_mode = false;
                                    app.confirm = ConfirmState::SudoWithTouchId;
                                    let items = std::mem::take(&mut app.sudo_failed_items);
                                    app.delete_done = 0;
                                    app.delete_total = items.len();
                                    start_sudo_delete_touchid(items, app.lang_en, delete_rx);
                                }
                                Err(e) => {
                                    app.sudo_error = Some(e);
                                    app.sudo_password_input.clear();
                                }
                            }
                            #[cfg(not(target_os = "macos"))]
                            {
                                // Windows 无 Touch ID，直接走密码删除
                                app.confirm = ConfirmState::Deleting;
                                let items = std::mem::take(&mut app.sudo_failed_items);
                                app.delete_done = 0;
                                app.delete_total = items.len();
                                start_sudo_delete(items, password, app.lang_en, delete_rx);
                            }
                        } else {
                            app.confirm = ConfirmState::Deleting;
                            let items = std::mem::take(&mut app.sudo_failed_items);
                            app.delete_done = 0;
                            app.delete_total = items.len();
                            start_sudo_delete(items, password, app.lang_en, delete_rx);
                        }
                    }

                    let lang_en = app.lang_en;
                    if ui
                        .button(egui::RichText::new(app.t("cancel")).size(14.0))
                        .clicked()
                    {
                        app.sudo_password_input.clear();
                        app.sudo_password = None;
                        app.sudo_error = None;
                        app.touch_id_setup_mode = false;
                        // 将需要 sudo 的项标记为失败，结束删除流程
                        for (path, category) in std::mem::take(&mut app.sudo_failed_items) {
                            app.receive_delete_log(
                                format!(
                                    "✗ {}: {}",
                                    App::t_lang(lang_en, "log_cancel_reason"),
                                    path
                                ),
                                path.clone(),
                                category.clone(),
                                false,
                            );
                            safety::log_deletion(
                                &path,
                                &category,
                                false,
                                Some(App::t_lang(lang_en, "log_cancel_reason")),
                            );
                        }
                        app.finish_delete();
                    }
                });
            });
        });
}

/// Touch ID 启用提示弹窗
fn show_touch_id_setup_window(
    ctx: &egui::Context,
    app: &mut App,
    _delete_rx: &mut Option<mpsc::Receiver<DeleteMessage>>,
) {
    egui::Window::new(app.t("touchid_setup_title"))
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ctx, |ui| {
            ui.set_min_width(440.0);
            ui.set_max_width(500.0);
            ui.add_space(10.0);

            ui.vertical(|ui| {
                ui.horizontal(|ui| {
                    ui.colored_label(
                        egui::Color32::from_rgb(0, 122, 255),
                        egui::RichText::new("👆").size(28.0),
                    );
                    ui.label(
                        egui::RichText::new(app.t("touchid_setup_headline"))
                            .size(17.0)
                            .strong(),
                    );
                });

                ui.add_space(8.0);
                ui.colored_label(
                    TEXT_PRIMARY,
                    egui::RichText::new(App::tf_lang(
                        app.lang_en,
                        "touchid_setup_desc",
                        &[&app.sudo_failed_items.len().to_string()],
                    ))
                    .size(13.0),
                );
                ui.add_space(4.0);
                ui.colored_label(
                    TEXT_SECONDARY,
                    egui::RichText::new(app.t("touchid_setup_detail")).size(12.0),
                );

                if let Some(ref err) = app.touch_id_error {
                    ui.add_space(8.0);
                    ui.colored_label(ADVANCED_COLOR, egui::RichText::new(err).size(13.0).strong());
                }

                ui.add_space(12.0);
                ui.separator();
                ui.add_space(8.0);

                ui.horizontal(|ui| {
                    // 启用 Touch ID
                    if ui
                        .add(
                            egui::Button::new(
                                egui::RichText::new(format!("👆 {}", app.t("touchid_enable")))
                                    .color(egui::Color32::WHITE)
                                    .size(14.0),
                            )
                            .fill(egui::Color32::from_rgb(0, 122, 255)),
                        )
                        .clicked()
                    {
                        app.touch_id_error = None;
                        app.sudo_password_input.clear();
                        app.sudo_error = None;
                        app.touch_id_setup_mode = true;
                        app.confirm = ConfirmState::NeedSudoPassword;
                    }

                    // 跳过，用密码
                    if ui
                        .button(egui::RichText::new(app.t("touchid_use_password")).size(14.0))
                        .clicked()
                    {
                        app.touch_id_error = None;
                        app.confirm = ConfirmState::NeedSudoPassword;
                    }

                    // 取消
                    let lang_en = app.lang_en;
                    if ui
                        .button(egui::RichText::new(app.t("cancel")).size(14.0))
                        .clicked()
                    {
                        app.touch_id_error = None;
                        for (path, category) in std::mem::take(&mut app.sudo_failed_items) {
                            app.receive_delete_log(
                                format!(
                                    "✗ {}: {}",
                                    App::t_lang(lang_en, "log_cancel_reason"),
                                    path
                                ),
                                path.clone(),
                                category.clone(),
                                false,
                            );
                            safety::log_deletion(
                                &path,
                                &category,
                                false,
                                Some(App::t_lang(lang_en, "log_cancel_reason")),
                            );
                        }
                        app.finish_delete();
                    }
                });
            });
        });
}

/// Touch ID 启用等待中弹窗（用户需要在 Terminal 中输入密码）
fn show_touch_id_waiting_window(ctx: &egui::Context, app: &mut App) {
    egui::Window::new(app.t("touchid_wait_title"))
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ctx, |ui| {
            ui.set_min_width(440.0);
            ui.set_max_width(500.0);
            ui.add_space(10.0);

            ui.vertical(|ui| {
                ui.horizontal(|ui| {
                    ui.colored_label(
                        egui::Color32::from_rgb(0, 122, 255),
                        egui::RichText::new("⏳").size(28.0),
                    );
                    ui.label(
                        egui::RichText::new(app.t("touchid_wait_headline"))
                            .size(15.0)
                            .strong(),
                    );
                });

                ui.add_space(8.0);
                ui.colored_label(
                    TEXT_SECONDARY,
                    egui::RichText::new(app.t("touchid_wait_desc")).size(13.0),
                );

                // 显示已等待时间
                if let Some(start) = app.touch_id_wait_start {
                    let elapsed = start.elapsed().as_secs();
                    ui.add_space(6.0);
                    ui.colored_label(
                        TEXT_TERTIARY,
                        egui::RichText::new(App::tf_lang(
                            app.lang_en,
                            "touchid_wait_time",
                            &[&elapsed.to_string()],
                        ))
                        .size(12.0),
                    );

                    // 进度条
                    let progress = (elapsed as f32) / 120.0;
                    ui.add_space(4.0);
                    ui.add(egui::ProgressBar::new(progress.min(1.0)));
                }

                ui.add_space(10.0);
                ui.separator();
                ui.add_space(8.0);

                // 取消按钮
                if ui
                    .button(egui::RichText::new(app.t("touchid_wait_cancel")).size(14.0))
                    .clicked()
                {
                    app.touch_id_wait_start = None;
                    app.touch_id_error = None;
                    app.confirm = ConfirmState::NeedSudoPassword;
                }
            });
        });

    // 请求持续重绘以更新计时器
    ctx.request_repaint_after(std::time::Duration::from_secs(1));
}

/// Touch ID 删除中弹窗
fn show_touch_id_deleting_window(ctx: &egui::Context, app: &mut App) {
    egui::Window::new(app.t("touchid_verify_title"))
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ctx, |ui| {
            ui.set_min_width(400.0);
            ui.set_max_width(440.0);
            ui.add_space(10.0);

            ui.vertical(|ui| {
                ui.horizontal(|ui| {
                    ui.colored_label(
                        egui::Color32::from_rgb(0, 122, 255),
                        egui::RichText::new("👆").size(28.0),
                    );
                    ui.label(
                        egui::RichText::new(app.t("touchid_verify_headline"))
                            .size(15.0)
                            .strong(),
                    );
                });

                ui.add_space(8.0);
                ui.colored_label(
                    TEXT_SECONDARY,
                    egui::RichText::new(App::tf_lang(
                        app.lang_en,
                        "touchid_verify_desc",
                        &[&app.delete_total.to_string()],
                    ))
                    .size(13.0),
                );

                ui.add_space(8.0);

                // 进度条
                let progress = if app.delete_total > 0 {
                    app.delete_done as f32 / app.delete_total as f32
                } else {
                    0.0
                };
                ui.add(
                    egui::ProgressBar::new(progress)
                        .text(format!("{}/{}", app.delete_done, app.delete_total)),
                );

                ui.add_space(4.0);
                ui.colored_label(
                    TEXT_TERTIARY,
                    egui::RichText::new(app.t("touchid_verify_hint")).size(11.0),
                );

                // 显示最近日志（帮助定位卡在哪里）
                let recent_logs: Vec<&String> = app
                    .logs
                    .iter()
                    .rev()
                    .take(5)
                    .collect::<Vec<_>>()
                    .into_iter()
                    .rev()
                    .collect();
                if !recent_logs.is_empty() {
                    ui.add_space(10.0);
                    egui::Frame::group(ui.style()).fill(SURFACE).show(ui, |ui| {
                        ui.set_min_width(360.0);
                        ui.label(
                            egui::RichText::new(app.t("touchid_verify_log"))
                                .size(11.0)
                                .strong(),
                        );
                        ui.add_space(4.0);
                        for log in recent_logs {
                            ui.colored_label(TEXT_SECONDARY, egui::RichText::new(log).size(10.0));
                        }
                    });
                }

                ui.add_space(12.0);
                ui.separator();
                ui.add_space(8.0);

                // 取消按钮：关闭弹窗并结束当前删除流程
                if ui
                    .button(egui::RichText::new(app.t("cancel")).size(14.0))
                    .clicked()
                {
                    app.confirm = ConfirmState::None;
                    app.touch_id_error = Some(app.t("touchid_cancelled").to_string());
                    app.finish_delete();
                }
            });
        });
}

/// 使用 Touch ID 的 sudo 删除（不需要密码，sudo 自动触发 Touch ID）
fn start_sudo_delete_touchid(
    failed_items: Vec<(String, String)>,
    lang_en: bool,
    delete_rx: &mut Option<mpsc::Receiver<DeleteMessage>>,
) {
    if failed_items.is_empty() {
        return;
    }

    let (tx, rx) = mpsc::channel();
    *delete_rx = Some(rx);

    std::thread::spawn(move || {
        let _ = tx.send(DeleteMessage::Info(format!(
            "👆 {}",
            App::t_lang(lang_en, "log_touchid_verifying")
        )));

        let sudo_debug_log = std::env::temp_dir().join("maclean_sudo_touchid.log");
        let mut debug_entries: Vec<String> = Vec::new();
        debug_entries.push(format!(
            "[touchid sudo] started, {} items",
            failed_items.len()
        ));

        // 预处理：模拟器镜像通过 xcrun simctl runtime delete
        let mut remaining_items: Vec<(String, String)> = Vec::new();
        for (path, category) in &failed_items {
            if category == "模拟器镜像" || category == "模拟器Cryptex" {
                let _ = tx.send(DeleteMessage::Info(format!(
                    "🔄 {}",
                    App::tf_lang(lang_en, "log_touchid_prepare_xcrun", &[category])
                )));

                let script = r#"#!/bin/bash
set +e
uuids=$(xcrun simctl runtime list 2>/dev/null | grep -oE '[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}' | sort -u)
count=0
fail=0
for uuid in $uuids; do
    xcrun simctl runtime delete "$uuid" 2>/dev/null
    rc=$?
    if [ $rc -eq 0 ]; then
        count=$((count + 1))
    else
        fail=$((fail + 1))
    fi
done
echo "xcrun_deleted:$count"
echo "xcrun_failed:$fail"
exit 0
"#;
                let xcrun_script = std::env::temp_dir().join("maclean_xcrun_delete.sh");
                let _ = std::fs::write(&xcrun_script, script);
                let _ = std::process::Command::new("/bin/chmod")
                    .arg("+x")
                    .arg(&xcrun_script)
                    .output();

                // 先清除 sudo 票据，确保能触发 Touch ID
                let _ = std::process::Command::new("/usr/bin/sudo")
                    .arg("-k")
                    .output();

                let _ = tx.send(DeleteMessage::Info(format!(
                    "⏳ {}",
                    App::t_lang(lang_en, "log_wait_touchid")
                )));

                let xcrun_log = std::env::temp_dir().join("maclean_xcrun_touchid.log");
                let xcrun_log_file = std::fs::OpenOptions::new()
                    .create(true)
                    .write(true)
                    .truncate(true)
                    .open(&xcrun_log)
                    .ok();

                // 不用 -S，sudo 会自动弹出 Touch ID；stdout 重定向到文件避免 pipe 死锁
                let mut cmd = std::process::Command::new("/usr/bin/sudo");
                cmd.arg("/bin/bash").arg(&xcrun_script);
                if let Some(file) = xcrun_log_file {
                    cmd.stdout(file);
                }
                let xcrun_result = cmd.status();

                let mut xcrun_success = false;
                let xcrun_stdout = std::fs::read_to_string(&xcrun_log).unwrap_or_default();
                debug_entries.push(format!("xcrun touchid stdout:\n{}", xcrun_stdout));

                let _ = tx.send(DeleteMessage::Info(format!(
                    "✅ {}",
                    App::tf_lang(
                        lang_en,
                        "log_xcrun_done",
                        &[&xcrun_stdout.trim().replace('\n', " ")]
                    )
                )));

                match &xcrun_result {
                    Ok(status) => {
                        if !status.success() {
                            // Touch ID 可能被取消
                            if xcrun_stdout.contains("canceled")
                                || xcrun_stdout.contains("cancelled")
                            {
                                let _ = tx.send(DeleteMessage::Info(format!(
                                    "🔒 {}",
                                    App::t_lang(lang_en, "log_touchid_cancel")
                                )));
                                for (p, c) in &failed_items {
                                    let _ = tx.send(DeleteMessage::Log(
                                        format!(
                                            "✗ {}: {}",
                                            App::t_lang(lang_en, "log_touchid_cancel"),
                                            p
                                        ),
                                        p.clone(),
                                        c.clone(),
                                        false,
                                    ));
                                    safety::log_deletion(
                                        p,
                                        c,
                                        false,
                                        Some(App::t_lang(lang_en, "log_touchid_cancel")),
                                    );
                                }
                                let _ = std::fs::write(&sudo_debug_log, debug_entries.join("\n"));
                                let _ = std::fs::remove_file(&xcrun_script);
                                let _ = std::fs::remove_file(&xcrun_log);
                                let _ = tx.send(DeleteMessage::Done);
                                return;
                            }
                        }

                        // xcrun 删除成功即可认为该模拟器镜像项已清理
                        // 不必要求 Volumes 目录为空（mount point 会残留，且受 SIP 保护）
                        let deleted_count = xcrun_stdout
                            .lines()
                            .find(|l| l.starts_with("xcrun_deleted:"))
                            .and_then(|l| l.strip_prefix("xcrun_deleted:"))
                            .and_then(|n| n.trim().parse::<usize>().ok())
                            .unwrap_or(0);
                        let failed_count = xcrun_stdout
                            .lines()
                            .find(|l| l.starts_with("xcrun_failed:"))
                            .and_then(|l| l.strip_prefix("xcrun_failed:"))
                            .and_then(|n| n.trim().parse::<usize>().ok())
                            .unwrap_or(0);

                        if deleted_count > 0 && failed_count == 0 {
                            xcrun_success = true;
                            let _ = tx.send(DeleteMessage::Log(
                                format!(
                                    "✓ {}",
                                    App::tf_lang(
                                        lang_en,
                                        "log_touchid_xcrun_deleted",
                                        &[&deleted_count.to_string()]
                                    )
                                ),
                                path.clone(),
                                category.clone(),
                                true,
                            ));
                            safety::log_deletion(path, category, true, None);
                        }
                    }
                    Err(e) => {
                        debug_entries.push(format!("xcrun touchid error: {}", e));
                    }
                }

                let _ = std::fs::remove_file(&xcrun_script);
                let _ = std::fs::remove_file(&xcrun_log);

                if !xcrun_success {
                    remaining_items.push((path.clone(), category.clone()));
                }
            } else {
                remaining_items.push((path.clone(), category.clone()));
            }
        }

        let failed_items = remaining_items;

        if failed_items.is_empty() {
            let _ = tx.send(DeleteMessage::Info(format!(
                "✅ {}",
                App::t_lang(lang_en, "log_no_sudo_needed")
            )));
            let _ = std::fs::write(&sudo_debug_log, debug_entries.join("\n"));
            let _ = tx.send(DeleteMessage::Done);
            return;
        }

        let _ = tx.send(DeleteMessage::Info(format!(
            "🔄 {}",
            App::tf_lang(
                lang_en,
                "log_sudo_execute",
                &[&failed_items.len().to_string()]
            )
        )));

        // 写临时删除脚本：并行删除
        let tmp_script = std::env::temp_dir().join("maclean_sudo_touchid.sh");
        let current_user = std::env::var("USER")
            .or_else(|_| std::env::var("LOGNAME"))
            .unwrap_or_else(|_| "root".to_string());
        let mut script_content = String::from("#!/bin/bash\nset +e\n");
        script_content.push_str("workdir=$(/usr/bin/mktemp -d)\n");
        script_content.push_str("trap \"/bin/rm -rf \\\"$workdir\\\"\" EXIT\n\n");
        script_content.push_str("process_one() {\n");
        script_content.push_str("  local idx=\"$1\"\n");
        script_content.push_str("  local path=\"$2\"\n");
        script_content.push_str("  local out=\"$workdir/${idx}.out\"\n");
        script_content.push_str("  echo \">MACLEAN_BEGIN:$path\" > \"$out\"\n");
        script_content.push_str("  /usr/bin/chflags -R nouchg \"$path\" 2>/dev/null\n");
        script_content.push_str("  /usr/sbin/chown -R '");
        script_content.push_str(&current_user.replace("'", "'\\''"));
        script_content.push_str(":staff' \"$path\" 2>/dev/null\n");
        script_content.push_str("  /bin/chmod -R u+w \"$path\" 2>/dev/null\n");
        script_content.push_str("  /bin/rm -rf \"$path\" 2>&1 >> \"$out\"\n");
        script_content.push_str("  echo \">MACLEAN_EXIT:$path:$?\" >> \"$out\"\n");
        script_content.push_str("}\n\n");

        for (i, (path, _)) in failed_items.iter().enumerate() {
            let escaped = path.replace("'", "'\\''");
            script_content.push_str(&format!("process_one {} '{}' &\n", i, escaped));
        }
        script_content.push_str("\nwait\n");
        script_content
            .push_str("for f in \"$workdir\"/*.out; do [ -f \"$f\" ] && /bin/cat \"$f\"; done\n");
        script_content.push_str("exit 0\n");
        let _ = std::fs::write(&tmp_script, &script_content);
        let _ = std::process::Command::new("/bin/chmod")
            .arg("+x")
            .arg(&tmp_script)
            .output();

        debug_entries.push(format!("delete script: {}", tmp_script.display()));

        let sudo_log = std::env::temp_dir().join("maclean_sudo_touchid_script.log");
        let sudo_log_file = std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&sudo_log)
            .ok();

        let _ = tx.send(DeleteMessage::Info(format!(
            "⏳ {}",
            App::t_lang(lang_en, "log_wait_touchid")
        )));

        // 不用 -S，sudo 自动触发 Touch ID；stdout 重定向到文件避免 pipe 死锁
        let mut sudo_cmd = std::process::Command::new("/usr/bin/sudo");
        sudo_cmd.arg("/bin/bash").arg(&tmp_script);
        if let Some(file) = sudo_log_file {
            sudo_cmd.stdout(file);
        }
        let sudo_result = sudo_cmd.status();

        let _ = tx.send(DeleteMessage::Info(format!(
            "✅ {}",
            App::t_lang(lang_en, "log_sudo_done2")
        )));

        // 解析输出
        let sudo_stdout = std::fs::read_to_string(&sudo_log).unwrap_or_default();
        debug_entries.push(format!(
            "sudo touchid exit code: {:?}",
            sudo_result.as_ref().ok().and_then(|s| s.code())
        ));
        debug_entries.push(format!("sudo touchid stdout:\n{}", sudo_stdout));

        let mut rm_stderr: std::collections::HashMap<String, String> =
            std::collections::HashMap::new();
        let mut current_path = String::new();
        for line in sudo_stdout.lines() {
            if let Some(p) = line.strip_prefix(">MACLEAN_BEGIN:") {
                current_path = p.to_string();
            } else if let Some(_rest) = line.strip_prefix(">MACLEAN_EXIT:") {
                current_path.clear();
            } else if !line.is_empty() && !current_path.is_empty() {
                rm_stderr
                    .entry(current_path.clone())
                    .or_default()
                    .push_str(line);
                rm_stderr
                    .entry(current_path.clone())
                    .or_default()
                    .push('\n');
            }
        }

        let _ = std::fs::write(&sudo_debug_log, debug_entries.join("\n"));
        let _ = std::fs::remove_file(&sudo_log);

        // 逐项验证
        match sudo_result {
            Ok(_) => {
                let user_cancelled = sudo_stdout.contains("canceled")
                    || sudo_stdout.contains("cancelled")
                    || sudo_stdout.contains("User canceled");

                if user_cancelled {
                    let _ = tx.send(DeleteMessage::Info(format!(
                        "🔒 {}",
                        App::t_lang(lang_en, "log_touchid_cancel")
                    )));
                    for (path, category) in &failed_items {
                        let _ = tx.send(DeleteMessage::Log(
                            format!("✗ {}: {}", App::t_lang(lang_en, "log_touchid_cancel"), path),
                            path.clone(),
                            category.clone(),
                            false,
                        ));
                        safety::log_deletion(
                            path,
                            category,
                            false,
                            Some(App::t_lang(lang_en, "log_touchid_cancel")),
                        );
                    }
                    let _ = std::fs::remove_file(&tmp_script);
                    let _ = tx.send(DeleteMessage::Done);
                    return;
                }

                for (path, category) in &failed_items {
                    let p = std::path::Path::new(path.as_str());
                    if !p.exists() && p.symlink_metadata().is_err() {
                        let _ = tx.send(DeleteMessage::Log(
                            format!(
                                "✓ {}",
                                App::tf_lang(lang_en, "log_deleted_touchid", &[category, path])
                            ),
                            path.clone(),
                            category.clone(),
                            true,
                        ));
                        safety::log_deletion(path, category, true, None);
                    } else {
                        let err_text = rm_stderr.get(path).map(|s| s.as_str()).unwrap_or("");
                        let is_sip = err_text.contains("Operation not permitted")
                            || path.starts_with("/Library/Developer/CoreSimulator/Caches");

                        if is_sip {
                            let _ = tx.send(DeleteMessage::Log(
                                format!(
                                    "🔒 {}",
                                    App::tf_lang(lang_en, "log_sip_protected", &[path])
                                ),
                                path.clone(),
                                category.clone(),
                                false,
                            ));
                            safety::log_deletion(
                                path,
                                category,
                                false,
                                Some(App::t_lang(lang_en, "log_sip_reason")),
                            );
                        } else {
                            let detail = if err_text.is_empty() {
                                App::t_lang(lang_en, "log_still_exists").to_string()
                            } else {
                                err_text.trim().to_string()
                            };
                            let _ = tx.send(DeleteMessage::Log(
                                format!(
                                    "✗ {}",
                                    App::tf_lang(lang_en, "log_delete_failed", &[path, &detail])
                                ),
                                path.clone(),
                                category.clone(),
                                false,
                            ));
                            safety::log_deletion(path, category, false, Some(&detail));
                        }
                    }
                }
            }
            Err(e) => {
                for (path, category) in &failed_items {
                    let _ = tx.send(DeleteMessage::Log(
                        format!(
                            "✗ {}",
                            App::tf_lang(lang_en, "log_cannot_start_sudo", &[path, &e.to_string()])
                        ),
                        path.clone(),
                        category.clone(),
                        false,
                    ));
                    safety::log_deletion(path, category, false, Some(&e.to_string()));
                }
            }
        }

        let _ = std::fs::remove_file(&tmp_script);
        let _ = tx.send(DeleteMessage::Done);
    });
}

/// 删除中弹窗（设计稿 4.5 样式）
fn show_deleting_window(ctx: &egui::Context, app: &mut App) {
    // 判断是否在 sudo 阶段（最近日志包含管理员权限关键词）
    let in_sudo_phase = app
        .logs
        .iter()
        .rev()
        .take(5)
        .any(|l| l.contains("管理员权限") || l.contains("administrator privileges"));

    let progress = if app.delete_total > 0 {
        app.delete_done as f32 / app.delete_total as f32
    } else {
        0.0
    };

    let failed_count = app.failed_paths.len();

    egui::Window::new(app.t("cleaning"))
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ctx, |ui| {
            ui.set_min_width(480.0);
            ui.set_max_width(520.0);

            // Header
            ui.vertical(|ui| {
                let title = if in_sudo_phase {
                    app.t("deleting_sudo_phase")
                } else {
                    app.t("cleaning_in_progress")
                };
                ui.label(
                    egui::RichText::new(title)
                        .size(15.0)
                        .strong()
                        .color(TEXT_PRIMARY),
                );
                ui.colored_label(
                    TEXT_SECONDARY,
                    egui::RichText::new(app.tf(
                        "deleting_subtitle",
                        &[&app.delete_done.to_string(), &app.delete_total.to_string()],
                    ))
                    .size(12.0),
                );
            });

            ui.add_space(14.0);

            // 进度条
            ui.add(
                egui::ProgressBar::new(progress)
                    .desired_width(ui.available_width().max(440.0))
                    .fill(if in_sudo_phase { CAUTION_COLOR } else { BRAND })
                    .text(format!("{}%", (progress * 100.0) as u32)),
            );

            ui.add_space(8.0);

            // 状态计数
            ui.horizontal(|ui| {
                ui.colored_label(
                    TEXT_SECONDARY,
                    egui::RichText::new(format!(
                        "{}/{} {}",
                        app.delete_done,
                        app.delete_total,
                        app.t("items")
                    ))
                    .size(12.0),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if failed_count > 0 {
                        ui.colored_label(
                            ADVANCED_COLOR,
                            egui::RichText::new(format!(
                                "{} {} {}",
                                failed_count,
                                app.t("items"),
                                app.t("summary_fail_hint")
                            ))
                            .size(12.0),
                        );
                    }
                });
            });

            ui.add_space(12.0);

            // 最新日志
            egui::Frame::none()
                .fill(SURFACE_MUTED)
                .stroke(egui::Stroke::new(1.0, BORDER_LIGHT))
                .rounding(egui::Rounding::same(6.0))
                .inner_margin(egui::Margin::same(10.0))
                .show(ui, |ui| {
                    ui.set_max_height(160.0);
                    egui::ScrollArea::vertical().show(ui, |ui| {
                        for log in app.logs.iter().rev().take(20) {
                            let color = if log.starts_with('✓') || log.starts_with('✅') {
                                SAFE_COLOR
                            } else if log.starts_with('✗') || log.starts_with('⛔') {
                                ADVANCED_COLOR
                            } else if log.starts_with('⚠') {
                                CAUTION_COLOR
                            } else {
                                TEXT_SECONDARY
                            };
                            ui.colored_label(
                                color,
                                egui::RichText::new(log).size(11.0).monospace(),
                            );
                        }
                    });
                });

            ui.add_space(12.0);

            // Footer
            ui.horizontal(|ui| {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let can_authorize = !app.sudo_failed_items.is_empty() || failed_count > 0;
                    let auth_btn = ui.add_enabled(
                        can_authorize,
                        egui::Button::new(
                            egui::RichText::new(app.t("progress_authorize"))
                                .color(egui::Color32::WHITE)
                                .size(13.0),
                        )
                        .fill(BRAND)
                        .rounding(egui::Rounding::same(8.0)),
                    );
                    if auth_btn.clicked() && can_authorize {
                        app.sudo_failed_items = app.failed_paths.clone();
                        app.confirm = ConfirmState::NeedSudoPassword;
                    }

                    ui.add_space(8.0);

                    if ui
                        .add(
                            egui::Button::new(
                                egui::RichText::new(app.t("progress_background_run")).size(13.0),
                            )
                            .fill(SURFACE_ELEVATED)
                            .stroke(egui::Stroke::new(1.0, BORDER_LIGHT))
                            .rounding(egui::Rounding::same(8.0)),
                        )
                        .clicked()
                    {
                        // 关闭进度弹窗，让删除在后台继续，完成后通过汇总弹窗提示
                        app.confirm = ConfirmState::None;
                    }
                });
            });
        });

    // 删除中持续刷新 UI
    ctx.request_repaint_after(std::time::Duration::from_millis(50));
}

/// 删除完成汇总弹窗
fn show_summary_window(
    ctx: &egui::Context,
    app: &mut App,
    ok: usize,
    fail: usize,
    _skip: usize,
    _delete_rx: &mut Option<mpsc::Receiver<DeleteMessage>>,
) {
    let _has_failures = !app.failed_paths.is_empty();

    egui::Window::new(app.t("summary_title"))
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ctx, |ui| {
            ui.set_min_width(480.0);
            ui.set_max_width(520.0);
            ui.add_space(10.0);
            ui.vertical(|ui| {
                // 成功
                ui.horizontal(|ui| {
                    ui.colored_label(SAFE_COLOR, "✅");
                    ui.label(egui::RichText::new(App::tf_lang(app.lang_en, "summary_success", &[&ok.to_string()])).size(15.0).color(SAFE_COLOR));
                });

                if fail > 0 {
                    ui.add_space(5.0);
                    ui.horizontal(|ui| {
                        ui.colored_label(ADVANCED_COLOR, "❌");
                        ui.label(egui::RichText::new(App::tf_lang(app.lang_en, "summary_fail", &[&fail.to_string()])).size(15.0).color(ADVANCED_COLOR));
                    });
                    ui.add_space(3.0);
                    ui.colored_label(
                        TEXT_SECONDARY,
                        egui::RichText::new(app.t("summary_fail_hint")).size(12.0),
                    );

                    // 引导用户处理失败项
                    ui.add_space(8.0);
                    egui::Frame::group(ui.style())
                        .fill(SURFACE)
                        .stroke(egui::Stroke::new(1.0, BRAND))
                        .inner_margin(egui::Margin::same(8.0))
                        .show(ui, |ui| {
                            ui.colored_label(
                                BRAND,
                                egui::RichText::new(format!("💡 {}", app.t("summary_solution_title"))).size(13.0).strong(),
                            );
                            ui.add_space(3.0);
                            ui.colored_label(
                                TEXT_PRIMARY,
                                egui::RichText::new(app.t("summary_sip_tip")).size(11.0),
                            );
                            ui.add_space(3.0);
                            ui.colored_label(
                                TEXT_PRIMARY,
                                egui::RichText::new(app.t("summary_perm_tip")).size(11.0),
                            );
                            ui.add_space(5.0);
                            ui.colored_label(
                                TEXT_SECONDARY,
                                egui::RichText::new(app.t("summary_solution_1")).size(11.0),
                            );
                            ui.colored_label(
                                TEXT_SECONDARY,
                                egui::RichText::new(app.t("summary_solution_2")).size(11.0),
                            );
                            ui.colored_label(
                                TEXT_SECONDARY,
                                egui::RichText::new(app.t("summary_solution_3")).size(11.0),
                            );
                            ui.add_space(5.0);
                            ui.horizontal(|ui| {
                                if ui.button(egui::RichText::new(format!("⚙️ {}", app.t("summary_open_settings"))).size(12.0)).clicked() {
                                    let _ = std::process::Command::new("open")
                                        .arg("x-apple.systempreferences:com.apple.preference.security?Privacy_AllFiles")
                                        .spawn();
                                }
                            });
                        });

                    // 列出所有失败的路径（可滚动+复制）
                    ui.add_space(5.0);
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new(app.t("summary_fail_list")).size(12.0).color(TEXT_SECONDARY));
                        let all_paths: String = app.failed_paths.iter()
                            .map(|(p, c)| format!("[{}] {}", c, p))
                            .collect::<Vec<_>>()
                            .join("\n");
                        let sudo_cmd: String = app.failed_paths.iter()
                            .map(|(p, _)| format!("'{}'", p.replace("'", "'\\''")))
                            .collect::<Vec<_>>()
                            .join(" ");
                        let sudo_text = format!("sudo /usr/bin/chflags -R nouchg {}; sudo /usr/sbin/chown -R $(whoami):staff {}; sudo /bin/chmod -R u+w {}; sudo /bin/rm -rf {}", sudo_cmd, sudo_cmd, sudo_cmd, sudo_cmd);
                        if ui.button(egui::RichText::new(format!("📋 {}", app.t("summary_copy_paths"))).size(11.0)).clicked() {
                            ui.output_mut(|o| o.copied_text = all_paths);
                        }
                        if ui.button(egui::RichText::new(format!("🔐 {}", app.t("summary_copy_sudo"))).size(11.0)).clicked() {
                            ui.output_mut(|o| o.copied_text = sudo_text);
                        }
                    });

                    egui::ScrollArea::vertical()
                        .max_height(180.0)
                        .stick_to_bottom(false)
                        .show(ui, |ui| {
                            for (path, category) in &app.failed_paths {
                                ui.horizontal(|ui| {
                                    ui.colored_label(
                                        ADVANCED_COLOR,
                                        egui::RichText::new("•").size(11.0),
                                    );
                                    ui.vertical(|ui| {
                                        ui.colored_label(
                                            egui::Color32::from_rgb(200, 120, 100),
                                            egui::RichText::new(category).size(11.0),
                                        );
                                        ui.add(
                                            egui::TextEdit::multiline(&mut path.as_str())
                                                .desired_width(400.0)
                                                .font(egui::TextStyle::Monospace)
                                                .text_color(TEXT_TERTIARY)
                                                .interactive(true),
                                        );
                                    });
                                });
                                ui.add_space(2.0);
                            }
                        });
                }

                ui.add_space(10.0);
                ui.separator();
                ui.add_space(5.0);

                // 磁盘空间变化
                ui.label(egui::RichText::new(App::tf_lang(app.lang_en, "summary_free_space", &[&format_size(app.disk_free)])).size(14.0));

                ui.add_space(15.0);
                ui.horizontal(|ui| {
                    if ui.button(egui::RichText::new(app.t("summary_ok")).size(14.0)).clicked() {
                        app.dismiss_summary();
                    }
                });
            });
        });
}

/// Tab 标题
fn tab_title<'a>(tab: &Tab, app: &'a App) -> &'a str {
    match tab {
        Tab::Overview => app.t("tab_overview"),
        Tab::DevCache => app.t("tab_dev_cache"),
        Tab::LargeFiles => app.t("tab_large_files"),
        Tab::AppCache => app.t("tab_app_cache"),
        Tab::AppData => app.t("tab_app_data"),
        Tab::AppUninstall => app.t("tab_app_uninstall"),
        Tab::SystemOptimize => app.t("tab_system_optimize"),
        Tab::Apfs => app.t("tab_apfs"),
        Tab::Settings => app.t("tab_settings"),
    }
}

/// Windows: 残留清理弹窗
///
/// 卸载应用后检测到残留时弹出，让用户选择：
/// - 一键全部清理
/// - 只清理选中的残留项
/// - 跳过（不清理）
#[cfg(target_os = "windows")]
fn show_residual_window(
    ctx: &egui::Context,
    app: &mut App,
    delete_rx: &mut Option<mpsc::Receiver<DeleteMessage>>,
) {
    let residual = match &app.uninstall_residual {
        Some(r) => r.clone(),
        None => {
            app.show_residual_dialog = false;
            return;
        }
    };

    let reg_len = residual.registry.len();
    let env_len = residual.env_vars.len();
    let fs_len = residual.filesystem.len();
    let total = reg_len + env_len + fs_len;
    let deletable_count = residual.deletable_count();
    let selected_count = app.residual_selected.iter().filter(|&&s| s).count();
    let fs_total_size: u64 = residual.filesystem.iter().map(|f| f.size).sum();

    // 确保选中列表长度正确
    if app.residual_selected.len() != total {
        app.residual_selected.resize(total, false);
    }

    let is_cleaning = app.residual_cleaning;
    let delete_in_progress = delete_rx.is_some();

    egui::Window::new("卸载残留清理")
        .collapsible(false)
        .resizable(true)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ctx, |ui| {
            ui.set_min_width(560.0);
            ui.set_min_height(300.0);
            ui.add_space(8.0);

            ui.vertical(|ui| {
                // 标题
                ui.horizontal(|ui| {
                    ui.colored_label(CAUTION_COLOR, "⚠️");
                    ui.label(egui::RichText::new("检测到卸载残留").size(16.0).strong());
                });
                ui.colored_label(
                    TEXT_TERTIARY,
                    egui::RichText::new(format!(
                        "共 {} 项残留 (可清理 {} 项), 文件总计 {}",
                        total,
                        deletable_count,
                        format_size(fs_total_size)
                    ))
                    .size(12.0),
                );
                ui.colored_label(
                    TEXT_TERTIARY,
                    egui::RichText::new("请选择要清理的项目，或一键清理全部可删除项").size(11.0),
                );

                ui.add_space(8.0);
                ui.separator();
                ui.add_space(4.0);

                // 残留列表（可滚动）
                egui::ScrollArea::vertical()
                    .max_height(300.0)
                    .show(ui, |ui| {
                        // 注册表残留
                        if reg_len > 0 {
                            ui.add_space(4.0);
                            ui.colored_label(
                                BRAND,
                                egui::RichText::new(format!("📋 注册表残留 ({} 项)", reg_len))
                                    .size(13.0)
                                    .strong(),
                            );
                            for (i, reg) in residual.registry.iter().enumerate() {
                                let idx = i;
                                let checked = &mut app.residual_selected[idx];
                                ui.horizontal(|ui| {
                                    ui.add_enabled(
                                        reg.deletable && !is_cleaning,
                                        egui::Checkbox::without_text(checked),
                                    );
                                    if reg.deletable {
                                        ui.colored_label(
                                            TEXT_PRIMARY,
                                            egui::RichText::new(&reg.key_path).size(11.0),
                                        );
                                    } else {
                                        ui.colored_label(
                                            TEXT_TERTIARY,
                                            egui::RichText::new(&reg.key_path).size(11.0),
                                        );
                                        ui.colored_label(
                                            ADVANCED_COLOR,
                                            egui::RichText::new(format!("({})", reg.reason))
                                                .size(10.0),
                                        );
                                    }
                                });
                            }
                            ui.add_space(4.0);
                        }

                        // 环境变量残留
                        if env_len > 0 {
                            ui.colored_label(
                                SAFE_COLOR,
                                egui::RichText::new(format!("环境变量残留 ({} 项)", env_len))
                                    .size(13.0)
                                    .strong(),
                            );
                            for (i, env) in residual.env_vars.iter().enumerate() {
                                let idx = reg_len + i;
                                let checked = &mut app.residual_selected[idx];
                                ui.horizontal(|ui| {
                                    ui.add_enabled(
                                        env.deletable && !is_cleaning,
                                        egui::Checkbox::without_text(checked),
                                    );
                                    if env.deletable {
                                        ui.colored_label(
                                            TEXT_PRIMARY,
                                            egui::RichText::new(format!(
                                                "{} = {}",
                                                env.var_name, env.current_value
                                            ))
                                            .size(11.0),
                                        );
                                    } else {
                                        ui.colored_label(
                                            TEXT_TERTIARY,
                                            egui::RichText::new(format!(
                                                "{} = {}",
                                                env.var_name, env.current_value
                                            ))
                                            .size(11.0),
                                        );
                                        ui.colored_label(
                                            ADVANCED_COLOR,
                                            egui::RichText::new(format!("({})", env.reason))
                                                .size(10.0),
                                        );
                                    }
                                });
                            }
                            ui.add_space(4.0);
                        }

                        // 文件系统残留
                        if fs_len > 0 {
                            ui.colored_label(
                                egui::Color32::from_rgb(255, 180, 100),
                                egui::RichText::new(format!(
                                    "文件系统残留 ({} 项, {})",
                                    fs_len,
                                    format_size(fs_total_size)
                                ))
                                .size(13.0)
                                .strong(),
                            );
                            for (i, fs) in residual.filesystem.iter().enumerate() {
                                let idx = reg_len + env_len + i;
                                let checked = &mut app.residual_selected[idx];
                                ui.horizontal(|ui| {
                                    ui.add_enabled(
                                        fs.deletable && !is_cleaning,
                                        egui::Checkbox::without_text(checked),
                                    );
                                    if fs.deletable {
                                        ui.colored_label(
                                            TEXT_PRIMARY,
                                            egui::RichText::new(format!(
                                                "{} ({})",
                                                fs.path,
                                                format_size(fs.size)
                                            ))
                                            .size(11.0),
                                        );
                                    } else {
                                        ui.colored_label(
                                            TEXT_TERTIARY,
                                            egui::RichText::new(format!(
                                                "{} ({})",
                                                fs.path,
                                                format_size(fs.size)
                                            ))
                                            .size(11.0),
                                        );
                                        ui.colored_label(
                                            ADVANCED_COLOR,
                                            egui::RichText::new(format!("({})", fs.reason))
                                                .size(10.0),
                                        );
                                    }
                                });
                            }
                        }
                    });

                ui.add_space(8.0);
                ui.separator();
                ui.add_space(5.0);

                // 操作按钮
                ui.horizontal(|ui| {
                    // 一键清理全部
                    let clean_all_enabled =
                        deletable_count > 0 && !is_cleaning && !delete_in_progress;
                    if ui
                        .add_enabled(
                            clean_all_enabled,
                            egui::Button::new(
                                egui::RichText::new(format!(
                                    "🧹 一键清理全部 ({} 项)",
                                    deletable_count
                                ))
                                .size(13.0),
                            ),
                        )
                        .clicked()
                    {
                        logger::info(&format!(
                            "用户选择: 一键清理全部 {} 项可删除残留",
                            deletable_count
                        ));
                        app.residual_cleaning = true;
                        let residual = app.uninstall_residual.take().unwrap();
                        let (tx, rx) = mpsc::channel();
                        *delete_rx = Some(rx);
                        std::thread::spawn(move || {
                            let (reg_c, env_c, fs_c) =
                                scanner::windows_apps::clean_all_residuals(&residual);
                            let _ = tx.send(DeleteMessage::ResidualCleaned(reg_c, env_c, fs_c));
                        });
                    }

                    // 清理选中
                    let clean_selected_enabled =
                        selected_count > 0 && !is_cleaning && !delete_in_progress;
                    if ui
                        .add_enabled(
                            clean_selected_enabled,
                            egui::Button::new(
                                egui::RichText::new(format!("✓ 清理选中 ({} 项)", selected_count))
                                    .size(13.0),
                            ),
                        )
                        .clicked()
                    {
                        logger::info(&format!("用户选择: 清理选中 {} 项残留", selected_count));
                        app.residual_cleaning = true;
                        let residual = app.uninstall_residual.take().unwrap();
                        let selected = app.residual_selected.clone();
                        let (tx, rx) = mpsc::channel();
                        *delete_rx = Some(rx);
                        std::thread::spawn(move || {
                            let mut reg_c = 0;
                            let mut env_c = 0;
                            let mut fs_c = 0;
                            let rl = residual.registry.len();
                            let el = residual.env_vars.len();

                            for (i, &sel) in selected.iter().enumerate() {
                                if !sel {
                                    continue;
                                }
                                if i < rl {
                                    if residual.registry[i].deletable {
                                        let (ok, _) =
                                            scanner::windows_apps::delete_registry_residual(
                                                &residual.registry[i].key_path,
                                            );
                                        if ok {
                                            reg_c += 1;
                                        }
                                    }
                                } else if i < rl + el {
                                    let j = i - rl;
                                    if residual.env_vars[j].deletable {
                                        let (ok, _) = scanner::windows_apps::clean_env_var_residual(
                                            &residual.env_vars[j],
                                        );
                                        if ok {
                                            env_c += 1;
                                        }
                                    }
                                } else {
                                    let j = i - rl - el;
                                    if residual.filesystem[j].deletable {
                                        let (ok, _) =
                                            scanner::windows_apps::delete_filesystem_residual(
                                                &residual.filesystem[j].path,
                                            );
                                        if ok {
                                            fs_c += 1;
                                        }
                                    }
                                }
                            }
                            let _ = tx.send(DeleteMessage::ResidualCleaned(reg_c, env_c, fs_c));
                        });
                    }

                    // 跳过
                    let skip_enabled = !is_cleaning;
                    if ui
                        .add_enabled(
                            skip_enabled,
                            egui::Button::new(egui::RichText::new("跳过").size(13.0)),
                        )
                        .clicked()
                    {
                        logger::info("用户选择: 跳过残留清理");
                        app.show_residual_dialog = false;
                        app.uninstall_residual = None;
                        app.residual_selected.clear();
                    }
                });

                // 清理中提示
                if is_cleaning {
                    ui.add_space(5.0);
                    ui.colored_label(BRAND, egui::RichText::new("⏳ 正在清理残留...").size(12.0));
                    ctx.request_repaint_after(std::time::Duration::from_millis(50));
                }
            });
        });
}

/// 截断路径
fn truncate_path(path: &str, max_len: usize) -> String {
    if path.len() <= max_len {
        return path.to_string();
    }
    let suffix = &path[path.len() - max_len + 3..];
    format!("...{}", suffix)
}

// =========================================================================
//  系统优化面板
// =========================================================================

/// 渲染系统优化面板（特殊 UI，不是列表选择模式）
/// 注意：直接复用外层 CentralPanel 传入的 ui，避免嵌套 CentralPanel 导致状态异常
/// 渲染磁盘分析器（目录钻取式磁盘浏览器）
///
/// 功能：
/// - 显示当前浏览路径（面包屑导航）
/// - 返回上一级按钮
/// - 列出当前目录下所有子项（按大小降序）
/// - 每项显示大小、进度条（相对于当前目录总大小）
/// - 目录可点击进入，文件可勾选删除
fn render_disk_analyzer(
    ui: &mut egui::Ui,
    app: &mut App,
    scan_rx: &mut Option<mpsc::Receiver<ScanMessage>>,
) {
    let tab_idx = app.tab_index();
    let items = app.results[tab_idx].clone();
    let is_scanning = matches!(app.scan_states[tab_idx], ScanState::Scanning);
    let current_path = app.disk_analyzer_current_path();
    let has_history = !app.disk_analyzer_history.is_empty();

    // ====== 面包屑导航栏 ======
    ui.horizontal(|ui| {
        // 返回上一级按钮
        let back_enabled = has_history && !is_scanning;
        if ui
            .add_enabled(
                back_enabled,
                egui::Button::new(format!("⬆ {}", app.t("back"))),
            )
            .clicked()
        {
            app.disk_analyzer_back();
            // 返回后重新扫描上一级目录
            start_scan(app, scan_rx);
            return;
        }

        ui.separator();

        // 显示当前路径（用 ~ 替换主目录）
        let home = scanner::home_dir();
        let display_path = if current_path.starts_with(&home) {
            let suffix = current_path
                .strip_prefix(&home)
                .unwrap_or(std::path::Path::new(""));
            format!("~ {}", suffix.display())
        } else {
            current_path.display().to_string()
        };

        ui.label(
            egui::RichText::new(format!("📁 {}", display_path))
                .size(14.0)
                .color(BRAND),
        );

        ui.separator();

        // 主目录按钮（快速回到 home）
        if has_history && !is_scanning {
            if ui.button(format!("🏠 {}", app.t("home"))).clicked() {
                app.disk_analyzer_path = None;
                app.disk_analyzer_history.clear();
                start_scan(app, scan_rx);
                return;
            }
        }
    });

    ui.add_space(5.0);

    if is_scanning {
        // 扫描中
        let scan_size = ui.available_size();
        egui::Frame::none()
            .fill(LIST_BG)
            .rounding(egui::Rounding::same(8.0))
            .show(ui, |ui| {
                ui.set_min_size(scan_size);
                ui.vertical_centered(|ui| {
                    ui.add_space(40.0);
                    ui.add(egui::Spinner::new().size(40.0));
                    ui.add_space(10.0);
                    ui.label(
                        egui::RichText::new(app.tf(
                            "analyzing",
                            &[&display_path_short(&current_path, app.lang_en)],
                        ))
                        .size(16.0)
                        .color(BRAND),
                    );
                    ui.add_space(15.0);
                    let pct = (app.scan_progress * 100.0) as u32;
                    ui.add(
                        egui::ProgressBar::new(app.scan_progress)
                            .desired_width(500.0)
                            .fill(BRAND)
                            .text(format!("{}%", pct)),
                    );
                });
            });
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_millis(100));
        return;
    }

    if items.is_empty() {
        let empty_size = ui.available_size();
        egui::Frame::none()
            .fill(LIST_BG)
            .rounding(egui::Rounding::same(8.0))
            .show(ui, |ui| {
                ui.set_min_size(empty_size);
                ui.vertical_centered(|ui| {
                    ui.add_space(80.0);
                    ui.label(
                        egui::RichText::new(app.t("no_large_files"))
                            .size(16.0)
                            .color(TEXT_TERTIARY),
                    );
                    ui.add_space(10.0);
                    if ui
                        .button(egui::RichText::new(format!("🔍 {}", app.t("rescan"))).size(16.0))
                        .clicked()
                    {
                        start_scan(app, scan_rx);
                    }
                });
            });
        return;
    }

    // ====== 汇总信息 ======
    let total_size: u64 = items.iter().map(|i| i.size_bytes).sum();
    let selected_cnt: usize = items.iter().filter(|i| i.selected && i.deletable).count();
    let selected_sz: u64 = items
        .iter()
        .filter(|i| i.selected && i.deletable)
        .map(|i| i.size_bytes)
        .sum();

    ui.horizontal(|ui| {
        ui.colored_label(
            BRAND,
            app.tf(
                "items_total_size",
                &[&items.len().to_string(), &format_size(total_size)],
            ),
        );
        if selected_cnt > 0 {
            ui.separator();
            ui.colored_label(
                CAUTION_COLOR,
                app.tf(
                    "selected_count_size",
                    &[&selected_cnt.to_string(), &format_size(selected_sz)],
                ),
            );
        }
    });

    ui.add_space(5.0);

    // ====== 操作按钮栏 ======
    ui.horizontal(|ui| {
        if ui.button(app.t("select_all")).clicked() {
            app.select_all();
        }
        if ui.button(app.t("deselect_all")).clicked() {
            app.deselect_all();
        }

        ui.separator();

        // 删除选中项
        let delete_enabled = selected_cnt > 0 && matches!(app.confirm, ConfirmState::None);
        if ui
            .add_enabled(
                delete_enabled,
                egui::Button::new(
                    egui::RichText::new(
                        app.tf("delete_selected_count", &[&selected_cnt.to_string()]),
                    )
                    .color(ADVANCED_COLOR),
                ),
            )
            .clicked()
        {
            app.prepare_delete();
        }
    });

    ui.add_space(5.0);

    // ====== 目录项列表 ======
    let max_size = items.first().map(|i| i.size_bytes).unwrap_or(1).max(1);
    let list_size = ui.available_size();

    egui::Frame::none()
        .fill(LIST_BG)
        .rounding(egui::Rounding::same(8.0))
        .show(ui, |ui| {
            ui.set_min_size(list_size);
            egui::ScrollArea::vertical()
                .auto_shrink([false; 2])
                .show(ui, |ui| {
                    for (i, item) in items.iter().enumerate() {
                        let is_dir = item.category == "目录";
                        let pct = if max_size > 0 {
                            item.size_bytes as f32 / max_size as f32
                        } else {
                            0.0
                        };

                        let name = std::path::Path::new(&item.path)
                            .file_name()
                            .and_then(|n| n.to_str())
                            .unwrap_or("unknown");

                        let icon = if is_dir { "📁" } else { "📄" };
                        let size_str = format_size(item.size_bytes);

                        // 每行：[图标+名称] [进度条] [大小] [操作]
                        ui.horizontal(|ui| {
                            // 选中 checkbox（仅文件或非当前目录可删除）
                            if item.deletable {
                                let mut selected = item.selected;
                                if ui.checkbox(&mut selected, "").changed() {
                                    app.results[tab_idx][i].selected = selected;
                                }
                            }

                            // 图标 + 名称（目录可点击进入）
                            let name_label = if is_dir {
                                format!("{} {}", icon, name)
                            } else {
                                format!("{} {}", icon, name)
                            };

                            let name_btn = ui.add(
                                egui::Label::new(egui::RichText::new(&name_label).size(13.0))
                                    .sense(egui::Sense::click()),
                            );

                            if is_dir && name_btn.clicked() && !is_scanning {
                                // 进入子目录
                                let new_path = std::path::PathBuf::from(&item.path);
                                app.disk_analyzer_enter(new_path);
                                start_scan(app, scan_rx);
                                return;
                            }

                            // 进度条（相对大小可视化）
                            ui.add(egui::ProgressBar::new(pct).desired_width(200.0).fill(
                                if pct > 0.5 {
                                    ADVANCED_COLOR
                                } else if pct > 0.2 {
                                    CAUTION_COLOR
                                } else {
                                    SAFE_COLOR
                                },
                            ));

                            // 大小
                            ui.label(egui::RichText::new(&size_str).size(13.0).strong());

                            // 百分比
                            let total_pct = if total_size > 0 {
                                item.size_bytes as f32 / total_size as f32 * 100.0
                            } else {
                                0.0
                            };
                            ui.label(
                                egui::RichText::new(format!("{:.1}%", total_pct))
                                    .size(11.0)
                                    .color(TEXT_TERTIARY),
                            );

                            // 目录：显示"进入"提示
                            if is_dir {
                                ui.label(egui::RichText::new("→").size(16.0).color(BRAND));
                            }
                        });

                        ui.separator();
                    }
                });
        });
}

/// 路径显示简化（用于扫描中提示）
fn display_path_short(path: &std::path::Path, lang_en: bool) -> String {
    let home = scanner::home_dir();
    if path == home.as_path() {
        App::t_lang(lang_en, "home").to_string()
    } else if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
        name.to_string()
    } else {
        path.display().to_string()
    }
}

/// 概览面板：聚合所有 Tab 的推荐清理项
fn render_overview_panel(
    ui: &mut egui::Ui,
    app: &mut App,
    scan_rx: &mut Option<mpsc::Receiver<ScanMessage>>,
) {
    // 聚合所有 Tab 的统计信息（跳过 Overview 自身，索引 0）
    let mut safe_total: u64 = 0;
    let mut caution_total: u64 = 0;
    let mut advanced_total: u64 = 0;
    let mut _safe_count = 0usize;
    let mut _caution_count = 0usize;
    let mut _advanced_count = 0usize;
    let mut recommendation_items: Vec<(usize, usize, ScanItem)> = Vec::new(); // (tab_index, item_index, item)

    for tab_idx in 1..app.results.len() {
        for (item_idx, item) in app.results[tab_idx].iter().enumerate() {
            if !item.deletable {
                continue;
            }
            match item.recommend {
                crate::scanner::Recommend::Safe | crate::scanner::Recommend::CacheOnly => {
                    safe_total += item.size_bytes;
                    _safe_count += 1;
                    recommendation_items.push((tab_idx, item_idx, item.clone()));
                }
                crate::scanner::Recommend::Caution => {
                    caution_total += item.size_bytes;
                    _caution_count += 1;
                }
                crate::scanner::Recommend::Advanced => {
                    advanced_total += item.size_bytes;
                    _advanced_count += 1;
                }
            }
        }
    }

    // 按大小降序排列推荐项
    recommendation_items.sort_by(|a, b| b.2.size_bytes.cmp(&a.2.size_bytes));

    // 判断是否有任一 Tab 正在扫描
    let any_scanning = app
        .scan_states
        .iter()
        .any(|s| matches!(s, ScanState::Scanning));

    // --- Summary Pills ---
    ui.add_space(5.0);
    ui.horizontal(|ui| {
        let is_blocked = !matches!(app.confirm, ConfirmState::None);

        // Safe 可释放空间（主 pill）
        let safe_resp = egui::Frame::none()
            .fill(egui::Color32::from_rgb(232, 255, 243))
            .stroke(egui::Stroke::new(1.0, SAFE_COLOR))
            .rounding(egui::Rounding::same(14.0))
            .inner_margin(egui::Margin::symmetric(16.0, 10.0))
            .show(ui, |ui| {
                ui.vertical(|ui| {
                    ui.colored_label(
                        TEXT_SECONDARY,
                        egui::RichText::new(app.t("overview_releasable")).size(12.0),
                    );
                    ui.add_space(2.0);
                    ui.horizontal(|ui| {
                        ui.colored_label(
                            TEXT_PRIMARY,
                            egui::RichText::new(format_size(safe_total))
                                .size(20.0)
                                .strong(),
                        );
                        ui.colored_label(
                            SAFE_COLOR,
                            egui::RichText::new("Safe").size(11.0).strong(),
                        );
                    });
                });
            });
        let safe_click = ui.interact(
            safe_resp.response.rect,
            egui::Id::new("overview_pill_safe"),
            egui::Sense::click(),
        );
        if safe_click.clicked() && !is_blocked {
            app.tab = Tab::Overview;
            app.list_index = 0;
        }
        ui.add_space(10.0);

        // Caution pill
        let caution_resp = egui::Frame::none()
            .fill(egui::Color32::from_rgb(255, 247, 230))
            .stroke(egui::Stroke::new(1.0, CAUTION_COLOR))
            .rounding(egui::Rounding::same(14.0))
            .inner_margin(egui::Margin::symmetric(14.0, 10.0))
            .show(ui, |ui| {
                ui.vertical(|ui| {
                    ui.colored_label(
                        TEXT_SECONDARY,
                        egui::RichText::new(app.t("caution_clean")).size(12.0),
                    );
                    ui.add_space(2.0);
                    ui.colored_label(
                        TEXT_PRIMARY,
                        egui::RichText::new(format_size(caution_total))
                            .size(18.0)
                            .strong(),
                    );
                });
            });
        let caution_click = ui.interact(
            caution_resp.response.rect,
            egui::Id::new("overview_pill_caution"),
            egui::Sense::click(),
        );
        if caution_click.clicked() && !is_blocked {
            app.tab = Tab::AppUninstall;
            app.list_index = 0;
        }
        ui.add_space(10.0);

        // Advanced pill
        let advanced_resp = egui::Frame::none()
            .fill(egui::Color32::from_rgb(255, 233, 230))
            .stroke(egui::Stroke::new(1.0, ADVANCED_COLOR))
            .rounding(egui::Rounding::same(14.0))
            .inner_margin(egui::Margin::symmetric(14.0, 10.0))
            .show(ui, |ui| {
                ui.vertical(|ui| {
                    ui.colored_label(
                        TEXT_SECONDARY,
                        egui::RichText::new(app.t("confirm_clean")).size(12.0),
                    );
                    ui.add_space(2.0);
                    ui.colored_label(
                        TEXT_PRIMARY,
                        egui::RichText::new(format_size(advanced_total))
                            .size(18.0)
                            .strong(),
                    );
                });
            });
        let advanced_click = ui.interact(
            advanced_resp.response.rect,
            egui::Id::new("overview_pill_advanced"),
            egui::Sense::click(),
        );
        if advanced_click.clicked() && !is_blocked {
            app.tab = Tab::AppData;
            app.list_index = 0;
        }
    });
    ui.add_space(16.0);

    // --- 推荐清理 ---
    ui.label(
        egui::RichText::new(app.t("overview_recommendation"))
            .size(16.0)
            .strong()
            .color(TEXT_PRIMARY),
    );
    ui.add_space(4.0);
    ui.colored_label(
        TEXT_TERTIARY,
        egui::RichText::new(app.t("overview_recommendation_hint")).size(12.0),
    );
    ui.add_space(10.0);

    if any_scanning {
        let scan_size = ui.available_size();
        egui::Frame::none()
            .fill(LIST_BG)
            .rounding(egui::Rounding::same(8.0))
            .show(ui, |ui| {
                ui.set_min_size(scan_size);
                ui_scanning(ui, app);
            });
    } else if recommendation_items.is_empty() {
        let empty_size = ui.available_size();
        egui::Frame::none()
            .fill(LIST_BG)
            .rounding(egui::Rounding::same(8.0))
            .show(ui, |ui| {
                ui.set_min_size(empty_size);
                ui.vertical_centered(|ui| {
                    ui.label(egui::RichText::new("🧹").size(48.0));
                    ui.add_space(10.0);
                    ui.label(
                        egui::RichText::new(app.t("overview_empty_title"))
                            .size(16.0)
                            .strong()
                            .color(TEXT_PRIMARY),
                    );
                    ui.add_space(6.0);
                    ui.label(
                        egui::RichText::new(app.t("overview_empty_hint"))
                            .size(13.0)
                            .color(TEXT_TERTIARY),
                    );
                });
            });
    } else {
        // 一键清理按钮
        let total_safe_size: u64 = recommendation_items
            .iter()
            .map(|(_, _, item)| item.size_bytes)
            .sum();
        let clean_btn = ui.add(
            egui::Button::new(
                egui::RichText::new(format!(
                    "🗑 {} {} ({})",
                    app.t("one_click_clean"),
                    recommendation_items.len(),
                    format_size(total_safe_size)
                ))
                .color(egui::Color32::WHITE),
            )
            .fill(BRAND)
            .rounding(egui::Rounding::same(8.0))
            .min_size(egui::vec2(ui.available_width(), 40.0)),
        );
        if clean_btn.clicked() {
            // 选中所有推荐项并准备删除
            for &(tab_idx, item_idx, _) in &recommendation_items {
                if let Some(item) = app.results[tab_idx].get_mut(item_idx) {
                    item.selected = true;
                }
            }
            app.prepare_delete();
        }
        ui.add_space(12.0);

        // 推荐项列表：占满剩余高度
        let list_size = ui.available_size();
        egui::Frame::none()
            .fill(LIST_BG)
            .rounding(egui::Rounding::same(8.0))
            .show(ui, |ui| {
                ui.set_min_size(list_size);
                egui::ScrollArea::vertical()
                    .auto_shrink([false; 2])
                    .show(ui, |ui| {
                        ui.add_space(10.0);
                        for &(tab_idx, _item_idx, ref item) in &recommendation_items {
                            let tab = Tab::all()[tab_idx];
                            let tab_title_text = tab_title(&tab, app);

                            egui::Frame::none()
                                .fill(SURFACE_ELEVATED)
                                .stroke(egui::Stroke::new(1.0, BORDER_LIGHT))
                                .rounding(egui::Rounding::same(8.0))
                                .inner_margin(egui::Margin::symmetric(14.0, 12.0))
                                .show(ui, |ui| {
                                    ui.horizontal(|ui| {
                                        ui.colored_label(
                                            SAFE_COLOR,
                                            egui::RichText::new("✓").size(14.0).strong(),
                                        );
                                        ui.add_space(10.0);
                                        ui.vertical(|ui| {
                                            ui.horizontal(|ui| {
                                                ui.colored_label(
                                                    TEXT_PRIMARY,
                                                    egui::RichText::new(&item.category)
                                                        .size(13.0)
                                                        .strong(),
                                                );
                                                ui.add_space(6.0);
                                                ui.colored_label(
                                                    TEXT_TERTIARY,
                                                    egui::RichText::new(format!(
                                                        "· {}",
                                                        tab_title_text
                                                    ))
                                                    .size(11.0),
                                                );
                                            });
                                            ui.colored_label(
                                                TEXT_SECONDARY,
                                                egui::RichText::new(truncate_path(&item.path, 70))
                                                    .size(11.0),
                                            );
                                            ui.colored_label(
                                                TEXT_SECONDARY,
                                                egui::RichText::new(&item.description).size(11.0),
                                            );
                                        });
                                        ui.with_layout(
                                            egui::Layout::right_to_left(egui::Align::Center),
                                            |ui| {
                                                ui.colored_label(
                                                    TEXT_PRIMARY,
                                                    egui::RichText::new(format_size(
                                                        item.size_bytes,
                                                    ))
                                                    .size(14.0)
                                                    .strong()
                                                    .monospace(),
                                                );
                                            },
                                        );
                                    });
                                });
                            ui.add_space(6.0);
                        }
                        ui.add_space(10.0);
                    });
            });
    }
}

fn render_optimize_panel(
    ui: &mut egui::Ui,
    app: &mut App,
    scan_rx: &mut Option<mpsc::Receiver<ScanMessage>>,
) {
    let tab_idx = app.tab_index();
    let items = app.results[tab_idx].clone();
    let is_scanning = matches!(app.scan_states[tab_idx], ScanState::Scanning);

    if is_scanning {
        let scan_size = ui.available_size();
        egui::Frame::none()
            .fill(LIST_BG)
            .rounding(egui::Rounding::same(8.0))
            .show(ui, |ui| {
                ui.set_min_size(scan_size);
                ui_scanning(ui, app);
            });
        return;
    }

    if items.is_empty() {
        let empty_size = ui.available_size();
        egui::Frame::none()
            .fill(LIST_BG)
            .rounding(egui::Rounding::same(8.0))
            .show(ui, |ui| {
                ui.set_min_size(empty_size);
                ui.vertical_centered(|ui| {
                    ui.add_space(80.0);
                    ui.label(
                        egui::RichText::new(app.t("optimize_click_to_scan"))
                            .size(16.0)
                            .color(TEXT_TERTIARY),
                    );
                    ui.add_space(10.0);
                    let scan_button = format!("🔍 {}", app.t("scan"));
                    if ui
                        .button(egui::RichText::new(scan_button).size(16.0))
                        .clicked()
                    {
                        start_scan(app, scan_rx);
                    }
                });
            });
        return;
    }

    ui.add_space(10.0);
    ui.heading(egui::RichText::new(app.t("tab_system_optimize")).size(18.0));
    ui.label(
        egui::RichText::new(app.t("optimize_safe_hint"))
            .size(12.0)
            .color(TEXT_TERTIARY),
    );
    ui.add_space(10.0);

    // 优化任务列表：占满剩余高度
    let mut task_to_run: Option<usize> = None;
    let list_size = ui.available_size();

    egui::Frame::none()
        .fill(LIST_BG)
        .rounding(egui::Rounding::same(8.0))
        .show(ui, |ui| {
            ui.set_min_size(list_size);
            egui::ScrollArea::vertical()
                .auto_shrink([false; 2])
                .show(ui, |ui| {
                    ui.add_space(10.0);
                    for (i, item) in items.iter().enumerate() {
                        egui::Frame::group(ui.style())
                            .fill(SURFACE)
                            .stroke(egui::Stroke::new(1.0, BORDER_LIGHT))
                            .inner_margin(12.0)
                            .outer_margin(4.0)
                            .show(ui, |ui| {
                                ui.horizontal(|ui| {
                                    // 图标
                                    ui.label(egui::RichText::new("⚙️").size(20.0));
                                    ui.vertical(|ui| {
                                        ui.horizontal(|ui| {
                                            let title_key = format!("optimize_{}", item.path);
                                            let title = app.t(&title_key);
                                            // 如果 key 不存在（返回空字符串），fallback 到原始 path
                                            let title =
                                                if title.is_empty() { &item.path } else { title };
                                            ui.label(
                                                egui::RichText::new(title).strong().size(14.0),
                                            );
                                            ui.label(
                                                egui::RichText::new(recommend_badge_text(
                                                    &item.recommend,
                                                    app.lang_en,
                                                ))
                                                .size(11.0),
                                            );
                                        });
                                        let desc_en = i18n::translate_description(
                                            &item.description,
                                            app.lang_en,
                                        );
                                        ui.label(
                                            egui::RichText::new(&desc_en)
                                                .size(12.0)
                                                .color(TEXT_SECONDARY),
                                        );
                                    });
                                    ui.with_layout(
                                        egui::Layout::right_to_left(egui::Align::Center),
                                        |ui| {
                                            let run_text =
                                                if app.lang_en { "▶ Run" } else { "▶ 执行" };
                                            if ui
                                                .button(egui::RichText::new(run_text).size(13.0))
                                                .clicked()
                                            {
                                                task_to_run = Some(i);
                                            }
                                        },
                                    );
                                });
                            });
                    }

                    // 显示优化日志（与任务列表一起滚动）
                    if !app.logs.is_empty() {
                        ui.add_space(5.0);
                        ui.collapsing(app.t("optimize_logs"), |ui| {
                            egui::ScrollArea::vertical()
                                .max_height(150.0)
                                .show(ui, |ui| {
                                    for log in &app.logs {
                                        ui.label(
                                            egui::RichText::new(log)
                                                .size(11.0)
                                                .color(TEXT_SECONDARY),
                                        );
                                    }
                                });
                        });
                    }
                    ui.add_space(10.0);
                });
        });

    // 执行选中的优化任务
    if let Some(task_idx) = task_to_run {
        if let Some(item) = items.get(task_idx) {
            let log = execute_optimize_task(&item.path, app.lang_en);
            app.logs.push(log);
        }
    }
}

/// 设置面板（设计稿 4.6 样式）
fn render_settings_panel(ui: &mut egui::Ui, app: &mut App) {
    let settings_size = ui.available_size();
    egui::Frame::none()
        .fill(LIST_BG)
        .rounding(egui::Rounding::same(8.0))
        .show(ui, |ui| {
            ui.set_min_size(settings_size);
            egui::ScrollArea::vertical()
                .auto_shrink([false; 2])
                .show(ui, |ui| {
                    ui.add_space(10.0);

                    // 通用设置卡片
                    settings_card(ui, app.t("settings_general"), |ui| {
                        render_settings_item(
                            ui,
                            app.t("setting_menubar_icon"),
                            app.t("setting_menubar_icon_desc"),
                            &mut app.settings_menubar_icon,
                            true,
                        );
                        render_settings_item(
                            ui,
                            app.t("setting_keep_sudo"),
                            app.t("setting_keep_sudo_desc"),
                            &mut app.settings_keep_sudo,
                            true,
                        );
                        render_settings_item(
                            ui,
                            app.t("setting_scan_cache"),
                            app.t("setting_scan_cache_desc"),
                            &mut app.settings_scan_cache,
                            true,
                        );
                    });

                    ui.add_space(12.0);

                    // 安全设置卡片
                    settings_card(ui, app.t("settings_safety"), |ui| {
                        render_settings_item(
                            ui,
                            app.t("setting_confirm_advanced"),
                            app.t("setting_confirm_advanced_desc"),
                            &mut app.settings_confirm_advanced,
                            true,
                        );
                        let lid_enabled = cfg!(target_os = "macos");
                        render_settings_item(
                            ui,
                            app.t("setting_prevent_lid_close"),
                            app.t("setting_prevent_lid_close_desc"),
                            &mut app.settings_prevent_lid_close,
                            lid_enabled,
                        );
                    });

                    ui.add_space(12.0);

                    // 语言设置卡片
                    settings_card(ui, app.t("settings_language"), |ui| {
                        ui.horizontal(|ui| {
                            ui.vertical(|ui| {
                                ui.colored_label(
                                    TEXT_PRIMARY,
                                    egui::RichText::new(app.t("setting_language"))
                                        .size(13.0)
                                        .strong(),
                                );
                                ui.colored_label(
                                    TEXT_SECONDARY,
                                    egui::RichText::new(app.t("setting_language_current"))
                                        .size(12.0),
                                );
                            });
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    let btn_text = if app.lang_en {
                                        app.t("switch_to_chinese")
                                    } else {
                                        app.t("switch_to_english")
                                    };
                                    if ui
                                        .add(
                                            egui::Button::new(
                                                egui::RichText::new(btn_text).size(13.0),
                                            )
                                            .fill(SURFACE_ELEVATED)
                                            .stroke(egui::Stroke::new(1.0, BORDER_LIGHT))
                                            .rounding(egui::Rounding::same(8.0)),
                                        )
                                        .clicked()
                                    {
                                        app.toggle_lang();
                                    }
                                },
                            );
                        });
                    });

                    ui.add_space(20.0);
                });
        });
}

/// 设置卡片容器
fn settings_card<R>(ui: &mut egui::Ui, title: &str, content: impl FnOnce(&mut egui::Ui) -> R) -> R {
    egui::Frame::none()
        .fill(SURFACE_ELEVATED)
        .stroke(egui::Stroke::new(1.0, BORDER_LIGHT))
        .rounding(egui::Rounding::same(12.0))
        .inner_margin(egui::Margin::same(16.0))
        .show(ui, |ui| {
            ui.label(
                egui::RichText::new(title)
                    .size(14.0)
                    .strong()
                    .color(TEXT_PRIMARY),
            );
            ui.add_space(12.0);
            content(ui)
        })
        .inner
}

/// 单个设置项（标签 + 描述 + Toggle）
fn render_settings_item(
    ui: &mut egui::Ui,
    label: &str,
    desc: &str,
    value: &mut bool,
    enabled: bool,
) {
    ui.horizontal(|ui| {
        ui.vertical(|ui| {
            ui.colored_label(TEXT_PRIMARY, egui::RichText::new(label).size(13.0).strong());
            ui.colored_label(TEXT_SECONDARY, egui::RichText::new(desc).size(12.0));
        });
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let _ = render_settings_toggle(ui, value, enabled);
        });
    });
    ui.add_space(8.0);
    // 分隔线
    let available = ui.available_rect_before_wrap();
    let sep_y = available.min.y;
    let sep_rect = egui::Rect::from_min_max(
        egui::pos2(available.min.x, sep_y),
        egui::pos2(available.max.x, sep_y + 1.0),
    );
    ui.painter()
        .rect_filled(sep_rect, egui::Rounding::ZERO, BORDER_LIGHT);
    ui.add_space(8.0);
}

/// 执行单个优化任务
fn execute_optimize_task(task_name: &str, lang_en: bool) -> String {
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    let result = match task_name {
        "dns_cache_flush" => {
            // dscacheutil 和 killall 在现代 macOS 上需要 sudo
            let r1 = std::process::Command::new("dscacheutil")
                .arg("-flushcache")
                .output();
            let r2 = std::process::Command::new("killall")
                .arg("-HUP")
                .arg("mDNSResponder")
                .output();
            let success = r1.map(|o| o.status.success()).unwrap_or(false)
                && r2.map(|o| o.status.success()).unwrap_or(false);
            if success {
                App::t_lang(lang_en, "opt_dns_success").to_string()
            } else {
                App::t_lang(lang_en, "opt_dns_fail").to_string()
            }
        }
        "quicklook_rebuild" => {
            let r = std::process::Command::new("qlmanage")
                .arg("-r")
                .arg("cache")
                .output();
            match r {
                Ok(_) => App::t_lang(lang_en, "opt_quicklook_success").to_string(),
                Err(_) => App::t_lang(lang_en, "opt_quicklook_fail").to_string(),
            }
        }
        "launchservices_rebuild" => {
            // lsregister 路径在 macOS 10.0-15 上一致，但加 fallback 更稳健
            let lsregister_candidates = [
                "/System/Library/Frameworks/CoreServices.framework/Versions/A/Frameworks/LaunchServices.framework/Versions/A/Support/lsregister",
                "/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister",
            ];
            let lsregister = lsregister_candidates
                .iter()
                .find(|p| std::path::Path::new(p).exists())
                .unwrap_or(&lsregister_candidates[0]);
            let r = std::process::Command::new(lsregister).arg("-gc").output();
            match r {
                Ok(_) => App::t_lang(lang_en, "opt_launchservices_success").to_string(),
                Err(_) => App::t_lang(lang_en, "opt_launchservices_fail").to_string(),
            }
        }
        "saved_state_cleanup" => {
            let home = std::env::var("HOME").unwrap_or_default();
            let state_dir = format!("{}/Library/Saved Application State", home);
            let mut count = 0;
            if let Ok(entries) = std::fs::read_dir(&state_dir) {
                for entry in entries.filter_map(|e| e.ok()) {
                    let path = entry.path();
                    if path.is_dir() {
                        // 检查修改时间是否超过 30 天
                        if let Ok(meta) = path.metadata() {
                            if let Ok(mtime) = meta.modified() {
                                if let Ok(age) = mtime.elapsed() {
                                    if age.as_secs() > 30 * 86400 {
                                        let _ = std::fs::remove_dir_all(&path);
                                        count += 1;
                                    }
                                }
                            }
                        }
                    }
                }
            }
            App::tf_lang(lang_en, "opt_saved_state_success", &[&count.to_string()])
        }
        "gatekeeper_cleanup" => {
            let home = std::env::var("HOME").unwrap_or_default();
            let db_path = format!(
                "{}/Library/Preferences/com.apple.LaunchServices.QuarantineEventsV2",
                home
            );
            if std::path::Path::new(&db_path).exists() {
                let r = std::process::Command::new("sqlite3")
                    .arg(&db_path)
                    .arg("DELETE FROM LSQuarantineEvent; VACUUM;")
                    .output();
                match r {
                    Ok(_) => App::t_lang(lang_en, "opt_gatekeeper_success").to_string(),
                    Err(_) => App::t_lang(lang_en, "opt_gatekeeper_fail").to_string(),
                }
            } else {
                App::t_lang(lang_en, "opt_gatekeeper_empty").to_string()
            }
        }
        "memory_pressure_release" => {
            // purge 在所有 macOS 版本上都需要 sudo
            let r = std::process::Command::new("purge").output();
            match r {
                Ok(o) if o.status.success() => {
                    App::t_lang(lang_en, "opt_memory_success").to_string()
                }
                _ => App::t_lang(lang_en, "opt_memory_fail").to_string(),
            }
        }
        "spotlight_reindex" => {
            // mdutil -E / 重建根卷的 Spotlight 索引
            let r = std::process::Command::new("mdutil")
                .arg("-E")
                .arg("/")
                .output();
            match r {
                Ok(o) if o.status.success() => {
                    App::t_lang(lang_en, "opt_spotlight_success").to_string()
                }
                _ => App::t_lang(lang_en, "opt_spotlight_fail").to_string(),
            }
        }
        "login_items_audit" => {
            // 打开系统设置 > 通用 > 登录项
            // macOS 13+ 使用 "x-apple.systempreferences:com.apple.LoginItems-Settings.extension"
            // macOS 12 及以下使用 "com.apple.preference.users"
            let url = "x-apple.systempreferences:com.apple.LoginItems-Settings.extension";
            let r = std::process::Command::new("open").arg(url).output();
            match r {
                Ok(o) if o.status.success() => {
                    App::t_lang(lang_en, "opt_login_items_opened").to_string()
                }
                _ => App::t_lang(lang_en, "opt_login_items_fail").to_string(),
            }
        }
        _ => App::tf_lang(lang_en, "opt_unknown", &[task_name]),
    };

    format!("[{}] {}", timestamp, result)
}

/// 扫描中 UI
fn ui_scanning(ui: &mut egui::Ui, app: &mut App) {
    let tab_idx = app.tab_index();
    let tab_title_text = tab_title(&app.tab, app);
    let pct = (app.scan_progress * 100.0).clamp(0.0, 100.0) as u32;
    let discovered = app.results[tab_idx].len();
    let discovered_size: u64 = app.results[tab_idx].iter().map(|i| i.size_bytes).sum();
    let current_path = if app.scan_current_path.is_empty() {
        app.t("scanning_hint").to_string()
    } else {
        app.scan_current_path.clone()
    };

    ui.vertical_centered(|ui| {
        ui.add_space(24.0);
        egui::Frame::none()
            .fill(SURFACE_ELEVATED)
            .stroke(egui::Stroke::new(1.0, BORDER_LIGHT))
            .rounding(egui::Rounding::same(12.0))
            .inner_margin(egui::Margin::same(20.0))
            .show(ui, |ui| {
                ui.set_min_width(420.0);
                ui.vertical_centered(|ui| {
                    // 自定义旋转 spinner
                    let spinner_size = 40.0;
                    let (rect, _resp) = ui.allocate_exact_size(
                        egui::vec2(spinner_size, spinner_size),
                        egui::Sense::hover(),
                    );
                    let time = ui.ctx().input(|i| i.time);
                    let start_angle = (time * 2.0) as f32;
                    let sweep = std::f32::consts::PI * 1.25;
                    let painter = ui.painter();
                    let center = rect.center();
                    let radius = spinner_size * 0.5 - 2.0;
                    let stroke_width = 3.0;
                    // 背景圆环
                    painter.circle_stroke(
                        center,
                        radius,
                        egui::Stroke::new(stroke_width, SURFACE_MUTED),
                    );
                    // 旋转弧
                    let segments = 24;
                    let mut points: Vec<egui::Pos2> = Vec::with_capacity(segments + 1);
                    for i in 0..=segments {
                        let t = i as f32 / segments as f32;
                        let a = start_angle + sweep * t;
                        points.push(egui::pos2(
                            center.x + radius * a.cos(),
                            center.y + radius * a.sin(),
                        ));
                    }
                    painter.add(egui::Shape::Path(egui::epaint::PathShape::line(
                        points,
                        egui::Stroke::new(stroke_width, BRAND),
                    )));
                    ui.ctx().request_repaint();

                    ui.add_space(16.0);
                    ui.label(
                        egui::RichText::new(format!(
                            "{} {}...",
                            app.t("analyzing"),
                            tab_title_text
                        ))
                        .size(16.0)
                        .strong()
                        .color(TEXT_PRIMARY),
                    );
                    ui.add_space(4.0);
                    ui.label(
                        egui::RichText::new(app.tf(
                            "items_total_size",
                            &[&discovered.to_string(), &format_size(discovered_size)],
                        ))
                        .size(13.0)
                        .color(TEXT_SECONDARY),
                    );
                });

                ui.add_space(16.0);
                // 进度条
                let progress_rect = ui.available_rect_before_wrap();
                let bar_rect = egui::Rect::from_min_size(
                    progress_rect.min,
                    egui::vec2(progress_rect.width().max(360.0), 8.0),
                );
                let painter = ui.painter();
                painter.rect_filled(bar_rect, egui::Rounding::same(4.0), SURFACE_MUTED);
                let fill_width = bar_rect.width() * app.scan_progress.clamp(0.0, 1.0);
                if fill_width > 0.0 {
                    let fill_rect = egui::Rect::from_min_size(
                        bar_rect.min,
                        egui::vec2(fill_width, bar_rect.height()),
                    );
                    painter.rect_filled(fill_rect, egui::Rounding::same(4.0), BRAND);
                }
                ui.allocate_rect(bar_rect, egui::Sense::hover());

                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    ui.colored_label(
                        TEXT_SECONDARY,
                        egui::RichText::new(format!(
                            "{}% · {} {}",
                            pct,
                            app.t("scanning"),
                            current_path
                        ))
                        .size(12.0)
                        .monospace(),
                    );
                });

                // 日志区域
                ui.add_space(12.0);
                egui::Frame::none()
                    .fill(SURFACE_MUTED)
                    .stroke(egui::Stroke::new(1.0, BORDER_LIGHT))
                    .rounding(egui::Rounding::same(6.0))
                    .inner_margin(egui::Margin::same(10.0))
                    .show(ui, |ui| {
                        egui::ScrollArea::vertical()
                            .max_height(120.0)
                            .stick_to_bottom(true)
                            .show(ui, |ui| {
                                if app.logs.is_empty() {
                                    ui.colored_label(
                                        TEXT_SECONDARY,
                                        egui::RichText::new(format!(
                                            "{} {}",
                                            app.t("scanning"),
                                            current_path
                                        ))
                                        .size(11.0)
                                        .monospace(),
                                    );
                                } else {
                                    for log in app.logs.iter().rev().take(20) {
                                        let color = if log.starts_with('✓') || log.starts_with('✅')
                                        {
                                            SAFE_COLOR
                                        } else if log.starts_with('✗') || log.starts_with('⛔') {
                                            ADVANCED_COLOR
                                        } else if log.starts_with('⚠') {
                                            CAUTION_COLOR
                                        } else {
                                            TEXT_SECONDARY
                                        };
                                        ui.colored_label(
                                            color,
                                            egui::RichText::new(log).size(11.0).monospace(),
                                        );
                                    }
                                }
                            });
                    });
            });
    });
    ui.ctx()
        .request_repaint_after(std::time::Duration::from_millis(100));
}
