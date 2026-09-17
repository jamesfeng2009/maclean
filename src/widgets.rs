//! UI 原子组件
//!
//! 对应设计稿 v2 第 03 节「组件库」。所有按钮 / 复选框 / 徽标 / 占比条 / 弹窗外壳
//! 都必须走这里，页面代码不要再就地 `egui::Button::new(...)`，否则控件高度、
//! 圆角、配色会再次失控（设计稿 8.1 已把"按钮高度不统一"列为待修项）。

use eframe::egui;

use crate::icons::{self, Icon};
use crate::scanner::Recommend;
use crate::theme;

// =========================================================================
//  按钮
// =========================================================================

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Btn {
    /// 实心主行动（一屏最多一个）
    Primary,
    /// 次级：白底 + 描边
    Secondary,
    /// 幽灵：透明底，hover 才出底
    Ghost,
    /// 危险实心
    Danger,
    /// 危险描边（软）
    DangerSoft,
}

impl Btn {
    /// (常态底, 文字, 描边, hover 底)
    fn colors(self) -> (egui::Color32, egui::Color32, egui::Color32, egui::Color32) {
        use egui::Color32 as C;
        match self {
            Btn::Primary => (theme::brand(), C::WHITE, C::TRANSPARENT, theme::brand_600()),
            Btn::Secondary => (
                theme::surface(),
                theme::text(),
                theme::line_2(),
                theme::surface_3(),
            ),
            Btn::Ghost => (
                C::TRANSPARENT,
                theme::text_2(),
                C::TRANSPARENT,
                theme::surface_3(),
            ),
            Btn::Danger => (
                theme::danger(),
                C::WHITE,
                C::TRANSPARENT,
                theme::danger_600(),
            ),
            Btn::DangerSoft => (
                theme::danger_50(),
                theme::danger(),
                theme::danger_100(),
                theme::danger_100(),
            ),
        }
    }
}

/// 标准按钮（高度 30，小号 26，大号 36）
pub fn button(
    ui: &mut egui::Ui,
    ic: Option<Icon>,
    label: &str,
    kind: Btn,
    height: f32,
) -> egui::Response {
    button_enabled(ui, true, ic, label, kind, height)
}

/// 可禁用按钮。禁用态按设计稿 3.1：`#EFEFF2` 底 + `#AFAFB8` 字，不是降透明度。
pub fn button_enabled(
    ui: &mut egui::Ui,
    enabled: bool,
    ic: Option<Icon>,
    label: &str,
    kind: Btn,
    height: f32,
) -> egui::Response {
    let font = egui::FontId::proportional(13.0);
    let galley = ui
        .painter()
        .layout_no_wrap(label.to_string(), font, egui::Color32::WHITE);

    let icon_size = if ic.is_some() { 16.0 } else { 0.0 };
    let gap = if ic.is_some() { theme::S1 + 2.0 } else { 0.0 };
    let pad_x = 12.0;
    let width = (pad_x * 2.0 + icon_size + gap + galley.size().x).max(height);

    let (rect, response) = ui.allocate_exact_size(
        egui::vec2(width, height),
        if enabled {
            egui::Sense::click()
        } else {
            egui::Sense::hover()
        },
    );

    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        let (base_bg, base_fg, base_stroke, hover_bg) = kind.colors();

        let bg = if !enabled {
            theme::surface_3()
        } else if response.hovered() || response.is_pointer_button_down_on() {
            hover_bg
        } else {
            base_bg
        };
        let fg = if !enabled { theme::text_4() } else { base_fg };
        let stroke_c = if !enabled {
            egui::Color32::TRANSPARENT
        } else {
            base_stroke
        };

        painter.rect_filled(rect, theme::r(theme::R), bg);
        if stroke_c != egui::Color32::TRANSPARENT {
            painter.rect_stroke(rect, theme::r(theme::R), egui::Stroke::new(1.0, stroke_c));
        }

        let mut x = rect.min.x + pad_x;
        if let Some(i) = ic {
            let ir = egui::Rect::from_min_size(
                egui::pos2(x, rect.center().y - icon_size / 2.0),
                egui::vec2(icon_size, icon_size),
            );
            icons::paint(painter, ir, i, fg);
            x += icon_size + gap;
        }
        let text_y = rect.center().y - galley.size().y / 2.0;
        painter.galley(egui::pos2(x, text_y), galley, fg);
    }

    response
}

