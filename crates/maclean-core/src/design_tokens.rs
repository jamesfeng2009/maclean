//! 共享设计 token（阶段 0 · P1）
//!
//! `maclean-Tauri-UI交互稿.html`（2026-10-01 评审版）中 CSS 变量的单一事实来源。
//! egui 壳（`theme.rs`）与 Tauri/Web 壳都引用这里的常量，保证两端色板、
//! 圆角、间距、阴影与动效曲线同源；改视觉只改这一处。
//!
//! 颜色用 `#RRGGBB` 字符串：Web 侧直接灌进 CSS 变量，egui 侧用
//! [`ColorToken::rgb`] 解析成 `[u8; 3]`。本模块不依赖任何 GUI/Web 技术，
//! 可在核心测试里钉住每个色值，悄悄改色会立刻让测试变红。

use serde::Serialize;

/// 一整套主题色板（对应交互稿 `:root` / `[data-theme="dark"]`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Palette {
    // 中性面
    pub bg: &'static str,
    pub panel: &'static str,
    pub panel_2: &'static str,
    pub rail_bg: &'static str,
    // 文本四级
    pub text: &'static str,
    pub text_2: &'static str,
    pub text_3: &'static str,
    // 描边两级
    pub line: &'static str,
    pub line_2: &'static str,
    // 品牌
    pub brand: &'static str,
    pub brand_600: &'static str,
    pub brand_50: &'static str,
    pub brand_100: &'static str,
    // 语义色（主色 + 浅底）
    pub safe: &'static str,
    pub safe_50: &'static str,
    pub cache: &'static str,
    pub cache_50: &'static str,
    pub caution: &'static str,
    pub caution_50: &'static str,
    pub danger: &'static str,
    pub danger_50: &'static str,
    pub danger_100: &'static str,
}

impl Palette {
    /// 浅色（交互稿 `:root`）。
    pub const LIGHT: Palette = Palette {
        bg: "#F6F7F9",
        panel: "#FFFFFF",
        panel_2: "#FBFCFE",
        rail_bg: "#FFFFFF",
        text: "#111827",
        text_2: "#4B5563",
        text_3: "#9CA3AF",
        line: "#E8EAEE",
        line_2: "#DDE1E7",
        brand: "#4F46E5",
        brand_600: "#4338CA",
        brand_50: "#EEF2FF",
        brand_100: "#E0E7FF",
        safe: "#10B981",
        safe_50: "#E7F8F1",
        cache: "#3B82F6",
        cache_50: "#EBF2FE",
        caution: "#F59E0B",
        caution_50: "#FDF3E3",
        danger: "#EF4444",
        danger_50: "#FEECEC",
        danger_100: "#FBD5D5",
    };

    /// 深色（交互稿 `[data-theme="dark"]`）。
    pub const DARK: Palette = Palette {
        bg: "#0E1013",
        panel: "#171A20",
        panel_2: "#1D2128",
        rail_bg: "#13161B",
        text: "#F2F4F7",
        text_2: "#B6BDC7",
        text_3: "#7C8491",
        line: "#262B33",
        line_2: "#2E343E",
        brand: "#7C7DF0",
        brand_600: "#9B9CF5",
        brand_50: "#262A4A",
        brand_100: "#2E3357",
        safe: "#34D399",
        safe_50: "#12352B",
        cache: "#60A5FA",
        cache_50: "#16283E",
        caution: "#FBBF24",
        caution_50: "#3A2F14",
        danger: "#F87171",
        danger_50: "#421B1D",
        danger_100: "#562428",
    };

    /// 把 `#RRGGBB` 解析为 `(r, g, b)`；非合法 hex 返回 `None`。
    ///
    /// token 全是编译期常量，测试会对整套 palette 调一遍，保证没有手滑
    /// 写成 5 位 / 非 hex 字符。
    pub fn rgb(hex: &str) -> Option<[u8; 3]> {
        let h = hex.strip_prefix('#')?;
        if h.len() != 6 || !h.bytes().all(|b| b.is_ascii_hexdigit()) {
            return None;
        }
        let r = u8::from_str_radix(&h[0..2], 16).ok()?;
        let g = u8::from_str_radix(&h[2..4], 16).ok()?;
        let b = u8::from_str_radix(&h[4..6], 16).ok()?;
        Some([r, g, b])
    }
}

