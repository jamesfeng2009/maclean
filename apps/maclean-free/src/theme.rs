//! 设计系统 Token
//!
//! 对应 UI 设计稿 v2（`maclean-ui-design-v2.html`）第 02 节「设计系统」与第 07 节「深色模式」。
//!
//! 这里是全应用颜色 / 圆角 / 间距 / 控件尺寸的唯一来源，UI 代码不要再就地写
//! `Color32::from_rgb(...)`。
//!
//! # 换肤机制
//!
//! 颜色不是 `const`，而是从「当前帧配色」里取：
//!
//! ```ignore
//! ui.colored_label(theme::text_2(), "…");   // 而不是 theme::TEXT_2
//! ```
//!
//! 当前配色存在一个 **线程局部** 的 `Cell<Palette>` 里，由
//! [`apply_visuals`] 在每帧开始（`NEEDS_INIT` 或主题切换）时写入。
//!
//! ## 为什么用线程局部，而不是把 `&Palette` 一路传下去
//!
//! - egui 是单线程立即模式：一个进程只有一个 UI 线程，一帧内只有一个配色。
//!   egui 自己也是这么做的（`ctx.set_style()` 就是挂在 Context 上的全局样式）。
//! - 传参方案要给 80 多个绘制函数加形参，而本文件的调用点有 360 余处；
//!   线程局部方案把这 360 处变成一次可编译校验的机械替换，**不动任何函数签名**，
//!   因此后续把 main.rs 拆成 `ui/` 时可以保持「纯移动、零改签名」。
//! - 代价是存在一处隐式状态。为把代价压到最小：写入点只有
//!   [`set_mode`] 一个，且必须在每帧绘制前调用；读取默认回退到浅色，
//!   不会 panic 也不会读到脏值。
//!
//! 如果将来需要「同一屏内多套配色」（例如内嵌主题预览），再改成显式传参即可，
//! 届时只需要把这些取值函数改成 `Palette` 上的方法。

// 取值函数 / 常量是「设计系统完整 token 面」：即使当前 UI 没用到也算公开 API，
// 不算死代码 —— 否则每加一个页面都要回来删 allow。
#![allow(dead_code)]

use std::cell::Cell;

use eframe::egui;

use crate::scanner::Recommend;

// =========================================================================
//  配色模式
// =========================================================================

// =========================================================================
//  配色表
// =========================================================================

use crate::app::Mode;

/// 一整套界面配色。
///
/// 字段语义固定，浅色 / 深色只是取值不同 —— 新增 token 必须两版都给，
/// 避免出现「深色下某个色还是浅色值」这类只在一套主题下才暴露的 bug。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Palette {
    /// 页面背景
    pub bg: egui::Color32,
    /// 表面 / 卡片
    pub surface: egui::Color32,
    /// 侧栏 / 表头
    pub surface_2: egui::Color32,
    /// 轨道 / 禁用
    pub surface_3: egui::Color32,
    /// hover 底
    pub surface_4: egui::Color32,

    /// 描边（默认）
    pub line: egui::Color32,
    /// 描边 strong（输入框、悬浮）
    pub line_2: egui::Color32,
    /// 描边（复选框未选中）
    pub line_3: egui::Color32,

    /// 主文本
    pub text: egui::Color32,
    /// 次文本
    pub text_2: egui::Color32,
    /// 辅助文本
    pub text_3: egui::Color32,
    /// 占位 / 禁用文本
    pub text_4: egui::Color32,

    /// 品牌主色
    pub brand: egui::Color32,
    /// 品牌深色（在品牌浅底上的文字）
    pub brand_600: egui::Color32,
    /// 品牌浅底
    pub brand_50: egui::Color32,
    /// 品牌浅底（更实一档，用于 hover）
    pub brand_100: egui::Color32,

    /// 安全 / 仅缓存 / 谨慎 / 危险 四档语义色
    pub safe: egui::Color32,
    pub safe_50: egui::Color32,
    pub cache: egui::Color32,
    pub cache_50: egui::Color32,
    pub caution: egui::Color32,
    pub caution_50: egui::Color32,
    pub danger: egui::Color32,
    pub danger_50: egui::Color32,
    /// 危险色描边（Danger 弱化按钮的边框）
    pub danger_100: egui::Color32,
    /// 危险色强调（Danger 按钮 hover）
    pub danger_600: egui::Color32,

    /// 信息
    pub info: egui::Color32,
    pub info_50: egui::Color32,

    /// 弹窗遮罩
    pub scrim: egui::Color32,
}

