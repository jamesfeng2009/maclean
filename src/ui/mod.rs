//! UI 层
//!
//! 面板 / 列表行 / 弹窗 / 菜单栏 HUD，以及持有每帧状态的 `Gui`。
//! 只负责画和收集输入，真正的活都交给 [`crate::ops`]。
//!
//! 2026-09 从 main.rs 拆出，内容与拆分前逐行一致。

use std::sync::mpsc;

use eframe::egui;

use crate::app::{App, ConfirmState, ScanState, Tab};
use crate::icons;
use crate::ops::{
    open_url, start_delete, start_optimize_task, start_scan, start_scan_all, start_sudo_delete,
    DeleteMessage, ScanMessage,
};
// Touch ID 提权删除是 macOS 专属 DRM 机制，Windows/Linux 上该函数不存在
#[cfg(target_os = "macos")]
use crate::ops::start_sudo_delete_touchid;
// 模块路径本身也要引入：代码里大量写成 `theme::text()` / `scanner::Foo`
// `platform` 只在 Windows 分支里用到（还原点 / 注册表备份入口），
// 不 cfg 限定的话本机（macOS）会报 unused import。
#[cfg(target_os = "windows")]
use crate::platform;
use crate::scanner::{format_size, Recommend, ScanItem};
use crate::theme::*;
use crate::widgets;
use crate::{config, i18n, menubar, safety, scanner, theme};
use crate::{get_disk_info, log_scan_step, logger};
#[cfg(target_os = "macos")]
use crate::{sudo_keepalive, touchid};

#[derive(Debug, Clone, Copy)]
pub(crate) enum HudAction {
    QuickScan,
    QuickClean,
    ShowWindow,
    Settings,
    Quit,
}

