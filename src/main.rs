//! maclean - macOS 磁盘清理 GUI 工具
//!
//! 使用 egui 构建，专注开发者缓存与深度清理。

mod app;
mod safety;
mod scanner;

use std::sync::mpsc;
use std::path::PathBuf;

use eframe::egui;

use app::{App, ConfirmState, ScanState, Tab};
use scanner::{format_size, Recommend, ScanItem, Scanner};

/// 后台扫描消息
enum ScanMessage {
    /// 扫描进度更新
    Progress(f32),
    /// 扫描完成
    Done(Vec<ScanItem>, u64, u64), // (items, scan_time_ms, tab_index)
}

/// 后台删除消息
enum DeleteMessage {
    /// 单项删除结果（日志, 路径, 类别, 是否成功）
    Log(String, String, String, bool),
    /// 进度信息（不计入成功/失败统计）
    Info(String),
    /// 全部删除完成
    Done,
}

/// 初始化崩溃日志文件，返回日志路径
fn init_crash_log() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
    let log_dir = PathBuf::from(&home).join(".maclean/logs");
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
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
    let log_path = PathBuf::from(&home).join(".maclean/logs/scan.log");
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
            .with_title("maclean - macOS 磁盘清理"),
        ..Default::default()
    };

    eframe::run_simple_native("maclean", options, move |ctx, _frame| {
        static mut APP: Option<App> = None;
        static mut SCAN_RX: Option<mpsc::Receiver<ScanMessage>> = None;
        static mut DELETE_RX: Option<mpsc::Receiver<DeleteMessage>> = None;
        static mut NEEDS_INIT: bool = true;

        unsafe {
            if NEEDS_INIT {
                APP = Some(App::new());
                NEEDS_INIT = false;
                setup_fonts(ctx);
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
                        Ok(DeleteMessage::Done) => {
                            if let Some(app) = &mut APP {
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
                render_gui(ctx, app, &mut SCAN_RX, &mut DELETE_RX);
            }
        }
    })
}

/// 加载 macOS 系统中文字体
fn setup_fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();

    let font_paths = [
        "/System/Library/Fonts/PingFang.ttc",
        "/System/Library/Fonts/STHeiti Medium.ttc",
        "/Library/Fonts/Arial Unicode.ttf",
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

/// 获取磁盘信息
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
        Recommend::Safe => "🟢 推荐",
        Recommend::Caution => "🟡 谨慎",
        Recommend::Advanced => "🔴 确认",
    }
}