impl Palette {
    /// 浅色配色（设计稿 02 节）
    pub const LIGHT: Palette = Palette {
        bg: egui::Color32::from_rgb(0xFA, 0xFA, 0xFA),
        surface: egui::Color32::from_rgb(0xFF, 0xFF, 0xFF),
        surface_2: egui::Color32::from_rgb(0xF5, 0xF5, 0xF7),
        surface_3: egui::Color32::from_rgb(0xEF, 0xEF, 0xF2),
        surface_4: egui::Color32::from_rgb(0xE9, 0xE9, 0xEE),

        line: egui::Color32::from_rgb(0xE8, 0xE8, 0xEC),
        line_2: egui::Color32::from_rgb(0xDC, 0xDC, 0xE2),
        line_3: egui::Color32::from_rgb(0xC9, 0xC9, 0xD1),

        text: egui::Color32::from_rgb(0x1B, 0x1B, 0x1F),
        text_2: egui::Color32::from_rgb(0x5C, 0x5C, 0x66),
        text_3: egui::Color32::from_rgb(0x8A, 0x8A, 0x94),
        text_4: egui::Color32::from_rgb(0xAF, 0xAF, 0xB8),

        brand: egui::Color32::from_rgb(0x4B, 0x3F, 0xE3),
        brand_600: egui::Color32::from_rgb(0x3D, 0x32, 0xC4),
        brand_50: egui::Color32::from_rgb(0xF1, 0xF0, 0xFE),
        brand_100: egui::Color32::from_rgb(0xE5, 0xE3, 0xFD),

        safe: egui::Color32::from_rgb(0x1F, 0x9D, 0x5B),
        safe_50: egui::Color32::from_rgb(0xE9, 0xF7, 0xEF),
        cache: egui::Color32::from_rgb(0x2F, 0x6F, 0xD0),
        cache_50: egui::Color32::from_rgb(0xEA, 0xF2, 0xFD),
        caution: egui::Color32::from_rgb(0xC9, 0x7C, 0x06),
        caution_50: egui::Color32::from_rgb(0xFD, 0xF3, 0xE2),
        danger: egui::Color32::from_rgb(0xDC, 0x4B, 0x3E),
        danger_50: egui::Color32::from_rgb(0xFD, 0xED, 0xEB),
        danger_100: egui::Color32::from_rgb(0xF7, 0xD4, 0xD0),
        danger_600: egui::Color32::from_rgb(0xC4, 0x3C, 0x30),

        info: egui::Color32::from_rgb(0x2F, 0x6F, 0xD0),
        info_50: egui::Color32::from_rgb(0xEA, 0xF2, 0xFD),

        // rgba(27,27,31,.28)
        scrim: egui::Color32::from_rgba_premultiplied(27, 27, 31, 71),
    };