/// 渲染菜单栏 HUD 悬浮窗口
pub(crate) fn render_hud_window(
    ctx: &egui::Context,
    app: &mut App,
    scan_rx: &mut Option<mpsc::Receiver<ScanMessage>>,
    auto_clean_after_scan: &mut bool,
) {
    if !app.hud_open {
        return;
    }

    let frame = egui::Frame::none()
        .fill(theme::surface())
        .stroke(egui::Stroke::new(1.0_f32, theme::line()))
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
                    theme::text(),
                    egui::RichText::new("maclean").size(14.0).strong(),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let size_str = format_size(app.total_releasable_size());
                    ui.colored_label(
                        theme::brand(),
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

            // 绘制可点击的 HUD 动作行（线性图标，不用 emoji）
            let row = |ui: &mut egui::Ui, ic: icons::Icon, text: &str| -> bool {
                let galley = ui.painter().layout_no_wrap(
                    text.to_string(),
                    egui::FontId::new(13.0, egui::FontFamily::Proportional),
                    theme::text(),
                );
                let padding = egui::vec2(8.0, 8.0);
                let icon_w = 16.0 + 8.0;
                let desired_size = galley.size() + padding * 2.0 + egui::vec2(icon_w, 0.0);
                let (rect, response) = ui.allocate_exact_size(desired_size, egui::Sense::click());
                if ui.is_rect_visible(rect) {
                    if response.hovered() {
                        ui.painter().rect_filled(
                            rect,
                            egui::Rounding::same(6.0),
                            theme::surface_3(),
                        );
                    }
                    let ir = egui::Rect::from_min_size(
                        egui::pos2(rect.min.x + padding.x, rect.min.y + padding.y),
                        egui::vec2(16.0, 16.0),
                    );
                    icons::paint(ui.painter(), ir, ic, theme::text());
                    ui.painter().galley(
                        egui::pos2(rect.min.x + padding.x + icon_w, rect.min.y + padding.y),
                        galley,
                        theme::text(),
                    );
                }
                response.clicked()
            };

            if row(ui, icons::Icon::Search, "快速扫描") {
                action_to_run = Some(HudAction::QuickScan);
            }
            if row(ui, icons::Icon::Shield, "清理 Safe 项目") {
                action_to_run = Some(HudAction::QuickClean);
            }
            if row(ui, icons::Icon::App, "打开主窗口") {
                action_to_run = Some(HudAction::ShowWindow);
            }
            if row(ui, icons::Icon::Gear, "设置") {
                action_to_run = Some(HudAction::Settings);
            }
            if row(ui, icons::Icon::X, "退出") {
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
            *auto_clean_after_scan = false;
            if !app
                .scan_states
                .iter()
                .any(|s| matches!(s, ScanState::Scanning))
            {
                start_scan_all(app, scan_rx);
            }
        }
        Some(HudAction::QuickClean) => {
            app.hud_open = false;
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
            ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
            ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
            app.tab = Tab::DevCache;
            if !app.any_scanning() {
                *auto_clean_after_scan = true;
                start_scan(app, scan_rx);
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

/// 概览页「一键清理」：跨 Tab 选中推荐项并进入确认
///
/// 单独抽成函数是为了可测 —— 按钮点击本身需要 egui 上下文，无法单测，
/// 但按钮背后的这条逻辑必须锁定。
///
/// 必须用 `prepare_delete_cross_tab`，不能用 `prepare_delete`：
/// 当前 tab 是 Overview，而 `prepare_delete` 只扫 `tab_index()`（=0）的
/// `results[0]` —— Overview 的 results 恒为空，会导致静默 return、
/// 确认框永不弹出，只在其他 Tab 留下莫名其妙的预选（历史 bug）。
pub(crate) fn one_click_clean(app: &mut App, items: &[(usize, usize)]) {
    for &(tab_idx, item_idx) in items {
        if let Some(item) = app.results[tab_idx].get_mut(item_idx) {
            item.selected = true;
        }
    }
    app.prepare_delete_cross_tab(items.to_vec());
}

/// GUI 应用层：持有 eframe 每帧回调之间需要保持的全部可变状态
///
/// 2026-09 之前这里是 `run_simple_native` 闭包里的 7 个 `static mut`，读写全靠
/// `unsafe {}` —— 编译器完全不检查，两个可变引用同时存在是静默 UB。
/// 现在收进实现 `eframe::App` 的结构体，由借用检查器兜底。
pub(crate) struct Gui {
    app: App,
    scan_rx: Option<mpsc::Receiver<ScanMessage>>,
    delete_rx: Option<mpsc::Receiver<DeleteMessage>>,
    menubar: menubar::MenuBarHud,
    /// 首帧标记：字体 / 配色 / 扫描缓存 / 托盘只初始化一次
    needs_init: bool,
    /// 上次刷新托盘「可释放空间」的时间戳（秒）
    last_disk_update: f64,
    /// QuickClean 标志：扫描完成后自动选中 Safe 项并删除
    auto_clean_after_scan: bool,
    /// 仅供测试：强制 Touch ID 状态（available, enabled），绕开真实系统查询。
    /// 否则测试只能在"机器真的启用了 Touch ID sudo"时才能覆盖该分支。
    #[cfg(test)]
    force_touch_id: Option<(bool, bool)>,
}

impl Gui {
    pub(crate) fn new() -> Self {
        Self {
            app: App::new(),
            scan_rx: None,
            delete_rx: None,
            menubar: menubar::MenuBarHud::new(),
            needs_init: true,
            last_disk_update: 0.0,
            auto_clean_after_scan: false,
            #[cfg(test)]
            force_touch_id: None,
        }
    }

    /// 首帧初始化
    fn init(&mut self, ctx: &egui::Context) {
        setup_fonts(ctx);
        // 初始配色（此后每帧由 render_gui 里的 sync_visuals 跟随用户切换）
        theme::apply_visuals(ctx, self.app.theme_mode());
        self.app.load_current_tab_cache();
        self.menubar.init();
    }

    /// 轮询后台系统优化任务结果（耗时任务线程化后，GUI 不再冻结）。
    ///
    /// 原实现把 `chmod -R ~/Library`（修复权限）/`diskutil verifyVolume /`
    /// （校验启动盘）等耗时数分钟的命令直接同步跑在 egui 主线程里，
    /// `.output()` 阻塞事件循环，界面定格、无法点取消。线程化后这里
    /// 每帧 try_recv 收结果写日志，并复位运行状态。
    fn poll_optimize(&mut self, ctx: &egui::Context) {
        if let Some(rx) = self.app.optimize_rx.as_ref() {
            match rx.try_recv() {
                Ok(log) => {
                    self.app.logs.push(log);
                    self.app.optimize_running = None;
                    self.app.optimize_rx = None;
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => {
                    // 仍在执行，保持运行态；持续请求重绘让按钮显示「执行中…」
                    ctx.request_repaint_after(std::time::Duration::from_millis(200));
                }
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    // 发送端已释放（线程提前退出/panic）：如实复位，不留死状态
                    self.app
                        .logs
                        .push(App::t_lang(self.app.lang_en, "optimize_aborted").to_string());
                    self.app.optimize_running = None;
                    self.app.optimize_rx = None;
                }
            }
        }
    }

    /// 轮询菜单栏事件：托盘动作 + 点击图标展开/收起 HUD
    fn poll_menubar(&mut self, ctx: &egui::Context) {
        // 轮询菜单栏事件
        let mut hud_click: Option<menubar::ClickInfo> = None;
        // 2026-09-18 删除了 menubar::poll_events 的整段消费逻辑：它恒返回空，
        // TrayAction 从未被构造。托盘动作由 HUD 窗口自己处理（见 hud_action 分支）。

        // 点击托盘图标：展开/收起 HUD
        if let Some(click_info) = self.menubar.poll_click() {
            hud_click = Some(click_info);
        }

        if let Some(click_info) = hud_click {
            if let Some(app) = Some(&mut self.app) {
                let minimized = ctx.input(|i| i.viewport().minimized).unwrap_or(false);
                // 无论最小化还是后台，都先把主窗口拉到前台
                ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
                ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                if minimized || !app.hud_open {
                    // 窗口最小化或 HUD 关闭时：打开 HUD
                    app.hud_open = true;
                } else {
                    // 窗口已显示且 HUD 已打开：收起 HUD
                    app.hud_open = false;
                }
                // 记录托盘点击位置，供 HUD 窗口定位使用
                app.last_hud_click_pos = Some((click_info.x, click_info.y));
            }
        }
    }

    /// 定期刷新托盘上显示的可释放空间（每 60 秒）
    fn poll_tray_releasable(&mut self) {
        // 定期更新托盘显示的可释放空间（每 60 秒，或数值变化时由 update_releasable 内部节流）
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs_f64())
            .unwrap_or(0.0);
        if now - self.last_disk_update > 60.0 || self.last_disk_update == 0.0 {
            self.last_disk_update = now;
            let releasable = self.app.total_releasable_size();
            let lang_en = self.app.lang_en;
            self.menubar.update_releasable(releasable, lang_en);

            // 托盘图标的磁盘占用环：此前 update_disk_usage 从未被调用，图标
            // 一直停在 init() 时的 create_icon(0.0)，70%/85% 的变黄/变红阈值
            // 永远触发不了。这里随周期一起刷新。
            let (total, free) = crate::platform::disk_info();
            if total > 0 {
                let used_pct = (total.saturating_sub(free)) as f32 / total as f32 * 100.0;
                self.menubar.update_disk_usage(used_pct);
            }
        }
    }

    /// 收取后台扫描线程的消息
    fn poll_scan(&mut self) {
        let mut clear_scan_rx = false;
        // 至少有一个 Tab 已经扫完。
        //
        // 进度估算线程每 200ms 发一次 Progress，扫描线程结束后它才退出，
        // 所以在 Done 之后仍会收到几条 Progress —— 不拦掉的话进度条会从
        // 100% 回退到 99.x%（多 Tab 扫描时尤其明显）。
        let mut scan_finished = false;
        // 检查后台扫描结果
        if let Some(rx) = self.scan_rx.as_ref() {
            loop {
                match rx.try_recv() {
                    Ok(ScanMessage::Progress(p)) => {
                        if !scan_finished {
                            if let Some(app) = Some(&mut self.app) {
                                app.scan_progress = p;
                            }
                        }
                    }
                    Ok(ScanMessage::PartialItems(items, tab_idx)) => {
                        // 增量结果：扫描中已发现的部分项，直接追加到当前 Tab 的结果列表
                        if let Some(app) = Some(&mut self.app) {
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
                        if let Some(app) = Some(&mut self.app) {
                            app.scan_current_path = path;
                        }
                    }
                    Ok(ScanMessage::Skipped(tab_idx, reason)) => {
                        if let Some(app) = Some(&mut self.app) {
                            let idx = tab_idx as usize;
                            app.results[idx] = Vec::new();
                            app.scan_states[idx] = ScanState::Done;
                            app.scan_time_ms[idx] = 0;
                            app.scan_current_path = format!("⚠ {reason}");
                        }
                        // 与 Done 一致：之后的 Progress 一律忽略，避免进度条回退
                        scan_finished = true;
                    }
                    Ok(ScanMessage::Done(items, time_ms, tab_idx)) => {
                        if let Some(app) = Some(&mut self.app) {
                            let idx = tab_idx as usize;
                            app.results[idx] = items;
                            app.scan_states[idx] = ScanState::Done;
                            app.scan_time_ms[idx] = time_ms;
                            app.scan_progress = 1.0;

                            // 扫描落库后的两步后处理。此前这两步只存在于
                            // 从未被调用的 `App::scan_current()` 里，生产路径
                            // 上从没执行过 —— 结果就是：safety 判定为 Danger 的
                            // 项在 UI 上仍然可勾选，点了才在删除时被拒。
                            app.precheck_deletability(idx);
                            if app.tab == Tab::AppUninstall {
                                app.populate_associated_details(idx);
                            }
                            let (total, free) = get_disk_info();
                            app.disk_total = total;
                            app.disk_free = free;
                        }
                        // 之后的 Progress 一律忽略，避免进度条回退
                        scan_finished = true;
                        // 单个 Tab 扫描完成后不 break，继续接收 AllDone 或更多 Done
                    }
                    Ok(ScanMessage::AllDone) => {
                        if let Some(app) = Some(&mut self.app) {
                            app.scan_progress = 1.0;
                            let (total, free) = get_disk_info();
                            app.disk_total = total;
                            app.disk_free = free;

                            // 一键清理模式：自动选择 Safe 项并删除
                            if self.auto_clean_after_scan {
                                self.auto_clean_after_scan = false;
                                app.select_safe_only();
                                let selected_count = app.selected_count();
                                if selected_count > 0 {
                                    log_scan_step(&app.tf(
                                        "log_quickclean_start",
                                        &[&selected_count.to_string()],
                                    ));
                                    app.prepare_delete();
                                    let to_delete = app.confirm_delete();
                                    start_delete(
                                        to_delete,
                                        app.lang_en,
                                        &mut self.delete_rx,
                                        app.settings_auto_restore_point,
                                        app.settings_prefer_official_uninstaller,
                                        app.delete_cancel.clone(),
                                    );
                                } else {
                                    log_scan_step(app.t("log_quickclean_none"));
                                }
                            }
                        }
                        clear_scan_rx = true;
                        break;
                    }
                    Err(_) => break,
                }
            }
        }

        if clear_scan_rx {
            self.scan_rx = None;
        }
    }

    /// 全局键盘快捷键
    ///
    /// 重构到 `impl eframe::App` 之后，`next_tab` / `move_up` / `toggle_select` /
    /// `quit` 这几个方法失去了调用点，界面却仍在提示「按 R 扫描」，
    /// 除托盘外也没有退出路径。这里把它们接回来。
    fn handle_shortcuts(&mut self, ctx: &egui::Context) {
        // 搜索框 / 密码框等获得焦点时不抢按键
        if ctx.memory(|m| m.focused().is_some()) {
            return;
        }

        let (cmd, q, r, tab, up, down, space, esc, slash) = ctx.input(|i| {
            (
                i.modifiers.command,
                i.key_pressed(egui::Key::Q),
                i.key_pressed(egui::Key::R),
                i.key_pressed(egui::Key::Tab),
                i.key_pressed(egui::Key::ArrowUp),
                i.key_pressed(egui::Key::ArrowDown),
                i.key_pressed(egui::Key::Space),
                i.key_pressed(egui::Key::Escape),
                i.key_pressed(egui::Key::Slash),
            )
        });

        if cmd && q {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            return;
        }

        // Esc：取消删除确认（弹窗内部自己的 Esc 处理仍然优先）
        if esc && !matches!(self.app.confirm, ConfirmState::None) {
            self.app.cancel_delete();
            return;
        }

        // 确认/删除进行中，其余快捷键一律不响应
        if !matches!(self.app.confirm, ConfirmState::None) {
            return;
        }

        if slash {
            self.app.filter_active = true;
            return;
        }

        if tab {
            self.app.next_tab();
            return;
        }
        if up {
            self.app.move_up();
            return;
        }
        if down {
            self.app.move_down();
            return;
        }
        if space {
            self.app.toggle_select();
            return;
        }

        // R：重新扫描当前 Tab（界面上就是这么提示的）
        if r && !self.app.any_scanning() && !self.app.is_non_scannable_tab() {
            start_scan(&mut self.app, &mut self.scan_rx);
        }
    }

    /// 处理「部分项需要管理员权限」
    ///
    /// 返回是否拉起了新的删除线程 —— 调用方据此决定 `delete_rx` 的去留。
    #[cfg(target_os = "macos")]
    fn handle_need_password(&mut self, items: Vec<(String, String)>) -> bool {
        #[cfg(test)]
        let (available, enabled) = match self.force_touch_id {
            Some(v) => v,
            None => (
                touchid::touch_id_available(),
                touchid::sudo_touch_id_enabled(),
            ),
        };
        #[cfg(not(test))]
        let (available, enabled) = (
            touchid::touch_id_available(),
            touchid::sudo_touch_id_enabled(),
        );
        let clamshell_closed = safety::is_clamshell_closed();
        self.route_need_password(items, enabled, available, clamshell_closed)
    }

    /// 同上（非 macOS：没有 Touch ID，一律走密码输入）
    #[cfg(not(target_os = "macos"))]
    fn handle_need_password(&mut self, items: Vec<(String, String)>) -> bool {
        let app = &mut self.app;
        app.sudo_failed_items = items;
        app.sudo_password_input.clear();
        app.sudo_password = None;
        app.sudo_error = None;
        app.touch_id_error = None;
        app.confirm = ConfirmState::NeedSudoPassword;
        false
    }

    /// 决策 + 副作用：不查询系统状态，三个条件由调用方传入，便于单测覆盖各分支
    ///
    /// 逻辑已下沉到 [`route_and_start_elevated_delete`]，GUI 流程与
    /// 删除失败后的「重试删除」共用同一决策，避免两处漂移。
    #[cfg(target_os = "macos")]
    fn route_need_password(
        &mut self,
        items: Vec<(String, String)>,
        touch_id_enabled: bool,
        touch_id_available: bool,
        clamshell_closed: bool,
    ) -> bool {
        route_and_start_elevated_delete(
            &mut self.app,
            items,
            touch_id_enabled,
            touch_id_available,
            clamshell_closed,
            &mut self.delete_rx,
        )
    }

    /// 收取后台删除线程的消息
    fn poll_delete(&mut self) {
        // 语义：**只在删除流程真正结束时**才置 true。
        // 若某个分支重新拉起了删除线程（如 Touch ID 路径），self.delete_rx 里
        // 已是新线程的 receiver，必须保持 false，否则后续消息无人接收。
        let mut clear_delete_rx = false;
        // 检查后台删除进度
        if let Some(rx) = self.delete_rx.as_ref() {
            loop {
                match rx.try_recv() {
                    Ok(DeleteMessage::Log(log, path, category, success)) => {
                        if let Some(app) = Some(&mut self.app) {
                            app.receive_delete_log(log, path, category, success);
                        }
                    }
                    Ok(DeleteMessage::Skip(log, _path, _category)) => {
                        // 跳过项：只进日志区展示，不计入成功/失败统计
                        if let Some(app) = Some(&mut self.app) {
                            app.logs.push(log);
                        }
                    }
                    Ok(DeleteMessage::Info(info)) => {
                        if let Some(app) = Some(&mut self.app) {
                            app.logs.push(info);
                        }
                    }
                    Ok(DeleteMessage::NeedPassword(items)) => {
                        // 本分支可能会**重新拉起**一条删除线程（Touch ID 路径），
                        // 那 self.delete_rx 里就是新线程的 receiver，绝不能清掉。
                        let started_new_delete = self.handle_need_password(items);
                        // 只有**没有**重新拉起删除线程时，这条通道才真的结束了。
                        // Touch ID 分支会把新线程的 receiver 写进 self.delete_rx，
                        // 此时清掉会导致消息无人接收、confirm 永久卡在 SudoWithTouchId。
                        clear_delete_rx = !started_new_delete;
                        break;
                    }
                    Ok(DeleteMessage::Cancelled) => {
                        // P0-3：用户中途停止。已删项已记录清单；未处理项未动。
                        if let Some(app) = Some(&mut self.app) {
                            app.sudo_password = None;
                            app.sudo_password_input.clear();
                            app.finish_delete();
                            app.logs.push(
                                crate::app::App::t_lang(app.lang_en, "log_delete_cancelled")
                                    .to_string(),
                            );
                        }
                        clear_delete_rx = true;
                        break;
                    }
                    Ok(DeleteMessage::Done) => {
                        if let Some(app) = Some(&mut self.app) {
                            app.sudo_password = None;
                            app.sudo_password_input.clear();
                            // 刷新 sudo 会话状态（keepalive 可能仍活跃）(macOS 专属)
                            #[cfg(target_os = "macos")]
                            {
                                app.sudo_session_active = sudo_keepalive::is_sudo_active();
                            }
                            app.finish_delete();
                        }
                        clear_delete_rx = true;
                        break;
                    }
                    Ok(DeleteMessage::BackupRecorded {
                        id,
                        restorable,
                        total,
                    }) => {
                        // M-2：记下清单位置与"真能还原"的项数。
                        // restorable 是 0 也要照样显示 —— 用户必须知道这次
                        // 删除没有后悔药，而不是看到一个含糊的"已备份"。
                        if let Some(app) = Some(&mut self.app) {
                            app.last_backup = Some((id, restorable, total));
                        }
                    }
                    #[cfg(target_os = "windows")]
                    Ok(DeleteMessage::ResidualFound(residual)) => {
                        if let Some(app) = Some(&mut self.app) {
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
                        if let Some(app) = Some(&mut self.app) {
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
                    Err(std::sync::mpsc::TryRecvError::Empty) => break,
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                        // 发送端已释放：删除线程可能 panic 或提前 return，
                        // 此时必须强制收尾并复位 confirm，否则弹窗会永久卡在 Deleting，
                        // 用户只能靠"后台运行"退出（历史 bug）。
                        if let Some(app) = Some(&mut self.app) {
                            logger::error("删除线程提前退出，强制收尾");
                            app.sudo_password = None;
                            app.sudo_password_input.clear();
                            app.finish_delete();
                        }
                        clear_delete_rx = true;
                        break;
                    }
                }
            }
        }

        if clear_delete_rx {
            self.delete_rx = None;
        }
    }
}

/// 决策并启动管理员提权删除（GUI 流程与「重试删除」共用，单一决策来源）
///
/// 返回是否拉起了新的删除线程 —— 调用方据此决定 `delete_rx` 的去留。
/// 三个系统条件（Touch ID 是否启用 / 是否可用 / 是否合盖）由调用方传入，
/// 便于单测覆盖各分支。
///
/// 决策顺序：
/// 1. Touch ID 已启用 **且已录入指纹**：直接走 Touch ID 删除
///    （两个条件缺一不可 —— sudo_local 配好但没指纹时，系统弹 Touch ID
///    却无法验证，删除会卡死在死路上，必须回退到密码输入）
/// 2. Touch ID 可用但未启用：提示用户是否启用
/// 3. 无 Touch ID / 未录指纹 / 合盖：走应用内密码输入提权
#[cfg(target_os = "macos")]
pub(crate) fn route_and_start_elevated_delete(
    app: &mut App,
    items: Vec<(String, String)>,
    touch_id_enabled: bool,
    touch_id_available: bool,
    clamshell_closed: bool,
    delete_rx: &mut Option<mpsc::Receiver<DeleteMessage>>,
) -> bool {
    app.sudo_failed_items = items;
    app.sudo_password_input.clear();
    app.sudo_password = None;
    app.sudo_error = None;
    app.touch_id_error = None;
    app.touch_id_available = touch_id_available;
    app.touch_id_enabled = touch_id_enabled;
    if clamshell_closed {
        // 合盖时 Touch ID 不可用，回退到密码输入
        app.touch_id_error = Some(app.t("touchid_clamshell_error").to_string());
        app.touch_id_available = false;
    }

    if touch_id_enabled && touch_id_available && !clamshell_closed {
        // Touch ID 已启用且已录入指纹：直接用 sudo（Touch ID 自动触发）。
        // 阶段1失败项将进入 sudo 阶段重试，先清掉阶段1的失败计数，
        // 避免最终弹窗把已重试成功的项也算作失败（双重计数）。
        app.failed_paths.clear();
        let items = app.sudo_failed_items.clone();
        if items.is_empty() {
            // 没有真正需要提权的项：直接收尾。
            // 否则会停在"Touch ID 验证中"却没有任何线程在跑。
            app.finish_delete();
            return false;
        }
        app.confirm = ConfirmState::SudoWithTouchId;
        app.delete_done = 0;
        app.delete_total = items.len();
        let lang_en = app.lang_en;
        start_sudo_delete_touchid(items, lang_en, delete_rx, app.delete_cancel.clone())
    } else if touch_id_available && !clamshell_closed {
        // Touch ID 可用但未启用：提示用户是否启用
        app.confirm = ConfirmState::OfferTouchIdSetup;
        false
    } else {
        // 无 Touch ID / 未录指纹 / 合盖：走密码输入流程
        app.confirm = ConfirmState::NeedSudoPassword;
        false
    }
}

impl eframe::App for Gui {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if self.needs_init {
            self.needs_init = false;
            self.init(ctx);
        }

        self.poll_menubar(ctx);
        self.poll_tray_releasable();
        self.poll_scan();
        self.poll_delete();
        self.poll_optimize(ctx);
        self.handle_shortcuts(ctx);

        // 磁盘监控：每 5 秒轮询磁盘空间
        self.app.poll_disk_space();

        // 扫描 / 删除进行中：持续请求重绘。
        // 关键：egui 是事件驱动的，若扫描线程因磁盘 IO 慢而阻塞、不产生
        // 新消息，且没有持续 repaint，事件循环会空闲休眠，整个界面定格
        // （进度条、路径、按钮全部"卡死"）。这里每 50ms 强制唤醒一次，
        // 保证流动进度条持续动画、当前路径/已发现项实时刷新、取消按钮可点。
        if self.scan_rx.is_some() || self.delete_rx.is_some() {
            ctx.request_repaint_after(std::time::Duration::from_millis(50));
        }

        // 请求重绘以保持告警 UI 实时更新
        if self.app.disk_alert_level().0 >= 2 {
            ctx.request_repaint();
        }

        let app = &mut self.app;
        render_gui(ctx, app, &mut self.scan_rx, &mut self.delete_rx);
        render_hud_window(ctx, app, &mut self.scan_rx, &mut self.auto_clean_after_scan);
    }
}

/// 加载系统中文字体（跨平台）
pub(crate) fn setup_fonts(ctx: &egui::Context) {
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
            fonts
                .font_data
                .insert("CJK".to_owned(), egui::FontData::from_owned(font_data));
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

/// 从 category 中提取应用名（用于 App卸载 Tab 分组）
///
/// 例如 "WeChat (卸载)" -> Some("WeChat")，"WeChat 数据" -> Some("WeChat")。
/// 如果 category 不符合已知的子项后缀，返回 None。
pub(crate) fn extract_app_name(category: &str) -> Option<&str> {
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
pub(crate) fn uninstall_child_title(category: &str, lang_en: bool) -> String {
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

/// App 卸载子项类型在徽章中的简称
///
/// 与 uninstall_child_title 对应，但使用更短的词汇以匹配设计稿胶囊。
pub(crate) fn uninstall_child_badge_label(category: &str, lang_en: bool) -> String {
    if category.ends_with(" (卸载)") {
        if lang_en {
            "App".to_string()
        } else {
            "应用".to_string()
        }
    } else if category.ends_with(" 数据") {
        if lang_en {
            "Data".to_string()
        } else {
            "数据".to_string()
        }
    } else if category.ends_with(" 缓存") {
        if lang_en {
            "Cache".to_string()
        } else {
            "缓存".to_string()
        }
    } else {
        uninstall_child_title(category, lang_en)
    }
}

/// 渲染支持未选 / 部分选中 / 全选三种状态的自定义复选框
///
/// state: 0=未选, 1=部分选中（短横线）, 2=全选（对勾）
pub(crate) fn render_custom_checkbox_tri(
    ui: &mut egui::Ui,
    state: u8,
    enabled: bool,
) -> egui::Response {
    // state: 0=未选, 1=部分选中, 2=全选
    let state = match state {
        2 => widgets::Check::On,
        1 => widgets::Check::Partial,
        _ => widgets::Check::Off,
    };
    widgets::checkbox(ui, state, enabled)
}

/// 将 App 卸载 Tab 的 ScanItem 按应用名分组
///
/// 输入为过滤后的原始索引列表，输出为 (应用名, 原始索引列表) 的分组列表。
pub(crate) fn build_uninstall_groups(
    items: &[ScanItem],
    filtered_indices: &[usize],
) -> Vec<(String, Vec<usize>)> {
    let mut groups: Vec<(String, Vec<usize>)> = Vec::new();
    let mut group_map: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    // 无主残留（提不出应用名，如 App残留/残留缓存/废纸篓残留等）合并为
    // 一个可展开的聚合组，避免每个残留独立成行占满列表。
    let mut orphan_indices: Vec<usize> = Vec::new();
    for &idx in filtered_indices {
        let item = &items[idx];
        if let Some(app_name) = extract_app_name(&item.category) {
            let gidx = *group_map.entry(app_name.to_string()).or_insert_with(|| {
                groups.push((app_name.to_string(), Vec::new()));
                groups.len() - 1
            });
            groups[gidx].1.push(idx);
        } else {
            orphan_indices.push(idx);
        }
    }
    if !orphan_indices.is_empty() {
        groups.push(("App残留".to_string(), orphan_indices));
    }
    groups
}

/// 统计 App 卸载页底部胶囊要展示的数字：应用数、总大小、各推荐等级的 (应用数, 大小)
///
/// 单独抽出来有两个原因：
///   1. 可测 —— 之前这段埋在渲染函数里，"CacheOnly 归入 Safe" 这类口径没有测试能锁；
///   2. 调用方手里才有 `&[ScanItem]`，把结果算好传下去可以避免同时持有
///      `&mut App` 和 `&App::results`（借用冲突）。
pub(crate) fn uninstall_pill_stats(
    items: &[ScanItem],
    groups: &[(String, Vec<usize>)],
) -> (
    usize,
    u64,
    std::collections::HashMap<Recommend, (usize, u64)>,
) {
    let mut stats: std::collections::HashMap<Recommend, (usize, u64)> =
        std::collections::HashMap::new();
    for (_, indices) in groups {
        let mut seen = std::collections::HashSet::new();
        for &idx in indices {
            if let Some(item) = items.get(idx) {
                // CacheOnly 在 UI 中归入 Safe
                let rec_key = if item.recommend == Recommend::CacheOnly {
                    Recommend::Safe
                } else {
                    item.recommend
                };
                let entry = stats.entry(rec_key).or_insert((0, 0));
                entry.1 += item.size_bytes;
                if seen.insert(rec_key) {
                    entry.0 += 1;
                }
            }
        }
    }
    let total_apps = groups.len();
    let total_size: u64 = items.iter().map(|i| i.size_bytes).sum();
    (total_apps, total_size, stats)
}

/// 渲染 App 卸载 Tab 的完整内容区
///
/// 严格按设计稿实现：顶部分类胶囊 + 操作工具栏，下方是单一垂直滚动列表，
/// 每个应用以可展开/收起的卡片呈现，卡片内展示子项（应用数据 / 缓存 / 应用本体）。
pub(crate) fn render_app_uninstall_panel(
    ui: &mut egui::Ui,
    app: &mut App,
    _scan_rx: &mut Option<mpsc::Receiver<ScanMessage>>,
    groups_unfiltered: &[(String, Vec<usize>)],
) {
    let tab_idx = app.tab_index();
    let items = &app.results[tab_idx];

    // App 卸载 Tab 没有分类标签栏，避免从其他 Tab 带入的 filter_category
    // 把全部项过滤掉而界面上又无入口清除。
    if app.filter_category.is_some() {
        app.filter_category = None;
    }

    let recommend_filter = app.app_uninstall_recommend_filter;
    // 分组由上层统一构建后传入。
    //
    // 之前每帧要构建 3 次：本函数 1 次，内部 render_app_uninstall_filter_pills
    // 再 1 次，render_gui 的副标题还要 1 次 —— 输入完全一样，纯浪费。
    // 这里 to_vec() 是必须的：本函数会按推荐等级 retain + 排序，不能改到
    // 上层那份（pills 要拿未过滤的分组算统计）。
    let mut groups = groups_unfiltered.to_vec();

    // 按推荐等级过滤应用分组
    if let Some(rec_filter) = recommend_filter {
        groups.retain(|(_, indices)| {
            indices.iter().any(|&idx| {
                let item_rec = items[idx].recommend;
                item_rec == rec_filter
                    || (rec_filter == crate::scanner::Recommend::Safe
                        && item_rec == crate::scanner::Recommend::CacheOnly)
            })
        });
    }

    // 按应用总大小降序
    groups.sort_by(|(_, a), (_, b)| {
        let size_a: u64 = a.iter().map(|&i| items[i].size_bytes).sum();
        let size_b: u64 = b.iter().map(|&i| items[i].size_bytes).sum();
        size_b.cmp(&size_a)
    });

    // 统计必须在 items 借用有效期内算完 —— 之后要独占借用 app 去渲染胶囊
    let (pill_apps, pill_size, pill_stats) = uninstall_pill_stats(items, groups_unfiltered);

    // 即使分组为空也要渲染过滤胶囊，否则用户无法看到/清除已激活的推荐过滤条件
    render_app_uninstall_filter_pills(ui, app, pill_apps, pill_size, &pill_stats);
    ui.add_space(10.0);

    if groups.is_empty() {
        ui.vertical_centered(|ui| {
            ui.add_space(40.0);
            ui.label(
                egui::RichText::new(app.t("no_match"))
                    .size(14.0)
                    .color(theme::text_3()),
            );
        });
        return;
    }

    render_app_uninstall_action_bar(ui, app, &groups);
    ui.add_space(10.0);

    // 不再用 set_min_height(视口高度-2)：内容高度恰在滚动条出现临界点时，
    // 滚动条出现→宽度变窄→内容重排变高→滚动条消失→宽度变宽……每帧循环，
    // 正是"扫描完成后列表仍抖动"的渲染层来源。去掉临界写法后，内容高度
    // 即真实高度：内容超视口时滚动条恒定（宽度稳定），不足时自适应。
    egui::ScrollArea::vertical()
        .id_salt("app_uninstall_list")
        .auto_shrink([false; 2])
        .show(ui, |ui| {
            for (group_name, indices) in &groups {
                render_app_uninstall_group_card(ui, app, group_name, &indices.clone());
                ui.add_space(12.0);
            }
        });
}

/// 渲染单个应用分组卡片（可展开/收起）
pub(crate) fn render_app_uninstall_group_card(
    ui: &mut egui::Ui,
    app: &mut App,
    group_name: &str,
    indices: &[usize],
) {
    let tab_idx = app.tab_index();
    let total_size: u64 = indices
        .iter()
        .map(|&i| app.results[tab_idx][i].size_bytes)
        .sum();
    let is_expanded = app.expanded_app_groups.contains(group_name);

    // 按设计稿顺序排列子项：应用数据 > 缓存与日志 > 应用本体
    let mut sorted_indices: Vec<usize> = indices.to_vec();
    sorted_indices.sort_by_key(|&idx| {
        let cat = &app.results[tab_idx][idx].category;
        if cat.ends_with(" 数据") {
            0u8
        } else if cat.ends_with(" 缓存") {
            1u8
        } else if cat.ends_with(" (卸载)") {
            2u8
        } else {
            3u8
        }
    });

    // 可删除子项的选中状态，用于分组级三态复选框
    let deletable_indices: Vec<usize> = sorted_indices
        .iter()
        .copied()
        .filter(|&i| app.results[tab_idx][i].deletable)
        .collect();
    let selected_count = deletable_indices
        .iter()
        .filter(|&&i| app.results[tab_idx][i].selected)
        .count();
    let group_state = if deletable_indices.is_empty() || selected_count == 0 {
        0
    } else if selected_count == deletable_indices.len() {
        2
    } else {
        1
    };

    // 卡片外框：白底、浅灰边框、12px 圆角
    let card_frame = egui::Frame::none()
        .fill(theme::surface())
        .stroke(egui::Stroke::new(1.0_f32, theme::line()))
        .rounding(egui::Rounding::same(12.0))
        .inner_margin(egui::Margin::same(0.0));

    let card_resp = card_frame.show(ui, |ui| {
        ui.vertical(|ui| {
            // ========== 卡片头部 ==========
            let header_margin = egui::Margin::symmetric(14.0, 12.0);
            egui::Frame::none()
                .inner_margin(header_margin)
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.set_min_width(ui.available_width());

                        // 分组复选框
                        let cb_resp = render_custom_checkbox_tri(
                            ui,
                            group_state,
                            !deletable_indices.is_empty(),
                        );
                        if cb_resp.clicked() && !deletable_indices.is_empty() {
                            let select_all = group_state != 2;
                            for &idx in &deletable_indices {
                                app.results[tab_idx][idx].selected = select_all;
                            }
                        }

                        ui.add_space(10.0);

                        // 展开/收起箭头
                        let arrow_ic = if is_expanded {
                            icons::Icon::ChevronUp
                        } else {
                            icons::Icon::ChevronDown
                        };
                        let arrow_color = if is_expanded {
                            theme::brand()
                        } else {
                            theme::text_3()
                        };
                        let (arrow_rect, arrow_resp) =
                            ui.allocate_exact_size(egui::vec2(22.0, 22.0), egui::Sense::click());
                        if ui.is_rect_visible(arrow_rect) {
                            icons::paint(
                                ui.painter(),
                                arrow_rect.shrink(3.0),
                                arrow_ic,
                                arrow_color,
                            );
                        }
                        if arrow_resp.clicked() {
                            if is_expanded {
                                app.expanded_app_groups.remove(group_name);
                            } else {
                                app.expanded_app_groups.insert(group_name.to_string());
                            }
                        }

                        ui.add_space(8.0);

                        // 应用名 + 路径（点击应用名也可展开/收起整个卡片）
                        ui.vertical(|ui| {
                            ui.set_min_width(120.0);
                            let name_resp = ui.add(
                                egui::Label::new(
                                    egui::RichText::new(group_name).size(13.0).strong(),
                                )
                                .sense(egui::Sense::click()),
                            );
                            if name_resp.clicked() {
                                if is_expanded {
                                    app.expanded_app_groups.remove(group_name);
                                } else {
                                    app.expanded_app_groups.insert(group_name.to_string());
                                }
                            }
                            let bundle_idx = indices
                                .iter()
                                .find(|&&i| app.results[tab_idx][i].category.ends_with(" (卸载)"));
                            let path_display = if let Some(&idx) = bundle_idx {
                                truncate_path(&app.results[tab_idx][idx].path, 40)
                            } else if let Some(&idx) = indices.first() {
                                truncate_path(&app.results[tab_idx][idx].path, 40)
                            } else {
                                String::new()
                            };
                            if !path_display.is_empty() {
                                ui.colored_label(
                                    theme::text_3(),
                                    egui::RichText::new(&path_display).size(11.0).monospace(),
                                );
                            }
                        });

                        // 右侧：类型徽章 + 总大小
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            let size_str = if total_size == 0 {
                                "—".to_string()
                            } else {
                                format_size(total_size)
                            };
                            ui.colored_label(
                                theme::text(),
                                egui::RichText::new(size_str)
                                    .size(13.0)
                                    .strong()
                                    .monospace(),
                            );

                            ui.add_space(10.0);

                            // 类型徽章：数据 / 缓存 / 应用
                            let mut type_counts: std::collections::HashMap<
                                String,
                                (crate::scanner::Recommend, usize),
                            > = std::collections::HashMap::new();
                            for &idx in sorted_indices.iter() {
                                let title = uninstall_child_badge_label(
                                    &app.results[tab_idx][idx].category,
                                    app.lang_en,
                                );
                                let rec = app.results[tab_idx][idx].recommend;
                                let entry = type_counts.entry(title).or_insert((rec, 0));
                                entry.1 += 1;
                            }
                            let mut type_badges: Vec<(String, crate::scanner::Recommend, usize)> =
                                type_counts
                                    .into_iter()
                                    .map(|(t, (r, c))| (t, r, c))
                                    .collect();
                            let order = ["数据", "缓存", "应用", "Data", "Cache", "App"];
                            type_badges.sort_by(|a, b| {
                                let pos_a =
                                    order.iter().position(|&o| o == a.0).unwrap_or(usize::MAX);
                                let pos_b =
                                    order.iter().position(|&o| o == b.0).unwrap_or(usize::MAX);
                                pos_a.cmp(&pos_b)
                            });
                            for (title, rec, count) in type_badges.iter().rev() {
                                let text = format!("{} · {}", title, count);
                                let (fg, bg, _) = recommend_badge_colors(rec);
                                render_status_badge(ui, &text, fg, bg);
                                ui.add_space(6.0);
                            }
                        });
                    });
                });

            // ========== 展开的子项列表 ==========
            if is_expanded {
                egui::Frame::none()
                    .inner_margin(egui::Margin {
                        left: 44.0,
                        right: 14.0,
                        top: 0.0,
                        bottom: 12.0,
                    })
                    .show(ui, |ui| {
                        ui.vertical(|ui| {
                            ui.set_min_width(ui.available_width());
                            for &display_idx in sorted_indices.iter() {
                                if let Some(toggled) = render_app_uninstall_child_row(
                                    ui,
                                    &app.results[tab_idx][display_idx],
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
                    });
            }
        });
    });

    // 整个卡片区域辅助记录，避免未使用警告（card_resp 已消费布局）
    let _ = card_resp.response.rect;
}

/// 渲染 App 卸载 Tab 的底部分类过滤胶囊
///
/// 设计稿：全部 / Safe / Caution / Advanced 四个胶囊，其中 Safe/Caution/Advanced
/// 的头部为对应颜色的徽章，全部胶囊头部为普通文本。
#[allow(clippy::too_many_arguments)]
pub(crate) fn render_app_uninstall_filter_pills(
    ui: &mut egui::Ui,
    app: &mut App,
    total_apps: usize,
    total_size: u64,
    stats: &std::collections::HashMap<crate::scanner::Recommend, (usize, u64)>,
) {
    struct PillInfo {
        rec: Option<crate::scanner::Recommend>,
        label: &'static str,
        is_badge: bool,
        count: usize,
        size: u64,
    }

    let pills: [PillInfo; 4] = [
        PillInfo {
            rec: None,
            label: app.t("all"),
            is_badge: false,
            count: total_apps,
            size: total_size,
        },
        PillInfo {
            rec: Some(crate::scanner::Recommend::Safe),
            label: "Safe",
            is_badge: true,
            count: stats
                .get(&crate::scanner::Recommend::Safe)
                .map(|s| s.0)
                .unwrap_or(0),
            size: stats
                .get(&crate::scanner::Recommend::Safe)
                .map(|s| s.1)
                .unwrap_or(0),
        },
        PillInfo {
            rec: Some(crate::scanner::Recommend::Caution),
            label: "Caution",
            is_badge: true,
            count: stats
                .get(&crate::scanner::Recommend::Caution)
                .map(|s| s.0)
                .unwrap_or(0),
            size: stats
                .get(&crate::scanner::Recommend::Caution)
                .map(|s| s.1)
                .unwrap_or(0),
        },
        PillInfo {
            rec: Some(crate::scanner::Recommend::Advanced),
            label: "Advanced",
            is_badge: true,
            count: stats
                .get(&crate::scanner::Recommend::Advanced)
                .map(|s| s.0)
                .unwrap_or(0),
            size: stats
                .get(&crate::scanner::Recommend::Advanced)
                .map(|s| s.1)
                .unwrap_or(0),
        },
    ];

    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        for pill in pills {
            let is_active = app.app_uninstall_recommend_filter == pill.rec;
            let (fg, bg, stroke) = if is_active {
                (egui::Color32::WHITE, theme::brand(), egui::Stroke::NONE)
            } else {
                (
                    theme::text(),
                    theme::surface(),
                    egui::Stroke::new(1.0_f32, theme::line()),
                )
            };

            let value_text = format!("{} · {}", pill.count, format_size(pill.size));

            let header_font = egui::FontId::new(11.0, egui::FontFamily::Proportional);
            let value_font = egui::FontId::new(14.0, egui::FontFamily::Proportional);
            let painter = ui.painter();

            let (header_size, badge_fg, badge_bg, header_galley) = if pill.is_badge {
                let (bfg, bbg, _) = recommend_badge_colors(&pill.rec.unwrap());
                let galley = painter.layout(
                    pill.label.to_string(),
                    header_font.clone(),
                    bfg,
                    f32::INFINITY,
                );
                let badge_padding = egui::vec2(8.0, 3.0);
                let size = galley.size() + badge_padding * 2.0;
                (size, bfg, bbg, Some(galley))
            } else {
                let galley = painter.layout(
                    pill.label.to_string(),
                    header_font.clone(),
                    fg,
                    f32::INFINITY,
                );
                (galley.size(), fg, bg, Some(galley))
            };

            let value_galley = painter.layout(value_text, value_font.clone(), fg, f32::INFINITY);

            let padding = egui::vec2(14.0, 10.0);
            let width = header_size.x.max(value_galley.size().x) + padding.x * 2.0 + 4.0;
            let height = header_size.y + value_galley.size().y + 2.0 + padding.y * 2.0;

            let (rect, response) =
                ui.allocate_exact_size(egui::vec2(width, height), egui::Sense::click());

            if ui.is_rect_visible(rect) {
                let painter = ui.painter();
                let rounding = egui::Rounding::same(999.0);
                painter.rect_filled(rect, rounding, bg);
                if stroke != egui::Stroke::NONE {
                    painter.rect_stroke(rect, rounding, stroke);
                }

                let total_h = header_size.y + value_galley.size().y + 2.0;
                let start_y = rect.center().y - total_h / 2.0;
                let value_x = rect.center().x - value_galley.size().x / 2.0;

                if pill.is_badge {
                    let badge_x = rect.center().x - header_size.x / 2.0;
                    painter.rect_filled(
                        egui::Rect::from_min_size(egui::pos2(badge_x, start_y), header_size),
                        egui::Rounding::same(999.0),
                        badge_bg,
                    );
                    if let Some(galley) = header_galley {
                        painter.galley(egui::pos2(badge_x + 8.0, start_y + 3.0), galley, badge_fg);
                    }
                } else if let Some(galley) = header_galley {
                    let header_x = rect.center().x - header_size.x / 2.0;
                    painter.galley(egui::pos2(header_x, start_y), galley, fg);
                }

                painter.galley(
                    egui::pos2(value_x, start_y + header_size.y + 2.0),
                    value_galley,
                    fg,
                );
            }

            if response.clicked() {
                app.app_uninstall_recommend_filter = if is_active { None } else { pill.rec };
            }
        }
    });
}

/// 渲染 App 卸载 Tab 的操作按钮栏
pub(crate) fn render_app_uninstall_action_bar(
    ui: &mut egui::Ui,
    app: &mut App,
    groups: &[(String, Vec<usize>)],
) {
    let tab_idx = app.tab_index();
    let filtered_indices = app.filtered_indices();

    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;

        let btn_height = 28.0;
        let btn_rounding = egui::Rounding::same(8.0);

        // 一键推荐清理：选中所有 Safe / CacheOnly 项
        if ui
            .add(
                egui::Button::new(
                    egui::RichText::new(app.t("one_click_recommended_clean"))
                        .size(12.0)
                        .color(theme::text()),
                )
                .stroke(egui::Stroke::new(1.0_f32, theme::line()))
                .fill(theme::surface())
                .rounding(btn_rounding)
                .min_size([0.0, btn_height].into()),
            )
            .clicked()
        {
            for &idx in &filtered_indices {
                let item = &app.results[tab_idx][idx];
                if item.deletable
                    && matches!(
                        item.recommend,
                        crate::scanner::Recommend::Safe | crate::scanner::Recommend::CacheOnly
                    )
                {
                    app.results[tab_idx][idx].selected = true;
                }
            }
        }

        // 全选
        if ui
            .add(
                egui::Button::new(egui::RichText::new(app.t("select_all")).size(12.0))
                    .stroke(egui::Stroke::NONE)
                    .fill(egui::Color32::TRANSPARENT)
                    .rounding(btn_rounding)
                    .min_size([0.0, btn_height].into()),
            )
            .clicked()
        {
            for &idx in &filtered_indices {
                if app.results[tab_idx][idx].deletable {
                    app.results[tab_idx][idx].selected = true;
                }
            }
        }

        // 取消全选
        if ui
            .add(
                egui::Button::new(egui::RichText::new(app.t("deselect_all")).size(12.0))
                    .stroke(egui::Stroke::NONE)
                    .fill(egui::Color32::TRANSPARENT)
                    .rounding(btn_rounding)
                    .min_size([0.0, btn_height].into()),
            )
            .clicked()
        {
            for &idx in &filtered_indices {
                if app.results[tab_idx][idx].deletable {
                    app.results[tab_idx][idx].selected = false;
                }
            }
        }

        // 右侧：展开/收起全部分组
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let expanded_count = groups
                .iter()
                .filter(|(name, _)| app.expanded_app_groups.contains(name))
                .count();
            let all_expanded = expanded_count == groups.len();
            let label = if all_expanded {
                app.tf("collapse_n_groups", &[&groups.len().to_string()])
            } else {
                app.tf("expand_n_groups", &[&groups.len().to_string()])
            };
            if ui
                .add(
                    egui::Button::new(egui::RichText::new(label).size(12.0).color(theme::text_3()))
                        .stroke(egui::Stroke::NONE)
                        .fill(egui::Color32::TRANSPARENT)
                        .rounding(btn_rounding)
                        .min_size([0.0, btn_height].into()),
                )
                .clicked()
            {
                if all_expanded {
                    app.expanded_app_groups.clear();
                } else {
                    for (name, _) in groups {
                        app.expanded_app_groups.insert(name.clone());
                    }
                }
            }
        });
    });
}

