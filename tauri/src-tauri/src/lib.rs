//! maclean Tauri 壳入口。
//!
//! 前端只通过 [`commands`] 中登记的白名单 command 触达 `maclean-core`；
//! 所有删除都在 core 的 safety 闸门内执行，前端拿不到裸文件句柄。

mod commands;

use std::sync::Mutex;
use std::time::Duration;

use commands::{
    app_open, app_uninstall, apps_inventory, apps_sizes, cache_children, clean_execute,
    clean_preview, disk_info, im_breakdown, launch_at_login_get, launch_at_login_set, logs_list,
    logs_read, logs_reveal, menu_bar_set, menu_bar_status, open_app_management_settings,
    open_full_disk_access_settings, optimize_list, optimize_run, palette, reveal_path,
    reveal_trash, scan, settings_get, settings_set, startup_set_enabled, startups_list,
};
use tauri::tray::{MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};
use tauri::Manager;

/// 菜单栏托盘持有者：`Some(tray)` = 已显示；`None` = 已隐藏。
/// 由 `menu_bar_apply` 统一增删；定时刷新线程只读引用更新 tooltip。
pub struct MenuBar(pub Mutex<Option<TrayIcon>>);

/// 菜单栏 tooltip 文本：磁盘可用空间（复用 core 的真实磁盘信息）。
fn menubar_tooltip() -> String {
    let (total, free) = maclean_core::platform::disk_info();
    let used = total.saturating_sub(free);
    let pct = if total > 0 { used as f32 / total as f32 * 100.0 } else { 0.0 };
    format!("maclean · 已用 {:.1}% · 可用 {:.1} GB / {:.1} GB", pct, free as f32 / 1e9, total as f32 / 1e9)
}

/// 应用 / 移除菜单栏托盘（幂等）。开启时创建托盘：左键单击唤起主窗口，
/// tooltip 每 60 秒刷新一次磁盘用量；关闭时直接 drop 托盘图标。
pub fn menu_bar_apply(app: &tauri::AppHandle, show: bool) -> Result<(), String> {
    let state = app.state::<MenuBar>();
    let mut guard = state.0.lock().map_err(|_| "菜单栏状态锁被占用".to_string())?;
    if show {
        if guard.is_some() {
            return Ok(());
        }
        let icon = tauri::image::Image::from_bytes(include_bytes!("../icons/128x128.png"))
            .map_err(|e| format!("加载菜单栏图标失败：{e}"))?;
        let tray = TrayIconBuilder::with_id("maclean-menubar")
            .icon(icon)
            .tooltip(menubar_tooltip())
            .show_menu_on_left_click(false)
            .on_tray_icon_event(|tray, event| {
                // 左键单击：唤起主窗口（不重复开窗，仅显示+聚焦）
                if let TrayIconEvent::Click {
                    button: MouseButton::Left,
                    button_state: MouseButtonState::Up,
                    ..
                } = event
                {
                    if let Some(w) = tray.app_handle().get_webview_window("main") {
                        let _ = w.show();
                        let _ = w.unminimize();
                        let _ = w.set_focus();
                    }
                }
            })
            .build(app)
            .map_err(|e| format!("创建菜单栏图标失败：{e}"))?;
        // 定时刷新 tooltip：后台线程每 60s 重算磁盘信息，不影响主线程
        {
            let app = app.clone();
            std::thread::spawn(move || loop {
                std::thread::sleep(Duration::from_secs(60));
                let tip = menubar_tooltip();
                let st = app.state::<MenuBar>();
                let guard = st.0.lock().ok();
                if let Some(g) = guard {
                    if let Some(t) = g.as_ref() {
                        let _ = t.set_tooltip(Some(tip.as_str()));
                    }
                }
            });
        }
        *guard = Some(tray);
    } else if let Some(t) = guard.take() {
        // drop TrayIcon 即从菜单栏移除
        drop(t);
    }
    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // 启用跨平台文件日志（~/.maclean/logs/maclean_YYYY-MM-DD.log），
    // 删除链路的拦截/成功都会落盘，供「设置 → 日志」查看与排障。
    maclean_core::logger::init();

    // 在任何并行迭代前安装「后台优先级」全局 rayon 池：扫描的目录大小统计等
    // par_iter 工作线程都带 UTILITY QoS，高负载时不与前台 App / WindowServer 抢 CPU/IO。
    // 必须尽早调用；若底层已初始化（重复启动）会返回 false，忽略即可。
    maclean_core::scanner::load::install_rayon_bg_pool();

    tauri::Builder::default()
        .manage(MenuBar(Mutex::new(None)))
        .invoke_handler(tauri::generate_handler![
            disk_info,
            reveal_trash,
            scan,
            startups_list,
            startup_set_enabled,
            optimize_list,
            optimize_run,
            clean_preview,
            clean_execute,
            settings_get,
            settings_set,
            palette,
            logs_list,
            logs_read,
            logs_reveal,
            app_open,
            im_breakdown,
            reveal_path,
            apps_inventory,
            apps_sizes,
            app_uninstall,
            open_full_disk_access_settings,
            open_app_management_settings,
            cache_children,
            launch_at_login_get,
            launch_at_login_set,
            menu_bar_status,
            menu_bar_set,
        ])
        .setup(|app| {
            // 按配置恢复菜单栏图标（默认开启）
            if maclean_core::config::load_config().settings_menubar_icon {
                let _ = menu_bar_apply(app.handle(), true);
            }
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running maclean tauri application");
}