    /// 深色配色（设计稿 07 节）
    ///
    /// 深色不是把浅色反转，而是重新配：表面越靠前越亮（#111114 → #35353F），
    /// 描边要比浅色更亮才看得见，品牌色与语义色提亮、底纹降饱和但不改色相。
    pub const DARK: Palette = Palette {
        bg: egui::Color32::from_rgb(0x11, 0x11, 0x14),
        surface: egui::Color32::from_rgb(0x1A, 0x1A, 0x1F),
        surface_2: egui::Color32::from_rgb(0x22, 0x22, 0x2A),
        surface_3: egui::Color32::from_rgb(0x2C, 0x2C, 0x35),
        surface_4: egui::Color32::from_rgb(0x35, 0x35, 0x3F),

        line: egui::Color32::from_rgb(0x33, 0x33, 0x3D),
        line_2: egui::Color32::from_rgb(0x44, 0x44, 0x50),
        line_3: egui::Color32::from_rgb(0x55, 0x55, 0x62),

        text: egui::Color32::from_rgb(0xF2, 0xF2, 0xF5),
        text_2: egui::Color32::from_rgb(0xA6, 0xA6, 0xB0),
        text_3: egui::Color32::from_rgb(0x7B, 0x7B, 0x86),
        text_4: egui::Color32::from_rgb(0x5C, 0x5C, 0x66),

        brand: egui::Color32::from_rgb(0x8B, 0x82, 0xF0),
        brand_600: egui::Color32::from_rgb(0xA2, 0x9B, 0xF5),
        brand_50: egui::Color32::from_rgb(0x24, 0x22, 0x3A),
        brand_100: egui::Color32::from_rgb(0x2F, 0x2C, 0x48),

        safe: egui::Color32::from_rgb(0x4C, 0xBF, 0x85),
        safe_50: egui::Color32::from_rgb(0x16, 0x2A, 0x21),
        cache: egui::Color32::from_rgb(0x5B, 0x96, 0xE8),
        cache_50: egui::Color32::from_rgb(0x15, 0x23, 0x34),
        caution: egui::Color32::from_rgb(0xE0, 0xA6, 0x3B),
        caution_50: egui::Color32::from_rgb(0x2F, 0x2A, 0x1F),
        danger: egui::Color32::from_rgb(0xF0, 0x70, 0x5F),
        danger_50: egui::Color32::from_rgb(0x33, 0x20, 0x1E),
        danger_100: egui::Color32::from_rgb(0x43, 0x2A, 0x26),
        danger_600: egui::Color32::from_rgb(0xF5, 0x8E, 0x80),

        info: egui::Color32::from_rgb(0x5B, 0x96, 0xE8),
        info_50: egui::Color32::from_rgb(0x15, 0x23, 0x34),

        // 深色下遮罩要更重才压得住背景：rgba(0,0,0,.5)
        scrim: egui::Color32::from_rgba_premultiplied(0, 0, 0, 128),
    };

    pub fn of(mode: Mode) -> Palette {
        match mode {
            Mode::Light => Palette::LIGHT,
            Mode::Dark => Palette::DARK,
        }
    }
}

// =========================================================================
//  当前帧配色
// =========================================================================

thread_local! {
    /// 当前帧生效的配色。
    ///
    /// 只在 [`set_mode`] 里写。读取永远有值（默认浅色），不会 panic。
    static CURRENT: Cell<Palette> = const { Cell::new(Palette::LIGHT) };
    /// 当前模式，供 UI 显示与测试断言使用
    static CURRENT_MODE: Cell<Mode> = const { Cell::new(Mode::Light) };
}

/// 切换当前帧配色。必须在绘制前调用（见 [`apply_visuals`]）。
pub fn set_mode(mode: Mode) {
    CURRENT.with(|c| c.set(Palette::of(mode)));
    CURRENT_MODE.with(|c| c.set(mode));
}

/// 当前模式
pub fn mode() -> Mode {
    CURRENT_MODE.with(|c| c.get())
}

/// 当前配色快照
pub fn current() -> Palette {
    CURRENT.with(|c| c.get())
}

// =========================================================================
//  取值函数
//
//  UI 代码一律走这里，不要引用具体颜色值。函数名与 Palette 字段同名。
// =========================================================================

