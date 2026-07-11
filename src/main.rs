//! maclean - macOS 磁盘清理 GUI 工具
//!
//! 使用 egui 构建，专注开发者缓存与深度清理。

mod app;
mod safety;
mod scanner;

use std::sync::mpsc;

use eframe::egui;

use app::{App, ConfirmState, ScanState, Tab};
use scanner::{format_size, Recommend, ScanItem, Scanner};

/// 后台扫描消息
enum ScanMessage {
    /// 扫描完成
    Done(Vec<ScanItem>, u64, u64), // (items, scan_time_ms, tab_index)
}

fn main() -> eframe::Result {
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
        static mut NEEDS_INIT: bool = true;

        unsafe {
            if NEEDS_INIT {
                APP = Some(App::new());
                NEEDS_INIT = false;
                setup_fonts(ctx);
            }

            // 检查后台扫描结果
            if let Some(rx) = &SCAN_RX {
                if let Ok(msg) = rx.try_recv() {
                    if let Some(app) = &mut APP {
                        match msg {
                            ScanMessage::Done(items, time_ms, tab_idx) => {
                                app.results[tab_idx as usize] = items;
                                app.scan_states[tab_idx as usize] = ScanState::Done;
                                app.scan_time_ms[tab_idx as usize] = time_ms;
                                let (total, free) = get_disk_info();
                                app.disk_total = total;
                                app.disk_free = free;
                            }
                        }
                    }
                }
            }

            if let Some(app) = &mut APP {
                render_gui(ctx, app, &mut SCAN_RX);
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
fn render_gui(ctx: &egui::Context, app: &mut App, scan_rx: &mut Option<mpsc::Receiver<ScanMessage>>) {
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
            // 扫描中：进度动画
            ui.vertical_centered(|ui| {
                ui.add_space(60.0);
                ui.add(egui::Spinner::new().size(60.0));
                ui.add_space(15.0);
                ui.label(egui::RichText::new(format!("⏳ {}...", app.t("scanning"))).size(18.0).color(egui::Color32::from_rgb(0, 200, 255)));
                ui.add_space(8.0);
                ui.label(egui::RichText::new(app.t("scanning_hint")).size(13.0).color(egui::Color32::GRAY));
                ui.add_space(20.0);
                let t = ctx.input(|i| i.time) as f32;
                let progress = (t.sin() * 0.5 + 0.5).clamp(0.0, 1.0);
                ui.add(egui::ProgressBar::new(progress).desired_width(300.0).fill(egui::Color32::from_rgb(0, 200, 255)));
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
                    let rec_color = recommend_color(&item.recommend);
                    let badge = recommend_badge(&item.recommend);

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
                                    ui.colored_label(category_color(&item.category), &item.category);
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
                                ui.colored_label(
                                    egui::Color32::from_gray(100),
                                    egui::RichText::new(&path_display).size(11.0),
                                );
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

        // 删除确认弹窗
        if matches!(app.confirm, ConfirmState::Pending) {
            show_confirm_window(ctx, app);
        }

        // 删除中弹窗
        if matches!(app.confirm, ConfirmState::Deleting) {
            show_deleting_window(ctx, app);
        }
    });
}

/// 启动后台扫描
fn start_scan(app: &mut App, scan_rx: &mut Option<mpsc::Receiver<ScanMessage>>) {
    let tab = app.tab;
    let tab_idx = app.tab_index();
    app.scan_states[tab_idx] = ScanState::Scanning;

    let (tx, rx) = mpsc::channel();
    *scan_rx = Some(rx);

    std::thread::spawn(move || {
        let result = match tab {
            Tab::DevCache => scanner::dev_cache::DevCacheScanner::new().scan(),
            Tab::LargeFiles => scanner::large_files::LargeFileScanner::new().scan(),
            Tab::AppCache => scanner::app_cache::AppCacheScanner::new().scan(),
            Tab::Apfs => scanner::apfs::ApfsScanner::new().scan(),
        };

        let _ = tx.send(ScanMessage::Done(result.items, result.scan_time_ms, tab_idx as u64));
    });
}

/// 删除确认弹窗
fn show_confirm_window(ctx: &egui::Context, app: &mut App) {
    let count = app.selected_count();
    let size = app.selected_total_size();

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
                ui.add_space(15.0);

                ui.horizontal(|ui| {
                    if ui.button(egui::RichText::new(format!("✓ {}", app.t("confirm_delete"))).color(egui::Color32::from_rgb(52, 199, 89))).clicked() {
                        app.confirm_delete();
                    }
                    if ui.button(egui::RichText::new(format!("✗ {}", app.t("cancel"))).color(egui::Color32::RED)).clicked() {
                        app.cancel_delete();
                    }
                });
            });
        });
}

/// 删除中弹窗
fn show_deleting_window(ctx: &egui::Context, app: &mut App) {
    egui::Window::new(app.t("cleaning"))
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ctx, |ui| {
            ui.set_min_width(500.0);
            ui.set_min_height(200.0);
            ui.add_space(5.0);

            ui.label(egui::RichText::new("⏳ ".to_string() + app.t("cleaning_in_progress")).size(14.0));
            ui.add_space(5.0);

            egui::ScrollArea::vertical().show(ui, |ui| {
                for log in app.logs.iter().rev().take(10) {
                    let color = if log.starts_with('✓') {
                        egui::Color32::GREEN
                    } else if log.starts_with('✗') {
                        egui::Color32::RED
                    } else if log.starts_with('⛔') {
                        egui::Color32::from_rgb(200, 100, 100)
                    } else if log.starts_with('⚠') {
                        egui::Color32::YELLOW
                    } else {
                        egui::Color32::from_rgb(0, 200, 255)
                    };
                    ui.colored_label(color, log);
                }
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