/// 渲染 App 卸载 Tab 中的子项行
///
/// 用于父卡片内部的「应用数据 / 缓存与日志 / 应用本体」行。
/// 返回被点击切换选中的原始索引。
pub(crate) fn render_app_uninstall_child_row(
    ui: &mut egui::Ui,
    item: &ScanItem,
    index: usize,
    app: &App,
) -> Option<usize> {
    let mut toggled: Option<usize> = None;
    let locked = !item.deletable;
    let selected = item.selected;

    let child_frame = egui::Frame::none()
        .fill(theme::surface_3())
        .inner_margin(egui::Margin::symmetric(12.0, 10.0))
        .stroke(egui::Stroke::new(1.0_f32, theme::line()))
        .rounding(egui::Rounding::same(8.0));

    let row_resp = child_frame.show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.set_min_width(ui.available_width());

            // 自定义复选框
            let cb_response = render_custom_checkbox(ui, selected && !locked, !locked);
            if cb_response.clicked() && !locked {
                toggled = Some(index);
            }

            ui.add_space(10.0);

            // 中间信息区
            ui.vertical(|ui| {
                ui.set_min_width(ui.available_width());

                // 标题行：标题 + 大小
                ui.horizontal(|ui| {
                    let title = uninstall_child_title(&item.category, app.lang_en);
                    ui.add(
                        egui::Label::new(egui::RichText::new(title).size(13.0).strong().color(
                            if locked {
                                theme::text_3()
                            } else {
                                theme::text()
                            },
                        ))
                        .truncate(),
                    );

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let size_str = if item.size_bytes == 0 {
                            "—".to_string()
                        } else {
                            format_size(item.size_bytes)
                        };
                        ui.colored_label(
                            if locked {
                                theme::text_3()
                            } else {
                                theme::text()
                            },
                            egui::RichText::new(&size_str)
                                .size(13.0)
                                .strong()
                                .monospace(),
                        );
                    });
                });

                // 路径 / 摘要
                let path_display = truncate_path(&item.path, 80);
                ui.colored_label(
                    theme::text_3(),
                    egui::RichText::new(&path_display).size(11.0).monospace(),
                );

                // 状态徽章 + 描述
                ui.horizontal(|ui| {
                    if locked {
                        render_status_badge(
                            ui,
                            app.t("badge_undeletable"),
                            theme::text_3(),
                            theme::surface_3(),
                        );
                    } else {
                        let (fg, bg, label) = recommend_badge_colors(&item.recommend);
                        render_status_badge(ui, label, fg, bg);
                    }
                    ui.add_space(6.0);
                    let desc = i18n::translate_description(&item.description, app.lang_en);
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(&desc).size(11.0).color(theme::text_2()),
                        )
                        .truncate(),
                    );
                });
            });
        });
    });

    // Frame 默认不响应点击，需手动分配整行可点击区域
    let row_click = ui.interact(
        row_resp.response.rect,
        egui::Id::new(("app_uninstall_child_row_click", index)),
        egui::Sense::click(),
    );
    if row_click.clicked() && !locked {
        toggled = Some(index);
    }

    ui.add_space(6.0);

    toggled
}

/// 渲染 App 卸载 Tab 的底部删除栏
pub(crate) fn render_app_uninstall_footer(ui: &mut egui::Ui, app: &mut App) {
    let tab_idx = app.tab_index();
    let items = &app.results[tab_idx];

    // 统计选中的应用数（至少选中一个子项）与选中总大小
    let mut selected_app_names = std::collections::HashSet::new();
    let mut selected_size: u64 = 0;
    for item in items.iter() {
        if item.selected && item.deletable {
            selected_size += item.size_bytes;
            if let Some(app_name) = extract_app_name(&item.category) {
                selected_app_names.insert(app_name.to_string());
            }
        }
    }
    let selected_apps = selected_app_names.len();

    egui::Frame::none()
        .fill(theme::surface())
        .stroke(egui::Stroke::new(1.0_f32, theme::line()))
        .inner_margin(egui::Margin::symmetric(16.0, 10.0))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.colored_label(
                    theme::text_2(),
                    egui::RichText::new(app.tf(
                        "selected_apps_partial",
                        &[&selected_apps.to_string(), &format_size(selected_size)],
                    ))
                    .size(13.0),
                );

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let delete_enabled =
                        selected_size > 0 && matches!(app.confirm, ConfirmState::None);

                    // 删除按钮
                    let delete_btn = ui.add_enabled(
                        delete_enabled,
                        egui::Button::new(
                            egui::RichText::new(
                                app.tf("delete_with_size", &[&format_size(selected_size)]),
                            )
                            .color(egui::Color32::WHITE)
                            .size(13.0),
                        )
                        .fill(theme::danger())
                        .rounding(egui::Rounding::same(8.0))
                        .min_size([0.0, 32.0].into()),
                    );
                    if delete_btn.clicked() {
                        app.prepare_delete();
                    }

                    ui.add_space(8.0);

                    // 取消按钮：清空当前选中
                    if ui
                        .add(
                            egui::Button::new(
                                egui::RichText::new(app.t("cancel"))
                                    .size(13.0)
                                    .color(theme::text_2()),
                            )
                            .fill(egui::Color32::TRANSPARENT)
                            .stroke(egui::Stroke::NONE)
                            .rounding(egui::Rounding::same(8.0))
                            .min_size([0.0, 32.0].into()),
                        )
                        .clicked()
                    {
                        for item in app.results[tab_idx].iter_mut() {
                            if item.deletable {
                                item.selected = false;
                            }
                        }
                    }
                });
            });
        });
}

/// 从 category 中提取前缀（用于分类过滤标签页）
///
/// 例如 "Docker dangling 镜像" -> "Docker"，"Xcode DerivedData — ProjectA" -> "Xcode"。
pub(crate) fn category_prefix(category: &str) -> String {
    let delimiters: &[char] = &[' ', '—', '-', '/', '(', '（', '·'];
    category
        .split(delimiters)
        .next()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| category.to_string())
}

/// 渲染分类过滤标签
pub(crate) fn render_category_tab(
    ui: &mut egui::Ui,
    label: &str,
    count: Option<usize>,
    active: bool,
) -> egui::Response {
    let text = match count {
        Some(c) => format!("{} ({})", label, c),
        None => label.to_string(),
    };
    let text_color = if active {
        theme::brand()
    } else {
        theme::text_2()
    };
    let bottom_stroke = if active {
        egui::Stroke::new(2.0_f32, theme::brand())
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
pub(crate) fn recommend_badge_colors(
    rec: &Recommend,
) -> (egui::Color32, egui::Color32, &'static str) {
    match rec {
        Recommend::Safe => (theme::safe(), theme::safe_50(), "Safe"),
        Recommend::CacheOnly => (theme::cache(), theme::cache_50(), "Cache only"),
        Recommend::Caution => (theme::caution(), theme::caution_50(), "Caution"),
        Recommend::Advanced => (theme::danger(), theme::danger_50(), "Advanced"),
    }
}

/// 渲染状态徽章
pub(crate) fn render_status_badge(
    ui: &mut egui::Ui,
    text: &str,
    fg: egui::Color32,
    bg: egui::Color32,
) {
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
pub(crate) fn render_custom_checkbox(
    ui: &mut egui::Ui,
    checked: bool,
    enabled: bool,
) -> egui::Response {
    // 设计稿 3.2：16×16、圆角 4、禁用态灰底但**不隐藏**
    widgets::checkbox(
        ui,
        if checked {
            widgets::Check::On
        } else {
            widgets::Check::Off
        },
        enabled,
    )
}

/// 渲染设置页 Toggle 开关
pub(crate) fn render_settings_toggle(
    ui: &mut egui::Ui,
    value: &mut bool,
    enabled: bool,
) -> egui::Response {
    widgets::toggle(ui, value, enabled)
}

/// 渲染单个扫描项行（设计稿 3.4：48px 紧凑行）
///
/// 一行只放：**复选框 · 名称 + 等级徽标 · 路径 · 占比条 · 容量 · 展开**。
/// 说明文字（删了会怎样）放进 tooltip 与展开区，不挤在主行。
///
/// `total_size` 用于计算占比条（该项占当前列表总量的比例）。
///
/// 返回 (被点击切换选中的原始索引, 请求切换展开的路径)。
#[allow(clippy::too_many_arguments)]
pub(crate) fn render_scan_item_row(
    ui: &mut egui::Ui,
    item: &ScanItem,
    index: usize,
    app: &App,
    is_uninstall_tab: bool,
    associated_details: &std::collections::HashMap<String, Vec<(String, u64, String)>>,
    expanded_items: &std::collections::HashSet<String>,
    total_size: u64,
) -> (Option<usize>, Option<String>) {
    let mut toggled: Option<usize> = None;
    let mut expand_toggle: Option<String> = None;
    let mut cb_clicked = false;
    let mut expand_clicked = false;

    let locked = !item.deletable;
    let selected = item.selected;

    let has_details = is_uninstall_tab
        && !item.batch_paths.is_empty()
        && associated_details.contains_key(&item.path);
    let is_expanded = expanded_items.contains(&item.path);

    // 占比条：该项占当前列表总量的比例（没有它用户无法快速判断「哪个大」）
    let ratio = if total_size > 0 {
        item.size_bytes as f32 / total_size as f32
    } else {
        0.0
    };
    let bar_color = if locked {
        theme::text_4()
    } else {
        theme::recommend_fg(&item.recommend)
    };

    let row_frame = egui::Frame::none()
        .fill(if selected {
            theme::brand_50()
        } else if locked {
            theme::surface_2()
        } else {
            theme::surface()
        })
        .inner_margin(egui::Margin::symmetric(S4, 0.0));

    let row_resp = row_frame.show(ui, |ui| {
        ui.set_min_height(ROW_H);
        ui.spacing_mut().item_spacing.x = S3;
        ui.horizontal(|ui| {
            // 复选框：禁用项依然可见（设计稿 3.2：不要隐藏，要让用户看见不能选）
            let state = if selected && !locked {
                widgets::Check::On
            } else {
                widgets::Check::Off
            };
            let cb = widgets::checkbox(ui, state, !locked);
            let cb = if locked && !item.undeletable_reason.is_empty() {
                cb.on_hover_text(i18n::translate_undeletable_reason(
                    &item.undeletable_reason,
                    app.lang_en,
                ))
            } else {
                cb
            };
            cb_clicked = cb.clicked();
            if cb_clicked && !locked {
                toggled = Some(index);
            }

            // 中间信息区：占满除右侧固定列外的全部宽度
            let right_w = BAR_W + S3 + 76.0 + S2 + BTN_H_SM;
            let mid_w = (ui.available_width() - right_w).max(120.0);
            let title_text = i18n::translate_category(&item.category, app.lang_en);
            let path_display = truncate_path(&item.path, 90);
            ui.allocate_ui_with_layout(
                egui::vec2(mid_w, ROW_H),
                egui::Layout::centered_and_justified(egui::Direction::TopDown),
                |ui| {
                    ui.horizontal(|ui| {
                        ui.add(
                            egui::Label::new(
                                egui::RichText::new(&title_text).size(13.0).strong().color(
                                    if locked {
                                        theme::text_3()
                                    } else {
                                        theme::text()
                                    },
                                ),
                            )
                            .truncate(),
                        );
                        if locked {
                            widgets::badge(
                                ui,
                                app.t("badge_undeletable"),
                                theme::text_3(),
                                theme::surface_3(),
                                Some(icons::Icon::Shield),
                            );
                        } else {
                            widgets::recommend_badge(ui, &item.recommend, app.lang_en);
                        }
                    });
                    ui.colored_label(
                        theme::text_3(),
                        egui::RichText::new(&path_display).size(11.0).monospace(),
                    );
                },
            );

            // 占比条 + 容量 + 展开
            widgets::ratio_bar(ui, ratio, bar_color);
            let size_str = if item.size_bytes == 0 {
                "—".to_string()
            } else {
                format_size(item.size_bytes)
            };
            ui.colored_label(
                if locked {
                    theme::text_4()
                } else {
                    theme::text()
                },
                egui::RichText::new(&size_str)
                    .size(13.0)
                    .strong()
                    .monospace(),
            );
            if has_details {
                let ic = if is_expanded {
                    icons::Icon::ChevronUp
                } else {
                    icons::Icon::ChevronDown
                };
                if widgets::icon_button(ui, ic, BTN_H_SM).clicked() {
                    expand_clicked = true;
                    expand_toggle = Some(item.path.clone());
                }
            } else {
                ui.add_space(BTN_H_SM);
            }
        });
    });

    // 整行点击切换选中（锁定项、复选框、展开按钮除外）
    let row_rect = row_resp.response.rect;
    let row_click = ui.interact(
        row_rect,
        egui::Id::new(("row_click", index)),
        egui::Sense::click(),
    );
    if row_click.clicked() && !locked && !cb_clicked && !expand_clicked {
        toggled = Some(index);
    }

    // 说明文字放 tooltip，不占主行
    let desc = i18n::translate_description(&item.description, app.lang_en);
    if !desc.is_empty() {
        row_click.on_hover_text(desc);
    }

    // 行底 1px 分割线（用线而不是间距，避免列表松散）
    ui.painter().line_segment(
        [
            egui::pos2(row_rect.min.x, row_rect.max.y),
            egui::pos2(row_rect.max.x, row_rect.max.y),
        ],
        egui::Stroke::new(1.0_f32, theme::line()),
    );

    // 展开时显示关联文件明细
    if has_details && is_expanded {
        if let Some(details) = associated_details.get(&item.path) {
            for (detail_path, detail_size, detail_label) in details {
                egui::Frame::none()
                    .fill(theme::surface_2())
                    .inner_margin(egui::Margin::symmetric(14.0, 6.0))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.add_space(42.0); // 缩进对齐主行文字
                            ui.colored_label(
                                theme::brand(),
                                egui::RichText::new(detail_label).size(11.0),
                            );
                            let sz_str = if *detail_size == 0 {
                                "—".to_string()
                            } else {
                                format_size(*detail_size)
                            };
                            ui.colored_label(
                                theme::text_2(),
                                egui::RichText::new(&sz_str).size(12.0).monospace(),
                            );
                            ui.add_space(S2);
                            let dp = truncate_path(detail_path, 55);
                            ui.colored_label(
                                theme::text_3(),
                                egui::RichText::new(&dp).size(10.0).monospace(),
                            );
                        });
                    });
            }
        }
    }

    (toggled, expand_toggle)
}

/// 空状态页（设计稿 5.7）
///
/// 「从未扫描」与「扫描完成但没有可清理项」是两种完全不同的情绪：
/// 前者要降低启动门槛（一句话说明不会删文件 + 主按钮），
/// 后者要给正反馈（你的机器很干净）。合并成一句"暂无数据"是偷懒。
pub(crate) fn render_empty_state(
    ui: &mut egui::Ui,
    app: &mut App,
    scan_rx: &mut Option<mpsc::Receiver<ScanMessage>>,
) {
    let tab_idx = app.tab_index();
    let never_scanned = matches!(app.scan_states[tab_idx], ScanState::Idle);

    if never_scanned {
        let scan_label = app.t("scan").to_string();
        if widgets::state_page(
            ui,
            icons::Icon::Box,
            theme::text_4(),
            theme::surface_3(),
            app.t("empty_never_scanned"),
            app.t("empty_never_scanned_desc"),
            Some((&scan_label, icons::Icon::Search)),
        ) {
            start_scan(app, scan_rx);
        }
    } else {
        let log_label = app.t("empty_view_log").to_string();
        // 超时跳过的 Tab：显示超时提示而不是误导性的"很干净"
        let timed_out = app.scan_current_path.starts_with('⚠');
        let (icon, fg, bg, title, desc) = if timed_out {
            (
                icons::Icon::Alert,
                theme::caution(),
                theme::caution_50(),
                app.t("empty_scan_timeout"),
                app.t("empty_scan_timeout_desc"),
            )
        } else {
            (
                icons::Icon::Check,
                theme::safe(),
                theme::safe_50(),
                app.t("empty_all_clean"),
                app.t("empty_all_clean_desc"),
            )
        };
        if widgets::state_page(
            ui,
            icon,
            fg,
            bg,
            title,
            desc,
            Some((&log_label, icons::Icon::Terminal)),
        ) {
            let log_dir = logger::log_dir();
            #[cfg(target_os = "macos")]
            let _ = std::process::Command::new("open").arg(&log_dir).spawn();
            #[cfg(target_os = "windows")]
            let _ = std::process::Command::new("explorer").arg(&log_dir).spawn();
            #[cfg(not(any(target_os = "macos", target_os = "windows")))]
            let _ = std::process::Command::new("xdg-open").arg(&log_dir).spawn();
        }
    }
}