/// 纯图标按钮（30×30 / 26×26，工具栏用）
pub fn icon_button(ui: &mut egui::Ui, ic: Icon, height: f32) -> egui::Response {
    icon_button_enabled(ui, ic, height, true)
}

/// 可禁用的图标按钮
pub fn icon_button_enabled(
    ui: &mut egui::Ui,
    ic: Icon,
    height: f32,
    enabled: bool,
) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(
        egui::vec2(height, height),
        if enabled {
            egui::Sense::click()
        } else {
            egui::Sense::hover()
        },
    );
    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        if enabled && response.hovered() {
            painter.rect_filled(rect, theme::r(theme::R), theme::surface_3());
        }
        let inset = (height - 16.0) / 2.0;
        let ir = rect.shrink(inset.max(5.0));
        let color = if !enabled {
            theme::text_4()
        } else if response.hovered() {
            theme::text()
        } else {
            theme::text_2()
        };
        icons::paint(painter, ir, ic, color);
    }
    response
}

// =========================================================================
//  选择与开关
// =========================================================================

/// 复选框状态
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Check {
    Off,
    On,
    /// 部分选中（分组用）
    Partial,
}

/// 三态复选框（16×16，圆角 4）
///
/// 禁用项用灰底但**不隐藏**——设计稿 3.2：「要让用户看见有东西不能选」。
pub fn checkbox(ui: &mut egui::Ui, state: Check, enabled: bool) -> egui::Response {
    let size = egui::vec2(16.0, 16.0);
    let (rect, response) = ui.allocate_exact_size(
        size,
        if enabled {
            egui::Sense::click()
        } else {
            egui::Sense::hover()
        },
    );

    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        let rr = theme::r(theme::R_XS);
        let filled = matches!(state, Check::On | Check::Partial);

        if filled {
            painter.rect_filled(
                rect,
                rr,
                if enabled {
                    theme::brand()
                } else {
                    theme::line_3()
                },
            );
            let ic = if state == Check::On {
                Icon::Check
            } else {
                Icon::Minus
            };
            let ir = egui::Rect::from_center_size(rect.center(), egui::vec2(12.0, 12.0));
            icons::paint(painter, ir, ic, egui::Color32::WHITE);
        } else {
            painter.rect_filled(
                rect,
                rr,
                if enabled {
                    theme::surface()
                } else {
                    theme::surface_3()
                },
            );
            painter.rect_stroke(rect, rr, egui::Stroke::new(1.5, theme::line_3()));
        }
    }

    response
}

/// 开关（36×22）
pub fn toggle(ui: &mut egui::Ui, value: &mut bool, enabled: bool) -> egui::Response {
    let size = egui::vec2(36.0, 22.0);
    let (rect, response) = ui.allocate_exact_size(
        size,
        if enabled {
            egui::Sense::click()
        } else {
            egui::Sense::hover()
        },
    );
    if response.clicked() && enabled {
        *value = !*value;
    }
    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        let rr = theme::r(theme::R_FULL);
        let on = *value && enabled;
        painter.rect_filled(
            rect,
            rr,
            if on {
                theme::brand()
            } else if enabled {
                theme::line_3()
            } else {
                theme::surface_3()
            },
        );
        let knob_r = 9.0;
        let knob_x = if on {
            rect.max.x - knob_r - 2.0
        } else {
            rect.min.x + knob_r + 2.0
        };
        painter.circle_filled(
            egui::pos2(knob_x, rect.center().y),
            knob_r,
            egui::Color32::WHITE,
        );
    }
    response
}

// =========================================================================
//  徽标与占比条
// =========================================================================

/// 状态徽标（高 20，圆角 4，11/600）
///
/// 左侧 6px 圆点或 11px 图标。等级徽标一律走 `recommend_badge`，颜色不要自己配。
pub fn badge(
    ui: &mut egui::Ui,
    text: &str,
    fg: egui::Color32,
    bg: egui::Color32,
    ic: Option<Icon>,
) {
    let height = 20.0;
    let font = egui::FontId::proportional(11.0);
    let galley = ui
        .painter()
        .layout_no_wrap(text.to_string(), font, egui::Color32::WHITE);

    let dot = if ic.is_some() { 11.0 } else { 6.0 };
    let gap = 5.0;
    let pad_x = 8.0;
    let width = pad_x * 2.0 + dot + gap + galley.size().x;

    let (rect, _resp) = ui.allocate_exact_size(egui::vec2(width, height), egui::Sense::hover());
    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        painter.rect_filled(rect, theme::r(theme::R_XS), bg);
        let cy = rect.center().y;
        if let Some(i) = ic {
            let ir = egui::Rect::from_center_size(
                egui::pos2(rect.min.x + pad_x + 5.5, cy),
                egui::vec2(11.0, 11.0),
            );
            icons::paint(painter, ir, i, fg);
        } else {
            painter.circle_filled(egui::pos2(rect.min.x + pad_x + 3.0, cy), 3.0, fg);
        }
        painter.galley(
            egui::pos2(rect.min.x + pad_x + dot + gap, cy - galley.size().y / 2.0),
            galley,
            fg,
        );
    }
}