/// 渲染 GUI 主界面
fn render_gui(ctx: &egui::Context, app: &mut App, scan_rx: &mut Option<mpsc::Receiver<ScanMessage>>, delete_rx: &mut Option<mpsc::Receiver<DeleteMessage>>) {
    // ========== 顶部：标题栏 + 磁盘概览 ==========
    egui::TopBottomPanel::top("menu_bar").show(ctx, |ui| {
        ui.horizontal(|ui| {
            ui.heading("🧹 maclean");
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
                        format!("✓ {} {} {}, {} {}", count, app.t("items_found"), app.t("items"), app.t("total"), format_size(total)),
                    );
                }
            }

            ui.separator();

            // 日志按钮
            if ui.small_button("📋 日志").clicked() {
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

                if button.clicked() && !matches!(app.confirm, ConfirmState::Pending | ConfirmState::Deleting) {
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
        });

        // --- 扫描结果区 ---
        let tab_idx = app.tab_index();
        let items = app.results[tab_idx].clone();
        let is_scanning = matches!(app.scan_states[tab_idx], ScanState::Scanning);

        if is_scanning {
            // 扫描中：显示带百分比的进度长条
            ui.add_space(40.0);
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
                let delete_enabled = selected_cnt > 0 && !matches!(app.confirm, ConfirmState::Pending | ConfirmState::Deleting);
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

            // ====== 可滚动列表 ======
            egui::ScrollArea::vertical().show(ui, |ui| {
                let mut toggled_indices: Vec<usize> = Vec::new();

                for (i, item) in items.iter().enumerate() {
                    let rec_color = if !item.deletable {
                        egui::Color32::from_gray(80)
                    } else {
                        recommend_color(&item.recommend)
                    };
                    let badge = if !item.deletable {
                        "🔒 不可删除"
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

                            ui.add_space(5.0);

                            // 类别 + 描述（用户看得懂的信息）
                            ui.vertical(|ui| {
                                ui.horizontal(|ui| {
                                    let cat_color = if !item.deletable {
                                        egui::Color32::from_gray(80)
                                    } else {
                                        category_color(&item.category)
                                    };
                                    ui.colored_label(cat_color, &item.category);
                                    // 大小
                                    let size_str = if item.size_bytes == 0 {
                                        app.t("unknown").to_string()
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
                                ui.colored_label(
                                    egui::Color32::from_gray(140),
                                    egui::RichText::new(&item.description).size(12.0),
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
                                    ui.colored_label(
                                        egui::Color32::from_rgb(200, 80, 80),
                                        egui::RichText::new(format!("⚠️ {}", item.undeletable_reason)).size(11.0),
                                    );
                                }
                            });
                        });
                    });

                    // 点击行也切换选中
                    if row_resp.response.clicked() && item.deletable {
                        toggled_indices.push(i);
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
            });
        }

        // 权限引导弹窗（首次启动时显示）
        if app.show_permission_guide {
            show_permission_guide_window(ctx, app);
        }

        // 删除确认弹窗
        if matches!(app.confirm, ConfirmState::Pending) {
            show_confirm_window(ctx, app, delete_rx);
        }

        // 删除中弹窗
        if matches!(app.confirm, ConfirmState::Deleting) {
            show_deleting_window(ctx, app);
        }

        // 删除完成汇总弹窗
        if let Some((ok, fail, skip)) = app.delete_summary {
            show_summary_window(ctx, app, ok, fail, skip, delete_rx);
        }
    });
}

/// 启动后台扫描
fn start_scan(app: &mut App, scan_rx: &mut Option<mpsc::Receiver<ScanMessage>>) {
    let tab = app.tab;
    let tab_idx = app.tab_index();
    app.scan_states[tab_idx] = ScanState::Scanning;
    app.scan_progress = 0.0;

    let (tx, rx) = mpsc::channel();
    *scan_rx = Some(rx);

    // 进度估算线程：每 200ms 发送进度更新
    // 扫描通常 2-8 秒完成，用渐近曲线估算进度
    let tx_progress = tx.clone();
    std::thread::spawn(move || {
        let start = std::time::Instant::now();
        loop {
            std::thread::sleep(std::time::Duration::from_millis(200));
            let elapsed = start.elapsed().as_secs_f32();
            // 渐近曲线：5秒内接近 90%，之后缓慢逼近 95%
            let progress = if elapsed < 5.0 {
                0.9 * (1.0 - (-elapsed / 2.5).exp())
            } else {
                0.9 + 0.05 * (1.0 - (-(elapsed - 5.0) / 5.0).exp())
            };
            // 如果通道关闭（扫描已完成），退出
            if tx_progress.send(ScanMessage::Progress(progress.min(0.95))).is_err() {
                break;
            }
        }
    });

    // 实际扫描线程
    std::thread::spawn(move || {
        // 用 catch_unwind 兜底，防止扫描 panic 后 UI 卡死
        let result = std::panic::catch_unwind(|| {
            match tab {
                Tab::DevCache => scanner::dev_cache::DevCacheScanner::new().scan(),
                Tab::LargeFiles => scanner::large_files::LargeFileScanner::new().scan(),
                Tab::AppCache => scanner::app_cache::AppCacheScanner::new().scan(),
                Tab::Apfs => scanner::apfs::ApfsScanner::new().scan(),
            }
        });

        match result {
            Ok(scan_result) => {
                // 后处理：检测每个 item 的可删除性
                let mut items = scan_result.items;
                for item in &mut items {
                    let (deletable, reason) = scanner::check_deletable(&item.path);
                    item.deletable = deletable && item.deletable;
                    if !item.deletable && !reason.is_empty() {
                        item.undeletable_reason = reason;
                    }
                }
                let _ = tx.send(ScanMessage::Done(items, scan_result.scan_time_ms, tab_idx as u64));
            }
            Err(_) => {
                // 扫描 panic，发送空结果让 UI 恢复正常
                let _ = tx.send(ScanMessage::Done(Vec::new(), 0, tab_idx as u64));
            }
        }
    });
}

/// 尽力删除：先 remove_dir_all，失败则用 rm -rf 命令，再失败就放弃
fn best_effort_delete(path: &std::path::Path) -> bool {
    // 先尝试直接删除
    if path.is_dir() {
        if std::fs::remove_dir_all(path).is_ok() {
            return !path.exists() && path.symlink_metadata().is_err();
        }
    } else {
        if std::fs::remove_file(path).is_ok() {
            return !path.exists() && path.symlink_metadata().is_err();
        }
    }

    // 直接删除失败，用 rm -rf 命令（比递归快得多）
    let path_str = path.to_string_lossy().to_string();
    let result = std::process::Command::new("rm")
        .arg("-rf")
        .arg(&path_str)
        .output();

    // 检查是否已删除
    !path.exists() && path.symlink_metadata().is_err()
}

/// 启动后台删除线程（两阶段自动删除）
/// 阶段1: 普通删除 (chflags + chmod + remove_dir_all)
/// 阶段2: 对失败项自动 sudo 批量删除 (只弹一次密码框)
fn start_delete(to_delete: Vec<(String, String)>, delete_rx: &mut Option<mpsc::Receiver<DeleteMessage>>) {
    let (tx, rx) = mpsc::channel();
    *delete_rx = Some(rx);

    std::thread::spawn(move || {
        let mut failed_items: Vec<(String, String)> = Vec::new();

        // ========== 阶段1: 普通删除 ==========
        for (path, category) in &to_delete {
            // 安全校验
            match safety::check_path_safety(path) {
                safety::SafetyCheck::Danger(reason) => {
                    let _ = tx.send(DeleteMessage::Log(
                        format!("⛔ 已拦截: {} - {}", path, reason), path.clone(), category.clone(), false));
                    safety::log_deletion(path, category, false, Some(&reason));
                    continue;
                }
                safety::SafetyCheck::Warning(reason) => {
                    let _ = tx.send(DeleteMessage::Log(
                        format!("⚠️ 已跳过: {} - {}", path, reason), path.clone(), category.clone(), false));
                    safety::log_deletion(path, category, false, Some(&reason));
                    continue;
                }
                safety::SafetyCheck::Safe => {}
            }

            // APFS 快照特殊处理
            if category == "APFS快照" {
                match scanner::apfs::delete_snapshot(path) {
                    Ok(_) => {
                        let _ = tx.send(DeleteMessage::Log(
                            format!("✓ 已删除快照: {}", path), path.clone(), category.clone(), true));
                        safety::log_deletion(path, category, true, None);
                    }
                    Err(e) => {
                        let _ = tx.send(DeleteMessage::Log(
                            format!("✗ 删除失败: {} - {}", path, e), path.clone(), category.clone(), false));
                        safety::log_deletion(path, category, false, Some(&e));
                    }
                }
                continue;
            }

            if category == "模拟器运行时" {
                match scanner::apfs::delete_simulator_runtime(path) {
                    Ok(_) => {
                        let _ = tx.send(DeleteMessage::Log(
                            format!("✓ 已删除运行时: {}", path), path.clone(), category.clone(), true));
                        safety::log_deletion(path, category, true, None);
                    }
                    Err(e) => {
                        let _ = tx.send(DeleteMessage::Log(
                            format!("✗ 删除失败: {} - {}", path, e), path.clone(), category.clone(), false));
                        safety::log_deletion(path, category, false, Some(&e));
                    }
                }
                continue;
            }

            // 普通文件/目录删除 - 尽力删除模式
            let p = std::path::Path::new(path.as_str());

            if !p.exists() && !p.symlink_metadata().is_ok() {
                let _ = tx.send(DeleteMessage::Log(
                    format!("✗ 路径不存在: {}", path), path.clone(), category.clone(), false));
                safety::log_deletion(path, category, false, Some("路径不存在"));
                continue;
            }

            // 拒绝删除符号链接
            if let Ok(meta) = p.symlink_metadata() {
                if meta.file_type().is_symlink() {
                    let _ = tx.send(DeleteMessage::Log(
                        format!("⛔ 拒绝删除符号链接: {}", path), path.clone(), category.clone(), false));
                    safety::log_deletion(path, category, false, Some("符号链接拒绝删除"));
                    continue;
                }
            }

            // 尽力删除：先直接删除，失败则递归逐个删除
            let deleted_ok = best_effort_delete(p);

            if deleted_ok {
                let _ = tx.send(DeleteMessage::Log(
                    format!("✓ 已删除 [{}] {}", category, path), path.clone(), category.clone(), true));
                safety::log_deletion(path, category, true, None);
            } else {
                // 普通删除失败，加入待 sudo 列表
                failed_items.push((path.clone(), category.clone()));
            }
        }

        // ========== 阶段2: 自动 sudo 批量删除失败项 ==========
        if !failed_items.is_empty() {
            let _ = tx.send(DeleteMessage::Info(
                format!("🔐 {} 项需要管理员权限，正在请求授权...", failed_items.len()),
            ));

            // 写临时脚本文件，避免 osascript 命令过长导致失败
            let tmp_script = std::env::temp_dir().join("maclean_sudo_delete.sh");
            let mut script_content = String::from("#!/bin/bash\n");
            for (path, _) in &failed_items {
                let escaped = path.replace("'", "'\\''");
                script_content.push_str(&format!(
                    "chflags -R nouchg '{}' 2>/dev/null; chmod -R u+rw '{}' 2>/dev/null; xattr -rc '{}' 2>/dev/null; rm -rf '{}' 2>/dev/null\n",
                    escaped, escaped, escaped, escaped
                ));
            }
            script_content.push_str("exit 0\n");
            let _ = std::fs::write(&tmp_script, &script_content);
            let _ = std::process::Command::new("chmod").arg("+x").arg(&tmp_script).output();

            let script_path = tmp_script.to_string_lossy().to_string();
            let apple_script = format!(
                "do shell script \"bash '{}'\" with administrator privileges",
                script_path.replace("\"", "\\\"")
            );

            let sudo_result = std::process::Command::new("osascript")
                .arg("-e")
                .arg(&apple_script)
                .output();

            // 记录 osascript 输出用于调试
            match &sudo_result {
                Ok(output) => {
                    let stdout = String::from_utf8_lossy(&output.stdout);
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    let _ = tx.send(DeleteMessage::Info(
                        format!("osascript 退出码: {}, stderr: {}", output.status.code().unwrap_or(-1), stderr.lines().last().unwrap_or("")),
                    ));
                    if !stdout.is_empty() {
                        let _ = tx.send(DeleteMessage::Info(format!("osascript stdout: {}", stdout.lines().last().unwrap_or(""))));
                    }
                }
                Err(e) => {
                    let _ = tx.send(DeleteMessage::Info(format!("osascript 启动失败: {}", e)));
                }
            }

            // 无论 sudo 整体成功或失败，都逐项验证
            match sudo_result {
                Ok(_output) => {
                    for (path, category) in &failed_items {
                        let p = std::path::Path::new(path.as_str());
                        if p.exists() || p.symlink_metadata().is_ok() {
                            let _ = tx.send(DeleteMessage::Log(
                                format!("✗ 删除失败: {}", path), path.clone(), category.clone(), false));
                            safety::log_deletion(path, category, false, Some("管理员权限删除后仍存在"));
                        } else {
                            let _ = tx.send(DeleteMessage::Log(
                                format!("✓ 已删除 [{}] {} (管理员权限)", category, path), path.clone(), category.clone(), true));
                            safety::log_deletion(path, category, true, None);
                        }
                    }
                }
                Err(e) => {
                    for (path, category) in &failed_items {
                        let _ = tx.send(DeleteMessage::Log(
                            format!("✗ 无法启动管理员授权: {} - {}", path, e), path.clone(), category.clone(), false));
                        safety::log_deletion(path, category, false, Some(&e.to_string()));
                    }
                }
            }

            // 清理临时脚本
            let _ = std::fs::remove_file(&tmp_script);
        }

        let _ = tx.send(DeleteMessage::Done);
    });
}

/// 权限引导弹窗
fn show_permission_guide_window(ctx: &egui::Context, app: &mut App) {
    egui::Window::new("权限设置")
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
                    ui.label(egui::RichText::new("授权完全磁盘访问").size(18.0).strong());
                });

                ui.add_space(8.0);
                ui.separator();
                ui.add_space(8.0);

                // 说明
                ui.colored_label(
                    egui::Color32::from_gray(200),
                    egui::RichText::new("maclean 需要完全磁盘访问权限才能删除开发者缓存文件。").size(13.0),
                );
                ui.add_space(3.0);
                ui.colored_label(
                    egui::Color32::from_gray(170),
                    egui::RichText::new("部分缓存文件由 root 创建且带有 macOS 安全属性，没有此权限将无法删除。").size(12.0),
                );

                ui.add_space(10.0);

                // 步骤
                ui.colored_label(
                    egui::Color32::from_rgb(100, 200, 100),
                    egui::RichText::new("请按以下步骤操作：").size(13.0).strong(),
                );
                ui.add_space(5.0);

                let steps = [
                    "点击下方「打开系统设置」按钮",
                    "在「完全磁盘访问」列表中找到 maclean",
                    "如果没有，点击 + 号添加 maclean.app",
                    "确保 maclean 旁边的开关已打开",
                    "重启 maclean 后即可正常删除",
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
                            egui::RichText::new("⚙️ 打开系统设置")
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
                            egui::RichText::new("📦 安装到应用程序")
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

                    if ui.button(egui::RichText::new("稍后再说").size(14.0)).clicked() {
                        app.dismiss_permission_guide();
                    }
                });

                ui.add_space(5.0);
                ui.colored_label(
                    egui::Color32::from_gray(120),
                    egui::RichText::new("提示: 授权后重启 maclean 即可正常使用所有删除功能").size(11.0),
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
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ctx, |ui| {
            ui.set_min_width(420.0);
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
                                egui::RichText::new("⚠️ 未授予完全磁盘访问权限").size(13.0).strong(),
                            );
                            ui.add_space(2.0);
                            ui.colored_label(
                                egui::Color32::from_gray(180),
                                egui::RichText::new("部分文件可能无法删除，建议先授权").size(12.0),
                            );
                            ui.add_space(3.0);
                            if ui.button(egui::RichText::new("⚙️ 去授权").size(12.0)).clicked() {
                                let _ = std::process::Command::new("open")
                                    .arg("x-apple.systempreferences:com.apple.preference.security?Privacy_AllFiles")
                                    .spawn();
                            }
                        });
                }

                ui.add_space(15.0);

                ui.horizontal(|ui| {
                    if ui.button(egui::RichText::new(format!("✓ {}", app.t("confirm_delete"))).color(egui::Color32::from_rgb(52, 199, 89))).clicked() {
                        let to_delete = app.confirm_delete();
                        start_delete(to_delete, delete_rx);
                    }
                    if ui.button(egui::RichText::new(format!("✗ {}", app.t("cancel"))).color(egui::Color32::RED)).clicked() {
                        app.cancel_delete();
                    }
                });
            });
        });
}

