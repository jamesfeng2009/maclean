//! maclean - macOS 磁盘清理 GUI 工具
//!
//! 使用 egui 构建，专注开发者缓存与深度清理。

#[cfg(target_os = "macos")]
mod aewp;
mod app;
mod app_protection;
mod cli;
mod i18n;
mod safety;
mod scanner;
#[cfg(target_os = "macos")]
mod sudo_keepalive;
#[cfg(target_os = "macos")]
mod touchid;
#[cfg(target_os = "macos")]
mod menubar;
mod platform;

use std::sync::mpsc;
use std::path::PathBuf;

use eframe::egui;

use app::{App, ConfirmState, ScanState, Tab};
use scanner::{format_size, Recommend, ScanItem, Scanner};

/// 后台扫描消息
enum ScanMessage {
    /// 扫描进度更新
    Progress(f32),
    /// 增量结果（扫描中部分项）— (items, tab_index)
    PartialItems(Vec<ScanItem>, u64),
    /// 扫描完成
    Done(Vec<ScanItem>, u64, u64), // (items, scan_time_ms, tab_index)
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
}

/// 初始化崩溃日志文件，返回日志路径
fn init_crash_log() -> PathBuf {
    let log_dir = platform::app_data_dir().join("logs");
    let _ = std::fs::create_dir_all(&log_dir);
    let log_path = log_dir.join("crash.log");

    // 写入启动分隔线
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .append(true)
        .create(true)
        .open(&log_path)
    {
        use std::io::Write;
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let _ = writeln!(f, "\n=== maclean 启动 @ {} ===", timestamp);
    }

    log_path
}

/// 写入扫描日志（用于追踪扫描进度，崩溃时定位问题）
pub fn log_scan_step(msg: &str) {
    let log_path = platform::app_data_dir().join("logs/scan.log");
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .append(true)
        .create(true)
        .open(&log_path)
    {
        use std::io::Write;
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let _ = writeln!(f, "[{}] {}", timestamp, msg);
    }
}