/// 圆角三级（px，交互稿 `--r-lg/md/sm`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Radii {
    pub lg: u32,
    pub md: u32,
    pub sm: u32,
}

impl Radii {
    pub const TOKEN: Radii = Radii {
        lg: 16,
        md: 12,
        sm: 8,
    };
}

/// 布局尺寸（px）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Layout {
    /// 左侧图标导航宽度
    pub rail_w: u32,
    /// 顶栏高度
    pub topbar_h: u32,
    /// 内容区内边距
    pub content_pad: u32,
}

impl Layout {
    pub const TOKEN: Layout = Layout {
        rail_w: 64,
        topbar_h: 60,
        content_pad: 24,
    };
}

/// 两档阴影（Web 直接用 CSS box-shadow 原文；egui 侧取意实现）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Shadows {
    pub shadow: &'static str,
    pub shadow_lg: &'static str,
}

impl Shadows {
    pub const LIGHT: Shadows = Shadows {
        shadow: "0 1px 2px rgba(16,24,40,.05),0 4px 16px rgba(16,24,40,.06)",
        shadow_lg: "0 4px 12px rgba(16,24,40,.10),0 16px 40px rgba(16,24,40,.12)",
    };

    pub const DARK: Shadows = Shadows {
        shadow: "0 1px 2px rgba(0,0,0,.3),0 6px 20px rgba(0,0,0,.35)",
        shadow_lg: "0 8px 20px rgba(0,0,0,.4),0 20px 48px rgba(0,0,0,.45)",
    };
}

/// 统一缓动曲线（交互稿 `--ease`）。
pub const EASE: &str = "cubic-bezier(.22,1,.36,1)";

/// 风险级别徽章语义（safe/cache/caution/danger/ghost），两端共用同一套 key。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum RiskLevel {
    Safe,
    Cache,
    Caution,
    Danger,
    Ghost,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 所有颜色 token 必须是合法 #RRGGBB。
    #[test]
    fn every_color_token_is_valid_hex() {
        for p in [Palette::LIGHT, Palette::DARK] {
            let colors = [
                p.bg,
                p.panel,
                p.panel_2,
                p.rail_bg,
                p.text,
                p.text_2,
                p.text_3,
                p.line,
                p.line_2,
                p.brand,
                p.brand_600,
                p.brand_50,
                p.brand_100,
                p.safe,
                p.safe_50,
                p.cache,
                p.cache_50,
                p.caution,
                p.caution_50,
                p.danger,
                p.danger_50,
                p.danger_100,
            ];
            for c in colors {
                assert!(Palette::rgb(c).is_some(), "非法色值 {c}");
            }
        }
    }

    /// 钉住交互稿评审版关键品牌/背景色，防止悄悄回退到旧 v2 色板。
    #[test]
    fn key_tokens_match_reviewed_mockup() {
        assert_eq!(Palette::LIGHT.bg, "#F6F7F9");
        assert_eq!(Palette::LIGHT.brand, "#4F46E5");
        assert_eq!(Palette::LIGHT.brand_600, "#4338CA");
        assert_eq!(Palette::LIGHT.safe, "#10B981");
        assert_eq!(Palette::LIGHT.cache, "#3B82F6");
        assert_eq!(Palette::LIGHT.caution, "#F59E0B");
        assert_eq!(Palette::LIGHT.danger, "#EF4444");
        assert_eq!(Palette::DARK.bg, "#0E1013");
        assert_eq!(Palette::DARK.panel, "#171A20");
        assert_eq!(Radii::TOKEN.lg, 16);
        assert_eq!(Radii::TOKEN.md, 12);
        assert_eq!(Radii::TOKEN.sm, 8);
        assert_eq!(Layout::TOKEN.rail_w, 64);
        assert_eq!(Layout::TOKEN.topbar_h, 60);
        assert_eq!(Layout::TOKEN.content_pad, 24);
    }

    /// 亮暗同名 token 不允许完全相同（深/浅模式必须可辨）。
    #[test]
    fn light_and_dark_palettes_are_distinct() {
        let l = Palette::LIGHT;
        let d = Palette::DARK;
        assert_ne!(l.bg, d.bg);
        assert_ne!(l.panel, d.panel);
        assert_ne!(l.text, d.text);
        assert_ne!(l.brand, d.brand);
    }
}
