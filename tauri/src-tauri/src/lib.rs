//! maclean Tauri 壳入口。
//!
//! 前端只通过 [`commands`] 中登记的白名单 command 触达 `maclean-core`；
//! 所有删除都在 core 的 safety 闸门内执行，前端拿不到裸文件句柄。

mod commands;

use commands::{
    app_open, clean_execute, clean_preview, disk_info, im_breakdown, logs_list, logs_read,
    logs_reveal, optimize_list, optimize_run, palette, reveal_path, reveal_trash, scan,
    settings_get, settings_set, startup_set_enabled, startups_list,
};

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
        ])
        .setup(|_app| Ok(()))
        .run(tauri::generate_context!())
        .expect("error while running maclean tauri application");
}