fn main() -> eframe::Result {
    // CLI 模式：有子命令时执行并退出，无子命令时启动 GUI
    if cli::run_cli() {
        return Ok(());
    }

    // 初始化崩溃日志
    let log_path = init_crash_log();

    // 设置全局 panic hook：写入崩溃日志文件，不崩溃
    let log_path_for_hook = log_path.clone();
    std::panic::set_hook(Box::new(move |info| {
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let msg = format!("[{}] PANIC: {}\n", timestamp, info);
        eprintln!("{}", msg);
        // 追加写入崩溃日志
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .append(true)
            .create(true)
            .open(&log_path_for_hook)
        {
            use std::io::Write;
            let _ = f.write_all(msg.as_bytes());
            let _ = f.write_all(format!("Backtrace: {}\n", std::backtrace::Backtrace::force_capture()).as_bytes());
        }
    }));

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
        #[cfg(target_os = "macos")]
        static mut MENUBAR: Option<menubar::MenuBarHud> = None;
        static mut NEEDS_INIT: bool = true;
        static mut LAST_DISK_UPDATE: f64 = 0.0;
        // QuickClean 标志：扫描完成后自动选择 Safe 项并删除
        static mut AUTO_CLEAN_AFTER_SCAN: bool = false;

        unsafe {
            if NEEDS_INIT {
                APP = Some(App::new());
                NEEDS_INIT = false;
                setup_fonts(ctx);

                // 初始化菜单栏 HUD (macOS 专属)
                #[cfg(target_os = "macos")]
                {
                    MENUBAR = Some(menubar::MenuBarHud::new());
                    if let Some(ref mut mb) = MENUBAR {
                        mb.init();
                    }
                }
            }

            // 轮询菜单栏事件 (macOS 专属)
            #[cfg(target_os = "macos")]
            if let Some(ref mb) = MENUBAR {
                let actions = mb.poll_events();
                if !actions.is_empty() {
                    let lang_en = APP.as_ref().map(|a| a.lang_en).unwrap_or(false);
                    log_scan_step(&App::tf_lang(lang_en, "log_menu_event", &[&format!("{:?}", actions)]));
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
            }

            // 定期更新菜单栏磁盘使用率（每 60 秒）
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs_f64())
                .unwrap_or(0.0);
            if now - LAST_DISK_UPDATE > 60.0 || LAST_DISK_UPDATE == 0.0 {
                LAST_DISK_UPDATE = now;
                if let Some(app) = &APP {
                    let used_pct = if app.disk_total > 0 {
                        (app.disk_total - app.disk_free) as f32 / app.disk_total as f32 * 100.0
                    } else {
                        0.0
                    };
                    #[cfg(target_os = "macos")]
                    if let Some(ref mut mb) = MENUBAR {
                        mb.update_disk_usage(used_pct);
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
                                app.results[idx].extend(items);
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

                                // 一键清理模式：自动选择 Safe 项并删除
                                if AUTO_CLEAN_AFTER_SCAN {
                                    AUTO_CLEAN_AFTER_SCAN = false;
                                    app.select_safe_only();
                                    let selected_count = app.selected_count();
                                    if selected_count > 0 {
                                        log_scan_step(&app.tf(
                                            "log_quickclean_start",
                                            &[&selected_count.to_string()]
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
                                    app.touch_id_error = Some(
                                        app.t("touchid_clamshell_error").to_string()
                                    );
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
                                        start_sudo_delete_touchid(items, app.lang_en, &mut DELETE_RX);
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
        "C:\\Windows\\Fonts\\msyh.ttc",      // 微软雅黑
        "C:\\Windows\\Fonts\\msyhbd.ttc",     // 微软雅黑粗体
        "C:\\Windows\\Fonts\\simhei.ttf",     // 黑体
        "C:\\Windows\\Fonts\\simsun.ttc",     // 宋体
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

/// 推荐等级颜色
fn recommend_color(rec: &Recommend) -> egui::Color32 {
    match rec {
        Recommend::Safe => egui::Color32::from_rgb(52, 199, 89),      // 绿色
        Recommend::Caution => egui::Color32::from_rgb(255, 159, 10),  // 橙色
        Recommend::Advanced => egui::Color32::from_rgb(255, 69, 58),  // 红色
    }
}

/// 推荐等级标签文字
fn recommend_badge(rec: &Recommend) -> &'static str {
    match rec {
        Recommend::Safe => "🟢",
        Recommend::Caution => "🟡",
        Recommend::Advanced => "🔴",
    }
}

/// 推荐等级标签文字（含多语言文字）
fn recommend_badge_text(rec: &Recommend, lang_en: bool) -> String {
    let icon = recommend_badge(rec);
    format!("{} {}", icon, i18n::translate_recommend(rec, lang_en))
}

/// 渲染 GUI 主界面
fn render_gui(ctx: &egui::Context, app: &mut App, scan_rx: &mut Option<mpsc::Receiver<ScanMessage>>, delete_rx: &mut Option<mpsc::Receiver<DeleteMessage>>) {
    // 动态更新窗口标题（跟随语言切换）
    ctx.send_viewport_cmd(egui::ViewportCommand::Title(app.t("window_title").to_string()));

    // ========== 顶部：标题栏 + 磁盘概览 ==========
    egui::TopBottomPanel::top("menu_bar").show(ctx, |ui| {
        ui.horizontal(|ui| {
            ui.heading("🧹 Maclean");
            ui.separator();

            if ui.button(if app.lang_en { "中文" } else { "EN" }).clicked() {
                app.toggle_lang();
            }
            ui.separator();

            // 磁盘概览
            let used = app.disk_total.saturating_sub(app.disk_free);
            let used_pct = if app.disk_total > 0 {
                used as f32 / app.disk_total as f32 * 100.0
            } else {
                0.0
            };

            ui.label(format!(
                "{}: {} ({:.0}%)  {}: {}  {}: {}",
                app.t("disk_used"),
                format_size(used),
                used_pct,
                app.t("disk_free"),
                format_size(app.disk_free),
                app.t("disk_total"),
                format_size(app.disk_total),
            ));

            let bar_color = if used_pct > 85.0 {
                egui::Color32::RED
            } else if used_pct > 70.0 {
                egui::Color32::YELLOW
            } else {
                egui::Color32::GREEN
            };

            ui.add(egui::ProgressBar::new(used_pct / 100.0)
                .fill(bar_color)
                .text(format!("{:.0}%", used_pct)));
        });
    });

    // ========== 磁盘告警横幅 ==========
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
            let msg = app.tf(msg_key, &[&format!("{:.1}", free_pct), &format!("{:.1}", free_gb)]);

            ui.horizontal(|ui| {
                ui.colored_label(alert_color, egui::RichText::new(format!("{} {}", icon, msg)).size(14.0));
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

    // ========== 底部状态栏 ==========
    egui::TopBottomPanel::bottom("status_bar").show(ctx, |ui| {
        ui.horizontal(|ui| {
            let tab_idx = app.tab_index();
            match &app.scan_states[tab_idx] {
                ScanState::Idle => {
                    ui.colored_label(egui::Color32::GRAY, app.t("press_r_to_scan"));
                }
                ScanState::Scanning => {
                    ui.colored_label(egui::Color32::from_rgb(0, 200, 255), "⏳ ".to_string() + app.t("scanning"));
                }
                ScanState::Done => {
                    let count = app.current_items().len();
                    let total: u64 = app.current_items().iter().map(|i| i.size_bytes).sum();
                    ui.colored_label(
                        egui::Color32::GREEN,
                        app.tf("found_items_total", &[&count.to_string(), &format_size(total)]),
                    );
                }
            }

            ui.separator();

            // 日志按钮
            if ui.small_button(format!("📋 {}", app.t("logs"))).clicked() {
                let home = std::env::var("HOME").unwrap_or_default();
                let log_dir = format!("{}/.maclean/logs", home);
                // 在 Finder 中打开日志目录
                let _ = std::process::Command::new("open")
                    .arg(&log_dir)
                    .spawn();
            }
        });
    });

    // ========== 中央内容区 ==========
    egui::CentralPanel::default().show(ctx, |ui| {
        // --- Tab 栏 ---
        ui.horizontal(|ui| {
            for (i, tab) in Tab::all().iter().enumerate() {
                let count = app.results[i].len();
                let tab_title = if count > 0 {
                    format!("{} ({})", tab_title(tab, app), count)
                } else {
                    tab_title(tab, app).to_string()
                };

                let is_selected = *tab == app.tab;
                let button = ui.selectable_label(is_selected, &tab_title);

                if button.clicked() && matches!(app.confirm, ConfirmState::None) {
                    app.tab = *tab;
                    app.list_index = 0;
                }
            }

            ui.separator();

            // 扫描按钮
            let is_scanning = matches!(app.current_scan_state(), ScanState::Scanning);
            let scan_button = ui.add_enabled(!is_scanning, egui::Button::new(if is_scanning { "⏳..." } else { app.t("scan") }));
            if scan_button.clicked() {
                start_scan(app, scan_rx);
            }

            // 强制刷新按钮（清除缓存后重新扫描）
            let refresh_btn = ui.add_enabled(!is_scanning, egui::Button::new("🔄"));
            if refresh_btn.clicked() {
                let tab_name = match app.tab {
                    Tab::DevCache => "dev_cache",
                    Tab::LargeFiles => "large_files",
                    Tab::AppCache => "app_cache",
                    Tab::AppData => "app_data",
                    Tab::AppUninstall => "app_uninstall",
                    Tab::SystemOptimize => "system_optimize",
                    Tab::Apfs => "apfs",
                };
                scanner::cache::invalidate_cache(tab_name);
                start_scan(app, scan_rx);
            }
        });

        // --- 系统优化 Tab：特殊渲染（操作面板而非列表选择）---
        if app.tab == Tab::SystemOptimize {
            render_optimize_panel(ui, app, scan_rx);
            return;
        }

        // --- 磁盘分析器 Tab：目录钻取式浏览 ---
        if app.tab == Tab::LargeFiles {
            render_disk_analyzer(ui, app, scan_rx);
            return;
        }

        // --- 扫描结果区 ---
        let tab_idx = app.tab_index();
        let items = app.results[tab_idx].clone();
        let is_scanning = matches!(app.scan_states[tab_idx], ScanState::Scanning);

        if is_scanning {
            // 扫描中：显示带百分比的进度长条 + 已发现的部分项
            ui.add_space(20.0);
            ui.vertical_centered(|ui| {
                ui.add(egui::Spinner::new().size(40.0));
                ui.add_space(10.0);
                ui.label(egui::RichText::new(format!("⏳ {}...", app.t("scanning"))).size(16.0).color(egui::Color32::from_rgb(0, 200, 255)));
                ui.add_space(5.0);
                ui.label(egui::RichText::new(app.t("scanning_hint")).size(12.0).color(egui::Color32::GRAY));
                ui.add_space(15.0);
                // 真实进度百分比长条
                let pct = (app.scan_progress * 100.0) as u32;
                ui.add(egui::ProgressBar::new(app.scan_progress)
                    .desired_width(500.0)
                    .fill(egui::Color32::from_rgb(0, 200, 255))
                    .text(format!("{}%", pct)));
            });

            // 增量显示已发现的部分项
            if !items.is_empty() {
                ui.add_space(10.0);
                let partial_count = items.len();
                let partial_size: u64 = items.iter().map(|i| i.size_bytes).sum();
                ui.colored_label(
                    egui::Color32::from_rgb(0, 200, 255),
                    egui::RichText::new(format!(
                        "🔍 {} {} ({})",
                        partial_count,
                        app.t("items_found"),
                        format_size(partial_size)
                    )).size(13.0),
                );
                ui.add_space(5.0);

                // 可滚动的部分项列表
                egui::ScrollArea::vertical()
                    .max_height(300.0)
                    .show(ui, |ui| {
                        for item in &items {
                            let rec_color = if !item.deletable {
                                egui::Color32::from_gray(80)
                            } else {
                                recommend_color(&item.recommend)
                            };
                            let badge = if !item.deletable {
                                "🔒".to_string()
                            } else {
                                recommend_badge_text(&item.recommend, app.lang_en)
                            };

                            let row_bg = egui::Color32::from_rgb(35, 35, 42);

                            egui::Frame::none()
                                .fill(row_bg)
                                .inner_margin(egui::Margin::symmetric(8.0, 4.0))
                                .stroke(egui::Stroke::new(0.5, egui::Color32::from_rgb(50, 50, 55)))
                                .show(ui, |ui| {
                                    ui.horizontal(|ui| {
                                        ui.colored_label(rec_color, &badge);
                                        ui.add_space(3.0);
                                        let cat_en = i18n::translate_category(&item.category, app.lang_en);
                                        ui.colored_label(category_color(&item.category), &cat_en);
                                        let size_str = if item.size_bytes == 0 { "—".to_string() } else { format_size(item.size_bytes) };
                                        ui.colored_label(egui::Color32::from_rgb(100, 200, 100), &size_str);
                                        ui.add_space(5.0);
                                        ui.colored_label(
                                            egui::Color32::from_gray(130),
                                            egui::RichText::new(truncate_path(&item.path, 60)).size(11.0),
                                        );
                                    });
                                });
                            ui.add_space(1.0);
                        }
                    });
            }

            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        } else if items.is_empty() {
            ui.vertical_centered(|ui| {
                ui.add_space(80.0);
                ui.label(egui::RichText::new(app.t("no_items_hint")).size(16.0).color(egui::Color32::GRAY));
                ui.add_space(10.0);
                let scan_hint = format!("🔍 {} → {}", app.t("scan"), app.t("click_to_start"));
                if ui.button(egui::RichText::new(&scan_hint).size(16.0)).clicked() {
                    start_scan(app, scan_rx);
                }
            });
        } else {
            // ====== 扫描结果汇总卡片 ======
            ui.add_space(5.0);

            let safe_cnt = app.safe_count();
            let safe_sz = app.safe_size();
            let caution_cnt = app.caution_count();
            let caution_sz = app.caution_size();
            let advanced_cnt = app.advanced_count();
            let advanced_sz = app.advanced_size();
            let selected_cnt = app.selected_count();
            let selected_sz = app.selected_total_size();

            egui::Frame::group(ui.style())
                .fill(egui::Color32::from_rgb(30, 30, 40))
                .stroke(egui::Stroke::new(1.0, egui::Color32::from_rgb(60, 60, 70)))
                .inner_margin(egui::Margin::same(10.0))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        // 推荐清理
                        ui.colored_label(
                            egui::Color32::from_rgb(52, 199, 89),
                            format!("🟢 {} {} ({}), {} {}", safe_cnt, app.t("items"), app.t("safe_clean"), app.t("total"), format_size(safe_sz)),
                        );
                        ui.separator();

                        // 谨慎清理
                        ui.colored_label(
                            egui::Color32::from_rgb(255, 159, 10),
                            format!("🟡 {} {} ({}), {} {}", caution_cnt, app.t("items"), app.t("caution_clean"), app.t("total"), format_size(caution_sz)),
                        );

                        // 需确认（如果有）
                        if advanced_cnt > 0 {
                            ui.separator();
                            ui.colored_label(
                                egui::Color32::from_rgb(255, 69, 58),
                                format!("🔴 {} {} ({}), {} {}", advanced_cnt, app.t("items"), app.t("confirm_clean"), app.t("total"), format_size(advanced_sz)),
                            );
                        }

                        ui.separator();

                        // 已选
                        if selected_cnt > 0 {
                            ui.colored_label(
                                egui::Color32::from_rgb(100, 200, 255),
                                format!("✓ {} {}, {} {}", selected_cnt, app.t("items_selected"), app.t("total"), format_size(selected_sz)),
                            );
                        }
                    });
                });

            ui.add_space(5.0);

            // ====== 操作按钮栏 ======
            ui.horizontal(|ui| {
                // 智能选择：只选推荐清理
                let smart_btn = ui.add(
                    egui::Button::new(egui::RichText::new(format!("🟢 {}", app.t("select_safe"))).color(egui::Color32::from_rgb(52, 199, 89)))
                );
                if smart_btn.clicked() {
                    app.select_safe_only();
                }

                if ui.button(app.t("select_all")).clicked() {
                    app.select_all();
                }
                if ui.button(app.t("deselect_all")).clicked() {
                    app.deselect_all();
                }

                ui.separator();

                // 删除按钮
                let delete_enabled = selected_cnt > 0 && matches!(app.confirm, ConfirmState::None);
                let delete_btn = ui.add_enabled(
                    delete_enabled,
                    egui::Button::new(egui::RichText::new(format!("🗑 {} ({})", app.t("delete"), format_size(selected_sz))).color(egui::Color32::WHITE)),
                );
                if delete_btn.clicked() {
                    app.prepare_delete();
                }
            });

            ui.separator();
            ui.add_space(3.0);

            // ====== 过滤/搜索输入框 ======
            // / 键聚焦输入框，Esc 清除过滤
            let filter_placeholder = app.t("filter_placeholder").to_string();
            let filter_response = ui.horizontal(|ui| {
                ui.label("🔍");
                let resp = ui.add(
                    egui::TextEdit::singleline(&mut app.filter_query)
                        .hint_text(&filter_placeholder)
                        .desired_width(400.0),
                );
                if resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                    app.clear_filter();
                }
                if !app.filter_query.is_empty() {
                    if ui.button("✕").clicked() {
                        app.clear_filter();
                    }
                }
                app.filter_active = resp.has_focus();
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
                    egui::Color32::from_rgb(100, 200, 255),
                    egui::RichText::new(app.tf("filter_results", &[&filtered_count.to_string(), &total_count.to_string()])).size(12.0),
                );
            }

            let _ = filter_response;

            ui.add_space(3.0);

            // 过滤后无结果
            if filtered_count == 0 && !app.filter_query.trim().is_empty() {
                let no_match_text = app.t("no_match").to_string();
                ui.vertical_centered(|ui| {
                    ui.add_space(40.0);
                    ui.label(egui::RichText::new(&no_match_text).size(14.0).color(egui::Color32::GRAY));
                });
                return;
            }

            // ====== 可滚动列表 ======
            // 提前克隆关联明细和展开状态，避免借用冲突
            let associated_details = app.associated_details.clone();
            let expanded_items = app.expanded_items.clone();
            let is_uninstall_tab = app.tab == crate::app::Tab::AppUninstall;

            egui::ScrollArea::vertical().show(ui, |ui| {
                let mut toggled_indices: Vec<usize> = Vec::new();
                let mut expand_toggles: Vec<String> = Vec::new();

                for display_idx in &filtered_indices {
                    let i = *display_idx;
                    let item = &items[i];
                    let rec_color = if !item.deletable {
                        egui::Color32::from_gray(80)
                    } else {
                        recommend_color(&item.recommend)
                    };
                    let badge = if !item.deletable {
                        app.t("badge_undeletable")
                    } else {
                        recommend_badge(&item.recommend)
                    };

                    // 行背景色
                    let row_bg = if item.selected {
                        egui::Color32::from_rgb(30, 50, 30)
                    } else if i % 2 == 0 {
                        egui::Color32::from_rgb(35, 35, 42)
                    } else {
                        egui::Color32::from_rgb(28, 28, 34)
                    };

                    let row_frame = egui::Frame::none()
                        .fill(row_bg)
                        .inner_margin(egui::Margin::symmetric(8.0, 6.0))
                        .stroke(egui::Stroke::new(0.5, egui::Color32::from_rgb(50, 50, 55)));

                    // 检查是否有可展开的关联明细
                    let has_details = is_uninstall_tab
                        && !item.batch_paths.is_empty()
                        && associated_details.contains_key(&item.path);
                    let is_expanded = expanded_items.contains(&item.path);

                    let row_resp = row_frame.show(ui, |ui| {
                        ui.horizontal(|ui| {
                            // 复选框
                            let checkbox_text = if !item.deletable { "🔒" } else if item.selected { "✅" } else { "⬜" };
                            let cb_color = if !item.deletable {
                                egui::Color32::GRAY
                            } else if item.selected {
                                egui::Color32::from_rgb(52, 199, 89)
                            } else {
                                egui::Color32::from_gray(160)
                            };
                            if ui.colored_label(cb_color, checkbox_text).clicked() && item.deletable {
                                toggled_indices.push(i);
                            }

                            // 推荐等级标签
                            ui.colored_label(rec_color, badge);

                            // 展开/收起按钮（仅 App卸载 tab 且有明细时显示）
                            if has_details {
                                let expand_icon = if is_expanded { "▼" } else { "▶" };
                                if ui.small_button(expand_icon).clicked() {
                                    expand_toggles.push(item.path.clone());
                                }
                            } else {
                                ui.add_space(14.0); // 占位对齐
                            }

                            ui.add_space(3.0);

                            // 类别 + 描述（用户看得懂的信息）
                            ui.vertical(|ui| {
                                ui.horizontal(|ui| {
                                    let cat_color = if !item.deletable {
                                        egui::Color32::from_gray(80)
                                    } else {
                                        category_color(&item.category)
                                    };
                                    let cat_en = i18n::translate_category(&item.category, app.lang_en);
                                    ui.colored_label(cat_color, &cat_en);
                                    // 大小（为 0 时显示 —，可能是空目录或未获取到）
                                    let size_str = if item.size_bytes == 0 {
                                        "—".to_string()
                                    } else {
                                        format_size(item.size_bytes)
                                    };
                                    let size_color = if item.size_bytes > 100 * 1024 * 1024 * 1024 {
                                        egui::Color32::RED
                                    } else if item.size_bytes > 1024 * 1024 * 1024 {
                                        egui::Color32::from_rgb(255, 159, 10)
                                    } else {
                                        egui::Color32::from_rgb(100, 200, 100)
                                    };
                                    ui.colored_label(size_color, &size_str);
                                });
                                // 描述说明（告诉用户这是什么，删除后有什么影响）
                                let desc_en = i18n::translate_description(&item.description, app.lang_en);
                                ui.colored_label(
                                    egui::Color32::from_gray(140),
                                    egui::RichText::new(&desc_en).size(12.0),
                                );
                                // 路径
                                let path_display = truncate_path(&item.path, 70);
                                let path_color = if item.deletable {
                                    egui::Color32::from_gray(100)
                                } else {
                                    egui::Color32::from_gray(70)
                                };
                                ui.colored_label(
                                    path_color,
                                    egui::RichText::new(&path_display).size(11.0),
                                );

                                // 不可删除时显示原因
                                if !item.deletable && !item.undeletable_reason.is_empty() {
                                    let reason_en = i18n::translate_undeletable_reason(&item.undeletable_reason, app.lang_en);
                                    ui.colored_label(
                                        egui::Color32::from_rgb(200, 80, 80),
                                        egui::RichText::new(format!("⚠️ {}", reason_en)).size(11.0),
                                    );
                                }
                            });
                        });
                    });

                    // 点击行也切换选中
                    if row_resp.response.clicked() && item.deletable {
                        toggled_indices.push(i);
                    }

                    // 展开时显示关联文件明细
                    if has_details && is_expanded {
                        if let Some(details) = associated_details.get(&item.path) {
                            for (detail_path, detail_size, detail_label) in details {
                                let detail_bg = egui::Color32::from_rgb(22, 22, 28);
                                let detail_frame = egui::Frame::none()
                                    .fill(detail_bg)
                                    .inner_margin(egui::Margin::symmetric(8.0, 3.0))
                                    .stroke(egui::Stroke::new(0.3, egui::Color32::from_rgb(40, 40, 45)));

                                detail_frame.show(ui, |ui| {
                                    ui.horizontal(|ui| {
                                        ui.add_space(40.0); // 缩进对齐
                                        // 标签
                                        ui.colored_label(
                                            egui::Color32::from_rgb(130, 160, 255),
                                            egui::RichText::new(detail_label).size(12.0),
                                        );
                                        // 大小
                                        let sz_str = if *detail_size == 0 {
                                            "—".to_string()
                                        } else {
                                            format_size(*detail_size)
                                        };
                                        let sz_color = if *detail_size > 1024 * 1024 * 1024 {
                                            egui::Color32::from_rgb(255, 159, 10)
                                        } else if *detail_size > 100 * 1024 * 1024 {
                                            egui::Color32::from_rgb(255, 200, 100)
                                        } else {
                                            egui::Color32::from_gray(140)
                                        };
                                        ui.colored_label(sz_color, egui::RichText::new(&sz_str).size(12.0));
                                        ui.add_space(8.0);
                                        // 路径
                                        let dp = truncate_path(detail_path, 55);
                                        ui.colored_label(
                                            egui::Color32::from_gray(90),
                                            egui::RichText::new(&dp).size(10.0),
                                        );
                                    });
                                });
                            }
                        }
                    }

                    ui.add_space(1.0);
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
            if tx_progress.send(ScanMessage::Progress(progress.min(0.995))).is_err() {
                break;
            }
        }
    });

    // 实际扫描线程
    std::thread::spawn(move || {
        // 获取 Tab 名称用于缓存
        // 磁盘分析器在非主目录浏览时不使用缓存（路径不同，结果不同）
        let tab_name = match tab {
            Tab::DevCache => Some("dev_cache"),
            Tab::LargeFiles => {
                // 仅在主目录时缓存
                if disk_path.as_ref().map(|p| *p == scanner::home_dir()).unwrap_or(true) {
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
                let _ = tx.send(ScanMessage::Done(items, cached.scan_time_ms, tab_idx as u64));
                return;
            }
        }

        // 缓存未命中：执行全量扫描
        // 用 catch_unwind 兜底，防止扫描 panic 后 UI 卡死
        let result = std::panic::catch_unwind(|| {
            match tab {
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
                    { scanner::windows_apps::WindowsAppCacheScanner::new().scan() }
                    #[cfg(not(target_os = "windows"))]
                    { scanner::ScanResult { items: Vec::new(), total_size: 0, scan_time_ms: 0 } }
                }
                #[cfg(not(target_os = "macos"))]
                Tab::AppData => {
                    #[cfg(target_os = "windows")]
                    { scanner::windows_apps::WindowsAppDataScanner::new().scan() }
                    #[cfg(not(target_os = "windows"))]
                    { scanner::ScanResult { items: Vec::new(), total_size: 0, scan_time_ms: 0 } }
                }
                #[cfg(not(target_os = "macos"))]
                Tab::AppUninstall => {
                    #[cfg(target_os = "windows")]
                    { scanner::windows_apps::WindowsUninstallScanner::new().scan() }
                    #[cfg(not(target_os = "windows"))]
                    { scanner::ScanResult { items: Vec::new(), total_size: 0, scan_time_ms: 0 } }
                }
                #[cfg(not(target_os = "macos"))]
                Tab::SystemOptimize | Tab::Apfs => {
                    scanner::ScanResult { items: Vec::new(), total_size: 0, scan_time_ms: 0 }
                }
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
                let _ = tx.send(ScanMessage::Done(items, scan_result.scan_time_ms, tab_idx as u64));
            }
            Err(_) => {
                // 扫描 panic，发送空结果让 UI 恢复正常
                let _ = tx.send(ScanMessage::Done(Vec::new(), 0, tab_idx as u64));
            }
        }
    });
}

/// 将扫描结果按大小降序分批发送，实现增量显示
///
/// 排序后按每批最多 10 项分割，每批之间 sleep 50ms，
/// 让 UI 有机会渲染已发现的项。发送完毕后调用方再发送 Done
/// （Done 携带完整列表，会覆盖累积的部分项，保证最终结果一致）。
fn send_items_in_batches(
    tx: &mpsc::Sender<ScanMessage>,
    items: &[ScanItem],
    tab_idx: u64,
) {
    // 按大小降序排序，让用户先看到最大的项
    let mut sorted: Vec<ScanItem> = items.to_vec();
    sorted.sort_by(|a, b| b.size_bytes.cmp(&a.size_bytes));

    const BATCH_SIZE: usize = 10;
    for chunk in sorted.chunks(BATCH_SIZE) {
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
    let output = std::process::Command::new("/sbin/mount")
        .output();
    match output {
        Ok(o) if o.status.success() => {
            let stdout = String::from_utf8_lossy(&o.stdout);
            stdout.lines()
                .filter_map(|line| {
                    let parts: Vec<&str> = line.split_whitespace().collect();
                    if parts.len() >= 3 { Some(parts[2].to_string()) } else { None }
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
        if chars[i..i+8].iter().all(|c| c.is_ascii_hexdigit())
            && chars[i+8] == '-'
            && chars[i+9..i+13].iter().all(|c| c.is_ascii_hexdigit())
            && chars[i+13] == '-'
            && chars[i+14..i+18].iter().all(|c| c.is_ascii_hexdigit())
            && chars[i+18] == '-'
            && chars[i+19..i+23].iter().all(|c| c.is_ascii_hexdigit())
            && chars[i+23] == '-'
            && chars[i+24..i+36].iter().all(|c| c.is_ascii_hexdigit())
        {
            let uuid: String = chars[i..i+36].iter().collect();
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
        return Err(App::t_lang(lang_en, "log_skip_running").replace("[{}]", "")
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
            App::tf_lang(lang_en, "log_sim_failed_suffix", &[&fail_msgs.len().to_string()])
        } else {
            String::new()
        };
        let msg = App::tf_lang(lang_en, "log_sim_deleted", &[&success_count.to_string(), &suffix]);
        Ok(msg)
    } else {
        Err(format!("{}: {}", App::t_lang(lang_en, "log_unknown"), fail_msgs.join("; ")))
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
fn start_delete(to_delete: Vec<(String, String, Vec<String>, bool)>, lang_en: bool, delete_rx: &mut Option<mpsc::Receiver<DeleteMessage>>) {
    let (tx, rx) = mpsc::channel();
    *delete_rx = Some(rx);

    std::thread::spawn(move || {
        let failed_items: std::sync::Mutex<Vec<(String, String)>> = std::sync::Mutex::new(Vec::new());

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

                        // Windows 应用卸载特殊处理
                        #[cfg(target_os = "windows")]
                        if path.starts_with("uwp:") || path.starts_with("uninstall:") {
                            let (success, msg) = scanner::windows_apps::uninstall_app(&path);
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
                            let _ = tx.send(DeleteMessage::Log(
                                format!("✗ {}", App::tf_lang(lang_en, "log_path_not_exist", &[&path])), path.clone(), category.clone(), false));
                            safety::log_deletion(&path, &category, false, Some(&App::t_lang(lang_en, "log_path_not_exist").replace("{}", "").trim().to_string()));
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

        let mut failed_items: Vec<(String, String)> = failed_items.into_inner().unwrap();

        // 普通删除完成后，若还有失败项，通知 GUI 弹出 egui 内置密码输入框
        if !failed_items.is_empty() {
            let _ = tx.send(DeleteMessage::Info(
                format!("🔐 {}", App::tf_lang(lang_en, "log_need_sudo", &[&failed_items.len().to_string()])),
            ));
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
        let _ = tx.send(DeleteMessage::Info(
            format!("🔐 {}", App::t_lang(lang_en, "log_sudo_phase")),
        ));

        let sudo_debug_log = std::env::temp_dir().join("maclean_sudo_debug.log");
        let mut debug_entries: Vec<String> = Vec::new();
        debug_entries.push(format!("[sudo phase] started, {} items", failed_items.len()));
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
                let _ = std::process::Command::new("/bin/chmod").arg("+x").arg(&xcrun_script).output();

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
                            let _ = tx.send(DeleteMessage::Info(format!("🔒 {}", App::t_lang(lang_en, "log_password_wrong"))));
                            let _ = tx.send(DeleteMessage::NeedPassword(failed_items.clone()));
                            let _ = std::fs::write(&sudo_debug_log, debug_entries.join("\n"));
                            let _ = std::fs::remove_file(&xcrun_script);
                            return;
                        }

                        // 检查 Volumes 目录是否已清空
                        let p = std::path::Path::new(path);
                        if !p.exists() || std::fs::read_dir(p).map(|mut d| d.next().is_none()).unwrap_or(true) {
                            xcrun_success = true;
                            let _ = tx.send(DeleteMessage::Log(
                                format!("✓ {}", App::tf_lang(lang_en, "log_touchid_runtime_deleted_path", &[path])),
                                path.clone(), category.clone(), true));
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
            script_content.push_str(&format!(
                "process_one {} '{}' &\n",
                i, escaped
            ));
        }
        script_content.push_str("\nwait\n");
        script_content.push_str("for f in \"$workdir\"/*.out; do [ -f \"$f\" ] && /bin/cat \"$f\"; done\n");
        script_content.push_str("exit 0\n");
        let _ = std::fs::write(&tmp_script, &script_content);
        let _ = std::process::Command::new("/bin/chmod").arg("+x").arg(&tmp_script).output();

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
        let mut rm_stderr: std::collections::HashMap<String, String> = std::collections::HashMap::new();
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
                        rm_stderr.entry(current_path.clone()).or_default().push_str(line);
                        rm_stderr.entry(current_path.clone()).or_default().push('\n');
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
                    let _ = tx.send(DeleteMessage::Info(
                        format!("🔒 {}", App::t_lang(lang_en, "log_password_wrong")),
                    ));
                    // 密码错误时 sudo 不会执行任何删除，全部项都需要重试
                    let _ = tx.send(DeleteMessage::NeedPassword(failed_items));
                    let _ = std::fs::remove_file(&tmp_script);
                    return;
                }

                for (path, category) in &failed_items {
                    let p = std::path::Path::new(path.as_str());
                    if !p.exists() && p.symlink_metadata().is_err() {
                        let _ = tx.send(DeleteMessage::Log(
                            format!("✓ {}", App::tf_lang(lang_en, "log_deleted_sudo", &[category, path])), path.clone(), category.clone(), true));
                        safety::log_deletion(path, category, true, None);
                        continue;
                    }

                    // 仍在：判断是 SIP 保护还是普通权限/占用问题
                    let err_text = rm_stderr.get(path).map(|s| s.as_str()).unwrap_or(&stderr_all);
                    let is_sip = err_text.contains("Operation not permitted")
                        || std::process::Command::new("/usr/bin/xattr")
                            .arg(path)
                            .output()
                            .map(|o| String::from_utf8_lossy(&o.stdout).contains("com.apple.provenance"))
                            .unwrap_or(false)
                        || path.starts_with("/Library/Developer/CoreSimulator/Caches");

                    if is_sip {
                        let _ = tx.send(DeleteMessage::Log(
                            format!("🔒 {}", App::tf_lang(lang_en, "log_sip_protected", &[path])), path.clone(), category.clone(), false));
                        safety::log_deletion(path, category, false, Some(App::t_lang(lang_en, "log_sip_reason")));
                    } else if user_cancelled {
                        let _ = tx.send(DeleteMessage::Log(
                            format!("✗ {}", App::tf_lang(lang_en, "log_cancelled_auth", &[path])), path.clone(), category.clone(), false));
                        safety::log_deletion(path, category, false, Some(App::t_lang(lang_en, "log_cancel_reason")));
                    } else if sudo_failed {
                        let detail = if stderr_all.is_empty() {
                            App::tf_lang(lang_en, "log_exit_code", &[&output.status.code().unwrap_or(-1).to_string()])
                        } else {
                            stderr_all.trim().to_string()
                        };
                        let _ = tx.send(DeleteMessage::Log(
                            format!("✗ {}", App::tf_lang(lang_en, "log_delete_failed", &[path, &detail])), path.clone(), category.clone(), false));
                        safety::log_deletion(path, category, false, Some(&detail));
                    } else {
                        let detail = if err_text.is_empty() {
                            App::t_lang(lang_en, "log_still_exists").to_string()
                        } else {
                            err_text.trim().to_string()
                        };
                        let _ = tx.send(DeleteMessage::Log(
                            format!("✗ {}", App::tf_lang(lang_en, "log_delete_failed", &[path, &detail])), path.clone(), category.clone(), false));
                        safety::log_deletion(path, category, false, Some(&detail));
                    }
                }
            }
            Err(e) => {
                for (path, category) in &failed_items {
                    let _ = tx.send(DeleteMessage::Log(
                        format!("✗ {}", App::tf_lang(lang_en, "log_cannot_start_sudo", &[path, &e.to_string()])), path.clone(), category.clone(), false));
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
                        egui::Color32::from_rgb(100, 150, 255),
                        egui::RichText::new("🔐").size(28.0),
                    );
                    ui.label(egui::RichText::new(app.t("permission_headline")).size(18.0).strong());
                });

                ui.add_space(8.0);
                ui.separator();
                ui.add_space(8.0);

                // 说明
                ui.colored_label(
                    egui::Color32::from_gray(200),
                    egui::RichText::new(app.t("permission_desc")).size(13.0),
                );
                ui.add_space(3.0);
                ui.colored_label(
                    egui::Color32::from_gray(170),
                    egui::RichText::new(app.t("permission_sub_desc")).size(12.0),
                );

                ui.add_space(10.0);

                // 步骤
                ui.colored_label(
                    egui::Color32::from_rgb(100, 200, 100),
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
                            egui::Color32::from_rgb(100, 200, 100),
                            egui::RichText::new(format!("{}. ", i + 1)).size(13.0),
                        );
                        ui.colored_label(
                            egui::Color32::from_gray(200),
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
                        .fill(egui::Color32::from_rgb(52, 199, 89))
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
                    egui::Color32::from_gray(120),
                    egui::RichText::new(app.t("permission_hint")).size(11.0),
                );
            });
        });
}

/// 删除确认弹窗
fn show_confirm_window(ctx: &egui::Context, app: &mut App, delete_rx: &mut Option<mpsc::Receiver<DeleteMessage>>) {
    let count = app.selected_count();
    let size = app.selected_total_size();
    let has_full_disk_access = App::check_full_disk_access();

    egui::Window::new(app.t("confirm_delete"))
        .collapsible(false)
        .resizable(true)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ctx, |ui| {
            ui.set_min_width(480.0);
            ui.set_min_height(200.0);
            ui.add_space(10.0);
            ui.vertical(|ui| {
                ui.label(egui::RichText::new(format!("{} {} {}", app.t("about_to_delete"), count, app.t("items"))).size(16.0));
                ui.label(egui::RichText::new(format!("{}: {}", app.t("total"), format_size(size))).size(20.0).color(egui::Color32::from_rgb(255, 159, 10)));
                ui.add_space(8.0);
                ui.colored_label(egui::Color32::RED, format!("⚠️ {}", app.t("irreversible")));

                // 如果没有完全磁盘访问权限，显示警告
                if !has_full_disk_access {
                    ui.add_space(8.0);
                    egui::Frame::group(ui.style())
                        .fill(egui::Color32::from_rgb(50, 40, 20))
                        .stroke(egui::Stroke::new(1.0, egui::Color32::from_rgb(150, 120, 40)))
                        .inner_margin(egui::Margin::same(8.0))
                        .show(ui, |ui| {
                            ui.colored_label(
                                egui::Color32::from_rgb(255, 200, 100),
                                egui::RichText::new(format!("⚠️ {}", app.t("confirm_fda_warning"))).size(13.0).strong(),
                            );
                            ui.add_space(2.0);
                            ui.colored_label(
                                egui::Color32::from_gray(180),
                                egui::RichText::new(app.t("confirm_fda_sub_warning")).size(12.0),
                            );
                            ui.add_space(3.0);
                            if ui.button(egui::RichText::new(format!("⚙️ {}", app.t("confirm_grant"))).size(12.0)).clicked() {
                                let _ = std::process::Command::new("open")
                                    .arg("x-apple.systempreferences:com.apple.preference.security?Privacy_AllFiles")
                                    .spawn();
                            }
                        });
                }

                // 预览按钮
                ui.add_space(8.0);
                if ui.button(egui::RichText::new(format!("🔍 {}", app.t("confirm_preview"))).size(13.0)).clicked() {
                    app.show_preview = !app.show_preview;
                }

                // 预览面板
                if app.show_preview {
                    ui.add_space(5.0);
                    egui::Frame::group(ui.style())
                        .inner_margin(egui::Margin::same(8.0))
                        .show(ui, |ui| {
                            ui.set_max_height(250.0);
                            egui::ScrollArea::vertical().show(ui, |ui| {
                                let idx = app.tab_index();
                                let items: Vec<_> = app.results[idx].iter()
                                    .filter(|item| item.selected && item.deletable)
                                    .collect();
                                for item in &items {
                                    ui.horizontal(|ui| {
                                        ui.label(recommend_badge_text(&item.recommend, app.lang_en));
                                        ui.label(format_size(item.size_bytes));
                                        let cat_en = i18n::translate_category(&item.category, app.lang_en);
                                        ui.label(&cat_en);
                                        ui.label(egui::RichText::new(&item.path).size(11.0).color(egui::Color32::from_gray(160)));
                                    });
                                }
                                ui.add_space(3.0);
                                let summary_text = App::tf_lang(app.lang_en, "confirm_preview_summary", &[&items.len().to_string()]);
                                ui.colored_label(
                                    egui::Color32::from_gray(140),
                                    egui::RichText::new(summary_text).size(11.0),
                                );
                            });
                        });
                }

                ui.add_space(15.0);

                ui.horizontal(|ui| {
                    if ui.button(egui::RichText::new(format!("✓ {}", app.t("confirm_delete"))).color(egui::Color32::from_rgb(52, 199, 89))).clicked() {
                        app.show_preview = false;
                        let to_delete = app.confirm_delete();
                        start_delete(to_delete, app.lang_en, delete_rx);
                    }
                    if ui.button(egui::RichText::new(format!("✗ {}", app.t("cancel"))).color(egui::Color32::RED)).clicked() {
                        app.show_preview = false;
                        app.cancel_delete();
                    }
                });
            });
        });
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
                    ui.colored_label(
                        egui::Color32::from_rgb(255, 159, 10),
                        egui::RichText::new(emoji).size(28.0),
                    );
                    ui.label(
                        egui::RichText::new(headline)
                            .size(18.0)
                            .strong(),
                    );
                });

                ui.add_space(8.0);
                ui.colored_label(
                    egui::Color32::from_gray(200),
                    egui::RichText::new(description).size(13.0),
                );
                if !is_setup_mode {
                    ui.add_space(2.0);
                    ui.colored_label(
                        egui::Color32::from_gray(150),
                        egui::RichText::new(app.t("sudo_password_note"))
                            .size(12.0),
                    );
                }

                if let Some(ref err) = app.sudo_error {
                    ui.add_space(8.0);
                    ui.colored_label(
                        egui::Color32::RED,
                        egui::RichText::new(err).size(13.0).strong(),
                    );
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
                                format!("✗ {}: {}", App::t_lang(lang_en, "log_cancel_reason"), path),
                                path.clone(),
                                category.clone(),
                                false,
                            );
                            safety::log_deletion(&path, &category, false, Some(App::t_lang(lang_en, "log_cancel_reason")));
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
    delete_rx: &mut Option<mpsc::Receiver<DeleteMessage>>,
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
                    egui::Color32::from_gray(200),
                    egui::RichText::new(
                        App::tf_lang(
                            app.lang_en,
                            "touchid_setup_desc",
                            &[&app.sudo_failed_items.len().to_string()],
                        )
                    )
                    .size(13.0),
                );
                ui.add_space(4.0);
                ui.colored_label(
                    egui::Color32::from_gray(170),
                    egui::RichText::new(app.t("touchid_setup_detail"))
                    .size(12.0),
                );

                if let Some(ref err) = app.touch_id_error {
                    ui.add_space(8.0);
                    ui.colored_label(
                        egui::Color32::RED,
                        egui::RichText::new(err).size(13.0).strong(),
                    );
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
                                format!("✗ {}: {}", App::t_lang(lang_en, "log_cancel_reason"), path),
                                path.clone(),
                                category.clone(),
                                false,
                            );
                            safety::log_deletion(&path, &category, false, Some(App::t_lang(lang_en, "log_cancel_reason")));
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
                    egui::Color32::from_gray(170),
                    egui::RichText::new(app.t("touchid_wait_desc"))
                    .size(13.0),
                );

                // 显示已等待时间
                if let Some(start) = app.touch_id_wait_start {
                    let elapsed = start.elapsed().as_secs();
                    ui.add_space(6.0);
                    ui.colored_label(
                        egui::Color32::from_gray(120),
                        egui::RichText::new(
                            App::tf_lang(app.lang_en, "touchid_wait_time", &[&elapsed.to_string()])
                        )
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
                    egui::Color32::from_gray(170),
                    egui::RichText::new(
                        App::tf_lang(app.lang_en, "touchid_verify_desc", &[&app.delete_total.to_string()])
                    )
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
                    egui::Color32::from_gray(120),
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
                    egui::Frame::group(ui.style())
                        .fill(egui::Color32::from_gray(25))
                        .show(ui, |ui| {
                            ui.set_min_width(360.0);
                            ui.label(egui::RichText::new(app.t("touchid_verify_log")).size(11.0).strong());
                            ui.add_space(4.0);
                            for log in recent_logs {
                                ui.colored_label(
                                    egui::Color32::from_gray(170),
                                    egui::RichText::new(log).size(10.0),
                                );
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
        let _ = tx.send(DeleteMessage::Info(
            format!("👆 {}", App::t_lang(lang_en, "log_touchid_verifying")),
        ));

        let sudo_debug_log = std::env::temp_dir().join("maclean_sudo_touchid.log");
        let mut debug_entries: Vec<String> = Vec::new();
        debug_entries.push(format!("[touchid sudo] started, {} items", failed_items.len()));

        // 预处理：模拟器镜像通过 xcrun simctl runtime delete
        let mut remaining_items: Vec<(String, String)> = Vec::new();
        for (path, category) in &failed_items {
            if category == "模拟器镜像" || category == "模拟器Cryptex" {
                let _ = tx.send(DeleteMessage::Info(
                    format!("🔄 {}", App::tf_lang(lang_en, "log_touchid_prepare_xcrun", &[category])),
                ));

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
                let _ = std::process::Command::new("/bin/chmod").arg("+x").arg(&xcrun_script).output();

                // 先清除 sudo 票据，确保能触发 Touch ID
                let _ = std::process::Command::new("/usr/bin/sudo").arg("-k").output();

                let _ = tx.send(DeleteMessage::Info(
                    format!("⏳ {}", App::t_lang(lang_en, "log_wait_touchid")),
                ));

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

                let _ = tx.send(DeleteMessage::Info(
                    format!("✅ {}", App::tf_lang(lang_en, "log_xcrun_done", &[&xcrun_stdout.trim().replace('\n', " ")])),
                ));

                match &xcrun_result {
                    Ok(status) => {
                        if !status.success() {
                            // Touch ID 可能被取消
                            if xcrun_stdout.contains("canceled") || xcrun_stdout.contains("cancelled") {
                                let _ = tx.send(DeleteMessage::Info(format!("🔒 {}", App::t_lang(lang_en, "log_touchid_cancel"))));
                                for (p, c) in &failed_items {
                                    let _ = tx.send(DeleteMessage::Log(
                                        format!("✗ {}: {}", App::t_lang(lang_en, "log_touchid_cancel"), p), p.clone(), c.clone(), false));
                                    safety::log_deletion(p, c, false, Some(App::t_lang(lang_en, "log_touchid_cancel")));
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
                                format!("✓ {}", App::tf_lang(lang_en, "log_touchid_xcrun_deleted", &[&deleted_count.to_string()])),
                                path.clone(), category.clone(), true));
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
            let _ = tx.send(DeleteMessage::Info(format!("✅ {}", App::t_lang(lang_en, "log_no_sudo_needed"))));
            let _ = std::fs::write(&sudo_debug_log, debug_entries.join("\n"));
            let _ = tx.send(DeleteMessage::Done);
            return;
        }

        let _ = tx.send(DeleteMessage::Info(
            format!("🔄 {}", App::tf_lang(lang_en, "log_sudo_execute", &[&failed_items.len().to_string()])),
        ));

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
            script_content.push_str(&format!(
                "process_one {} '{}' &\n",
                i, escaped
            ));
        }
        script_content.push_str("\nwait\n");
        script_content.push_str("for f in \"$workdir\"/*.out; do [ -f \"$f\" ] && /bin/cat \"$f\"; done\n");
        script_content.push_str("exit 0\n");
        let _ = std::fs::write(&tmp_script, &script_content);
        let _ = std::process::Command::new("/bin/chmod").arg("+x").arg(&tmp_script).output();

        debug_entries.push(format!("delete script: {}", tmp_script.display()));

        let sudo_log = std::env::temp_dir().join("maclean_sudo_touchid_script.log");
        let sudo_log_file = std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&sudo_log)
            .ok();

        let _ = tx.send(DeleteMessage::Info(
            format!("⏳ {}", App::t_lang(lang_en, "log_wait_touchid")),
        ));

        // 不用 -S，sudo 自动触发 Touch ID；stdout 重定向到文件避免 pipe 死锁
        let mut sudo_cmd = std::process::Command::new("/usr/bin/sudo");
        sudo_cmd.arg("/bin/bash").arg(&tmp_script);
        if let Some(file) = sudo_log_file {
            sudo_cmd.stdout(file);
        }
        let sudo_result = sudo_cmd.status();

        let _ = tx.send(DeleteMessage::Info(
            format!("✅ {}", App::t_lang(lang_en, "log_sudo_done2")),
        ));

        // 解析输出
        let sudo_stdout = std::fs::read_to_string(&sudo_log).unwrap_or_default();
        debug_entries.push(format!("sudo touchid exit code: {:?}", sudo_result.as_ref().ok().and_then(|s| s.code())));
        debug_entries.push(format!("sudo touchid stdout:\n{}", sudo_stdout));

        let mut rm_stderr: std::collections::HashMap<String, String> = std::collections::HashMap::new();
        let mut current_path = String::new();
        for line in sudo_stdout.lines() {
            if let Some(p) = line.strip_prefix(">MACLEAN_BEGIN:") {
                current_path = p.to_string();
            } else if let Some(_rest) = line.strip_prefix(">MACLEAN_EXIT:") {
                current_path.clear();
            } else if !line.is_empty() && !current_path.is_empty() {
                rm_stderr.entry(current_path.clone()).or_default().push_str(line);
                rm_stderr.entry(current_path.clone()).or_default().push('\n');
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
                    let _ = tx.send(DeleteMessage::Info(format!("🔒 {}", App::t_lang(lang_en, "log_touchid_cancel"))));
                    for (path, category) in &failed_items {
                        let _ = tx.send(DeleteMessage::Log(
                            format!("✗ {}: {}", App::t_lang(lang_en, "log_touchid_cancel"), path), path.clone(), category.clone(), false));
                        safety::log_deletion(path, category, false, Some(App::t_lang(lang_en, "log_touchid_cancel")));
                    }
                    let _ = std::fs::remove_file(&tmp_script);
                    let _ = tx.send(DeleteMessage::Done);
                    return;
                }

                for (path, category) in &failed_items {
                    let p = std::path::Path::new(path.as_str());
                    if !p.exists() && p.symlink_metadata().is_err() {
                        let _ = tx.send(DeleteMessage::Log(
                            format!("✓ {}", App::tf_lang(lang_en, "log_deleted_touchid", &[category, path])),
                            path.clone(), category.clone(), true));
                        safety::log_deletion(path, category, true, None);
                    } else {
                        let err_text = rm_stderr.get(path).map(|s| s.as_str()).unwrap_or("");
                        let is_sip = err_text.contains("Operation not permitted")
                            || path.starts_with("/Library/Developer/CoreSimulator/Caches");

                        if is_sip {
                            let _ = tx.send(DeleteMessage::Log(
                                format!("🔒 {}", App::tf_lang(lang_en, "log_sip_protected", &[path])),
                                path.clone(), category.clone(), false));
                            safety::log_deletion(path, category, false, Some(App::t_lang(lang_en, "log_sip_reason")));
                        } else {
                            let detail = if err_text.is_empty() {
                                App::t_lang(lang_en, "log_still_exists").to_string()
                            } else {
                                err_text.trim().to_string()
                            };
                            let _ = tx.send(DeleteMessage::Log(
                                format!("✗ {}", App::tf_lang(lang_en, "log_delete_failed", &[path, &detail])),
                                path.clone(), category.clone(), false));
                            safety::log_deletion(path, category, false, Some(&detail));
                        }
                    }
                }
            }
            Err(e) => {
                for (path, category) in &failed_items {
                    let _ = tx.send(DeleteMessage::Log(
                        format!("✗ {}", App::tf_lang(lang_en, "log_cannot_start_sudo", &[path, &e.to_string()])),
                        path.clone(), category.clone(), false));
                    safety::log_deletion(path, category, false, Some(&e.to_string()));
                }
            }
        }

        let _ = std::fs::remove_file(&tmp_script);
        let _ = tx.send(DeleteMessage::Done);
    });
}

/// 删除中弹窗（带进度条）
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

    egui::Window::new(app.t("cleaning"))
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ctx, |ui| {
            ui.set_min_width(520.0);
            ui.set_min_height(220.0);
            ui.add_space(10.0);

            // 标题
            let title = if in_sudo_phase {
                app.t("deleting_sudo_phase")
            } else {
                app.t("cleaning_in_progress")
            };
            ui.label(egui::RichText::new(format!("⏳ {}...", title)).size(15.0).color(egui::Color32::from_rgb(0, 200, 255)));
            ui.add_space(10.0);

            // 进度条
            let progress_text = format!(
                "{}/{} ({}%)",
                app.delete_done,
                app.delete_total,
                (progress * 100.0) as u32
            );
            ui.add(
                egui::ProgressBar::new(progress)
                    .desired_width(480.0)
                    .fill(if in_sudo_phase { egui::Color32::from_rgb(255, 159, 10) } else { egui::Color32::from_rgb(52, 199, 89) })
                    .text(progress_text)
            );
            ui.add_space(8.0);

            // 最新日志
            ui.label(egui::RichText::new(app.t("cleaning_log")).size(12.0).color(egui::Color32::GRAY));
            egui::ScrollArea::vertical().max_height(120.0).show(ui, |ui| {
                for log in app.logs.iter().rev().take(15) {
                    let color = if log.starts_with('✓') || log.starts_with('✅') {
                        egui::Color32::from_rgb(52, 199, 89)
                    } else if log.starts_with('✗') {
                        egui::Color32::RED
                    } else if log.starts_with('⛔') {
                        egui::Color32::from_rgb(200, 100, 100)
                    } else if log.starts_with('⚠') {
                        egui::Color32::from_rgb(255, 159, 10)
                    } else {
                        egui::Color32::from_rgb(0, 200, 255)
                    };
                    ui.colored_label(color, egui::RichText::new(log).size(12.0));
                }
            });
        });

    // 删除中持续刷新 UI
    ctx.request_repaint_after(std::time::Duration::from_millis(50));
}

/// 删除完成汇总弹窗
fn show_summary_window(ctx: &egui::Context, app: &mut App, ok: usize, fail: usize, _skip: usize, _delete_rx: &mut Option<mpsc::Receiver<DeleteMessage>>) {
    let has_failures = !app.failed_paths.is_empty();

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
                    ui.colored_label(egui::Color32::from_rgb(52, 199, 89), "✅");
                    ui.label(egui::RichText::new(App::tf_lang(app.lang_en, "summary_success", &[&ok.to_string()])).size(15.0).color(egui::Color32::from_rgb(52, 199, 89)));
                });

                if fail > 0 {
                    ui.add_space(5.0);
                    ui.horizontal(|ui| {
                        ui.colored_label(egui::Color32::RED, "❌");
                        ui.label(egui::RichText::new(App::tf_lang(app.lang_en, "summary_fail", &[&fail.to_string()])).size(15.0).color(egui::Color32::RED));
                    });
                    ui.add_space(3.0);
                    ui.colored_label(
                        egui::Color32::from_gray(150),
                        egui::RichText::new(app.t("summary_fail_hint")).size(12.0),
                    );

                    // 引导用户处理失败项
                    ui.add_space(8.0);
                    egui::Frame::group(ui.style())
                        .fill(egui::Color32::from_rgb(30, 35, 50))
                        .stroke(egui::Stroke::new(1.0, egui::Color32::from_rgb(60, 80, 120)))
                        .inner_margin(egui::Margin::same(8.0))
                        .show(ui, |ui| {
                            ui.colored_label(
                                egui::Color32::from_rgb(100, 150, 255),
                                egui::RichText::new(format!("💡 {}", app.t("summary_solution_title"))).size(13.0).strong(),
                            );
                            ui.add_space(3.0);
                            ui.colored_label(
                                egui::Color32::from_gray(180),
                                egui::RichText::new(app.t("summary_sip_tip")).size(11.0),
                            );
                            ui.add_space(3.0);
                            ui.colored_label(
                                egui::Color32::from_gray(180),
                                egui::RichText::new(app.t("summary_perm_tip")).size(11.0),
                            );
                            ui.add_space(5.0);
                            ui.colored_label(
                                egui::Color32::from_rgb(200, 200, 200),
                                egui::RichText::new(app.t("summary_solution_1")).size(11.0),
                            );
                            ui.colored_label(
                                egui::Color32::from_rgb(200, 200, 200),
                                egui::RichText::new(app.t("summary_solution_2")).size(11.0),
                            );
                            ui.colored_label(
                                egui::Color32::from_rgb(200, 200, 200),
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
                        ui.label(egui::RichText::new(app.t("summary_fail_list")).size(12.0).color(egui::Color32::from_gray(170)));
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
                                        egui::Color32::from_rgb(200, 100, 100),
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
                                                .text_color(egui::Color32::from_gray(160))
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
        Tab::DevCache => app.t("tab_dev_cache"),
        Tab::LargeFiles => app.t("tab_large_files"),
        Tab::AppCache => app.t("tab_app_cache"),
        Tab::AppData => app.t("tab_app_data"),
        Tab::AppUninstall => app.t("tab_app_uninstall"),
        Tab::SystemOptimize => app.t("tab_system_optimize"),
        Tab::Apfs => app.t("tab_apfs"),
    }
}

/// 类别颜色
fn category_color(category: &str) -> egui::Color32 {
    match category {
        c if c.contains("Rust") => egui::Color32::from_rgb(222, 120, 50),
        c if c.contains("Xcode") => egui::Color32::from_rgb(100, 150, 255),
        c if c.contains("模拟器") => egui::Color32::from_rgb(150, 100, 255),
        c if c.contains("Node") => egui::Color32::from_rgb(100, 200, 100),
        c if c.contains("pnpm") | c.contains("npm") => egui::Color32::from_rgb(180, 120, 80),
        c if c.contains("Go") => egui::Color32::from_rgb(100, 200, 200),
        c if c.contains("Homebrew") => egui::Color32::from_rgb(255, 150, 50),
        c if c.contains("pip") => egui::Color32::from_rgb(100, 200, 255),
        c if c.contains("IDE") => egui::Color32::from_rgb(200, 150, 255),
        c if c.contains("APFS") => egui::Color32::from_rgb(255, 100, 150),
        c if c.contains("大目录") | c.contains("大文件") => egui::Color32::from_rgb(255, 200, 100),
        _ => egui::Color32::from_gray(180),
    }
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
fn render_disk_analyzer(ui: &mut egui::Ui, app: &mut App, scan_rx: &mut Option<mpsc::Receiver<ScanMessage>>) {
    let tab_idx = app.tab_index();
    let items = app.results[tab_idx].clone();
    let is_scanning = matches!(app.scan_states[tab_idx], ScanState::Scanning);
    let current_path = app.disk_analyzer_current_path();
    let has_history = !app.disk_analyzer_history.is_empty();

    // ====== 面包屑导航栏 ======
    ui.horizontal(|ui| {
        // 返回上一级按钮
        let back_enabled = has_history && !is_scanning;
        if ui.add_enabled(back_enabled, egui::Button::new(format!("⬆ {}", app.t("back")))).clicked() {
            app.disk_analyzer_back();
            // 返回后重新扫描上一级目录
            start_scan(app, scan_rx);
            return;
        }

        ui.separator();

        // 显示当前路径（用 ~ 替换主目录）
        let home = scanner::home_dir();
        let display_path = if current_path.starts_with(&home) {
            let suffix = current_path.strip_prefix(&home).unwrap_or(std::path::Path::new(""));
            format!("~ {}", suffix.display())
        } else {
            current_path.display().to_string()
        };

        ui.label(egui::RichText::new(format!("📁 {}", display_path))
            .size(14.0)
            .color(egui::Color32::from_rgb(100, 200, 255)));

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
        ui.add_space(40.0);
        ui.vertical_centered(|ui| {
            ui.add(egui::Spinner::new().size(40.0));
            ui.add_space(10.0);
            ui.label(
                egui::RichText::new(app.tf("analyzing", &[&display_path_short(&current_path, app.lang_en)]))
                    .size(16.0)
                    .color(egui::Color32::from_rgb(0, 200, 255)),
            );
            ui.add_space(15.0);
            let pct = (app.scan_progress * 100.0) as u32;
            ui.add(egui::ProgressBar::new(app.scan_progress)
                .desired_width(500.0)
                .fill(egui::Color32::from_rgb(0, 200, 255))
                .text(format!("{}%", pct)));
        });
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(100));
        return;
    }

    if items.is_empty() {
        ui.vertical_centered(|ui| {
            ui.add_space(80.0);
            ui.label(egui::RichText::new(app.t("no_large_files")).size(16.0).color(egui::Color32::GRAY));
            ui.add_space(10.0);
            if ui.button(egui::RichText::new(format!("🔍 {}", app.t("rescan"))).size(16.0)).clicked() {
                start_scan(app, scan_rx);
            }
        });
        return;
    }

    // ====== 汇总信息 ======
    let total_size: u64 = items.iter().map(|i| i.size_bytes).sum();
    let selected_cnt: usize = items.iter().filter(|i| i.selected && i.deletable).count();
    let selected_sz: u64 = items.iter().filter(|i| i.selected && i.deletable).map(|i| i.size_bytes).sum();

    ui.horizontal(|ui| {
        ui.colored_label(
            egui::Color32::from_rgb(100, 200, 255),
            app.tf("items_total_size", &[&items.len().to_string(), &format_size(total_size)]),
        );
        if selected_cnt > 0 {
            ui.separator();
            ui.colored_label(
                egui::Color32::from_rgb(255, 159, 10),
                app.tf("selected_count_size", &[&selected_cnt.to_string(), &format_size(selected_sz)]),
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
        if ui.add_enabled(delete_enabled, egui::Button::new(
            egui::RichText::new(app.tf("delete_selected_count", &[&selected_cnt.to_string()]))
                .color(egui::Color32::from_rgb(255, 69, 58))
        )).clicked() {
            app.prepare_delete();
        }
    });

    ui.add_space(5.0);

    // ====== 目录项列表 ======
    let max_size = items.first().map(|i| i.size_bytes).unwrap_or(1).max(1);

    egui::ScrollArea::vertical()
        .auto_shrink([false, true])
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
                            .sense(egui::Sense::click())
                    );

                    if is_dir && name_btn.clicked() && !is_scanning {
                        // 进入子目录
                        let new_path = std::path::PathBuf::from(&item.path);
                        app.disk_analyzer_enter(new_path);
                        start_scan(app, scan_rx);
                        return;
                    }

                    // 进度条（相对大小可视化）
                    ui.add(egui::ProgressBar::new(pct)
                        .desired_width(200.0)
                        .fill(if pct > 0.5 {
                            egui::Color32::from_rgb(255, 69, 58)
                        } else if pct > 0.2 {
                            egui::Color32::from_rgb(255, 159, 10)
                        } else {
                            egui::Color32::from_rgb(52, 199, 89)
                        }));

                    // 大小
                    ui.label(egui::RichText::new(&size_str).size(13.0).strong());

                    // 百分比
                    let total_pct = if total_size > 0 {
                        item.size_bytes as f32 / total_size as f32 * 100.0
                    } else {
                        0.0
                    };
                    ui.label(egui::RichText::new(format!("{:.1}%", total_pct))
                        .size(11.0)
                        .color(egui::Color32::GRAY));

                    // 目录：显示"进入"提示
                    if is_dir {
                        ui.label(egui::RichText::new("→")
                            .size(16.0)
                            .color(egui::Color32::from_rgb(100, 200, 255)));
                    }
                });

                ui.separator();
            }
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

fn render_optimize_panel(ui: &mut egui::Ui, app: &mut App, scan_rx: &mut Option<mpsc::Receiver<ScanMessage>>) {
    let tab_idx = app.tab_index();
    let items = app.results[tab_idx].clone();
    let is_scanning = matches!(app.scan_states[tab_idx], ScanState::Scanning);

    if is_scanning {
        ui_scanning(ui, app);
        return;
    }

    if items.is_empty() {
        ui.vertical_centered(|ui| {
            ui.add_space(80.0);
            ui.label(egui::RichText::new(app.t("optimize_click_to_scan")).size(16.0).color(egui::Color32::GRAY));
            ui.add_space(10.0);
            let scan_button = format!("🔍 {}", app.t("scan"));
            if ui.button(egui::RichText::new(scan_button).size(16.0)).clicked() {
                start_scan(app, scan_rx);
            }
        });
        return;
    }

    ui.add_space(10.0);
    ui.heading(egui::RichText::new(app.t("tab_system_optimize")).size(18.0));
    ui.label(egui::RichText::new(app.t("optimize_safe_hint")).size(12.0).color(egui::Color32::GRAY));
    ui.add_space(10.0);

    // 优化任务列表
    let mut task_to_run: Option<usize> = None;

    egui::ScrollArea::vertical().show(ui, |ui| {
        for (i, item) in items.iter().enumerate() {
            egui::Frame::group(ui.style())
                .fill(egui::Color32::from_rgb(30, 30, 40))
                .stroke(egui::Stroke::new(1.0, egui::Color32::from_rgb(60, 60, 70)))
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
                                let title = if title.is_empty() { &item.path } else { title };
                                ui.label(egui::RichText::new(title).strong().size(14.0));
                                ui.label(egui::RichText::new(recommend_badge_text(&item.recommend, app.lang_en)).size(11.0));
                            });
                            let desc_en = i18n::translate_description(&item.description, app.lang_en);
                            ui.label(egui::RichText::new(&desc_en).size(12.0).color(egui::Color32::from_rgb(160, 160, 170)));
                        });
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            let run_text = if app.lang_en { "▶ Run" } else { "▶ 执行" };
                            if ui.button(egui::RichText::new(run_text).size(13.0)).clicked() {
                                task_to_run = Some(i);
                            }
                        });
                    });
                });
        }
    });

    // 执行选中的优化任务
    if let Some(task_idx) = task_to_run {
        if let Some(item) = items.get(task_idx) {
            let log = execute_optimize_task(&item.path, app.lang_en);
            app.logs.push(log);
        }
    }

    // 显示优化日志
    if !app.logs.is_empty() {
        ui.add_space(5.0);
        ui.collapsing(app.t("optimize_logs"), |ui| {
            egui::ScrollArea::vertical()
                .max_height(150.0)
                .show(ui, |ui| {
                    for log in &app.logs {
                        ui.label(egui::RichText::new(log).size(11.0).color(egui::Color32::from_rgb(160, 160, 170)));
                    }
                });
        });
    }
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
            let r = std::process::Command::new(lsregister)
                .arg("-gc")
                .output();
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
            let db_path = format!("{}/Library/Preferences/com.apple.LaunchServices.QuarantineEventsV2", home);
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
                Ok(o) if o.status.success() => App::t_lang(lang_en, "opt_memory_success").to_string(),
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
            let r = std::process::Command::new("open")
                .arg(url)
                .output();
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
    ui.add_space(40.0);
    ui.vertical_centered(|ui| {
        ui.add(egui::Spinner::new().size(40.0));
        ui.add_space(10.0);
        ui.label(egui::RichText::new(format!("⏳ {}...", app.t("scanning"))).size(16.0).color(egui::Color32::from_rgb(0, 200, 255)));
        ui.add_space(15.0);
        let pct = (app.scan_progress * 100.0) as u32;
        ui.add(egui::ProgressBar::new(app.scan_progress)
            .desired_width(500.0)
            .fill(egui::Color32::from_rgb(0, 200, 255))
            .text(format!("{}%", pct)));
    });
    ui.ctx().request_repaint_after(std::time::Duration::from_millis(100));
}