/// 推荐等级徽标（文案与配色唯一入口）
pub fn recommend_badge(ui: &mut egui::Ui, rec: &Recommend, lang_en: bool) {
    let fg = theme::recommend_fg(rec);
    let bg = theme::recommend_bg(rec);
    badge(ui, theme::recommend_label(rec, lang_en), fg, bg, None);
}

/// 占比条（80×6，表示该项占当前列表总量的比例）
pub fn ratio_bar(ui: &mut egui::Ui, ratio: f32, color: egui::Color32) {
    let (rect, _resp) =
        ui.allocate_exact_size(egui::vec2(theme::BAR_W, theme::BAR_H), egui::Sense::hover());
    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        painter.rect_filled(rect, theme::r(3.0), theme::surface_3());
        let w = rect.width() * ratio.clamp(0.0, 1.0);
        if w > 0.5 {
            let fill = egui::Rect::from_min_size(rect.min, egui::vec2(w, theme::BAR_H));
            painter.rect_filled(fill, theme::r(3.0), color);
        }
    }
}

/// 通用进度条（用于扫描 / 删除进度）
pub fn progress_bar(ui: &mut egui::Ui, width: f32, ratio: f32, color: egui::Color32) {
    let (rect, _resp) =
        ui.allocate_exact_size(egui::vec2(width, theme::BAR_H), egui::Sense::hover());
    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        painter.rect_filled(rect, theme::r(3.0), theme::surface_3());
        let w = rect.width() * ratio.clamp(0.0, 1.0);
        if w > 0.5 {
            let fill = egui::Rect::from_min_size(rect.min, egui::vec2(w, theme::BAR_H));
            painter.rect_filled(fill, theme::r(3.0), color);
        }
    }
}

// =========================================================================
//  弹窗外壳
// =========================================================================

/// 全屏遮罩 rgba(27,27,31,.28)。
///
/// 必须在弹窗 `egui::Window` **之前**调用，否则会盖住弹窗内容。
pub fn scrim(ctx: &egui::Context) {
    let screen = ctx.screen_rect();
    egui::Area::new(egui::Id::new("modal_scrim"))
        .order(egui::Order::Foreground)
        .fixed_pos(screen.min)
        .interactable(false)
        .show(ctx, |ui| {
            ui.painter()
                .rect_filled(screen, egui::Rounding::ZERO, theme::scrim());
        });
}

/// 弹窗内容外框（圆角 14 + 大阴影）
///
/// 注意：自定义 frame 会覆盖 egui 默认的 `window_margin`，所以这里显式给
/// 20/16 的内边距，否则所有弹窗内容都会贴边。
pub fn modal_frame() -> egui::Frame {
    egui::Frame::none()
        .fill(theme::surface())
        .rounding(theme::r(theme::R_LG))
        .inner_margin(egui::Margin::symmetric(theme::S5, theme::S4))
        .shadow(egui::epaint::Shadow {
            offset: egui::vec2(0.0, 12.0),
            blur: 48.0,
            spread: 0.0,
            color: egui::Color32::from_rgba_premultiplied(17, 17, 26, 51),
        })
}

/// 弹窗标题区：36×36 图标块 + 标题 + 副标题
pub fn modal_header(
    ui: &mut egui::Ui,
    ic: Icon,
    ic_fg: egui::Color32,
    ic_bg: egui::Color32,
    title: &str,
    subtitle: &str,
) {
    egui::Frame::none()
        .inner_margin(egui::Margin {
            left: 0.0,
            right: 0.0,
            top: 0.0,
            bottom: theme::S2,
        })
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                let (rect, _r) =
                    ui.allocate_exact_size(egui::vec2(36.0, 36.0), egui::Sense::hover());
                ui.painter().rect_filled(rect, theme::r(theme::R_MD), ic_bg);
                let ir = egui::Rect::from_center_size(rect.center(), egui::vec2(20.0, 20.0));
                icons::paint(ui.painter(), ir, ic, ic_fg);

                ui.add_space(theme::S3);
                ui.vertical(|ui| {
                    ui.label(
                        egui::RichText::new(title)
                            .size(15.0)
                            .strong()
                            .color(theme::text()),
                    );
                    ui.colored_label(theme::text_2(), egui::RichText::new(subtitle).size(12.0));
                });
            });
        });
}