#[inline]
pub fn bg() -> egui::Color32 {
    current().bg
}
#[inline]
pub fn surface() -> egui::Color32 {
    current().surface
}
#[inline]
pub fn surface_2() -> egui::Color32 {
    current().surface_2
}
#[inline]
pub fn surface_3() -> egui::Color32 {
    current().surface_3
}
#[inline]
pub fn surface_4() -> egui::Color32 {
    current().surface_4
}
#[inline]
pub fn line() -> egui::Color32 {
    current().line
}
#[inline]
pub fn line_2() -> egui::Color32 {
    current().line_2
}
#[inline]
pub fn line_3() -> egui::Color32 {
    current().line_3
}
#[inline]
pub fn text() -> egui::Color32 {
    current().text
}
#[inline]
pub fn text_2() -> egui::Color32 {
    current().text_2
}
#[inline]
pub fn text_3() -> egui::Color32 {
    current().text_3
}
#[inline]
pub fn text_4() -> egui::Color32 {
    current().text_4
}
#[inline]
pub fn brand() -> egui::Color32 {
    current().brand
}
#[inline]
pub fn brand_600() -> egui::Color32 {
    current().brand_600
}
#[inline]
pub fn brand_50() -> egui::Color32 {
    current().brand_50
}
#[inline]
pub fn brand_100() -> egui::Color32 {
    current().brand_100
}
#[inline]
pub fn safe() -> egui::Color32 {
    current().safe
}
#[inline]
pub fn safe_50() -> egui::Color32 {
    current().safe_50
}
#[inline]
pub fn cache() -> egui::Color32 {
    current().cache
}
#[inline]
pub fn cache_50() -> egui::Color32 {
    current().cache_50
}
#[inline]
pub fn caution() -> egui::Color32 {
    current().caution
}
#[inline]
pub fn caution_50() -> egui::Color32 {
    current().caution_50
}
#[inline]
pub fn danger() -> egui::Color32 {
    current().danger
}
#[inline]
pub fn danger_50() -> egui::Color32 {
    current().danger_50
}
#[inline]
pub fn danger_100() -> egui::Color32 {
    current().danger_100
}
#[inline]
pub fn danger_600() -> egui::Color32 {
    current().danger_600
}
#[inline]
pub fn info() -> egui::Color32 {
    current().info
}
#[inline]
pub fn info_50() -> egui::Color32 {
    current().info_50
}
#[inline]
pub fn scrim() -> egui::Color32 {
    current().scrim
}

// =========================================================================
//  圆角
// =========================================================================

pub const R_XS: f32 = 4.0;
pub const R_SM: f32 = 6.0;
/// 控件
pub const R: f32 = 8.0;
/// 卡片
pub const R_MD: f32 = 10.0;
/// 弹窗
pub const R_LG: f32 = 14.0;
pub const R_FULL: f32 = 999.0;

#[inline]
pub fn r(v: f32) -> egui::Rounding {
    egui::Rounding::same(v)
}

// =========================================================================
//  间距（8pt 基准栅格）
// =========================================================================

pub const S1: f32 = 4.0;
pub const S2: f32 = 8.0;
pub const S3: f32 = 12.0;
/// 卡片内边距 / 列表行左右留白
pub const S4: f32 = 16.0;
/// 内容区左右留白
pub const S5: f32 = 20.0;
pub const S6: f32 = 24.0;
pub const S8: f32 = 32.0;

// =========================================================================
//  尺寸
// =========================================================================

/// 控件高度（按钮 / 输入框）
pub const BTN_H: f32 = 30.0;
/// 小控件（列表行内 / 工具栏）
pub const BTN_H_SM: f32 = 26.0;
/// 大控件（弹窗主行动）
pub const BTN_H_LG: f32 = 36.0;
/// 列表行高（两行文本）
pub const ROW_H: f32 = 48.0;
/// 列表行高（带进度条）
pub const ROW_H_TALL: f32 = 56.0;
/// 侧栏固定宽度
pub const SIDEBAR_W: f32 = 220.0;
/// 底部操作栏
pub const FOOTER_H: f32 = 52.0;
/// 内容型弹窗宽度
pub const MODAL_W: f32 = 480.0;
/// 确认型弹窗宽度
pub const MODAL_W_SM: f32 = 420.0;
/// 占比条宽度
pub const BAR_W: f32 = 80.0;
/// 占比条高度
pub const BAR_H: f32 = 6.0;

// =========================================================================
//  语义映射
// =========================================================================

/// 推荐等级 → 前景色
pub fn recommend_fg(rec: &Recommend) -> egui::Color32 {
    match rec {
        Recommend::Safe => safe(),
        Recommend::CacheOnly => cache(),
        Recommend::Caution => caution(),
        Recommend::Advanced => danger(),
    }
}

/// 推荐等级 → 底纹色
pub fn recommend_bg(rec: &Recommend) -> egui::Color32 {
    match rec {
        Recommend::Safe => safe_50(),
        Recommend::CacheOnly => cache_50(),
        Recommend::Caution => caution_50(),
        Recommend::Advanced => danger_50(),
    }
}

