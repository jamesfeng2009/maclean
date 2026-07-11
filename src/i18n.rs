//! 国际化模块
//!
//! 使用 rust-i18n 实现中英文双语支持。
//! 按 L 键切换语言。

// 注意: rust_i18n::i18n! 宏必须在 crate root (main.rs) 中调用
// 这里只提供辅助函数

/// 切换语言 (zh-CN <-> en)
pub fn toggle_language() {
    let current = rust_i18n::t!("_locale");
    if current == "zh-CN" {
        rust_i18n::set_locale("en");
    } else {
        rust_i18n::set_locale("zh-CN");
    }
}

/// 获取当前语言显示名
pub fn current_lang_name() -> &'static str {
    let current = rust_i18n::t!("_locale");
    if current == "zh-CN" {
        "中"
    } else {
        "EN"
    }
}