/// 删除中弹窗（带进度条）
fn show_deleting_window(ctx: &egui::Context, app: &mut App) {
    // 判断是否在 sudo 阶段（阶段1已完成，等待管理员权限）
    let in_sudo_phase = app.delete_done >= app.delete_total
        && app.logs.iter().any(|l| l.contains("管理员权限"));

    let progress = if in_sudo_phase {
        0.95 // sudo 阶段显示 95%
    } else if app.delete_total > 0 {
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
                "🔐 正在请求管理员权限，请在系统弹窗中输入密码..."
            } else {
                app.t("cleaning_in_progress")
            };
            ui.label(egui::RichText::new(format!("⏳ {}...", title)).size(15.0).color(egui::Color32::from_rgb(0, 200, 255)));
            ui.add_space(10.0);

            // 进度条
            let progress_text = if in_sudo_phase {
                "请求管理员权限中...".to_string()
            } else {
                format!("{}/{} ({}%)", app.delete_done, app.delete_total, (progress * 100.0) as u32)
            };
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
fn show_summary_window(ctx: &egui::Context, app: &mut App, ok: usize, fail: usize, skip: usize, _delete_rx: &mut Option<mpsc::Receiver<DeleteMessage>>) {
    let has_failures = !app.failed_paths.is_empty();

    egui::Window::new("清理结果")
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
                    ui.label(egui::RichText::new(format!("成功删除 {} 项", ok)).size(15.0).color(egui::Color32::from_rgb(52, 199, 89)));
                });

                if fail > 0 {
                    ui.add_space(5.0);
                    ui.horizontal(|ui| {
                        ui.colored_label(egui::Color32::RED, "❌");
                        ui.label(egui::RichText::new(format!("删除失败 {} 项", fail)).size(15.0).color(egui::Color32::RED));
                    });
                    ui.add_space(3.0);
                    ui.colored_label(
                        egui::Color32::from_gray(150),
                        egui::RichText::new("这些文件由 root 创建且带有 macOS 安全属性 (com.apple.provenance)，受 SIP 保护无法删除。").size(12.0),
                    );

                    // 引导用户授予完全磁盘访问权限
                    ui.add_space(8.0);
                    egui::Frame::group(ui.style())
                        .fill(egui::Color32::from_rgb(30, 35, 50))
                        .stroke(egui::Stroke::new(1.0, egui::Color32::from_rgb(60, 80, 120)))
                        .inner_margin(egui::Margin::same(8.0))
                        .show(ui, |ui| {
                            ui.colored_label(
                                egui::Color32::from_rgb(100, 150, 255),
                                egui::RichText::new("💡 提示: macOS 保护机制阻止了删除").size(13.0).strong(),
                            );
                            ui.add_space(3.0);
                            ui.colored_label(
                                egui::Color32::from_gray(180),
                                egui::RichText::new("这些文件由 root 创建且带有 macOS 安全属性 (com.apple.provenance)，").size(11.0),
                            );
                            ui.colored_label(
                                egui::Color32::from_gray(180),
                                egui::RichText::new("即使管理员权限也无法删除。请尝试以下方法：").size(11.0),
                            );
                            ui.add_space(3.0);
                            ui.colored_label(
                                egui::Color32::from_rgb(200, 200, 200),
                                egui::RichText::new("1. 终端执行: sudo rm -rf 路径 (可能仍失败)").size(11.0),
                            );
                            ui.colored_label(
                                egui::Color32::from_rgb(200, 200, 200),
                                egui::RichText::new("2. 关闭SIP: 重启→按住Cmd+R→终端→csrutil disable→重启").size(11.0),
                            );
                            ui.colored_label(
                                egui::Color32::from_rgb(200, 200, 200),
                                egui::RichText::new("3. 用项目工具删除: cd 项目目录 && npm run clean / npx rimraf .next").size(11.0),
                            );
                            ui.add_space(5.0);
                            if ui.button(egui::RichText::new("⚙️ 打开系统设置").size(12.0)).clicked() {
                                let _ = std::process::Command::new("open")
                                    .arg("x-apple.systempreferences:com.apple.preference.security?Privacy_AllFiles")
                                    .spawn();
                            }
                        });

                    // 列出所有失败的路径（可滚动+复制）
                    ui.add_space(5.0);
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new("失败列表:").size(12.0).color(egui::Color32::from_gray(170)));
                        let all_paths: String = app.failed_paths.iter()
                            .map(|(p, c)| format!("[{}] {}", c, p))
                            .collect::<Vec<_>>()
                            .join("\n");
                        if ui.button(egui::RichText::new("📋 复制全部").size(11.0)).clicked() {
                            ui.output_mut(|o| o.copied_text = all_paths);
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
                ui.label(egui::RichText::new(format!("当前可用空间: {}", format_size(app.disk_free))).size(14.0));

                ui.add_space(15.0);
                ui.horizontal(|ui| {
                    if ui.button(egui::RichText::new("确定").size(14.0)).clicked() {
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
        c if c.contains("微信") => egui::Color32::from_rgb(100, 200, 100),
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