// =========================================================================
//  egui 全局样式
// =========================================================================

thread_local! {
    /// 已经写进 egui Context 的模式（避免每帧重建 style）
    static APPLIED: Cell<Option<Mode>> = const { Cell::new(None) };
}

/// 每帧同步配色：模式没变就什么都不做。
///
/// UI 入口处调用一次即可，开销是「读一个 thread-local + 一次比较」。
pub fn sync_visuals(ctx: &egui::Context, mode: Mode) {
    let applied = APPLIED.with(|c| c.get());
    if applied == Some(mode) {
        return;
    }
    apply_visuals(ctx, mode);
    APPLIED.with(|c| c.set(Some(mode)));
}

/// 应用配色：先写线程局部配色，再把 egui 内建 widget 拉到同一套语言。
///
/// 直接调用会重建整个 `Style`；正常路径请用 [`sync_visuals`]。
pub fn apply_visuals(ctx: &egui::Context, mode: Mode) {
    set_mode(mode);
    let p = current();

    let mut style = (*ctx.style()).clone();

    style.spacing.item_spacing = egui::vec2(S2, S2);
    style.spacing.button_padding = egui::vec2(S3, 5.0);
    style.spacing.icon_width = 16.0;
    style.spacing.menu_margin = egui::Margin::same(S2);

    let mut visuals = match mode {
        Mode::Light => egui::Visuals::light(),
        Mode::Dark => egui::Visuals::dark(),
    };
    visuals.dark_mode = matches!(mode, Mode::Dark);
    visuals.panel_fill = p.bg;
    visuals.window_fill = p.surface;
    visuals.faint_bg_color = p.surface_2;
    visuals.extreme_bg_color = p.surface_3;
    visuals.window_stroke = egui::Stroke::new(1.0_f32, p.line_2);
    visuals.hyperlink_color = p.brand;

    // 非交互：卡片 / 面板
    visuals.widgets.noninteractive.bg_fill = p.surface;
    visuals.widgets.noninteractive.fg_stroke = egui::Stroke::new(1.0_f32, p.text);
    visuals.widgets.noninteractive.bg_stroke = egui::Stroke::new(1.0_f32, p.line);
    visuals.widgets.noninteractive.rounding = r(R);

    // 可交互未激活：描边 + 白底（设计稿 2.4：卡片优先「1px 描边 + 无阴影」）
    visuals.widgets.inactive.bg_fill = p.surface;
    visuals.widgets.inactive.bg_stroke = egui::Stroke::new(1.0_f32, p.line_2);
    visuals.widgets.inactive.fg_stroke = egui::Stroke::new(1.0_f32, p.text_2);
    visuals.widgets.inactive.rounding = r(R);

    visuals.widgets.hovered.bg_fill = p.surface_3;
    visuals.widgets.hovered.bg_stroke = egui::Stroke::new(1.0_f32, p.line_2);
    visuals.widgets.hovered.fg_stroke = egui::Stroke::new(1.0_f32, p.text);
    visuals.widgets.hovered.rounding = r(R);

    visuals.widgets.active.bg_fill = p.surface_4;
    visuals.widgets.active.bg_stroke = egui::Stroke::new(1.0_f32, p.line_3);
    visuals.widgets.active.fg_stroke = egui::Stroke::new(1.0_f32, p.text);
    visuals.widgets.active.rounding = r(R);

    // 选中态：品牌底 + 品牌描边
    visuals.selection.bg_fill = p.brand_50;
    visuals.selection.stroke = egui::Stroke::new(1.0_f32, p.brand);

    // 禁用态不在 egui 里配：egui 0.29 的 `Widgets` 没有 disabled 变体，
    // 禁用样式由 `widgets::button` 自己画（灰底 + 灰字，而不是降透明度）。

    style.visuals = visuals;
    ctx.set_style(style);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn light_and_dark_palettes_differ_on_every_token() {
        // 防止「新增 token 只填了一版」——浅深两版必须在所有 token 上都不同
        let l = Palette::LIGHT;
        let d = Palette::DARK;
        let pairs: Vec<(&str, egui::Color32, egui::Color32)> = vec![
            ("bg", l.bg, d.bg),
            ("surface", l.surface, d.surface),
            ("surface_2", l.surface_2, d.surface_2),
            ("surface_3", l.surface_3, d.surface_3),
            ("surface_4", l.surface_4, d.surface_4),
            ("line", l.line, d.line),
            ("line_2", l.line_2, d.line_2),
            ("line_3", l.line_3, d.line_3),
            ("text", l.text, d.text),
            ("text_2", l.text_2, d.text_2),
            ("text_3", l.text_3, d.text_3),
            ("text_4", l.text_4, d.text_4),
            ("brand", l.brand, d.brand),
            ("brand_600", l.brand_600, d.brand_600),
            ("brand_50", l.brand_50, d.brand_50),
            ("brand_100", l.brand_100, d.brand_100),
            ("safe", l.safe, d.safe),
            ("safe_50", l.safe_50, d.safe_50),
            ("cache", l.cache, d.cache),
            ("cache_50", l.cache_50, d.cache_50),
            ("caution", l.caution, d.caution),
            ("caution_50", l.caution_50, d.caution_50),
            ("danger", l.danger, d.danger),
            ("danger_50", l.danger_50, d.danger_50),
            ("danger_100", l.danger_100, d.danger_100),
            ("danger_600", l.danger_600, d.danger_600),
            ("info", l.info, d.info),
            ("info_50", l.info_50, d.info_50),
            ("scrim", l.scrim, d.scrim),
        ];
        for (name, a, b) in pairs {
            assert_ne!(a, b, "token `{name}` 在浅/深两版取值相同，漏配了？");
        }
    }

    #[test]
    fn set_mode_switches_current_palette() {
        set_mode(Mode::Dark);
        assert_eq!(mode(), Mode::Dark);
        assert_eq!(text(), Palette::DARK.text);
        set_mode(Mode::Light);
        assert_eq!(mode(), Mode::Light);
        assert_eq!(text(), Palette::LIGHT.text);
    }

    /// WCAG 相对亮度，用于对比度检查
    fn luminance(c: egui::Color32) -> f64 {
        fn ch(v: u8) -> f64 {
            let s = v as f64 / 255.0;
            if s <= 0.03928 {
                s / 12.92
            } else {
                ((s + 0.055) / 1.055).powf(2.4)
            }
        }
        let [r, g, b, _] = c.to_array();
        0.2126 * ch(r) + 0.7152 * ch(g) + 0.0722 * ch(b)
    }

    fn contrast(a: egui::Color32, b: egui::Color32) -> f64 {
        let (la, lb) = (luminance(a), luminance(b));
        let (hi, lo) = if la > lb { (la, lb) } else { (lb, la) };
        (hi + 0.05) / (lo + 0.05)
    }

    #[test]
    fn body_text_meets_wcag_aa_on_both_themes() {
        // 正文 4.5:1，次要文本（用于非关键说明）放宽到 3:1
        for (label, pal) in [("light", Palette::LIGHT), ("dark", Palette::DARK)] {
            assert!(
                contrast(pal.text, pal.surface) >= 4.5,
                "{label}: 主文本对比度 {:.2} < 4.5",
                contrast(pal.text, pal.surface)
            );
            assert!(
                contrast(pal.text_2, pal.surface) >= 4.5,
                "{label}: 次文本对比度 {:.2} < 4.5",
                contrast(pal.text_2, pal.surface)
            );
            assert!(
                contrast(pal.text_3, pal.surface) >= 3.0,
                "{label}: 辅助文本对比度 {:.2} < 3.0",
                contrast(pal.text_3, pal.surface)
            );
        }
    }

    #[test]
    fn semantic_colors_are_readable_on_surface() {
        // 语义色同时用于文字（等级徽标），必须能看清
        for (label, pal) in [("light", Palette::LIGHT), ("dark", Palette::DARK)] {
            for (name, c) in [
                ("safe", pal.safe),
                ("cache", pal.cache),
                ("caution", pal.caution),
                ("danger", pal.danger),
            ] {
                assert!(
                    contrast(c, pal.surface) >= 3.0,
                    "{label}: {name} 在表面色上对比度 {:.2} < 3.0",
                    contrast(c, pal.surface)
                );
            }
        }
    }
}