/// 弹窗底部操作栏（主行动在右，取消在左）
pub fn modal_footer<R>(ui: &mut egui::Ui, content: impl FnOnce(&mut egui::Ui) -> R) -> R {
    // 负 outer_margin 让底部栏撑满弹窗宽度（抵消 modal_frame 的左右内边距）
    egui::Frame::none()
        .fill(theme::surface_2())
        .inner_margin(egui::Margin::symmetric(theme::S5, theme::S3))
        .outer_margin(egui::Margin {
            left: -theme::S5,
            right: -theme::S5,
            top: theme::S4,
            bottom: -theme::S4,
        })
        .stroke(egui::Stroke::new(1.0, theme::line()))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.spacing_mut().item_spacing.x = theme::S2;
                    content(ui)
                })
                .inner
            })
            .inner
        })
        .inner
}

// =========================================================================
//  状态页
// =========================================================================

/// 空状态 / 状态页
///
/// 设计稿 5.7：「扫描完成但为空」和「从未扫描」是两种完全不同的情绪，
/// 不能合并成一句"暂无数据"。
pub fn state_page(
    ui: &mut egui::Ui,
    ic: Icon,
    ic_color: egui::Color32,
    ic_bg: egui::Color32,
    title: &str,
    desc: &str,
    action: Option<(&str, Icon)>,
) -> bool {
    let mut clicked = false;
    let size = ui.available_size();
    egui::Frame::none()
        .fill(theme::bg())
        .rounding(theme::r(theme::R_MD))
        .show(ui, |ui| {
            ui.set_min_size(size);
            ui.vertical_centered(|ui| {
                ui.add_space(56.0);
                let (rect, _r) =
                    ui.allocate_exact_size(egui::vec2(48.0, 48.0), egui::Sense::hover());
                ui.painter().rect_filled(rect, theme::r(theme::R_LG), ic_bg);
                let ir = egui::Rect::from_center_size(rect.center(), egui::vec2(24.0, 24.0));
                icons::paint(ui.painter(), ir, ic, ic_color);

                ui.add_space(theme::S4);
                ui.label(
                    egui::RichText::new(title)
                        .size(14.0)
                        .strong()
                        .color(theme::text()),
                );
                ui.add_space(theme::S1);
                ui.label(egui::RichText::new(desc).size(12.0).color(theme::text_3()));
                if let Some((label, aic)) = action {
                    ui.add_space(theme::S4);
                    if button(ui, Some(aic), label, Btn::Primary, theme::BTN_H).clicked() {
                        clicked = true;
                    }
                }
            });
        });
    clicked
}

// =========================================================================
//  横幅
// =========================================================================

/// 顶部横幅（信息 / 警告 / 严重）
pub enum BannerKind {
    Info,
    Warn,
    Crit,
}

impl BannerKind {
    fn colors(self) -> (egui::Color32, egui::Color32) {
        match self {
            BannerKind::Info => (theme::info_50(), theme::info()),
            BannerKind::Warn => (theme::caution_50(), theme::caution()),
            BannerKind::Crit => (theme::danger_50(), theme::danger()),
        }
    }
}

/// 一行式横幅，右侧可放行动按钮。返回行动区是否被点击由调用方自行处理。
pub fn banner<R>(
    ui: &mut egui::Ui,
    kind: BannerKind,
    ic: Icon,
    text: &str,
    action: impl FnOnce(&mut egui::Ui) -> R,
) -> R {
    let (bg, fg) = kind.colors();
    egui::Frame::none()
        .fill(bg)
        .inner_margin(egui::Margin::symmetric(theme::S5, theme::S2))
        .stroke(egui::Stroke::new(1.0, theme::line()))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                icons::show(ui, ic, 16.0, fg);
                ui.add_space(theme::S1);
                ui.colored_label(fg, egui::RichText::new(text).size(12.0));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    action(ui)
                })
                .inner
            })
            .inner
        })
        .inner
}
