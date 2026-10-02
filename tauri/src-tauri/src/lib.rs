//! maclean Tauri 壳入口。
//!
//! 前端只通过 [`commands`] 中登记的白名单 command 触达 `maclean-core`；
//! 所有删除都在 core 的 safety 闸门内执行，前端拿不到裸文件句柄。

mod commands;

use commands::{
    clean_execute, clean_preview, disk_info, logs_list, logs_read, logs_reveal, optimize_list,
    optimize_run, palette, scan, settings_get, settings_set, startup_set_enabled, startups_list,
};

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // 启用跨平台文件日志（~/.maclean/logs/maclean_YYYY-MM-DD.log），
    // 删除链路的拦截/成功都会落盘，供「设置 → 日志」查看与排障。
    maclean_core::logger::init();

    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![
            disk_info,
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
        ])
        .setup(|_app| Ok(()))
        .run(tauri::generate_context!())
        .expect("error while running maclean tauri application");
}