/// 渲染 GUI 主界面
pub(crate) fn render_gui(
    ctx: &egui::Context,
    app: &mut App,
    scan_rx: &mut Option<mpsc::Receiver<ScanMessage>>,
    delete_rx: &mut Option<mpsc::Receiver<DeleteMessage>>,
) {
    // 配色：用户切了深色就在这里生效（模式没变时是空操作）
    theme::sync_visuals(ctx, app.theme_mode());

    // 动态更新窗口标题（跟随语言切换）
    ctx.send_viewport_cmd(egui::ViewportCommand::Title(
        app.t("window_title").to_string(),
    ));

    // App 卸载 Tab 的分组每帧要用到 3 次（侧栏副标题、面板本体、底部胶囊），
    // 三处的输入完全一致 —— 在这里构建一次往下传，避免重复 O(n) 分组。
    // （Scanning / Idle 时 AppUninstall 列表是空的，构建成本可忽略，
    //   但为避免每帧多一次分配，非该 Tab 时直接给空 Vec。）
    let uninstall_groups: Vec<(String, Vec<usize>)> = if app.tab == Tab::AppUninstall {
        let idx = app.tab_index();
        // 「显示受保护/系统项」默认关：过滤掉系统保护/ACL 等任何权限都无法
        // 删除的项，避免用户反复勾选 → Touch ID 授权 → 删除失败 → 重试。
        let show_protected = app.settings_show_protected_items;
        let mut indices: Vec<usize> = app
            .filtered_indices()
            .into_iter()
            .filter(|&i| show_protected || app.results[idx][i].deletable)
            .collect();
        indices.sort_unstable();
        build_uninstall_groups(&app.results[idx], &indices)
    } else {
        Vec::new()
    };

    // 轮询后台更新检查结果
    app.poll_update();

    // ========== 更新提示横幅（顶部）==========
    if !app.update_dismissed {
        if let Some(info) = app.update_available.clone() {
            egui::TopBottomPanel::top("update_banner").show(ctx, |ui| {
                let mut msg = if app.lang_en {
                    format!("New version v{} is available", info.version)
                } else {
                    format!("发现新版本 v{}", info.version)
                };
                // updater 抓了 Release 摘要前 3 行，此前字段从未被读取 ——
                // 横幅只有一行空间，这里带第一行，避免抓了不用。
                if let Some(first) = info.notes.lines().next() {
                    if !first.trim().is_empty() {
                        msg.push_str(&format!(" · {}", first.trim()));
                    }
                }
                widgets::banner(
                    ui,
                    widgets::BannerKind::Info,
                    icons::Icon::ArrowUp,
                    &msg,
                    |ui| {
                        if widgets::icon_button(ui, icons::Icon::X, BTN_H_SM).clicked() {
                            app.update_dismissed = true;
                        }
                        // C-2：有匹配本机的产物就真下载（含 sha256 校验），
                        // 没有才退回"打开 Release 页面"。此前只有后者，
                        // 等于把"检查更新"做成了"跳网页"。
                        let has_asset = info.asset.is_some();
                        let label = if app.update_downloading {
                            if app.lang_en {
                                "Downloading…"
                            } else {
                                "下载中…"
                            }
                        } else if has_asset {
                            if app.lang_en {
                                "Download & install"
                            } else {
                                "下载并安装"
                            }
                        } else if app.lang_en {
                            "Download"
                        } else {
                            "前往下载"
                        };
                        // 结果行（成功/失败原文）显示在按钮上方
                        if let Some(ref r) = app.update_result {
                            ui.colored_label(theme::text_2(), egui::RichText::new(r).size(11.0));
                        }
                        let clicked =
                            widgets::button(ui, None, label, widgets::Btn::Primary, BTN_H_SM)
                                .clicked();
                        if clicked && !app.update_downloading && !app.start_update_download() {
                            open_url(&info.url);
                        }
                    },
                );
            });
        }
    }

    // ========== 磁盘告警横幅（顶部）==========
    let (alert_level, _alert_color, free_pct) = app.disk_alert_level();
    if alert_level >= 1 {
        egui::TopBottomPanel::top("disk_alert_banner").show(ctx, |ui| {
            let (kind, msg_key) = match alert_level {
                3 => (widgets::BannerKind::Crit, "disk_alert_critical"),
                2 => (widgets::BannerKind::Warn, "disk_alert_warning"),
                _ => (widgets::BannerKind::Info, "disk_alert_notice"),
            };
            let free_gb = app.disk_free as f64 / 1_073_741_824.0;
            let msg = app.tf(
                msg_key,
                &[&format!("{:.1}", free_pct), &format!("{:.1}", free_gb)],
            );

            widgets::banner(ui, kind, icons::Icon::Alert, &msg, |ui| {
                if alert_level >= 2
                    && widgets::button(
                        ui,
                        None,
                        app.t("disk_alert_clean_now"),
                        widgets::Btn::Secondary,
                        BTN_H_SM,
                    )
                    .clicked()
                {
                    // 跳转到开发者缓存 Tab（通常回收空间最大）
                    //
                    // 必须有守卫：确认框弹出或删除进行中时切 Tab，会让
                    // 确认框的统计（依赖当前 Tab）与 pending_delete 错位。
                    // 导航栏点击是带这个守卫的，这里之前漏了。
                    if matches!(app.confirm, ConfirmState::None) && !app.any_scanning() {
                        app.tab = crate::app::Tab::DevCache;
                        app.list_index = 0;
                    }
                }
            });
        });
    }

    // ========== 底部 Footer ==========
    // App卸载 / 概览 Tab 自带固定底部删除栏，避免与全局 Footer 重复
    let app_uninstall_show_footer = app.tab == crate::app::Tab::AppUninstall
        && !app.tab_scanning(app.tab_index())
        && !app.results[app.tab_index()].is_empty();
    let overview_show_footer = app.tab == crate::app::Tab::Overview && !app.any_scanning();
    if app_uninstall_show_footer {
        egui::TopBottomPanel::bottom("app_uninstall_footer")
            .frame(egui::Frame::side_top_panel(&ctx.style()).fill(theme::surface()))
            .show(ctx, |ui| {
                render_app_uninstall_footer(ui, app);
            });
    } else if overview_show_footer {
        egui::TopBottomPanel::bottom("overview_footer")
            .frame(egui::Frame::side_top_panel(&ctx.style()).fill(theme::surface()))
            .show(ctx, |ui| {
                render_overview_footer(ui, app);
            });
    } else if app.tab != crate::app::Tab::AppUninstall && app.tab != crate::app::Tab::Overview {
        egui::TopBottomPanel::bottom("footer").show(ctx, |ui| {
            egui::Frame::none()
                .fill(theme::surface())
                .stroke(egui::Stroke::new(1.0_f32, theme::line()))
                .inner_margin(egui::Margin::symmetric(S5, (FOOTER_H - BTN_H) / 2.0))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        let tab_idx_f = app.tab_index();
                        let total_cnt = app.results[tab_idx_f].len();
                        let selected_cnt = app.selected_count();
                        let selected_sz = app.selected_total_size();

                        // 左侧：三态全选 + 已选中 N 项 · 可释放 XX GB
                        let state = if selected_cnt == 0 {
                            widgets::Check::Off
                        } else if total_cnt > 0 && selected_cnt >= total_cnt {
                            widgets::Check::On
                        } else {
                            widgets::Check::Partial
                        };
                        if widgets::checkbox(ui, state, total_cnt > 0).clicked() {
                            if selected_cnt > 0 {
                                app.deselect_all();
                            } else {
                                app.select_all();
                            }
                        }

                        ui.add_space(S2);
                        ui.colored_label(
                            theme::text_2(),
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
                            // 设计稿 3.1：危险按钮必须带图标 + 明确宾语
                            let delete_btn = widgets::button_enabled(
                                ui,
                                delete_enabled,
                                Some(icons::Icon::Trash),
                                &format!("{} {}", app.t("delete"), format_size(selected_sz)),
                                widgets::Btn::Danger,
                                BTN_H,
                            );
                            if delete_btn.clicked() {
                                app.prepare_delete();
                            }
                        });
                    });
                });
        });
    }

    // ========== 左侧导航栏 ==========
    egui::SidePanel::left("sidebar")
        .exact_width(220.0)
        .frame(egui::Frame::side_top_panel(&ctx.style()).fill(theme::surface_2()))
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
                        painter.rect_filled(icon_rect, egui::Rounding::same(8.0), theme::brand());
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
                                .color(theme::brand_600()),
                        );
                        ui.label(
                            egui::RichText::new(format!("v{}", env!("CARGO_PKG_VERSION")))
                                .size(11.0)
                                .color(theme::text_3()),
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
                    .fill(theme::surface())
                    .stroke(egui::Stroke::new(1.0_f32, theme::line()))
                    .rounding(egui::Rounding::same(8.0))
                    .inner_margin(egui::Margin::same(10.0))
                    .show(ui, |ui| {
                        ui.set_min_width(180.0);
                        ui.colored_label(
                            theme::text_2(),
                            egui::RichText::new(app.t("disk_free")).size(11.0),
                        );
                        ui.add_space(2.0);
                        ui.horizontal(|ui| {
                            ui.label(
                                egui::RichText::new(format_size(app.disk_free))
                                    .size(16.0)
                                    .strong()
                                    .color(theme::text()),
                            );
                            ui.label(
                                egui::RichText::new(format!("/ {}", format_size(app.disk_total)))
                                    .size(11.0)
                                    .color(theme::text_3()),
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
                            theme::surface_3(),
                        );
                        let fill_width = bar_rect.width() * (free_pct / 100.0);
                        if fill_width > 0.0 {
                            let fill_rect = egui::Rect::from_min_size(
                                bar_rect.min,
                                egui::vec2(fill_width, bar_height),
                            );
                            ui.painter().rect_filled(
                                fill_rect,
                                egui::Rounding::same(3.0),
                                theme::brand(),
                            );
                        }
                        ui.allocate_rect(bar_rect, egui::Sense::hover());
                    });
                ui.add_space(10.0);

                // --- 导航项列表 ---
                ui.spacing_mut().item_spacing.y = 2.0;
                for (i, tab) in Tab::all().iter().enumerate() {
                    // APFS 在 Windows 上不存在，页签必须整个隐藏；只是让它空着，
                    // 用户会以为"扫过了没东西"。下标 i 仍按 all() 走，
                    // 与 results[9] 的寻址保持一致。
                    if !tab.is_supported() {
                        continue;
                    }
                    let count = app.results[i].len();
                    let is_selected = *tab == app.tab;
                    let title = tab_title(tab, app);

                    let nav_frame = if is_selected {
                        egui::Frame::none()
                            .fill(theme::brand_50())
                            .rounding(egui::Rounding::same(8.0))
                            .inner_margin(egui::Margin::symmetric(10.0, 7.0))
                            .stroke(egui::Stroke::NONE)
                    } else {
                        egui::Frame::none()
                            .inner_margin(egui::Margin::symmetric(10.0, 7.0))
                            .stroke(egui::Stroke::NONE)
                    };

                    let nav_resp = nav_frame.show(ui, |ui| {
                        ui.set_min_width(180.0);
                        ui.horizontal(|ui| {
                            // 设计稿 2.5：导航图标 20px 线性，不用 emoji
                            icons::show(
                                ui,
                                icons::NAV_ICONS[i],
                                16.0,
                                if is_selected {
                                    theme::brand()
                                } else {
                                    theme::text_2()
                                },
                            );
                            ui.add_space(9.0);
                            ui.label(egui::RichText::new(title).size(13.0).color(if is_selected {
                                theme::brand_600()
                            } else {
                                theme::text()
                            }));
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    if count > 0 {
                                        ui.label(
                                            egui::RichText::new(count.to_string())
                                                .size(11.0)
                                                .color(if is_selected {
                                                    theme::brand()
                                                } else {
                                                    theme::text_3()
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
                    let lang_label = if app.lang_en { "中文" } else { "EN" };
                    if widgets::button(ui, None, lang_label, widgets::Btn::Ghost, BTN_H_SM)
                        .clicked()
                    {
                        app.toggle_lang();
                    }
                    let logs_label = app.t("logs").to_string();
                    if widgets::button(
                        ui,
                        Some(icons::Icon::Terminal),
                        &logs_label,
                        widgets::Btn::Ghost,
                        BTN_H_SM,
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
                                    .color(theme::text()),
                            );
                            // 副标题：扫描状态/项目数
                            let tab_idx_h = app.tab_index();
                            let subtitle = if app.tab == Tab::AppUninstall {
                                match &app.scan_states[tab_idx_h] {
                                    ScanState::Idle => app.t("press_r_to_scan").to_string(),
                                    ScanState::Scanning => format!("⏳ {}", app.t("scanning")),
                                    ScanState::Done => {
                                        let items = &app.results[tab_idx_h];
                                        let total: u64 = items.iter().map(|i| i.size_bytes).sum();
                                        app.tf(
                                            "app_uninstall_subtitle",
                                            &[
                                                &uninstall_groups.len().to_string(),
                                                &format_size(total),
                                            ],
                                        )
                                    }
                                }
                            } else {
                                match &app.scan_states[tab_idx_h] {
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
                                }
                            };
                            ui.colored_label(
                                theme::text_3(),
                                egui::RichText::new(subtitle).size(11.0),
                            );
                        });

                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            // 强制刷新按钮（清除缓存后重新扫描）
                            let is_scanning = app.any_scanning();
                            let refresh_btn = widgets::icon_button_enabled(
                                ui,
                                icons::Icon::Refresh,
                                BTN_H,
                                !is_scanning,
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
                                        Tab::CustomRules => Some("custom_rules"),
                                        Tab::DuplicateFiles => Some("dup_files"),
                                        Tab::StartupItems => None,
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
                            let scan_label = if is_scanning {
                                app.t("scanning").to_string()
                            } else {
                                app.t("scan").to_string()
                            };
                            let scan_button = widgets::button_enabled(
                                ui,
                                !is_scanning,
                                Some(icons::Icon::Search),
                                &scan_label,
                                widgets::Btn::Primary,
                                BTN_H,
                            );
                            if scan_button.clicked() {
                                if app.tab == Tab::Overview {
                                    start_scan_all(app, scan_rx);
                                } else {
                                    start_scan(app, scan_rx);
                                }
                            }

                            // App 卸载：顶部搜索框
                            if app.tab == Tab::AppUninstall {
                                ui.add_space(8.0);
                                let search_hint =
                                    app.t("app_uninstall_search_placeholder").to_string();
                                let search_resp = ui.add(
                                    egui::TextEdit::singleline(&mut app.filter_query)
                                        .hint_text(search_hint)
                                        .desired_width(180.0)
                                        .min_size([120.0, 28.0].into()),
                                );
                                if search_resp.lost_focus()
                                    && ui.input(|i| i.key_pressed(egui::Key::Escape))
                                {
                                    app.clear_filter();
                                }
                                app.filter_active = search_resp.has_focus();
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

            // --- 启动项 Tab：macOS Login Items / launchd（P3） ---
            if app.tab == Tab::StartupItems {
                render_startup_panel(ui, app);
                return;
            }

            // --- 磁盘分析器 Tab：目录钻取式浏览 ---
            if app.tab == Tab::LargeFiles {
                render_disk_analyzer(ui, app, scan_rx);
                return;
            }

            // --- App 卸载 Tab：三栏布局 + 双滚动条 ---
            if app.tab == Tab::AppUninstall {
                let is_scanning = app.tab_scanning(app.tab_index());
                let is_empty = app.results[app.tab_index()].is_empty();
                if is_scanning {
                    ui_scanning(ui, app);
                } else if is_empty {
                    render_empty_state(ui, app, scan_rx);
                } else {
                    render_app_uninstall_panel(ui, app, scan_rx, &uninstall_groups);
                }
                return;
            }

            // --- 扫描结果区 ---
            let tab_idx = app.tab_index();
            let items = app.results[tab_idx].clone();
            let is_scanning = app.tab_scanning(tab_idx);

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
            } else if items.is_empty() {
                render_empty_state(ui, app, scan_rx);
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

                // 风险汇总：常驻、可扫读的色标（设计稿原则 2：风险可视化而非警示化）
                egui::Frame::none()
                    .inner_margin(egui::Margin::symmetric(S4, S2))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = S2;
                            widgets::badge(
                                ui,
                                &format!(
                                    "{} {} · {}",
                                    safe_cnt,
                                    app.t("safe_clean"),
                                    format_size(safe_sz)
                                ),
                                theme::safe(),
                                theme::safe_50(),
                                None,
                            );
                            widgets::badge(
                                ui,
                                &format!(
                                    "{} {} · {}",
                                    caution_cnt,
                                    app.t("caution_clean"),
                                    format_size(caution_sz)
                                ),
                                theme::caution(),
                                theme::caution_50(),
                                None,
                            );
                            if advanced_cnt > 0 {
                                widgets::badge(
                                    ui,
                                    &format!(
                                        "{} {} · {}",
                                        advanced_cnt,
                                        app.t("confirm_clean"),
                                        format_size(advanced_sz)
                                    ),
                                    theme::danger(),
                                    theme::danger_50(),
                                    None,
                                );
                            }
                            if selected_cnt > 0 {
                                widgets::badge(
                                    ui,
                                    &format!(
                                        "{} {} · {}",
                                        selected_cnt,
                                        app.t("items_selected"),
                                        format_size(selected_sz)
                                    ),
                                    theme::brand(),
                                    theme::brand_50(),
                                    None,
                                );
                            }
                        });
                    });

                // ====== 操作按钮栏（选择分组 + 搜索）======
                egui::Frame::none()
                    .inner_margin(egui::Margin::symmetric(S4, S1))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = S2;

                            let safe_label = app.t("select_safe").to_string();
                            if widgets::button(
                                ui,
                                Some(icons::Icon::Shield),
                                &safe_label,
                                widgets::Btn::Secondary,
                                BTN_H,
                            )
                            .clicked()
                            {
                                app.select_safe_only();
                            }

                            let all_label = app.t("select_all").to_string();
                            if widgets::button(ui, None, &all_label, widgets::Btn::Ghost, BTN_H)
                                .clicked()
                            {
                                app.select_all();
                            }
                            let none_label = app.t("deselect_all").to_string();
                            if widgets::button(ui, None, &none_label, widgets::Btn::Ghost, BTN_H)
                                .clicked()
                            {
                                app.deselect_all();
                            }

                            ui.add_space(S2);

                            // 过滤/搜索输入框（/ 键聚焦，Esc 清除过滤）
                            let filter_placeholder = app.t("filter_placeholder").to_string();
                            let resp = ui.add(
                                egui::TextEdit::singleline(&mut app.filter_query)
                                    .hint_text(&filter_placeholder)
                                    .desired_width(280.0)
                                    .min_size([160.0, BTN_H].into()),
                            );
                            if resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                                app.clear_filter();
                            }
                            if !app.filter_query.is_empty()
                                && widgets::icon_button(ui, icons::Icon::X, BTN_H_SM).clicked()
                            {
                                app.clear_filter();
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
                // 不可删除项（SIP/系统保护/仓库元数据等）对用户无操作价值，
                // 统一从列表过滤，避免「勾选 → 授权 → 删除失败 → 重试」循环。
                // 统计徽章与概览推荐区本就只计 deletable 项，此处过滤后口径一致。
                let filtered_indices: Vec<usize> = app
                    .filtered_indices()
                    .into_iter()
                    .filter(|&i| items[i].deletable)
                    .collect();
                let filtered_count = filtered_indices.len();
                if !app.filter_query.trim().is_empty() {
                    ui.colored_label(
                        theme::brand(),
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
                    .fill(theme::bg())
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
                                        .color(theme::text_3()),
                                );
                            });
                            return;
                        }

                        // ====== 可滚动列表 ======
                        // 提前克隆关联明细和展开状态，避免借用冲突
                        let associated_details = app.associated_details.clone();
                        let expanded_items = app.expanded_items.clone();
                        // 占比条分母：当前可见列表的总容量
                        let visible_total: u64 =
                            filtered_indices.iter().map(|&i| items[i].size_bytes).sum();

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
                                        visible_total,
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

    // License 激活弹窗（额度用尽时弹出）
    if app.show_license_dialog {
        show_license_window(ctx, app);
    }

    // Windows: 残留清理弹窗（卸载后检测到残留时弹出）
    #[cfg(target_os = "windows")]
    if app.show_residual_dialog {
        show_residual_window(ctx, app, delete_rx);
    }
}

/// 权限引导弹窗
pub(crate) fn show_permission_guide_window(ctx: &egui::Context, app: &mut App) {
    widgets::scrim(ctx);
    egui::Window::new(app.t("permission_title"))
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .frame(widgets::modal_frame())
        .show(ctx, |ui| {
            ui.set_min_width(480.0);
            ui.set_max_width(520.0);
            ui.add_space(10.0);
            ui.vertical(|ui| {
                // 标题
                ui.horizontal(|ui| {
                    icons::show(ui, icons::Icon::Shield, 28.0, theme::brand());
                    ui.label(egui::RichText::new(app.t("permission_headline")).size(18.0).strong());
                });

                ui.add_space(8.0);
                ui.separator();
                ui.add_space(8.0);

                // 说明
                ui.colored_label(
                    theme::text(),
                    egui::RichText::new(app.t("permission_desc")).size(13.0),
                );
                ui.add_space(3.0);
                ui.colored_label(
                    theme::text_2(),
                    egui::RichText::new(app.t("permission_sub_desc")).size(12.0),
                );

                ui.add_space(10.0);

                // 步骤
                ui.colored_label(
                    theme::safe(),
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
                            theme::safe(),
                            egui::RichText::new(format!("{}. ", i + 1)).size(13.0),
                        );
                        ui.colored_label(
                            theme::text(),
                            egui::RichText::new(*step).size(13.0),
                        );
                    });
                }

                ui.add_space(12.0);

                // 按钮
                ui.horizontal(|ui| {
                    let btn = ui.add(
                        egui::Button::new(
                            egui::RichText::new(format!("️ {}", app.t("permission_open_settings")))
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
                            egui::RichText::new(app.t("permission_install_app"))
                                .color(egui::Color32::WHITE)
                                .size(14.0)
                        )
                        .fill(theme::safe())
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
                    theme::text_3(),
                    egui::RichText::new(app.t("permission_hint")).size(11.0),
                );
            });
        });
}

/// 删除确认弹窗（设计稿 4.4 样式）
pub(crate) fn show_confirm_window(
    ctx: &egui::Context,
    app: &mut App,
    delete_rx: &mut Option<mpsc::Receiver<DeleteMessage>>,
) {
    // 统计一律以 pending_delete 为准（可能跨 Tab），不能按当前 Tab 统计：
    // 概览页触发的跨 Tab 删除时当前 tab == Overview，results[0] 恒为空，
    // 会显示「0 项 / 0 B」却在确认后真实删除 N 个文件（历史 bug）。
    let count = app.pending_count();
    let size = app.pending_total_size();

    let mut safe_cnt = 0usize;
    let mut safe_sz = 0u64;
    let mut caution_cnt = 0usize;
    let mut caution_sz = 0u64;
    let mut advanced_cnt = 0usize;
    let mut advanced_sz = 0u64;
    let mut needs_admin = false;
    // P1 强确认：命令类目（Docker/OrbStack 清理）走官方 CLI，范围由官方
    // 命令决定（docker system prune -a / orbctl delete），不可逐文件恢复，
    // 确认弹窗必须显式点名。
    let mut cmd_cnt = 0usize;
    let mut cmd_names: Vec<String> = Vec::new();

    for item in app.pending_items() {
        if !item.deletable {
            continue;
        }
        if item.category == "Docker清理" || item.category == "OrbStack清理" {
            cmd_cnt += 1;
            let n = if item.category == "Docker清理" {
                "docker system prune"
            } else {
                "orbctl delete/prune"
            };
            if !cmd_names.iter().any(|c| c == n) {
                cmd_names.push(n.to_string());
            }
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

    // 设计稿 3.5：遮罩 + 内容型弹窗 480 宽、圆角 14
    widgets::scrim(ctx);
    egui::Window::new("confirm_delete_modal")
        .title_bar(false)
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .frame(widgets::modal_danger_frame())
        .show(ctx, |ui| {
            ui.set_min_width(MODAL_W);
            ui.set_max_width(MODAL_W);

            // Header：标题必须包含具体数字，不能是"确定要继续吗"
            let title = format!("{} {} {}？", app.t("confirm_delete"), count, app.t("items"));
            widgets::modal_header(
                ui,
                icons::Icon::Alert,
                theme::danger(),
                theme::danger_50(),
                &title,
                app.t("confirm_subtitle"),
            );

            ui.add_space(10.0);

            // 危险警示条：不可逆操作一眼可辨（红色描边 + 红底警示条）
            widgets::danger_banner(ui, app.t("confirm_irreversible"));

            ui.add_space(12.0);

            // Body rows
            egui::Frame::none()
                .fill(theme::surface_3())
                .stroke(egui::Stroke::NONE)
                .rounding(egui::Rounding::same(8.0))
                .inner_margin(egui::Margin::same(12.0))
                .show(ui, |ui| {
                    render_confirm_row(
                        ui,
                        app.t("confirm_selected_items"),
                        &format!("{} {}", count, app.t("items")),
                        theme::text(),
                    );
                    render_confirm_row(
                        ui,
                        app.t("confirm_releasable"),
                        &format_size(size),
                        theme::brand(),
                    );
                    // 保护闸门挡掉的项数。正常路径应为 0；非 0 说明有程序化入口
                    // 或某处 UI 漏判，必须显式告诉用户，否则他会以为清理没生效。
                    if app.protection_blocked > 0 {
                        render_confirm_row(
                            ui,
                            app.t("confirm_protection_skipped"),
                            &app.protection_blocked.to_string(),
                            theme::caution(),
                        );
                    }
                    // M-1：哪些应用会被交给官方卸载器。必须提前说清楚 ——
                    // 用户点了确认之后会弹出一个不属于本工具的窗口，不说就是惊吓。
                    if !app.official_handoffs.is_empty() {
                        render_confirm_row(
                            ui,
                            app.t("confirm_official_uninstaller_hint"),
                            &app.official_handoffs.len().to_string(),
                            theme::caution(),
                        );
                    }
                    if safe_cnt > 0 {
                        render_confirm_row(
                            ui,
                            app.t("confirm_safe"),
                            &format!("{} {} · {}", safe_cnt, app.t("items"), format_size(safe_sz)),
                            theme::safe(),
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
                            theme::caution(),
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
                            theme::danger(),
                        );
                    }
                    if cmd_cnt > 0 {
                        render_confirm_row(
                            ui,
                            app.t("confirm_official_cmd"),
                            &format!("{} · {}", cmd_cnt, cmd_names.join(" + ")),
                            theme::danger(),
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
                        theme::text(),
                    );
                });

            ui.add_space(8.0);

            // 预览折叠
            let preview_label = app.t("confirm_preview").to_string();
            if widgets::button(
                ui,
                Some(icons::Icon::Eye),
                &preview_label,
                widgets::Btn::Ghost,
                BTN_H_SM,
            )
            .clicked()
            {
                app.show_preview = !app.show_preview;
            }
            if app.show_preview {
                ui.add_space(4.0);
                egui::Frame::none()
                    .stroke(egui::Stroke::new(1.0_f32, theme::line()))
                    .rounding(egui::Rounding::same(6.0))
                    .inner_margin(egui::Margin::same(8.0))
                    .show(ui, |ui| {
                        ui.set_max_height(150.0);
                        egui::ScrollArea::vertical().show(ui, |ui| {
                            // 预览列表同样以 pending_delete 为准（跨 Tab 安全）
                            let items: Vec<_> = app
                                .pending_items()
                                .into_iter()
                                .filter(|item| item.deletable)
                                .collect();
                            for item in &items {
                                ui.horizontal(|ui| {
                                    let (fg, _, badge) = recommend_badge_colors(&item.recommend);
                                    render_status_badge(ui, badge, fg, theme::surface_3());
                                    ui.colored_label(
                                        theme::text(),
                                        egui::RichText::new(format_size(item.size_bytes))
                                            .size(12.0)
                                            .monospace(),
                                    );
                                    let cat = i18n::translate_category(&item.category, app.lang_en);
                                    ui.colored_label(
                                        theme::text_2(),
                                        egui::RichText::new(&cat).size(12.0),
                                    );
                                    ui.colored_label(
                                        theme::text_3(),
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
                                theme::text_3(),
                                egui::RichText::new(summary_text).size(11.0),
                            );
                        });
                    });
            }

            ui.add_space(12.0);

            // Footer：主行动在右，取消在左（设计稿 3.5）
            widgets::modal_footer(ui, |ui| {
                let danger_label = format!("{} {}", app.t("confirm_delete"), format_size(size));
                if widgets::button(
                    ui,
                    Some(icons::Icon::Trash),
                    &danger_label,
                    widgets::Btn::Danger,
                    BTN_H,
                )
                .clicked()
                {
                    app.show_preview = false;
                    let to_delete = app.confirm_delete();
                    start_delete(
                        to_delete,
                        app.lang_en,
                        delete_rx,
                        app.settings_auto_restore_point,
                        app.settings_prefer_official_uninstaller,
                        app.delete_cancel.clone(),
                    );
                }
                let cancel_label = app.t("cancel").to_string();
                if widgets::button(ui, None, &cancel_label, widgets::Btn::Secondary, BTN_H)
                    .clicked()
                {
                    app.show_preview = false;
                    app.cancel_delete();
                }
            });
        });
}

/// 确认弹窗中的单行 Key-Value
pub(crate) fn render_confirm_row(
    ui: &mut egui::Ui,
    label: &str,
    value: &str,
    value_color: egui::Color32,
) {
    ui.horizontal(|ui| {
        ui.colored_label(theme::text_2(), egui::RichText::new(label).size(13.0));
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
        .rect_filled(sep_rect, egui::Rounding::ZERO, theme::line());
    ui.add_space(4.0);
}

/// sudo 密码输入弹窗（egui 内置输入框）
///
/// 两种模式：
/// - 正常删除：输入密码后直接 sudo -S 删除 failed_items
/// - Touch ID 启用模式（touch_id_setup_mode=true）：输入密码后先创建 sudo_local，
///   启用成功再继续删除（后续 sudo 会触发 Touch ID）
pub(crate) fn show_sudo_password_window(
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
    let emoji_icon = if is_setup_mode {
        icons::Icon::Fingerprint
    } else {
        icons::Icon::Lock
    };
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

    widgets::scrim(ctx);
    egui::Window::new(window_title)
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .frame(widgets::modal_frame())
        .show(ctx, |ui| {
            ui.set_min_width(420.0);
            ui.set_max_width(480.0);
            ui.add_space(10.0);

            ui.vertical(|ui| {
                ui.horizontal(|ui| {
                    icons::show(ui, emoji_icon, 28.0, theme::caution());
                    ui.label(egui::RichText::new(headline).size(18.0).strong());
                });

                ui.add_space(8.0);
                ui.colored_label(theme::text(), egui::RichText::new(description).size(13.0));
                if !is_setup_mode {
                    ui.add_space(2.0);
                    ui.colored_label(
                        theme::text_2(),
                        egui::RichText::new(app.t("sudo_password_note")).size(12.0),
                    );
                }

                if let Some(ref err) = app.sudo_error {
                    ui.add_space(8.0);
                    ui.colored_label(
                        theme::danger(),
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
                                    app.failed_paths.clear();
                                    let items = std::mem::take(&mut app.sudo_failed_items);
                                    app.delete_done = 0;
                                    app.delete_total = items.len();
                                    start_sudo_delete_touchid(
                                        items,
                                        app.lang_en,
                                        delete_rx,
                                        app.delete_cancel.clone(),
                                    );
                                }
                                Ok(false) => {
                                    // 已启用，直接走 Touch ID 删除
                                    app.touch_id_enabled = true;
                                    app.touch_id_setup_mode = false;
                                    app.confirm = ConfirmState::SudoWithTouchId;
                                    app.failed_paths.clear();
                                    let items = std::mem::take(&mut app.sudo_failed_items);
                                    app.delete_done = 0;
                                    app.delete_total = items.len();
                                    start_sudo_delete_touchid(
                                        items,
                                        app.lang_en,
                                        delete_rx,
                                        app.delete_cancel.clone(),
                                    );
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
                                app.failed_paths.clear();
                                let items = std::mem::take(&mut app.sudo_failed_items);
                                app.delete_done = 0;
                                app.delete_total = items.len();
                                start_sudo_delete(
                                    items,
                                    password,
                                    app.lang_en,
                                    delete_rx,
                                    app.delete_cancel.clone(),
                                );
                            }
                        } else {
                            app.confirm = ConfirmState::Deleting;
                            app.failed_paths.clear();
                            let items = std::mem::take(&mut app.sudo_failed_items);
                            app.delete_done = 0;
                            app.delete_total = items.len();
                            start_sudo_delete(
                                items,
                                password,
                                app.lang_en,
                                delete_rx,
                                app.delete_cancel.clone(),
                            );
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
pub(crate) fn show_touch_id_setup_window(
    ctx: &egui::Context,
    app: &mut App,
    _delete_rx: &mut Option<mpsc::Receiver<DeleteMessage>>,
) {
    widgets::scrim(ctx);
    egui::Window::new(app.t("touchid_setup_title"))
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .frame(widgets::modal_frame())
        .show(ctx, |ui| {
            ui.set_min_width(440.0);
            ui.set_max_width(500.0);
            ui.add_space(10.0);

            ui.vertical(|ui| {
                ui.horizontal(|ui| {
                    icons::show(ui, icons::Icon::Fingerprint, 28.0, theme::brand());
                    ui.label(
                        egui::RichText::new(app.t("touchid_setup_headline"))
                            .size(17.0)
                            .strong(),
                    );
                });

                ui.add_space(8.0);
                ui.colored_label(
                    theme::text(),
                    egui::RichText::new(App::tf_lang(
                        app.lang_en,
                        "touchid_setup_desc",
                        &[&app.sudo_failed_items.len().to_string()],
                    ))
                    .size(13.0),
                );
                ui.add_space(4.0);
                ui.colored_label(
                    theme::text_2(),
                    egui::RichText::new(app.t("touchid_setup_detail")).size(12.0),
                );

                if let Some(ref err) = app.touch_id_error {
                    ui.add_space(8.0);
                    ui.colored_label(
                        theme::danger(),
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
                                egui::RichText::new(app.t("touchid_enable"))
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
/// Touch ID 删除中弹窗
pub(crate) fn show_touch_id_deleting_window(ctx: &egui::Context, app: &mut App) {
    widgets::scrim(ctx);
    egui::Window::new(app.t("touchid_verify_title"))
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .frame(widgets::modal_frame())
        .show(ctx, |ui| {
            ui.set_min_width(400.0);
            ui.set_max_width(440.0);
            ui.add_space(10.0);

            ui.vertical(|ui| {
                ui.horizontal(|ui| {
                    icons::show(ui, icons::Icon::Fingerprint, 28.0, theme::brand());
                    ui.label(
                        egui::RichText::new(app.t("touchid_verify_headline"))
                            .size(15.0)
                            .strong(),
                    );
                });

                ui.add_space(8.0);
                ui.colored_label(
                    theme::text_2(),
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
                    theme::text_3(),
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
                        .fill(theme::surface())
                        .show(ui, |ui| {
                            ui.set_min_width(360.0);
                            ui.label(
                                egui::RichText::new(app.t("touchid_verify_log"))
                                    .size(11.0)
                                    .strong(),
                            );
                            ui.add_space(4.0);
                            for log in recent_logs {
                                ui.colored_label(
                                    theme::text_2(),
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

/// 删除中弹窗（设计稿 4.5 样式）
pub(crate) fn show_deleting_window(ctx: &egui::Context, app: &mut App) {
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

    widgets::scrim(ctx);
    egui::Window::new(app.t("cleaning"))
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .frame(widgets::modal_frame())
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
                        .color(theme::text()),
                );
                ui.colored_label(
                    theme::text_2(),
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
                    .fill(if in_sudo_phase {
                        theme::caution()
                    } else {
                        theme::brand()
                    })
                    .text(format!("{}%", (progress * 100.0) as u32)),
            );

            ui.add_space(8.0);

            // 状态计数
            ui.horizontal(|ui| {
                ui.colored_label(
                    theme::text_2(),
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
                            theme::danger(),
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
                .fill(theme::surface_3())
                .stroke(egui::Stroke::new(1.0_f32, theme::line()))
                .rounding(egui::Rounding::same(6.0))
                .inner_margin(egui::Margin::same(10.0))
                .show(ui, |ui| {
                    ui.set_max_height(160.0);
                    egui::ScrollArea::vertical().show(ui, |ui| {
                        for log in app.logs.iter().rev().take(20) {
                            let color = if log.starts_with('✓') || log.starts_with('✅') {
                                theme::safe()
                            } else if log.starts_with('✗') || log.starts_with('⛔') {
                                theme::danger()
                            } else if log.starts_with('⚠') {
                                theme::caution()
                            } else {
                                theme::text_2()
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
                        .fill(theme::brand())
                        .rounding(egui::Rounding::same(8.0)),
                    );
                    if auth_btn.clicked() && can_authorize {
                        app.sudo_failed_items = app.failed_paths.clone();
                        app.confirm = ConfirmState::NeedSudoPassword;
                    }

                    ui.add_space(8.0);

                    // P0-3：停止删除 —— 置取消标志，后台线程在子项边界优雅退出，
                    // 已删项照常落清单，未处理项原地保留。
                    let stop_btn = ui.add(
                        egui::Button::new(egui::RichText::new(app.t("deleting_stop")).size(13.0))
                            .fill(theme::surface())
                            .stroke(egui::Stroke::new(1.0_f32, theme::danger()))
                            .rounding(egui::Rounding::same(8.0)),
                    );
                    if stop_btn.clicked() {
                        app.delete_cancel
                            .store(true, std::sync::atomic::Ordering::Relaxed);
                    }

                    ui.add_space(8.0);

                    if ui
                        .add(
                            egui::Button::new(
                                egui::RichText::new(app.t("progress_background_run")).size(13.0),
                            )
                            .fill(theme::surface())
                            .stroke(egui::Stroke::new(1.0_f32, theme::line()))
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

/// License 激活弹窗（免费额度用尽时弹出）
pub(crate) fn show_license_window(ctx: &egui::Context, app: &mut App) {
    egui::Window::new(if app.lang_en {
        "Upgrade to Pro"
    } else {
        "升级到 Pro 版"
    })
    .collapsible(false)
    .resizable(false)
    .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
    .show(ctx, |ui| {
        ui.set_min_width(460.0);
        ui.set_max_width(500.0);
        ui.add_space(8.0);

        // 额度说明
        let used = crate::license::quota_used();
        let total = crate::license::FREE_CLEAN_QUOTA_BYTES;
        let msg = if app.lang_en {
            format!(
                "Free cleanup quota exhausted ({}/{}). Activate Pro for unlimited cleaning.",
                crate::scanner::format_size(used),
                crate::scanner::format_size(total)
            )
        } else {
            format!(
                "免费清理额度已用完（{}/{}）。激活 Pro 版后可无限清理。",
                crate::scanner::format_size(used),
                crate::scanner::format_size(total)
            )
        };
        ui.colored_label(theme::text(), egui::RichText::new(msg).size(14.0));
        ui.add_space(8.0);

        // Pro 权益
        let benefits: &[&str] = if app.lang_en {
            &[
                "Unlimited cleanup, uninstall & optimization",
                "60+ developer cache categories",
                "APFS snapshot management",
                "Lifetime updates, one-time purchase",
            ]
        } else {
            &[
                "无限清理、卸载与系统优化",
                "60+ 开发者缓存类别",
                "APFS 快照管理",
                "买断制，终身免费更新",
            ]
        };
        for b in benefits {
            ui.colored_label(
                theme::safe(),
                egui::RichText::new(format!("✓ {}", b)).size(12.0),
            );
        }
        ui.add_space(10.0);

        // License 输入
        ui.colored_label(
            theme::text_2(),
            egui::RichText::new(if app.lang_en {
                "Enter your license key:"
            } else {
                "输入 License Key："
            })
            .size(12.0),
        );
        ui.add(
            egui::TextEdit::singleline(&mut app.license_input)
                .desired_width(f32::INFINITY)
                .hint_text("MACL-..."),
        );

        if let Some(err) = &app.license_error {
            ui.add_space(4.0);
            ui.colored_label(theme::danger(), egui::RichText::new(err).size(12.0));
        }
        ui.add_space(10.0);

        // 按钮行
        ui.horizontal(|ui| {
            if ui
                .button(
                    egui::RichText::new(if app.lang_en { "Activate" } else { "激活" }).size(13.0),
                )
                .clicked()
            {
                app.try_activate_license();
            }
            if ui
                .button(
                    egui::RichText::new(if app.lang_en { "Buy Pro" } else { "购买 Pro" })
                        .size(13.0),
                )
                .clicked()
            {
                open_url("https://maclean.app/buy");
            }
            if ui
                .button(egui::RichText::new(app.t("cancel")).size(13.0))
                .clicked()
            {
                app.show_license_dialog = false;
            }
        });
        ui.add_space(4.0);
    });
}

/// 删除完成汇总弹窗
pub(crate) fn show_summary_window(
    ctx: &egui::Context,
    app: &mut App,
    ok: usize,
    fail: usize,
    _skip: usize,
    delete_rx: &mut Option<mpsc::Receiver<DeleteMessage>>,
) {
    let _has_failures = !app.failed_paths.is_empty();

    widgets::scrim(ctx);
    egui::Window::new(app.t("summary_title"))
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .frame(widgets::modal_frame())
        .show(ctx, |ui| {
            ui.set_min_width(480.0);
            ui.set_max_width(520.0);
            ui.add_space(10.0);
            ui.vertical(|ui| {
                // 成功
                ui.horizontal(|ui| {
                    icons::show(ui, icons::Icon::Check, 16.0, theme::safe());
                    ui.label(egui::RichText::new(App::tf_lang(app.lang_en, "summary_success", &[&ok.to_string()])).size(15.0).color(theme::safe()));
                });

                // M-2：如实告知这次删除留了多少后悔药。
                // restorable=0 时也要显示 —— 用户刚删完东西，最需要知道的
                // 恰恰是"这批回不来了"，藏起来就是欺骗。
                if let Some((_id, restorable, total)) = &app.last_backup {
                    ui.add_space(5.0);
                    let text = if *restorable > 0 {
                        App::tf_lang(
                            app.lang_en,
                            "summary_backup",
                            &[&restorable.to_string(), &total.to_string()],
                        )
                    } else {
                        App::t_lang(app.lang_en, "summary_backup_none").to_string()
                    };
                    ui.colored_label(theme::text_2(), egui::RichText::new(text).size(12.0));
                }

                if fail > 0 {
                    ui.add_space(5.0);
                    ui.horizontal(|ui| {
                        icons::show(ui, icons::Icon::X, 16.0, theme::danger());
                        ui.label(egui::RichText::new(App::tf_lang(app.lang_en, "summary_fail", &[&fail.to_string()])).size(15.0).color(theme::danger()));
                    });
                    ui.add_space(3.0);
                    ui.colored_label(
                        theme::text_2(),
                        egui::RichText::new(app.t("summary_fail_hint")).size(12.0),
                    );

                    // 引导用户处理失败项
                    ui.add_space(8.0);
                    egui::Frame::group(ui.style())
                        .fill(theme::surface())
                        .stroke(egui::Stroke::new(1.0_f32, theme::brand()))
                        .inner_margin(egui::Margin::same(8.0))
                        .show(ui, |ui| {
                            ui.colored_label(
                                theme::brand(),
                                egui::RichText::new(app.t("summary_solution_title")).size(13.0).strong(),
                            );
                            ui.add_space(3.0);
                            ui.colored_label(
                                theme::text(),
                                egui::RichText::new(app.t("summary_sip_tip")).size(11.0),
                            );
                            ui.add_space(3.0);
                            ui.colored_label(
                                theme::text(),
                                egui::RichText::new(app.t("summary_perm_tip")).size(11.0),
                            );
                            ui.add_space(5.0);
                            ui.colored_label(
                                theme::text_2(),
                                egui::RichText::new(app.t("summary_solution_1")).size(11.0),
                            );
                            ui.colored_label(
                                theme::text_2(),
                                egui::RichText::new(app.t("summary_solution_2")).size(11.0),
                            );
                            ui.colored_label(
                                theme::text_2(),
                                egui::RichText::new(app.t("summary_solution_3")).size(11.0),
                            );
                            ui.add_space(5.0);
                            ui.horizontal(|ui| {
                                if ui.button(egui::RichText::new(format!("️ {}", app.t("summary_open_settings"))).size(12.0)).clicked() {
                                    let _ = std::process::Command::new("open")
                                        .arg("x-apple.systempreferences:com.apple.preference.security?Privacy_AllFiles")
                                        .spawn();
                                }
                            });
                        });

                    // 主操作行：关闭（无论是否有失败项都必须能关掉弹窗）+ 重试提权删除。
                    // 关闭按钮放在显眼位置：弹窗超高、底部"确定"滚出视口时也能直接关。
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if ui
                            .button(egui::RichText::new(app.t("summary_close")).size(13.0))
                            .clicked()
                        {
                            app.dismiss_summary();
                        }
                        ui.add_space(6.0);
                        if ui
                            .add(
                                egui::Button::new(
                                    egui::RichText::new(app.t("summary_retry_delete"))
                                        .color(egui::Color32::WHITE)
                                        .size(13.0),
                                )
                                .fill(egui::Color32::from_rgb(0, 122, 255)),
                            )
                            .clicked()
                        {
                            let items = app.failed_paths.clone();
                            app.delete_summary = None;
                            app.deleted_paths.clear();
                            app.failed_paths.clear();
                            app.logs.clear();
                            app.delete_done = 0;
                            app.delete_total = items.len();
                            #[cfg(target_os = "macos")]
                            {
                                let (enabled, available) = (
                                    touchid::sudo_touch_id_enabled(),
                                    touchid::touch_id_available(),
                                );
                                let clamshell_closed = safety::is_clamshell_closed();
                                route_and_start_elevated_delete(
                                    app,
                                    items,
                                    enabled,
                                    available,
                                    clamshell_closed,
                                    delete_rx,
                                );
                            }
                            #[cfg(not(target_os = "macos"))]
                            {
                                app.sudo_failed_items = items;
                                app.sudo_password_input.clear();
                                app.sudo_password = None;
                                app.sudo_error = None;
                                app.touch_id_error = None;
                                app.confirm = ConfirmState::NeedSudoPassword;
                            }
                        }
                        ui.colored_label(
                            theme::text_2(),
                            egui::RichText::new(app.t("summary_retry_hint")).size(11.0),
                        );
                    });

                    // 列出所有失败的路径（可滚动+复制路径，不提供 sudo 命令——
                    // 提权删除全部由应用内自动授权完成，用户无需手动执行任何命令）
                    ui.add_space(5.0);
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new(app.t("summary_fail_list")).size(12.0).color(theme::text_2()));
                        let all_paths: String = app.failed_paths.iter()
                            .map(|(p, c)| format!("[{}] {}", c, p))
                            .collect::<Vec<_>>()
                            .join("\n");
                        if ui.button(egui::RichText::new(app.t("summary_copy_paths")).size(11.0)).clicked() {
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
                                        theme::danger(),
                                        egui::RichText::new("•").size(11.0),
                                    );
                                    ui.vertical(|ui| {
                                        ui.colored_label(
                                            theme::text_2(),
                                            egui::RichText::new(category).size(11.0),
                                        );
                                        ui.add(
                                            egui::TextEdit::multiline(&mut path.as_str())
                                                .desired_width(400.0)
                                                .font(egui::TextStyle::Monospace)
                                                .text_color(theme::text_3())
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
pub(crate) fn tab_title<'a>(tab: &Tab, app: &'a App) -> &'a str {
    match tab {
        Tab::Overview => app.t("tab_overview"),
        Tab::DevCache => app.t("tab_dev_cache"),
        Tab::LargeFiles => app.t("tab_large_files"),
        Tab::AppCache => app.t("tab_app_cache"),
        Tab::AppData => app.t("tab_app_data"),
        Tab::AppUninstall => app.t("tab_app_uninstall"),
        Tab::SystemOptimize => app.t("tab_system_optimize"),
        Tab::Apfs => app.t("tab_apfs"),
        Tab::CustomRules => app.t("tab_custom_rules"),
        Tab::DuplicateFiles => app.t("tab_dup_files"),
        Tab::StartupItems => app.t("tab_startup_items"),
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
pub(crate) fn show_residual_window(
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
                    icons::show(ui, icons::Icon::Alert, 16.0, theme::caution());
                    ui.label(egui::RichText::new("检测到卸载残留").size(16.0).strong());
                });
                ui.colored_label(
                    theme::text_3(),
                    egui::RichText::new(format!(
                        "共 {} 项残留 (可清理 {} 项), 文件总计 {}",
                        total,
                        deletable_count,
                        format_size(fs_total_size)
                    ))
                    .size(12.0),
                );
                ui.colored_label(
                    theme::text_3(),
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
                                theme::brand(),
                                egui::RichText::new(format!("注册表残留 ({} 项)", reg_len))
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
                                            theme::text(),
                                            egui::RichText::new(&reg.key_path).size(11.0),
                                        );
                                    } else {
                                        ui.colored_label(
                                            theme::text_3(),
                                            egui::RichText::new(&reg.key_path).size(11.0),
                                        );
                                        ui.colored_label(
                                            theme::danger(),
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
                                theme::safe(),
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
                                            theme::text(),
                                            egui::RichText::new(format!(
                                                "{} = {}",
                                                env.var_name, env.current_value
                                            ))
                                            .size(11.0),
                                        );
                                    } else {
                                        ui.colored_label(
                                            theme::text_3(),
                                            egui::RichText::new(format!(
                                                "{} = {}",
                                                env.var_name, env.current_value
                                            ))
                                            .size(11.0),
                                        );
                                        ui.colored_label(
                                            theme::danger(),
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
                                theme::caution(),
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
                                            theme::text(),
                                            egui::RichText::new(format!(
                                                "{} ({})",
                                                fs.path,
                                                format_size(fs.size)
                                            ))
                                            .size(11.0),
                                        );
                                    } else {
                                        ui.colored_label(
                                            theme::text_3(),
                                            egui::RichText::new(format!(
                                                "{} ({})",
                                                fs.path,
                                                format_size(fs.size)
                                            ))
                                            .size(11.0),
                                        );
                                        ui.colored_label(
                                            theme::danger(),
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
                    ui.colored_label(
                        theme::brand(),
                        egui::RichText::new("⏳ 正在清理残留...").size(12.0),
                    );
                    ctx.request_repaint_after(std::time::Duration::from_millis(50));
                }
            });
        });
}

/// 截断路径（按字符数，字节边界安全）
///
/// 原实现按字节索引切片（`&path[len - max + 3..]`），切点落在多字节 UTF-8
/// 字符（中文/emoji）中间会直接 panic（"byte index is not a char boundary"）。
/// 改为按字符数保留尾部 `max_len - 3` 个字符，前缀省略号，显示总字符数为 max_len。
pub(crate) fn truncate_path(path: &str, max_len: usize) -> String {
    let count = path.chars().count();
    if count <= max_len {
        return path.to_string();
    }
    let keep = max_len.saturating_sub(3);
    if keep == 0 {
        return "...".to_string();
    }
    let start = path
        .char_indices()
        .nth(count - keep)
        .map(|(i, _)| i)
        .unwrap_or(0);
    format!("...{}", &path[start..])
}

// =========================================================================
//  系统优化面板
// =========================================================================

/// 渲染系统优化面板（特殊 UI，不是列表选择模式）
/// 按类别聚合大小并降序排序；零大小类别剔除（返回空表时调用方直接跳过）。
fn aggregate_category_sizes(items: &[ScanItem]) -> Vec<(String, u64)> {
    let mut by_cat: Vec<(String, u64)> = Vec::new();
    for it in items {
        if let Some(entry) = by_cat.iter_mut().find(|(name, _)| *name == it.category) {
            entry.1 += it.size_bytes;
        } else {
            by_cat.push((it.category.clone(), it.size_bytes));
        }
    }
    by_cat.retain(|(_, sz)| *sz > 0);
    by_cat.sort_by(|a, b| b.1.cmp(&a.1));
    by_cat
}

/// 磁盘分析分类占比总览：按类别聚合大小，绘制横向堆叠条与图例
///
/// 每个目录层级都会基于当前 items 实时计算，随钻取同步更新；
/// 类别超过上限时归并为"其他"，保证条与图例可读。
fn render_disk_category_overview(ui: &mut egui::Ui, app: &mut App, items: &[ScanItem]) {
    if items.is_empty() {
        return;
    }
    let by_cat = aggregate_category_sizes(items);
    if by_cat.is_empty() {
        return;
    }
    let total: u64 = by_cat.iter().map(|(_, sz)| sz).sum();

    // 常量色板：8 色，随类别索引取模（避免大面积品牌色）
    const PALETTE: [egui::Color32; 8] = [
        egui::Color32::from_rgb(0x2F, 0x80, 0xED), // 蓝
        egui::Color32::from_rgb(0xE8, 0x5D, 0x4A), // 红
        egui::Color32::from_rgb(0x2E, 0xA0, 0x43), // 绿
        egui::Color32::from_rgb(0xF2, 0x9E, 0x38), // 橙
        egui::Color32::from_rgb(0x8E, 0x5C, 0xA2), // 紫
        egui::Color32::from_rgb(0x16, 0xA0, 0x85), // 青
        egui::Color32::from_rgb(0x7F, 0x8C, 0x8D), // 灰
        egui::Color32::from_rgb(0xB0, 0x6B, 0x3C), // 棕
    ];

    // 最多展示 7 个类别，其余归并为"其他"
    let mut shown: Vec<(String, u64)> = by_cat.into_iter().take(7).collect();
    let rest: u64 = items.iter().map(|i| i.size_bytes).sum::<u64>() - total;
    if rest > 0 {
        shown.push((app.t("others").to_string(), rest));
    }

    ui.label(
        egui::RichText::new(app.t("disk_category_overview"))
            .size(13.0)
            .color(theme::text_2()),
    );
    ui.add_space(4.0);

    // 堆叠条
    let bar_h = 22.0;
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(ui.available_width().min(760.0), bar_h),
        egui::Sense::hover(),
    );
    let painter = ui.painter();
    painter.rect_filled(rect, egui::Rounding::same(3.0), theme::line());
    let mut x = rect.min.x;
    for (i, (_, sz)) in shown.iter().enumerate() {
        let w = (*sz as f32 / total.max(1) as f32) * rect.width();
        if w < 1.0 {
            continue;
        }
        let seg = egui::Rect::from_min_max(
            egui::pos2(x, rect.min.y),
            egui::pos2((x + w).min(rect.max.x), rect.max.y),
        );
        painter.rect_filled(seg, egui::Rounding::same(3.0), PALETTE[i % PALETTE.len()]);
        x += w;
    }

    ui.add_space(6.0);

    // 图例：每行 色块 + 类别名 + 大小 + 占比
    let legend_w = (ui.available_width().min(760.0) - 12.0) / 2.0;
    egui::Grid::new("disk_category_legend")
        .num_columns(2)
        .min_col_width(0.0)
        .spacing([24.0, 4.0])
        .show(ui, |ui| {
            for (i, (name, sz)) in shown.iter().enumerate() {
                let pct = (*sz as f32 / total.max(1) as f32) * 100.0;
                ui.horizontal(|ui| {
                    let (sw, _) =
                        ui.allocate_exact_size(egui::vec2(10.0, 10.0), egui::Sense::hover());
                    ui.painter().rect_filled(
                        sw,
                        egui::Rounding::same(2.0),
                        PALETTE[i % PALETTE.len()],
                    );
                    ui.add_space(4.0);
                    ui.label(egui::RichText::new(name).size(12.0).color(theme::text()));
                    ui.label(
                        egui::RichText::new(format_size(*sz))
                            .size(12.0)
                            .color(theme::text_2()),
                    );
                    ui.label(
                        egui::RichText::new(format!("{:.1}%", pct))
                            .size(12.0)
                            .color(theme::text_3()),
                    );
                });
                if (i + 1) % 2 == 0 {
                    ui.end_row();
                } else {
                    // 占位让 Grid 对齐
                    ui.allocate_space(egui::vec2(legend_w, 0.0));
                }
            }
            if !shown.len().is_multiple_of(2) {
                ui.end_row();
            }
        });
}

/// 渲染磁盘分析器（目录钻取式磁盘浏览器）
///
/// 功能：
/// - 显示当前浏览路径（面包屑导航）
/// - 返回上一级按钮
/// - 列出当前目录下所有子项（按大小降序）
/// - 每项显示大小、进度条（相对于当前目录总大小）
/// - 目录可点击进入，文件可勾选删除
///
/// 注意：直接复用外层 CentralPanel 传入的 ui，避免嵌套 CentralPanel 导致状态异常
pub(crate) fn render_disk_analyzer(
    ui: &mut egui::Ui,
    app: &mut App,
    scan_rx: &mut Option<mpsc::Receiver<ScanMessage>>,
) {
    let tab_idx = app.tab_index();
    let items = app.results[tab_idx].clone();
    let is_scanning = app.tab_scanning(tab_idx);
    let current_path = app.disk_analyzer_current_path();
    let has_history = !app.disk_analyzer_history.is_empty();

    // ====== 面包屑导航栏 ======
    ui.horizontal(|ui| {
        // 返回上一级按钮
        let back_enabled = has_history && !is_scanning;
        if ui
            .add_enabled(back_enabled, egui::Button::new(app.t("back")))
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
            egui::RichText::new(display_path)
                .size(14.0)
                .color(theme::brand()),
        );

        ui.separator();

        // 主目录按钮（快速回到 home）
        if has_history && !is_scanning && ui.button(app.t("home")).clicked() {
            app.disk_analyzer_path = None;
            app.disk_analyzer_history.clear();
            start_scan(app, scan_rx);
        }
    });

    ui.add_space(5.0);

    if is_scanning {
        // 扫描中
        let scan_size = ui.available_size();
        egui::Frame::none()
            .fill(theme::bg())
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
                        .color(theme::brand()),
                    );
                    ui.add_space(15.0);
                    // 不确定进度：流动动画，不显示估算百分比
                    let bar_rect = egui::Rect::from_min_size(
                        ui.available_rect_before_wrap().min,
                        egui::vec2(ui.available_rect_before_wrap().width().min(500.0), 8.0),
                    );
                    let painter = ui.painter();
                    painter.rect_filled(bar_rect, egui::Rounding::same(4.0), theme::surface_3());
                    let time_f = ui.ctx().input(|i| i.time) as f32;
                    let seg_w = (bar_rect.width() * 0.35).max(40.0);
                    let phase = (time_f * 0.6).rem_euclid(1.0);
                    let seg_start = bar_rect.min.x + (bar_rect.width() - seg_w) * phase;
                    painter.rect_filled(
                        egui::Rect::from_min_size(
                            egui::pos2(seg_start, bar_rect.min.y),
                            egui::vec2(seg_w, bar_rect.height()),
                        ),
                        egui::Rounding::same(4.0),
                        theme::brand(),
                    );
                    ui.allocate_rect(bar_rect, egui::Sense::hover());
                });
            });
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_millis(100));
        return;
    }

    if items.is_empty() {
        let empty_size = ui.available_size();
        egui::Frame::none()
            .fill(theme::bg())
            .rounding(egui::Rounding::same(8.0))
            .show(ui, |ui| {
                ui.set_min_size(empty_size);
                ui.vertical_centered(|ui| {
                    ui.add_space(80.0);
                    ui.label(
                        egui::RichText::new(app.t("no_large_files"))
                            .size(16.0)
                            .color(theme::text_3()),
                    );
                    ui.add_space(10.0);
                    if ui
                        .button(egui::RichText::new(app.t("rescan")).size(16.0))
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
            theme::brand(),
            app.tf(
                "items_total_size",
                &[&items.len().to_string(), &format_size(total_size)],
            ),
        );
        if selected_cnt > 0 {
            ui.separator();
            ui.colored_label(
                theme::caution(),
                app.tf(
                    "selected_count_size",
                    &[&selected_cnt.to_string(), &format_size(selected_sz)],
                ),
            );
        }
    });

    ui.add_space(5.0);

    // ====== 分类占比总览（P2：按类别聚合的堆叠条 + 图例） ======
    render_disk_category_overview(ui, app, &items);

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
                    .color(theme::danger()),
                ),
            )
            .clicked()
        {
            app.prepare_delete();
        }
    });

    // ====== 视图切换：列表 / 矩形树图（P3） ======
    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new(app.t("view"))
                .size(12.0)
                .color(theme::text_3()),
        );
        let list_active = !app.disk_view_mode;
        if ui
            .selectable_label(list_active, app.t("disk_view_list"))
            .clicked()
        {
            app.disk_view_mode = false;
        }
        if ui
            .selectable_label(!list_active, app.t("disk_view_tree"))
            .clicked()
        {
            app.disk_view_mode = true;
        }
    });

    ui.add_space(5.0);

    if app.disk_view_mode {
        render_disk_treemap(ui, app, &items, total_size, is_scanning, scan_rx);
        return;
    }

    ui.add_space(5.0);

    // ====== 目录项列表 ======
    let max_size = items.first().map(|i| i.size_bytes).unwrap_or(1).max(1);
    let list_size = ui.available_size();

    egui::Frame::none()
        .fill(theme::bg())
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

                        let icon = if is_dir {
                            icons::Icon::Folder
                        } else {
                            icons::Icon::File
                        };
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
                            icons::show(ui, icon, 16.0, theme::text_3());
                            let name_btn = ui.add(
                                egui::Label::new(egui::RichText::new(name).size(13.0))
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
                                    theme::danger()
                                } else if pct > 0.2 {
                                    theme::caution()
                                } else {
                                    theme::safe()
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
                                    .color(theme::text_3()),
                            );

                            // 目录：显示"进入"提示
                            if is_dir {
                                ui.label(egui::RichText::new("→").size(16.0).color(theme::brand()));
                            }
                        });

                        ui.separator();
                    }
                });
        });
}

/// 矩形树图渲染（P3 磁盘分析）
///
/// Squarified treemap：目录/文件按大小占比画矩形，颜色随占比从绿到红渐变；
/// 点击目录可进入下一级（与列表视图共用 disk_analyzer_enter）。
fn render_disk_treemap(
    ui: &mut egui::Ui,
    app: &mut App,
    items: &[crate::scanner::ScanItem],
    total_size: u64,
    is_scanning: bool,
    scan_rx: &mut Option<mpsc::Receiver<ScanMessage>>,
) {
    let list_size = ui.available_size();
    egui::Frame::none()
        .fill(theme::bg())
        .rounding(egui::Rounding::same(8.0))
        .show(ui, |ui| {
            ui.set_min_size(list_size);
            let rect = ui.max_rect().shrink(8.0);
            if rect.width() < 50.0 || rect.height() < 50.0 {
                return;
            }

            let data: Vec<(usize, f64)> = items
                .iter()
                .enumerate()
                .map(|(i, it)| (i, it.size_bytes as f64))
                .collect();
            let layout = squarified_layout(&data, rect);

            let painter = ui.painter();
            let hover_pos = ui.input(|i| i.pointer.hover_pos());
            let click = ui.input(|i| i.pointer.primary_clicked());

            for (idx, r) in &layout {
                let item = &items[*idx];
                let t = if total_size > 0 {
                    (item.size_bytes as f32 / total_size as f32).min(1.0)
                } else {
                    0.0
                };
                // 颜色：小占比偏绿，大占比偏红；选中项加深
                let mut color = lerp_color(theme::safe(), theme::danger(), t);
                if item.selected {
                    color = color.gamma_multiply(1.25);
                }
                painter.rect_filled(*r, 2.0, color);
                painter.rect_stroke(*r, 2.0, egui::Stroke::new(1.0, theme::bg()));

                // 空间足够才画名字
                if r.width() > 56.0 && r.height() > 16.0 {
                    let name = std::path::Path::new(&item.path)
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("?");
                    let text = if r.width() > 140.0 {
                        format!("{} · {}", name, format_size(item.size_bytes))
                    } else {
                        name.to_string()
                    };
                    painter.text(
                        r.center(),
                        egui::Align2::CENTER_CENTER,
                        text,
                        egui::FontId::proportional(11.0),
                        theme::text(),
                    );
                }

                // 点击进入目录
                if item.category == "目录" && !is_scanning && click {
                    if let Some(hover) = hover_pos {
                        if r.contains(hover) {
                            let new_path = std::path::PathBuf::from(&item.path);
                            app.disk_analyzer_enter(new_path);
                            start_scan(app, scan_rx);
                            return;
                        }
                    }
                }
            }

            // 空布局兜底：有数据却一个矩形都没画出来时提示
            if layout.is_empty() && !items.is_empty() {
                ui.vertical_centered(|ui| {
                    ui.add_space(20.0);
                    ui.label(egui::RichText::new(app.t("no_large_files")).size(14.0));
                });
            }
        });
}

/// Squarified treemap 布局
///
/// 输入：(item 下标, 大小) 列表与可用矩形；输出：(下标, 子矩形)。
/// 经典 squarified 算法：贪心构建"最长边优先、宽高比最优"的行。
fn squarified_layout(items: &[(usize, f64)], rect: egui::Rect) -> Vec<(usize, egui::Rect)> {
    if items.is_empty() {
        return Vec::new();
    }
    let total: f64 = items.iter().map(|(_, s)| *s).sum();
    if total <= 0.0 {
        return Vec::new();
    }
    let mut data: Vec<(usize, f64)> = items.to_vec();
    data.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

    let rect_area = (rect.width() * rect.height()) as f64;
    let mut out = Vec::new();
    let mut row: Vec<(usize, f64)> = Vec::new();
    let mut row_sum = 0.0f64;
    let mut cur = rect;

    while !data.is_empty() {
        let (idx, size) = data.remove(0);
        let area = size / total * rect_area;
        if row.is_empty() {
            row.push((idx, area));
            row_sum = area;
            continue;
        }
        let short = cur.width().min(cur.height()) as f64;
        let long = cur.width().max(cur.height()) as f64;
        let candidate = row_sum + area;
        if worst_ratio(&row, area, candidate, long, short)
            <= worst_ratio(&row, 0.0, row_sum, long, short)
        {
            row.push((idx, area));
            row_sum = candidate;
        } else {
            layout_row(&mut row, row_sum, &mut cur, &mut out);
            row.push((idx, area));
            row_sum = area;
        }
    }
    if !row.is_empty() {
        layout_row(&mut row, row_sum, &mut cur, &mut out);
    }
    out
}

/// 当前行的最差宽高比（含待加入项，add<=0 表示不含）
fn worst_ratio(row: &[(usize, f64)], add: f64, sum: f64, long: f64, short: f64) -> f64 {
    if row.is_empty() && add <= 0.0 {
        return f64::MAX;
    }
    if short <= 0.0 || long <= 0.0 {
        return f64::MAX;
    }
    let mut worst = 0.0f64;
    for (_, area) in row {
        let r1 = long * long * area / (sum * short * short);
        let r2 = sum * short * short / (long * long * area);
        let r = r1.max(r2);
        if r > worst {
            worst = r;
        }
    }
    if add > 0.0 {
        let r1 = long * long * add / (sum * short * short);
        let r2 = sum * short * short / (long * long * add);
        let r = r1.max(r2);
        if r > worst {
            worst = r;
        }
    }
    worst
}

/// 把一行矩形铺进当前可用区域（沿短边方向）
fn layout_row(
    row: &mut Vec<(usize, f64)>,
    sum: f64,
    cur: &mut egui::Rect,
    out: &mut Vec<(usize, egui::Rect)>,
) {
    if row.is_empty() || sum <= 0.0 {
        row.clear();
        return;
    }
    let (x0, y0, w, h) = (cur.min.x, cur.min.y, cur.width(), cur.height());
    if h >= w {
        // 垂直切分：占满当前宽度，按比例分配高度
        let mut y = y0;
        for (idx, area) in row.iter() {
            let rh = (area / sum * h as f64) as f32;
            out.push((
                *idx,
                egui::Rect::from_min_size(egui::pos2(x0, y), egui::vec2(w, rh)),
            ));
            y += rh;
        }
        cur.min.x += w;
    } else {
        // 水平切分
        let mut x = x0;
        for (idx, area) in row.iter() {
            let rw = (area / sum * w as f64) as f32;
            out.push((
                *idx,
                egui::Rect::from_min_size(egui::pos2(x, y0), egui::vec2(rw, h)),
            ));
            x += rw;
        }
        cur.min.y += h;
    }
    row.clear();
}

/// 两个颜色按 t 线性插值（0.0 → a，1.0 → b）
fn lerp_color(a: egui::Color32, b: egui::Color32, t: f32) -> egui::Color32 {
    let t = t.clamp(0.0, 1.0);
    egui::Color32::from_rgb(
        (a.r() as f32 + (b.r() as f32 - a.r() as f32) * t).round() as u8,
        (a.g() as f32 + (b.g() as f32 - a.g() as f32) * t).round() as u8,
        (a.b() as f32 + (b.b() as f32 - a.b() as f32) * t).round() as u8,
    )
}

/// 路径显示简化（用于扫描中提示）
pub(crate) fn display_path_short(path: &std::path::Path, lang_en: bool) -> String {
    let home = scanner::home_dir();
    if path == home.as_path() {
        App::t_lang(lang_en, "home").to_string()
    } else if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
        name.to_string()
    } else {
        path.display().to_string()
    }
}

/// 概览 Tab 底部删除栏
///
/// 显示当前在概览页中选中的推荐项，并提供删除/取消入口。
/// 该函数由外层 TopBottomPanel 调用，确保底部栏始终固定在窗口底部。
pub(crate) fn render_overview_footer(ui: &mut egui::Ui, app: &mut App) {
    // 重新聚合所有 Tab 的 Safe/CacheOnly 推荐项（与 render_overview_panel 保持一致）
    let mut recommendation_items: Vec<(usize, usize, u64)> = Vec::new();
    for tab_idx in 1..app.results.len() {
        for (item_idx, item) in app.results[tab_idx].iter().enumerate() {
            if !item.deletable {
                continue;
            }
            if matches!(
                item.recommend,
                crate::scanner::Recommend::Safe | crate::scanner::Recommend::CacheOnly
            ) {
                recommendation_items.push((tab_idx, item_idx, item.size_bytes));
            }
        }
    }

    let selected_in_overview: Vec<(usize, usize)> = recommendation_items
        .iter()
        .filter(|(tab_idx, item_idx, _)| {
            app.results[*tab_idx]
                .get(*item_idx)
                .map(|i| i.selected)
                .unwrap_or(false)
        })
        .map(|(tab_idx, item_idx, _)| (*tab_idx, *item_idx))
        .collect();
    let selected_cnt = selected_in_overview.len();
    let selected_sz: u64 = selected_in_overview
        .iter()
        .map(|(tab_idx, item_idx)| {
            app.results[*tab_idx]
                .get(*item_idx)
                .map(|i| i.size_bytes)
                .unwrap_or(0)
        })
        .sum();

    egui::Frame::none()
        .fill(theme::surface())
        .stroke(egui::Stroke::new(1.0_f32, theme::line()))
        .inner_margin(egui::Margin::symmetric(16.0, 10.0))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.set_width(ui.available_width());

                if selected_cnt == 0 {
                    ui.colored_label(
                        theme::text_3(),
                        egui::RichText::new("未选择任何项目").size(13.0),
                    );
                } else {
                    ui.colored_label(
                        theme::text(),
                        egui::RichText::new(format!(
                            "已选中 {} 项 · 可释放 {}",
                            selected_cnt,
                            format_size(selected_sz)
                        ))
                        .size(13.0)
                        .strong(),
                    );
                }

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let delete_enabled =
                        selected_cnt > 0 && matches!(app.confirm, ConfirmState::None);
                    let delete_btn = ui.add_enabled(
                        delete_enabled,
                        egui::Button::new(
                            egui::RichText::new(format!(
                                "{} {}",
                                app.t("delete"),
                                format_size(selected_sz)
                            ))
                            .color(egui::Color32::WHITE)
                            .size(13.0),
                        )
                        .fill(theme::danger())
                        .rounding(egui::Rounding::same(8.0))
                        .min_size([0.0, 32.0].into()),
                    );
                    if delete_btn.clicked() {
                        app.prepare_delete_cross_tab(selected_in_overview.clone());
                    }

                    ui.add_space(8.0);

                    if ui
                        .button(egui::RichText::new(app.t("cancel")).color(theme::text_2()))
                        .clicked()
                    {
                        for (tab_idx, item_idx) in &selected_in_overview {
                            if let Some(item) = app.results[*tab_idx].get_mut(*item_idx) {
                                item.selected = false;
                            }
                        }
                    }
                });
            });
        });
}

/// 概览面板：聚合所有 Tab 的推荐清理项
pub(crate) fn render_overview_panel(
    ui: &mut egui::Ui,
    app: &mut App,
    _scan_rx: &mut Option<mpsc::Receiver<ScanMessage>>,
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
    recommendation_items.sort_by_key(|a| std::cmp::Reverse(a.2.size_bytes));

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
            .fill(theme::safe_50())
            .stroke(egui::Stroke::new(1.0_f32, theme::safe()))
            .rounding(egui::Rounding::same(14.0))
            .inner_margin(egui::Margin::symmetric(16.0, 10.0))
            .show(ui, |ui| {
                ui.vertical(|ui| {
                    ui.colored_label(
                        theme::text_2(),
                        egui::RichText::new(app.t("overview_releasable")).size(12.0),
                    );
                    ui.add_space(2.0);
                    ui.horizontal(|ui| {
                        ui.colored_label(
                            theme::text(),
                            egui::RichText::new(format_size(safe_total))
                                .size(20.0)
                                .strong(),
                        );
                        ui.colored_label(
                            theme::safe(),
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
            .fill(theme::caution_50())
            .stroke(egui::Stroke::new(1.0_f32, theme::caution()))
            .rounding(egui::Rounding::same(14.0))
            .inner_margin(egui::Margin::symmetric(14.0, 10.0))
            .show(ui, |ui| {
                ui.vertical(|ui| {
                    ui.colored_label(
                        theme::text_2(),
                        egui::RichText::new(app.t("caution_clean")).size(12.0),
                    );
                    ui.add_space(2.0);
                    ui.colored_label(
                        theme::text(),
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
            .fill(theme::danger_50())
            .stroke(egui::Stroke::new(1.0_f32, theme::danger()))
            .rounding(egui::Rounding::same(14.0))
            .inner_margin(egui::Margin::symmetric(14.0, 10.0))
            .show(ui, |ui| {
                ui.vertical(|ui| {
                    ui.colored_label(
                        theme::text_2(),
                        egui::RichText::new(app.t("confirm_clean")).size(12.0),
                    );
                    ui.add_space(2.0);
                    ui.colored_label(
                        theme::text(),
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
            .color(theme::text()),
    );
    ui.add_space(4.0);
    ui.colored_label(
        theme::text_3(),
        egui::RichText::new(app.t("overview_recommendation_hint")).size(12.0),
    );
    ui.add_space(10.0);

    if any_scanning {
        let scan_size = ui.available_size();
        egui::Frame::none()
            .fill(theme::bg())
            .rounding(egui::Rounding::same(8.0))
            .show(ui, |ui| {
                ui.set_min_size(scan_size);
                ui_scanning(ui, app);
            });
    } else if recommendation_items.is_empty() {
        let _ = widgets::state_page(
            ui,
            icons::Icon::Sparkle,
            theme::brand(),
            theme::brand_50(),
            app.t("overview_empty_title"),
            app.t("overview_empty_hint"),
            None,
        );
    } else {
        // 一键清理按钮
        let total_safe_size: u64 = recommendation_items
            .iter()
            .map(|(_, _, item)| item.size_bytes)
            .sum();
        let clean_label = format!(
            "{} {} ({})",
            app.t("one_click_clean"),
            recommendation_items.len(),
            format_size(total_safe_size)
        );
        let clean_btn = widgets::button(
            ui,
            Some(icons::Icon::Trash),
            &clean_label,
            widgets::Btn::Danger,
            BTN_H_LG,
        );
        if clean_btn.clicked() {
            let cross: Vec<(usize, usize)> = recommendation_items
                .iter()
                .map(|(tab_idx, item_idx, _)| (*tab_idx, *item_idx))
                .collect();
            one_click_clean(app, &cross);
        }
        ui.add_space(12.0);

        // 推荐项列表：占满 CentralPanel 剩余高度（底部删除栏已移到外层 TopBottomPanel）
        let list_size = egui::vec2(ui.available_width(), ui.available_height().max(0.0));
        egui::Frame::none()
            .fill(theme::bg())
            .rounding(egui::Rounding::same(8.0))
            .show(ui, |ui| {
                ui.set_min_size(list_size);
                egui::ScrollArea::vertical()
                    .auto_shrink([false; 2])
                    .show(ui, |ui| {
                        ui.add_space(10.0);
                        let mut toggled: std::collections::HashSet<(usize, usize)> =
                            std::collections::HashSet::new();
                        for &(tab_idx, item_idx, ref item) in &recommendation_items {
                            let tab = Tab::all()[tab_idx];
                            let tab_title_text = tab_title(&tab, app);
                            let selected = item.selected;

                            let frame_resp = egui::Frame::none()
                                .fill(theme::surface())
                                .stroke(egui::Stroke::new(1.0_f32, theme::line()))
                                .rounding(egui::Rounding::same(8.0))
                                .inner_margin(egui::Margin::symmetric(14.0, 12.0))
                                .show(ui, |ui| {
                                    ui.horizontal(|ui| {
                                        let checkbox_resp =
                                            render_custom_checkbox(ui, selected, true);
                                        if checkbox_resp.clicked() {
                                            toggled.insert((tab_idx, item_idx));
                                        }

                                        ui.add_space(10.0);
                                        ui.vertical(|ui| {
                                            ui.horizontal(|ui| {
                                                ui.colored_label(
                                                    theme::text(),
                                                    egui::RichText::new(&item.category)
                                                        .size(13.0)
                                                        .strong(),
                                                );
                                                ui.add_space(6.0);
                                                ui.colored_label(
                                                    theme::text_3(),
                                                    egui::RichText::new(format!(
                                                        "· {}",
                                                        tab_title_text
                                                    ))
                                                    .size(11.0),
                                                );
                                            });
                                            ui.colored_label(
                                                theme::text_2(),
                                                egui::RichText::new(truncate_path(&item.path, 70))
                                                    .size(11.0),
                                            );
                                            ui.colored_label(
                                                theme::text_2(),
                                                egui::RichText::new(&item.description).size(11.0),
                                            );
                                        });
                                        ui.with_layout(
                                            egui::Layout::right_to_left(egui::Align::Center),
                                            |ui| {
                                                ui.colored_label(
                                                    theme::text(),
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

                            // Frame 默认不响应点击，需手动分配整格可点击区域
                            let cell_click = ui.interact(
                                frame_resp.response.rect,
                                egui::Id::new(("overview_item_click", tab_idx, item_idx)),
                                egui::Sense::click(),
                            );
                            if cell_click.clicked() {
                                toggled.insert((tab_idx, item_idx));
                            }

                            ui.add_space(6.0);
                        }

                        // 应用选中变更到实际结果
                        for (tab_idx, item_idx) in toggled {
                            if let Some(item) = app.results[tab_idx].get_mut(item_idx) {
                                item.selected = !item.selected;
                            }
                        }

                        ui.add_space(10.0);
                    });
            });
    }
}

pub(crate) fn render_optimize_panel(
    ui: &mut egui::Ui,
    app: &mut App,
    scan_rx: &mut Option<mpsc::Receiver<ScanMessage>>,
) {
    let tab_idx = app.tab_index();
    let items = app.results[tab_idx].clone();
    let is_scanning = app.tab_scanning(tab_idx);

    if is_scanning {
        let scan_size = ui.available_size();
        egui::Frame::none()
            .fill(theme::bg())
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
            .fill(theme::bg())
            .rounding(egui::Rounding::same(8.0))
            .show(ui, |ui| {
                ui.set_min_size(empty_size);
                ui.vertical_centered(|ui| {
                    ui.add_space(80.0);
                    ui.label(
                        egui::RichText::new(app.t("optimize_click_to_scan"))
                            .size(16.0)
                            .color(theme::text_3()),
                    );
                    ui.add_space(10.0);
                    let scan_button = app.t("scan");
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
            .color(theme::text_3()),
    );
    ui.add_space(10.0);

    // 优化任务列表：占满剩余高度
    let mut task_to_run: Option<usize> = None;
    let list_size = ui.available_size();

    egui::Frame::none()
        .fill(theme::bg())
        .rounding(egui::Rounding::same(8.0))
        .show(ui, |ui| {
            ui.set_min_size(list_size);
            egui::ScrollArea::vertical()
                .auto_shrink([false; 2])
                .show(ui, |ui| {
                    ui.add_space(10.0);
                    for (i, item) in items.iter().enumerate() {
                        egui::Frame::group(ui.style())
                            .fill(theme::surface())
                            .stroke(egui::Stroke::new(1.0_f32, theme::line()))
                            .inner_margin(12.0)
                            .outer_margin(4.0)
                            .show(ui, |ui| {
                                ui.horizontal(|ui| {
                                    // 图标
                                    icons::show(ui, icons::Icon::Sparkle, 20.0, theme::brand());
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
                                            widgets::recommend_badge(
                                                ui,
                                                &item.recommend,
                                                app.lang_en,
                                            );
                                        });
                                        let desc_en = i18n::translate_description(
                                            &item.description,
                                            app.lang_en,
                                        );
                                        ui.label(
                                            egui::RichText::new(&desc_en)
                                                .size(12.0)
                                                .color(theme::text_2()),
                                        );
                                    });
                                    ui.with_layout(
                                        egui::Layout::right_to_left(egui::Align::Center),
                                        |ui| {
                                            // 该任务正在后台执行：按钮禁用并显示「执行中…」。
                                            // 耗时任务线程化后 GUI 不冻结，但同一时刻只跑一个。
                                            let is_running = app.optimize_running.as_deref()
                                                == Some(item.path.as_str());
                                            let run_text = if is_running {
                                                app.t("optimize_running").to_string()
                                            } else if app.lang_en {
                                                "Run".to_string()
                                            } else {
                                                "执行".to_string()
                                            };
                                            let run_btn = widgets::button_enabled(
                                                ui,
                                                !is_running,
                                                Some(icons::Icon::ChevronRight),
                                                &run_text,
                                                widgets::Btn::Secondary,
                                                BTN_H,
                                            );
                                            if run_btn.clicked() && app.optimize_running.is_none() {
                                                // P1：Advanced 级维护任务先确认再执行
                                                if item.recommend
                                                    == crate::scanner::Recommend::Advanced
                                                {
                                                    app.pending_optimize_task = Some(i);
                                                } else {
                                                    task_to_run = Some(i);
                                                }
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
                                                .color(theme::text_2()),
                                        );
                                    }
                                });
                        });
                    }
                    ui.add_space(10.0);
                });
        });

    // P1：高风险维护任务确认弹窗
    if let Some(pending_idx) = app.pending_optimize_task {
        // 已有任务在后台执行时不再弹新确认（避免重复执行/状态错乱）
        if app.optimize_running.is_some() {
            app.pending_optimize_task = None;
        } else if let Some(pending_item) = items.get(pending_idx) {
            egui::Window::new("confirm_optimize_modal")
                .title_bar(false)
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .frame(widgets::modal_frame())
                .show(ui.ctx(), |ui| {
                    ui.set_min_width(MODAL_W);
                    ui.set_max_width(MODAL_W);
                    widgets::modal_header(
                        ui,
                        icons::Icon::Alert,
                        theme::danger(),
                        theme::danger_50(),
                        app.t("optimize_confirm_title"),
                        app.t("optimize_confirm_body"),
                    );
                    ui.add_space(12.0);
                    egui::Frame::none()
                        .fill(theme::surface_3())
                        .stroke(egui::Stroke::NONE)
                        .rounding(egui::Rounding::same(8.0))
                        .inner_margin(egui::Margin::same(12.0))
                        .show(ui, |ui| {
                            render_confirm_row(
                                ui,
                                app.t("confirm_selected_items"),
                                &pending_item.path,
                                theme::text(),
                            );
                            render_confirm_row(
                                ui,
                                app.t("confirm_releasable"),
                                &pending_item.description,
                                theme::brand(),
                            );
                        });
                    ui.add_space(16.0);
                    ui.horizontal(|ui| {
                        if widgets::button(
                            ui,
                            None,
                            app.t("optimize_confirm_no"),
                            widgets::Btn::Secondary,
                            BTN_H,
                        )
                        .clicked()
                        {
                            app.pending_optimize_task = None;
                        }
                        if widgets::button(
                            ui,
                            Some(icons::Icon::ChevronRight),
                            app.t("optimize_confirm_yes"),
                            widgets::Btn::Primary,
                            BTN_H,
                        )
                        .clicked()
                        {
                            task_to_run = Some(pending_idx);
                            app.pending_optimize_task = None;
                        }
                    });
                });
        }
    }

    // 执行选中的优化任务
    if let Some(task_idx) = task_to_run {
        if let Some(item) = items.get(task_idx) {
            // P1-1: Windows 优化任务执行前自动创建系统还原点（20h 频率限制，开关控制）
            #[cfg(target_os = "windows")]
            if app.settings_auto_restore_point {
                let (ok, msg) = platform::windows_backup::ensure_restore_point(false);
                let text = match (ok, msg.as_str()) {
                    (true, "created") => App::t_lang(app.lang_en, "restore_point_created"),
                    (true, _) => App::t_lang(app.lang_en, "restore_point_skipped"),
                    (false, _) => App::t_lang(app.lang_en, "restore_point_failed"),
                };
                app.logs.push(text.to_string());
            }
            // 耗时任务（chmod -R ~/Library / diskutil verifyVolume / 等）一律
            // 后台线程执行：GUI 主线程同步跑 `.output()` 会阻塞 egui 事件循环，
            // 界面冻结、无法点取消（历史 bug）。结果经 optimize_rx 回传写日志。
            if app.optimize_running.is_none() {
                let rx = start_optimize_task(item.path.clone(), app.lang_en);
                app.optimize_rx = Some(rx);
                app.optimize_running = Some(item.path.clone());
                app.logs
                    .push(App::t_lang(app.lang_en, "optimize_running").to_string());
            } else {
                app.logs
                    .push(App::t_lang(app.lang_en, "optimize_busy").to_string());
            }
        }
    }
}

/// 启动项管理面板（macOS Login Items / launchd）
///
/// 同步扫描（不走统一扫描管道），首次进入自动扫描；支持刷新、禁用/启用。
/// 禁用 = 把 plist 移到 `~/.maclean/disabled_launchd/<scope>`（可逆）；
/// 不 bootout 运行中的服务，避免误杀系统组件。
fn render_startup_panel(ui: &mut egui::Ui, app: &mut App) {
    use crate::scanner::startup::{disable_startup_item, enable_startup_item, scan_startup_items};

    // 首次进入或列表为空时自动扫描
    if app.startup_items.is_empty() {
        app.startup_items = scan_startup_items();
    }

    ui.add_space(10.0);
    ui.heading(egui::RichText::new(app.t("tab_startup_items")).size(18.0));
    ui.label(
        egui::RichText::new(app.t("startup_hint"))
            .size(12.0)
            .color(theme::text_3()),
    );
    ui.add_space(8.0);

    // 操作栏：刷新 + 说明
    ui.horizontal(|ui| {
        if ui.button(app.t("startup_refresh")).clicked() {
            app.startup_items = scan_startup_items();
            app.logs.push(app.t("startup_refreshed").to_string());
        }
        ui.separator();
        let enabled_cnt = app.startup_items.iter().filter(|i| i.enabled).count();
        ui.label(
            egui::RichText::new(app.tf(
                "startup_count",
                &[
                    &app.startup_items.len().to_string(),
                    &enabled_cnt.to_string(),
                ],
            ))
            .size(12.0)
            .color(theme::text_2()),
        );
    });

    ui.add_space(10.0);

    // 列表
    let list_size = ui.available_size();
    egui::Frame::none()
        .fill(theme::bg())
        .rounding(egui::Rounding::same(8.0))
        .show(ui, |ui| {
            ui.set_min_size(list_size);
            egui::ScrollArea::vertical()
                .auto_shrink([false; 2])
                .show(ui, |ui| {
                    ui.add_space(8.0);
                    // 克隆避免循环内可变借用 app
                    let startup_snapshot = app.startup_items.clone();
                    for item in &startup_snapshot {
                        egui::Frame::group(ui.style())
                            .fill(theme::surface())
                            .stroke(egui::Stroke::new(1.0_f32, theme::line()))
                            .inner_margin(12.0)
                            .outer_margin(4.0)
                            .show(ui, |ui| {
                                ui.horizontal(|ui| {
                                    icons::show(ui, icons::Icon::Power, 18.0, theme::brand());
                                    ui.vertical(|ui| {
                                        ui.horizontal(|ui| {
                                            ui.label(
                                                egui::RichText::new(&item.label)
                                                    .size(14.0)
                                                    .strong()
                                                    .color(theme::text()),
                                            );
                                            // 状态徽标
                                            if item.enabled {
                                                ui.label(
                                                    egui::RichText::new(app.t("startup_enabled"))
                                                        .size(11.0)
                                                        .color(theme::safe()),
                                                );
                                            } else {
                                                ui.label(
                                                    egui::RichText::new(app.t("startup_disabled"))
                                                        .size(11.0)
                                                        .color(theme::text_3()),
                                                );
                                            }
                                        });
                                        ui.label(
                                            egui::RichText::new(item.plist.display().to_string())
                                                .size(11.0)
                                                .color(theme::text_3()),
                                        );
                                    });
                                    ui.with_layout(
                                        egui::Layout::right_to_left(egui::Align::Center),
                                        |ui| {
                                            if item.enabled {
                                                if ui
                                                    .button(app.t("startup_disable"))
                                                    .on_hover_text(item.plist.display().to_string())
                                                    .clicked()
                                                {
                                                    match disable_startup_item(
                                                        item,
                                                        &app.startup_backup_root,
                                                    ) {
                                                        Ok(target) => {
                                                            app.logs.push(app.tf(
                                                                "startup_disabled_log",
                                                                &[&item.label, &target],
                                                            ));
                                                            app.startup_items =
                                                                scan_startup_items();
                                                        }
                                                        Err(e) => {
                                                            app.logs.push(format!(
                                                                "{}: {e}",
                                                                app.t("startup_disable_fail")
                                                            ));
                                                        }
                                                    }
                                                }
                                            } else if ui
                                                .button(app.t("startup_enable"))
                                                .on_hover_text(item.plist.display().to_string())
                                                .clicked()
                                            {
                                                match enable_startup_item(
                                                    item,
                                                    &app.startup_backup_root,
                                                ) {
                                                    Ok(target) => {
                                                        app.logs.push(app.tf(
                                                            "startup_enabled_log",
                                                            &[&item.label, &target],
                                                        ));
                                                        app.startup_items = scan_startup_items();
                                                    }
                                                    Err(e) => {
                                                        app.logs.push(format!(
                                                            "{}: {e}",
                                                            app.t("startup_enable_fail")
                                                        ));
                                                    }
                                                }
                                            }
                                        },
                                    );
                                });
                            });
                    }
                    ui.add_space(8.0);
                });
        });

    // 操作日志
    if !app.logs.is_empty() {
        ui.add_space(5.0);
        ui.collapsing(app.t("optimize_logs"), |ui| {
            egui::ScrollArea::vertical()
                .max_height(120.0)
                .show(ui, |ui| {
                    for log in &app.logs {
                        ui.label(egui::RichText::new(log).size(11.0).color(theme::text_2()));
                    }
                });
        });
    }
}

/// 设置面板（设计稿 4.6 样式）
pub(crate) fn render_settings_panel(ui: &mut egui::Ui, app: &mut App) {
    // 入口快照：用于离开时检测设置变更并自动保存（P2-2）
    let settings_snapshot = (
        app.lang_en,
        app.settings_menubar_icon,
        app.settings_keep_sudo,
        app.settings_scan_cache,
        app.settings_scan_all_disks,
        app.settings_prefer_official_uninstaller,
        app.settings_show_protected_items,
        app.schedule_enabled,
        app.schedule_interval_days,
        app.settings_confirm_advanced,
        app.settings_prevent_lid_close,
        app.settings_auto_restore_point,
    );

    let settings_size = ui.available_size();
    egui::Frame::none()
        .fill(theme::bg())
        .rounding(egui::Rounding::same(8.0))
        .show(ui, |ui| {
            ui.set_min_size(settings_size);
            egui::ScrollArea::vertical()
                .auto_shrink([false; 2])
                .show(ui, |ui| {
                    ui.add_space(10.0);

                    // 外观卡片：深色模式（切换即时生效，持久化到 config.json）
                    settings_card(ui, app.t("settings_appearance"), |ui| {
                        let mut dark = app.dark_mode;
                        ui.horizontal(|ui| {
                            ui.vertical(|ui| {
                                ui.colored_label(
                                    theme::text(),
                                    egui::RichText::new(app.t("setting_dark_mode"))
                                        .size(13.0)
                                        .strong(),
                                );
                                ui.colored_label(
                                    theme::text_2(),
                                    egui::RichText::new(app.t("setting_dark_mode_desc")).size(12.0),
                                );
                            });
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    if render_settings_toggle(ui, &mut dark, true).clicked() {
                                        app.toggle_dark_mode();
                                    }
                                },
                            );
                        });
                    });

                    ui.add_space(12.0);

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
                        // 只在 Windows 显示：macOS 没有盘符概念，显示了是噪音
                        #[cfg(target_os = "windows")]
                        render_settings_item(
                            ui,
                            app.t("setting_scan_all_disks"),
                            app.t("setting_scan_all_disks_desc"),
                            &mut app.settings_scan_all_disks,
                            true,
                        );
                        // M-1：只在 macOS 显示 —— 官方卸载器的识别针对 .app 包，
                        // Windows 走 UninstallString，是另一套机制
                        #[cfg(target_os = "macos")]
                        render_settings_item(
                            ui,
                            app.t("setting_official_uninstaller"),
                            app.t("setting_official_uninstaller_desc"),
                            &mut app.settings_prefer_official_uninstaller,
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
                        render_settings_item(
                            ui,
                            app.t("setting_show_protected_items"),
                            app.t("setting_show_protected_items_desc"),
                            &mut app.settings_show_protected_items,
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
                        // P1-1: 操作前自动创建还原点（Windows only）
                        let win_enabled = cfg!(target_os = "windows");
                        render_settings_item(
                            ui,
                            app.t("setting_auto_restore_point"),
                            app.t("setting_auto_restore_point_desc"),
                            &mut app.settings_auto_restore_point,
                            win_enabled,
                        );

                        // P1-3: 还原上次注册表修改入口（Windows only）
                        #[cfg(target_os = "windows")]
                        {
                            ui.horizontal(|ui| {
                                ui.vertical(|ui| {
                                    ui.colored_label(
                                        theme::text(),
                                        egui::RichText::new(app.t("setting_restore_last"))
                                            .size(13.0)
                                            .strong(),
                                    );
                                    ui.colored_label(
                                        theme::text_2(),
                                        egui::RichText::new(app.t("setting_restore_last_desc"))
                                            .size(12.0),
                                    );
                                    // 最近备份信息
                                    if let Some((time, source, count)) =
                                        platform::windows_backup::last_backup_summary()
                                    {
                                        let info = app.tf(
                                            "restore_last_backup_info",
                                            &[&time, &source, &count.to_string()],
                                        );
                                        ui.colored_label(
                                            theme::text_3(),
                                            egui::RichText::new(info).size(11.0),
                                        );
                                    }
                                    // 上次执行结果
                                    if let Some(ref result) = app.last_restore_result {
                                        ui.colored_label(
                                            theme::text_2(),
                                            egui::RichText::new(result).size(11.0),
                                        );
                                    }
                                });
                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| {
                                        if ui
                                            .add(
                                                egui::Button::new(
                                                    egui::RichText::new(app.t("restore_last_btn"))
                                                        .size(13.0),
                                                )
                                                .fill(theme::surface())
                                                .stroke(egui::Stroke::new(1.0_f32, theme::line()))
                                                .rounding(egui::Rounding::same(8.0)),
                                            )
                                            .clicked()
                                        {
                                            let (ok, msg) =
                                                platform::windows_backup::restore_last_backup();
                                            app.last_restore_result =
                                                Some(match (ok, msg.as_str()) {
                                                    (true, m) => {
                                                        app.tf("restore_last_success", &[m])
                                                    }
                                                    (false, "none") => {
                                                        app.t("restore_last_none").to_string()
                                                    }
                                                    _ => app.t("restore_last_failed").to_string(),
                                                });
                                        }
                                    },
                                );
                            });
                        }
                    });

                    ui.add_space(12.0);

                    // C-3：定时清理卡片
                    settings_card(ui, app.t("settings_schedule"), |ui| {
                        // 等级闸门（C-1）：定时清理是付费能力
                        let tier_ok = crate::license::current_tier().allows_scheduled_cleanup();
                        if !tier_ok {
                            ui.colored_label(
                                theme::caution(),
                                egui::RichText::new(app.t("schedule_pro_only")).size(12.0),
                            );
                            ui.add_space(6.0);
                        }

                        let before_enabled = app.schedule_enabled;
                        let before_interval = app.schedule_interval_days;
                        render_settings_item(
                            ui,
                            app.t("setting_schedule_enable"),
                            app.t("setting_schedule_enable_desc"),
                            &mut app.schedule_enabled,
                            tier_ok,
                        );

                        ui.add_space(6.0);
                        ui.colored_label(
                            theme::text(),
                            egui::RichText::new(app.t("setting_schedule_interval")).size(13.0),
                        );
                        ui.horizontal(|ui| {
                            for d in crate::scheduler::INTERVAL_OPTIONS {
                                let label = match d {
                                    1 => app.t("schedule_every_day"),
                                    7 => app.t("schedule_every_week"),
                                    _ => app.t("schedule_every_month"),
                                };
                                if ui
                                    .selectable_label(app.schedule_interval_days == d, label)
                                    .clicked()
                                    && tier_ok
                                {
                                    app.schedule_interval_days = d;
                                }
                            }
                        });

                        ui.add_space(6.0);
                        let last_text = if app.schedule_last_run == 0 {
                            app.t("schedule_never_run").to_string()
                        } else {
                            app.tf(
                                "schedule_last_run",
                                &[&crate::cli::format_timestamp(app.schedule_last_run)],
                            )
                        };
                        ui.colored_label(
                            theme::text_2(),
                            egui::RichText::new(last_text).size(11.0),
                        );
                        if let Some(ref r) = app.schedule_result {
                            ui.colored_label(theme::text_2(), egui::RichText::new(r).size(11.0));
                        }

                        // 开关或间隔变了才去动系统任务：每次保存都注册一遍，
                        // launchd / schtasks 都会报"已存在"，噪音且没意义。
                        if tier_ok
                            && (app.schedule_enabled != before_enabled
                                || app.schedule_interval_days != before_interval)
                        {
                            app.apply_schedule();
                        }
                    });

                    ui.add_space(12.0);

                    // 语言设置卡片
                    settings_card(ui, app.t("settings_language"), |ui| {
                        ui.horizontal(|ui| {
                            ui.vertical(|ui| {
                                ui.colored_label(
                                    theme::text(),
                                    egui::RichText::new(app.t("setting_language"))
                                        .size(13.0)
                                        .strong(),
                                );
                                ui.colored_label(
                                    theme::text_2(),
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
                                            .fill(theme::surface())
                                            .stroke(egui::Stroke::new(1.0_f32, theme::line()))
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

                    ui.add_space(12.0);

                    // P2-2: 配置管理卡片（导入/导出 + 配置目录）
                    settings_card(ui, app.t("settings_config_mgmt"), |ui| {
                        ui.horizontal(|ui| {
                            ui.vertical(|ui| {
                                ui.colored_label(
                                    theme::text(),
                                    egui::RichText::new(app.t("config_dir_label"))
                                        .size(13.0)
                                        .strong(),
                                );
                                ui.colored_label(
                                    theme::text_3(),
                                    egui::RichText::new(config::config_dir().display().to_string())
                                        .size(11.0),
                                );
                                ui.colored_label(
                                    theme::text_2(),
                                    egui::RichText::new(app.t("config_apps_hint")).size(11.0),
                                );
                                // 上次执行结果
                                if let Some(ref result) = app.config_manage_result {
                                    ui.colored_label(
                                        theme::text_2(),
                                        egui::RichText::new(result).size(11.0),
                                    );
                                }
                            });
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    if ui
                                        .button(
                                            egui::RichText::new(app.t("config_open_btn"))
                                                .size(12.0),
                                        )
                                        .clicked()
                                    {
                                        config::open_in_file_manager(&config::config_dir());
                                    }
                                    ui.add_space(6.0);
                                    if ui
                                        .button(
                                            egui::RichText::new(app.t("config_import_btn"))
                                                .size(12.0),
                                        )
                                        .clicked()
                                    {
                                        let dir = config::default_export_dir();
                                        let (ok, msg) = config::import_config(&dir);
                                        app.config_manage_result = Some(match (ok, msg.as_str()) {
                                            (true, m) => app.tf("config_import_success", &[m]),
                                            (false, "no_config_found") => {
                                                app.t("config_no_config_found").to_string()
                                            }
                                            (false, m) => app.tf("config_import_failed", &[m]),
                                        });
                                    }
                                    ui.add_space(6.0);
                                    if ui
                                        .button(
                                            egui::RichText::new(app.t("config_export_btn"))
                                                .size(12.0),
                                        )
                                        .clicked()
                                    {
                                        // 导出前先落盘当前设置，保证导出内容最新
                                        app.save_settings();
                                        let dir = config::default_export_dir();
                                        let dir_display = dir.display().to_string();
                                        let (ok, msg) = config::export_config(&dir);
                                        app.config_manage_result = Some(match (ok, msg.as_str()) {
                                            (true, _) => {
                                                app.tf("config_export_success", &[&dir_display])
                                            }
                                            (false, "nothing_to_export") => {
                                                app.t("config_nothing_to_export").to_string()
                                            }
                                            (false, m) => app.tf("config_export_failed", &[m]),
                                        });
                                    }
                                },
                            );
                        });
                    });

                    ui.add_space(12.0);

                    // Pro 授权卡片
                    let lic_status = app.license_status.clone();
                    settings_card(
                        ui,
                        if app.lang_en {
                            "Pro License"
                        } else {
                            "Pro 授权"
                        },
                        |ui| match &lic_status {
                            crate::license::LicenseStatus::Activated { email, plan } => {
                                let is_dev = crate::license::is_dev_mode();
                                ui.colored_label(
                                    if is_dev {
                                        theme::caution()
                                    } else {
                                        theme::safe()
                                    },
                                    egui::RichText::new(if is_dev {
                                        if app.lang_en {
                                            "Developer mode"
                                        } else {
                                            "开发者模式"
                                        }
                                    } else if app.lang_en {
                                        "Pro activated"
                                    } else {
                                        "Pro 版已激活"
                                    })
                                    .size(13.0)
                                    .strong(),
                                );
                                let plan_label = if plan == "lifetime" {
                                    if app.lang_en {
                                        "Lifetime"
                                    } else {
                                        "终身版"
                                    }
                                } else {
                                    plan.as_str()
                                };
                                ui.colored_label(
                                    theme::text_2(),
                                    egui::RichText::new(format!("{} · {}", email, plan_label))
                                        .size(12.0),
                                );
                                // C-1：等级由 plan 推导，不是另一个硬编码布尔
                                ui.colored_label(
                                    theme::text_2(),
                                    egui::RichText::new(format!(
                                        "{} · {}",
                                        app.t(crate::license::PlanTier::from_plan(plan)
                                            .label_key()),
                                        app.t("tier_capability_unlimited")
                                    ))
                                    .size(11.0),
                                );
                                ui.add_space(8.0);
                                if ui
                                    .button(
                                        egui::RichText::new(if app.lang_en {
                                            "Deactivate"
                                        } else {
                                            "解除激活"
                                        })
                                        .size(12.0),
                                    )
                                    .clicked()
                                {
                                    crate::license::deactivate();
                                    app.license_status = crate::license::LicenseStatus::Free;
                                }
                            }
                            crate::license::LicenseStatus::Free => {
                                let remaining = crate::license::quota_remaining();
                                let total = crate::license::FREE_CLEAN_QUOTA_BYTES;
                                let used_ratio = 1.0 - (remaining as f32 / total as f32);
                                ui.colored_label(
                                    theme::text(),
                                    egui::RichText::new(if app.lang_en {
                                        "Free plan"
                                    } else {
                                        "免费版"
                                    })
                                    .size(13.0)
                                    .strong(),
                                );
                                ui.colored_label(
                                    theme::text_2(),
                                    egui::RichText::new(if app.lang_en {
                                        format!(
                                            "Cleanup quota remaining: {} / {}",
                                            crate::scanner::format_size(remaining),
                                            crate::scanner::format_size(total)
                                        )
                                    } else {
                                        format!(
                                            "清理额度剩余：{} / {}",
                                            crate::scanner::format_size(remaining),
                                            crate::scanner::format_size(total)
                                        )
                                    })
                                    .size(12.0),
                                );
                                ui.add_space(4.0);
                                ui.add(
                                    egui::ProgressBar::new(used_ratio).desired_height(4.0).fill(
                                        if used_ratio > 0.9 {
                                            theme::danger()
                                        } else {
                                            theme::safe()
                                        },
                                    ),
                                );
                                ui.add_space(10.0);

                                // C-1：把"付费能多得到什么"说出来。
                                // 只显示一个进度条，用户根本不知道付钱买到的是什么 ——
                                // 那不是定价模糊，是没定价。
                                {
                                    let caps: Vec<String> = vec![
                                        app.t("tier_capability_unlimited").to_string(),
                                        app.t("tier_capability_scheduled").to_string(),
                                    ];
                                    ui.colored_label(
                                        theme::text_2(),
                                        egui::RichText::new(App::tf_lang(
                                            app.lang_en,
                                            "tier_pro_includes",
                                            &[&caps.join(" · ")],
                                        ))
                                        .size(11.0),
                                    );
                                }
                                ui.add_space(10.0);

                                ui.add(
                                    egui::TextEdit::singleline(&mut app.license_input)
                                        .desired_width(f32::INFINITY)
                                        .hint_text("MACL-..."),
                                );
                                if let Some(err) = &app.license_error {
                                    ui.add_space(4.0);
                                    ui.colored_label(
                                        theme::danger(),
                                        egui::RichText::new(err).size(12.0),
                                    );
                                }
                                ui.add_space(8.0);
                                ui.horizontal(|ui| {
                                    if ui
                                        .button(
                                            egui::RichText::new(if app.lang_en {
                                                "Activate"
                                            } else {
                                                "激活"
                                            })
                                            .size(13.0),
                                        )
                                        .clicked()
                                    {
                                        app.try_activate_license();
                                    }
                                    if ui
                                        .button(
                                            egui::RichText::new(if app.lang_en {
                                                "Buy Pro"
                                            } else {
                                                "购买 Pro"
                                            })
                                            .size(13.0),
                                        )
                                        .clicked()
                                    {
                                        open_url("https://maclean.app/buy");
                                    }
                                });
                            }
                        },
                    );

                    ui.add_space(20.0);
                });
        });

    // P2-2: 设置变更自动保存（对比入口快照，有变化才写盘）
    let current = (
        app.lang_en,
        app.settings_menubar_icon,
        app.settings_keep_sudo,
        app.settings_scan_cache,
        app.settings_scan_all_disks,
        app.settings_prefer_official_uninstaller,
        app.settings_show_protected_items,
        app.schedule_enabled,
        app.schedule_interval_days,
        app.settings_confirm_advanced,
        app.settings_prevent_lid_close,
        app.settings_auto_restore_point,
    );
    if current != settings_snapshot {
        app.save_settings();
    }
}

/// 设置卡片容器
pub(crate) fn settings_card<R>(
    ui: &mut egui::Ui,
    title: &str,
    content: impl FnOnce(&mut egui::Ui) -> R,
) -> R {
    egui::Frame::none()
        .fill(theme::surface())
        .stroke(egui::Stroke::new(1.0_f32, theme::line()))
        .rounding(egui::Rounding::same(12.0))
        .inner_margin(egui::Margin::same(16.0))
        .show(ui, |ui| {
            ui.label(
                egui::RichText::new(title)
                    .size(14.0)
                    .strong()
                    .color(theme::text()),
            );
            ui.add_space(12.0);
            content(ui)
        })
        .inner
}

/// 单个设置项（标签 + 描述 + Toggle）
pub(crate) fn render_settings_item(
    ui: &mut egui::Ui,
    label: &str,
    desc: &str,
    value: &mut bool,
    enabled: bool,
) {
    ui.horizontal(|ui| {
        ui.vertical(|ui| {
            ui.colored_label(
                theme::text(),
                egui::RichText::new(label).size(13.0).strong(),
            );
            ui.colored_label(theme::text_2(), egui::RichText::new(desc).size(12.0));
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
        .rect_filled(sep_rect, egui::Rounding::ZERO, theme::line());
    ui.add_space(8.0);
}

/// 扫描中 UI
/// 概览扫描中的聚合进度：统计所有 Tab 当前可删除的项数与大小
///
/// Overview 自身的扫描器恒空（聚合视图，`results[0]` 无 item），扫描中
/// 若直接用本 Tab 结果会显示"0 项 总计 0B"，与顶部聚合色块（如 Safe
/// 18.4G）自相矛盾。这里跨 Tab 聚合可删项，让"扫描中"显示真实进度。
fn overview_aggregate_progress(results: &[Vec<ScanItem>]) -> (usize, u64) {
    let mut n = 0usize;
    let mut size = 0u64;
    for tab in results.iter().skip(1) {
        for item in tab {
            if item.deletable {
                n += 1;
                size += item.size_bytes;
            }
        }
    }
    (n, size)
}

pub(crate) fn ui_scanning(ui: &mut egui::Ui, app: &mut App) {
    let tab_idx = app.tab_index();
    let tab_title_text = tab_title(&app.tab, app);
    let (discovered, discovered_size) = if app.tab == Tab::Overview {
        // Overview 自身扫描器恒空：扫描中显示跨 Tab 聚合进度，
        // 避免"顶部 18.4G 可释放却显示 0 项 0B"的误导
        overview_aggregate_progress(&app.results)
    } else {
        (
            app.results[tab_idx].len(),
            app.results[tab_idx].iter().map(|i| i.size_bytes).sum(),
        )
    };
    let current_path = if app.scan_current_path.is_empty() {
        app.t("scanning_hint").to_string()
    } else {
        app.scan_current_path.clone()
    };

    ui.vertical_centered(|ui| {
        ui.add_space(24.0);
        egui::Frame::none()
            .fill(theme::surface())
            .stroke(egui::Stroke::new(1.0_f32, theme::line()))
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
                    let stroke_width = 3.0_f32;
                    // 背景圆环
                    painter.circle_stroke(
                        center,
                        radius,
                        egui::Stroke::new(stroke_width, theme::surface_3()),
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
                        egui::Stroke::new(stroke_width, theme::brand()),
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
                        .color(theme::text()),
                    );
                    ui.add_space(4.0);
                    ui.label(
                        egui::RichText::new(app.tf(
                            "items_total_size",
                            &[&discovered.to_string(), &format_size(discovered_size)],
                        ))
                        .size(13.0)
                        .color(theme::text_2()),
                    );
                });

                ui.add_space(16.0);
                // 进度条：扫描阶段用流动动画（不确定进度），
                // 不再显示估算百分比——避免"99% 却一直不动"的僵死观感
                let progress_rect = ui.available_rect_before_wrap();
                let bar_rect = egui::Rect::from_min_size(
                    progress_rect.min,
                    egui::vec2(progress_rect.width().max(360.0), 8.0),
                );
                let painter = ui.painter();
                painter.rect_filled(bar_rect, egui::Rounding::same(4.0), theme::surface_3());
                let time_f = ui.ctx().input(|i| i.time) as f32;
                let seg_w = (bar_rect.width() * 0.35).max(40.0);
                let phase = (time_f * 0.6).rem_euclid(1.0);
                let seg_start = bar_rect.min.x + (bar_rect.width() - seg_w) * phase;
                let seg_rect = egui::Rect::from_min_size(
                    egui::pos2(seg_start, bar_rect.min.y),
                    egui::vec2(seg_w, bar_rect.height()),
                );
                painter.rect_filled(seg_rect, egui::Rounding::same(4.0), theme::brand());
                ui.allocate_rect(bar_rect, egui::Sense::hover());

                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    ui.colored_label(
                        theme::text_2(),
                        egui::RichText::new(format!("{} {}", app.t("scanning"), current_path))
                            .size(12.0)
                            .monospace(),
                    );
                });

                // 取消扫描按钮：点后置取消标志，扫描线程在各 Tab 间检查后提前退出
                ui.add_space(8.0);
                let cancelling = app.scan_cancel.load(std::sync::atomic::Ordering::Relaxed);
                let cancel_label = if cancelling {
                    app.t("cancelling_scan").to_string()
                } else {
                    app.t("cancel_scan").to_string()
                };
                if ui
                    .add_enabled(!cancelling, egui::Button::new(cancel_label))
                    .clicked()
                {
                    app.scan_cancel
                        .store(true, std::sync::atomic::Ordering::Relaxed);
                }

                // 日志区域
                ui.add_space(12.0);
                egui::Frame::none()
                    .fill(theme::surface_3())
                    .stroke(egui::Stroke::new(1.0_f32, theme::line()))
                    .rounding(egui::Rounding::same(6.0))
                    .inner_margin(egui::Margin::same(10.0))
                    .show(ui, |ui| {
                        egui::ScrollArea::vertical()
                            .max_height(120.0)
                            .stick_to_bottom(true)
                            .show(ui, |ui| {
                                if app.logs.is_empty() {
                                    ui.colored_label(
                                        theme::text_2(),
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
                                            theme::safe()
                                        } else if log.starts_with('✗') || log.starts_with('⛔') {
                                            theme::danger()
                                        } else if log.starts_with('⚠') {
                                            theme::caution()
                                        } else {
                                            theme::text_2()
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scanner::{Recommend, ScanItem};

    #[test]
    fn overview_progress_aggregates_deletable_across_tabs() {
        // 概览自身扫描器恒空（results[0] 无 item）；扫描中进度必须聚合
        // 其他 Tab 的可删项，否则界面显示"顶部 18.4G 却 0 项 0B"矛盾。
        let mut results: Vec<Vec<ScanItem>> = vec![Vec::new(); 12];
        results[0].push(mk_item("/tmp/overview_self", 1)); // Overview 自身不计入
        results[3].push(mk_item("/tmp/a", 100));
        results[3].push(mk_item("/tmp/b", 200));
        let mut protected = mk_item("/tmp/c", 500);
        protected.deletable = false;
        results[5].push(protected);
        let (n, size) = overview_aggregate_progress(&results);
        assert_eq!(n, 2, "应只统计可删项，且跳过 Overview 自身");
        assert_eq!(size, 300, "大小应只累加可删项");
    }

    /// 构造一个可删除的测试条目
    fn mk_item(path: &str, size: u64) -> ScanItem {
        ScanItem {
            path: path.to_string(),
            size_bytes: size,
            category: "test".to_string(),
            selected: false,
            deletable: true,
            undeletable_reason: String::new(),
            recommend: Recommend::Safe,
            description: String::new(),
            batch_paths: Vec::new(),
        }
    }

    fn empty_app() -> App {
        let mut app = App::new();
        for v in app.results.iter_mut() {
            v.clear();
        }
        app
    }

    // ---------- P0-1 / P0-3: 跨 Tab 删除的统计必须真实 ----------

    #[test]
    fn truncate_path_is_byte_boundary_safe_for_multibyte() {
        // 复现崩溃路径：字节索引 34 落在 '超' (bytes 33..36) 中间
        let path =
            "/Users/fengyu/Downloads/【永轩超市】10.27 数据库实例切换后服务异常复盘报告.docx";
        for max_len in [3usize, 10, 20, 34, 40, 55, 80, 1000] {
            let t = truncate_path(path, max_len);
            let chars = t.chars().count();
            assert!(
                chars <= max_len,
                "max_len={} 结果 {} 超限: {}",
                max_len,
                chars,
                t
            );
            assert!(t.starts_with("...") || t == path, "超长应带省略号: {}", t);
        }
        assert_eq!(truncate_path("abc", 10), "abc");
        assert_eq!(truncate_path(path, 2), "...");
        let t = truncate_path(path, 10);
        assert!(t.ends_with(".docx"), "应保留文件后缀: {}", t);
    }

    #[test]
    fn overview_prepare_delete_is_noop_but_cross_tab_works() {
        let mut app = empty_app();
        app.results[1].push(mk_item("/tmp/tab1_item", 100));
        app.results[3].push(mk_item("/tmp/tab3_item", 200));
        app.tab = Tab::Overview;

        // 原 bug 复现：prepare_delete 只看当前 Tab（Overview=0），
        // results[0] 恒为空 → 静默 return，确认框永不弹出，
        // 只在其他 Tab 留下莫名其妙的预选。
        app.prepare_delete();
        assert!(
            app.pending_delete.is_empty(),
            "Overview 上 prepare_delete 选不到任何东西 —— 这正是 P0-1 的根因"
        );

        // 正确入口：走 UI「一键清理」真正调用的那个函数
        one_click_clean(&mut app, &[(1, 0), (3, 0)]);

        // P0-3：确认框现在按 pending_delete 统计，必须是真实的 2 项 / 300B
        assert_eq!(app.pending_count(), 2, "确认框不能显示 0 项");
        assert_eq!(app.pending_total_size(), 300, "确认框不能显示 0 B");
        assert!(
            matches!(app.confirm, ConfirmState::Pending),
            "应进入确认状态, 实际 {:?}",
            app.confirm
        );

        // 对照组：旧的按当前 Tab 统计方式会给出 0，锁死这个差异
        assert_eq!(
            app.selected_count(),
            0,
            "当前 Tab(Overview) 选中数为 0 —— 证明确认框必须走 pending_* 而非 selected_*"
        );
    }

    #[test]
    fn pending_items_ignores_out_of_range_indices() {
        let mut app = empty_app();
        app.results[2].push(mk_item("/tmp/only", 42));

        app.pending_delete = vec![(2, 0), (99, 0), (2, 99)];
        assert_eq!(app.pending_count(), 1, "越界索引必须被忽略而不是 panic");
        assert_eq!(app.pending_total_size(), 42);
    }

    #[test]
    fn pending_items_skips_undeletable() {
        let mut app = empty_app();
        let mut blocked = mk_item("/tmp/blocked", 999);
        blocked.deletable = false;
        app.results[1].push(mk_item("/tmp/ok", 10));
        app.results[1].push(blocked);

        // pending_delete 是权威列表，但确认框只统计可删除的
        app.pending_delete = vec![(1, 0), (1, 1)];
        assert_eq!(app.pending_count(), 2, "pending 本身不去重/过滤");
        let deletable_sum: u64 = app
            .pending_items()
            .iter()
            .filter(|i| i.deletable)
            .map(|i| i.size_bytes)
            .sum();
        assert_eq!(deletable_sum, 10, "不可删除项不应计入可释放空间");
    }

    // ---------- P0-2: Touch ID 分支不得丢掉新 receiver ----------

    /// Touch ID 分支的**决策**：不查系统状态，三个条件直接传入
    #[test]
    #[cfg(target_os = "macos")]
    fn touch_id_route_decisions() {
        // 用一个会被 sanitize 拒绝的系统路径：线程只会发一条 Done 就退出，
        // 不会真的执行 sudo 或删除任何东西。
        let rejected = || {
            vec![(
                "/System/Library/ShouldBeRejected".to_string(),
                "test".to_string(),
            )]
        };

        // 已启用 → 走 Touch ID
        let mut gui = Gui::new();
        let (_keep, rx) = mpsc::channel::<DeleteMessage>();
        gui.delete_rx = Some(rx);
        let started = gui.route_need_password(rejected(), true, true, false);
        assert!(started, "Touch ID 已启用时应拉起删除线程");
        assert!(matches!(gui.app.confirm, ConfirmState::SudoWithTouchId));
        assert!(gui.delete_rx.is_some(), "拉起线程后 receiver 必须还在");

        // 可用但未启用 → 提示开启
        let mut gui = Gui::new();
        let started = gui.route_need_password(rejected(), false, true, false);
        assert!(!started);
        assert!(matches!(gui.app.confirm, ConfirmState::OfferTouchIdSetup));

        // 不支持 → 密码输入
        let mut gui = Gui::new();
        let started = gui.route_need_password(rejected(), false, false, false);
        assert!(!started);
        assert!(matches!(gui.app.confirm, ConfirmState::NeedSudoPassword));

        // 合盖 → 即使已启用也退化为密码输入
        let mut gui = Gui::new();
        let started = gui.route_need_password(rejected(), true, true, true);
        assert!(!started, "合盖时 Touch ID 不可用");
        assert!(matches!(gui.app.confirm, ConfirmState::NeedSudoPassword));

        // 已启用但没录指纹（sudo_local 配好、bioutil 0 模板）→ 必须回退密码输入，
        // 不能走 Touch ID 死路：系统会弹 Touch ID 却无指纹可验证，删除必失败（历史 bug）。
        let mut gui = Gui::new();
        let started = gui.route_need_password(rejected(), true, false, false);
        assert!(!started, "无指纹时不应拉起 Touch ID 线程");
        assert!(matches!(gui.app.confirm, ConfirmState::NeedSudoPassword));
    }

    /// P0-2 回归：走完 poll_delete 后，Touch ID 分支拉起的 receiver 不得被清掉
    #[test]
    #[cfg(target_os = "macos")]
    fn poll_delete_keeps_rx_when_touch_id_thread_started() {
        if safety::is_clamshell_closed() {
            eprintln!("跳过：合盖状态下 Touch ID 不可用");
            return;
        }
        let mut gui = Gui::new();
        // 强制启用 Touch ID，否则本测试只能在真的配了 pam_tid 的机器上生效
        gui.force_touch_id = Some((true, true));

        let (tx, rx) = mpsc::channel::<DeleteMessage>();
        gui.delete_rx = Some(rx);
        tx.send(DeleteMessage::NeedPassword(vec![(
            "/System/Library/ShouldBeRejected".to_string(),
            "test".to_string(),
        )]))
        .unwrap();

        gui.poll_delete();

        assert!(
            matches!(gui.app.confirm, ConfirmState::SudoWithTouchId),
            "强制启用后应走 Touch ID 分支, 实际 {:?}",
            gui.app.confirm
        );
        assert!(
            gui.delete_rx.is_some(),
            "Touch ID 分支拉起了新线程，delete_rx 必须保留 —— 修复前这里被无条件清空，\
             导致后续消息无人接收、confirm 永久卡在 SudoWithTouchId"
        );
    }

    /// 未启用 Touch ID 时，本轮没有新线程，通道应当被清理
    #[test]
    #[cfg(target_os = "macos")]
    fn poll_delete_clears_rx_when_no_thread_started() {
        let mut gui = Gui::new();
        gui.force_touch_id = Some((false, false));

        let (tx, rx) = mpsc::channel::<DeleteMessage>();
        gui.delete_rx = Some(rx);
        tx.send(DeleteMessage::NeedPassword(vec![(
            "/System/Library/ShouldBeRejected".to_string(),
            "test".to_string(),
        )]))
        .unwrap();

        gui.poll_delete();

        assert!(matches!(gui.app.confirm, ConfirmState::NeedSudoPassword));
        assert!(
            gui.delete_rx.is_none(),
            "未拉起新线程时应清理通道，避免残留 receiver"
        );
    }

    fn mk_rec_item(recommend: Recommend, size: u64) -> ScanItem {
        ScanItem {
            path: format!("/tmp/rec_{:?}_{}", recommend, size),
            size_bytes: size,
            category: "test".to_string(),
            selected: false,
            deletable: true,
            undeletable_reason: String::new(),
            recommend,
            description: String::new(),
            batch_paths: Vec::new(),
        }
    }

    // ---------- P1-18: App 卸载胶囊统计口径（从渲染函数里抽出的纯逻辑） ----------

    #[test]
    fn pill_stats_merges_cache_only_into_safe() {
        // UI 口径：CacheOnly 归入 Safe 展示。这条口径之前埋在渲染函数里，
        // 没有任何测试能锁住它。
        let items = vec![
            mk_rec_item(Recommend::Safe, 100),
            mk_rec_item(Recommend::CacheOnly, 50),
            mk_rec_item(Recommend::Caution, 70),
        ];
        // App A 同时含 Safe 与 CacheOnly 两项
        let groups = vec![
            ("App A".to_string(), vec![0, 1]),
            ("App B".to_string(), vec![2]),
        ];

        let (apps, total, stats) = uninstall_pill_stats(&items, &groups);
        assert_eq!(apps, 2, "应用数按分组数统计");
        assert_eq!(total, 220, "总大小统计全部条目");
        assert_eq!(
            stats.get(&Recommend::Safe),
            Some(&(1, 150)),
            "CacheOnly 必须并入 Safe：App A 算 1 个应用、150 字节"
        );
        assert_eq!(stats.get(&Recommend::Caution), Some(&(1, 70)));
        assert!(
            !stats.contains_key(&Recommend::CacheOnly),
            "CacheOnly 不应作为独立分类出现在胶囊上"
        );
    }

    #[test]
    fn pill_stats_ignores_out_of_range_indices() {
        // 分组里可能残留上一帧扫描结果的索引，越界不能 panic
        let items = vec![mk_rec_item(Recommend::Safe, 10)];
        let groups = vec![("App".to_string(), vec![0, 99])];

        let (apps, total, stats) = uninstall_pill_stats(&items, &groups);
        assert_eq!(apps, 1);
        assert_eq!(total, 10, "总大小按 items 本身算，与分组索引无关");
        assert_eq!(stats.get(&Recommend::Safe), Some(&(1, 10)));
    }

    #[test]
    fn pill_stats_handles_empty_input() {
        let (apps, total, stats) = uninstall_pill_stats(&[], &[]);
        assert_eq!(apps, 0);
        assert_eq!(total, 0);
        assert!(stats.is_empty());
    }

    // ---------- P1-19: Done 之后到达的 Progress 不得让进度条回退 ----------

    // ---------- P1-7: 二次「扫描全部」不能叠加旧结果 ----------

    #[test]
    fn reset_for_full_scan_clears_previous_results() {
        let mut app = empty_app();
        for idx in 1..app.results.len() {
            app.results[idx].push(mk_item("/tmp/old_stale_item", 999));
        }
        // Overview（索引 0）不参与扫描，必须保持原样
        app.results[0].push(mk_item("/tmp/never_scanned", 1));
        app.scan_progress = 0.9;

        app.reset_for_full_scan();

        for idx in 1..app.results.len() {
            assert!(
                app.results[idx].is_empty(),
                "results[{idx}] 必须被清空 —— 否则 PartialItems extend 会让二次扫描结果翻倍"
            );
            assert!(app.tab_scanning(idx), " results[{idx}] 应被标记为扫描中");
        }
        assert_eq!(
            app.results[0].len(),
            1,
            "Overview 的结果槽不属于任何扫描任务，不应被清掉"
        );
        assert_eq!(app.scan_progress, 0.0);
    }

    // ---------- P1-8: 扫描态判定必须区分「当前 Tab」与「任意 Tab」 ----------

    #[test]
    fn any_scanning_covers_other_tabs() {
        let mut app = empty_app();
        assert!(!app.any_scanning());

        // 场景：用户正在 DevCache(3) 单扫，却切到 Overview 点了「扫描全部」
        app.tab = Tab::Overview;
        app.scan_states[3] = ScanState::Scanning;

        assert!(
            app.any_scanning(),
            "别的 Tab 在扫描时也算正在扫描 —— 否则能并发拉起多轮全量扫描把 I/O 打满"
        );
        assert!(
            !app.tab_scanning(app.tab_index()),
            "Overview 自身没在扫描，tab_scanning 应为 false"
        );
        assert!(app.tab_scanning(3), "DevCache 正在扫描");

        // 对全部非 Overview Tab 成立
        for idx in 1..app.results.len() {
            let mut a = empty_app();
            a.scan_states[idx] = ScanState::Scanning;
            a.tab = Tab::Overview;
            assert!(a.any_scanning(), "Tab {idx} 扫描中应被 any_scanning 发现");
        }
    }

    #[test]
    fn scan_progress_does_not_regress_after_done() {
        let mut gui = Gui::new();
        let (tx, rx) = mpsc::channel::<ScanMessage>();
        gui.scan_rx = Some(rx);

        // 模拟：扫描线程报进度 → 完成 → 200ms 估算线程又补发一条旧进度
        tx.send(ScanMessage::Progress(0.6)).unwrap();
        gui.poll_scan();
        assert!(
            (gui.app.scan_progress - 0.6).abs() < 1e-6,
            "正常进度应生效, 实际 {}",
            gui.app.scan_progress
        );

        tx.send(ScanMessage::Done(vec![], 1, 1)).unwrap();
        // 估算线程晚到的那条 —— 修复前会把 1.0 拉回 0.7，进度条肉眼可见地倒退
        tx.send(ScanMessage::Progress(0.7)).unwrap();
        gui.poll_scan();

        assert_eq!(
            gui.app.scan_progress, 1.0,
            "Done 之后收到的 Progress 必须被忽略，否则多 Tab 扫描时进度条会来回跳"
        );
    }

    #[test]
    fn poll_delete_finishes_when_nothing_needs_privilege() {
        // 空 items 进 Touch ID 分支时不能停在"Touch ID 验证中"——
        // 没有任何线程在跑，弹窗永远等不到消息。
        let mut gui = Gui::new();
        let (tx, rx) = mpsc::channel::<DeleteMessage>();
        gui.delete_rx = Some(rx);
        tx.send(DeleteMessage::NeedPassword(vec![])).unwrap();

        gui.poll_delete();

        assert!(
            !matches!(gui.app.confirm, ConfirmState::SudoWithTouchId),
            "没有需要提权的项时不应停在 SudoWithTouchId, 实际 {:?}",
            gui.app.confirm
        );
        assert!(gui.delete_rx.is_none(), "本轮没有新线程，通道应被清理");
    }
}

#[cfg(test)]
mod disk_analyzer_tests {
    use super::*;

    fn item(category: &str, size: u64) -> ScanItem {
        ScanItem {
            path: "x".to_string(),
            size_bytes: size,
            category: category.to_string(),
            selected: false,
            deletable: true,
            undeletable_reason: String::new(),
            batch_paths: Vec::new(),
            recommend: crate::scanner::Recommend::Safe,
            description: String::new(),
        }
    }

    #[test]
    fn aggregate_sums_by_category_and_drops_zero() {
        let items = vec![
            item("视频", 100),
            item("文档", 30),
            item("视频", 50),
            item("缓存", 0),
        ];
        let agg = aggregate_category_sizes(&items);
        assert_eq!(agg.len(), 2, "零大小类别应被剔除");
        assert_eq!(agg[0], ("视频".to_string(), 150), "应降序且同类别求和");
        assert_eq!(agg[1], ("文档".to_string(), 30));
    }

    #[test]
    fn aggregate_empty_returns_empty() {
        assert!(aggregate_category_sizes(&[]).is_empty());
    }

    #[test]
    fn overview_total_matches_sum() {
        // 与 UI 使用的 total 口径一致：只统计展示类别（已剔除零大小）
        let items = vec![item("A", 70), item("B", 30), item("C", 0)];
        let agg = aggregate_category_sizes(&items);
        let total: u64 = agg.iter().map(|(_, sz)| sz).sum();
        assert_eq!(total, 100);
    }
}
