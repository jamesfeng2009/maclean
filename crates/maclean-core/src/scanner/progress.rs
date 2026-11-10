//! 扫描进度钩子（跨 crate 的轻量回调）。
//!
//! ## 解决什么
//!
//! 扫描器在 core 里跑，但进度只有壳层（Tauri / CLI）能推给用户。早期实现只在
//! **模块级**上报（一个模块扫完才推进一格），而 `dev_cache` 内部 19 个子扫描要跑
//! 40 秒以上，用户看到的就是「进度条 43 秒不动，一直 10%」。
//!
//! 这里提供一个进程级的单槽钩子：core 的扫描器每完成一个子扫描就调用
//! [`report`]，壳层在启动时用 [`set_hook`] 注册一个把事件转发给前端的回调。
//! 钩子不存在时 [`report`] 是 no-op，所以 CLI / 单测不需要关心它。
//!
//! 约定：`label` 是「扫描器名 · 子扫描名」形式（如 `dev_cache · Rust编译缓存`），
//! `pct` 是该**扫描器内部**的完成百分比（0-100），由壳层决定映射到整体进度区间。

use std::sync::{Arc, OnceLock};

pub type Hook = Arc<dyn Fn(&str, u8) + Send + Sync + 'static>;

static HOOK: OnceLock<Hook> = OnceLock::new();

/// 注册进度钩子；同一进程内只有第一次生效（后续全量扫描复用同一个回调）。
pub fn set_hook(hook: Hook) {
    let _ = HOOK.set(hook);
}

/// 上报一次子扫描进度；未注册钩子时静默返回。
pub fn report(label: &str, pct: u8) {
    if let Some(hook) = HOOK.get() {
        hook(label, pct.min(100));
    }
}
