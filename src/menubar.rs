//! 菜单栏 HUD
//!
//! 在 macOS 菜单栏 / Windows 托盘显示常驻图标，
//! 实时显示可释放空间，点击展开快捷操作 HUD。

use std::time::Instant;
use tray_icon::{Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};

/// 菜单栏操作事件
#[derive(Debug, Clone)]
pub enum TrayAction {
    /// 打开主窗口
    ShowWindow,
    /// 快速扫描
    QuickScan,
    /// 一键清理安全项
    QuickClean,
    /// 退出应用
    Quit,
}

/// 托盘图标点击信息
#[derive(Debug, Clone, Copy)]
pub struct ClickInfo {
    /// 托盘图标在屏幕坐标系中的中心 X（逻辑像素）
    pub x: f32,
    /// 托盘图标在屏幕坐标系中的底部 Y（逻辑像素）
    pub y: f32,
}

/// 菜单栏 HUD 管理器
pub struct MenuBarHud {
    tray_icon: Option<TrayIcon>,
    /// 上次更新的磁盘使用率
    last_disk_pct: f32,
    /// 上次显示的可释放空间（字节）
    last_releasable_bytes: u64,
    /// 上次更新托盘标题的时间
    last_releasable_update: Option<Instant>,
}

impl MenuBarHud {
    /// 创建菜单栏 HUD
    pub fn new() -> Self {
        Self {
            tray_icon: None,
            last_disk_pct: -1.0,
            last_releasable_bytes: u64::MAX,
            last_releasable_update: None,
        }
    }

    /// 初始化菜单栏图标
    pub fn init(&mut self) {
        let icon = create_icon(0.0);

        // 不使用原生菜单，具体交互由 egui 绘制的 HUD 窗口处理
        let tray = TrayIconBuilder::new()
            .with_menu_on_left_click(false)
            .with_tooltip("Maclean")
            .with_icon(icon)
            .build();

        match tray {
            Ok(t) => {
                self.tray_icon = Some(t);
                log::log("菜单栏 HUD 初始化成功");
            }
            Err(e) => {
                log::log(&format!("菜单栏 HUD 初始化失败: {}", e));
            }
        }
    }

    /// 更新磁盘使用率显示（如果变化超过 1%）
    pub fn update_disk_usage(&mut self, used_pct: f32) {
        if (used_pct - self.last_disk_pct).abs() < 1.0 {
            return;
        }
        self.last_disk_pct = used_pct;

        if let Some(ref tray) = self.tray_icon {
            let icon = create_icon(used_pct);
            tray.set_icon(Some(icon)).ok();
            let tooltip = format!("Maclean - 磁盘使用 {:.0}%", used_pct);
            tray.set_tooltip(Some(&tooltip)).ok();
        }
    }

    /// 更新托盘显示的可释放空间
    ///
    /// - macOS：设置托盘标题为 "🧹 XX GB"（同时更新 tooltip）
    /// - Windows / Linux：仅更新 tooltip
    pub fn update_releasable(&mut self, releasable_bytes: u64, lang_en: bool) {
        let now = Instant::now();
        let changed = releasable_bytes != self.last_releasable_bytes;
        let throttle = self
            .last_releasable_update
            .map(|t| now.duration_since(t).as_secs() < 5)
            .unwrap_or(false);
        if !changed && throttle {
            return;
        }
        self.last_releasable_bytes = releasable_bytes;
        self.last_releasable_update = Some(now);

        if let Some(ref tray) = self.tray_icon {
            let size_str = crate::scanner::format_size(releasable_bytes);

            #[cfg(target_os = "macos")]
            {
                let title = format!("🧹 {}", size_str);
                tray.set_title(Some(&title));
            }

            let tooltip = if lang_en {
                format!("Maclean - {} releasable", size_str)
            } else {
                format!("Maclean - 可释放 {}", size_str)
            };
            tray.set_tooltip(Some(&tooltip)).ok();
        }
    }

    /// 轮询菜单事件（保留兼容，当前已不再依赖菜单触发）
    pub fn poll_events(&self) -> Vec<TrayAction> {
        Vec::new()
    }

    /// 轮询托盘图标点击事件
    ///
    /// 返回点击位置信息。仅对左键松开（Up）事件响应，避免 macOS 上
    /// mouseDown + mouseUp 产生两次 Click 事件导致 HUD 被连续切换回原始状态。
    pub fn poll_click(&self) -> Option<ClickInfo> {
        let receiver = TrayIconEvent::receiver();
        let mut click_info: Option<ClickInfo> = None;
        while let Ok(event) = receiver.try_recv() {
            log::log(&format!("托盘事件: {:?}", event));
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                rect,
                ..
            } = event
            {
                let scale_factor = rect.size.width as f32 / rect.position.x.max(1.0) as f32;
                let scale_factor = if scale_factor.is_finite() && scale_factor > 0.0 {
                    scale_factor
                } else {
                    1.0
                };
                let x = (rect.position.x + rect.size.width as f64 / 2.0) as f32 / scale_factor;
                let y = (rect.position.y + rect.size.height as f64) as f32 / scale_factor;
                click_info = Some(ClickInfo { x, y });
            }
        }
        click_info
    }
}

/// 根据磁盘使用率创建图标
/// 绿色: <70%, 黄色: 70-85%, 红色: >85%
fn create_icon(used_pct: f32) -> Icon {
    let size = 32u32;
    let mut rgba = Vec::with_capacity((size * size * 4) as usize);

    // 颜色选择
    let (r, g, b) = if used_pct > 85.0 {
        (255, 69, 58) // 红色
    } else if used_pct > 70.0 {
        (255, 159, 10) // 橙色
    } else {
        (52, 199, 89) // 绿色
    };

    // 绘制圆形背景
    let center = size as f32 / 2.0;
    let radius = center - 2.0;

    for y in 0..size {
        for x in 0..size {
            let dx = x as f32 + 0.5 - center;
            let dy = y as f32 + 0.5 - center;
            let dist = (dx * dx + dy * dy).sqrt();

            if dist <= radius {
                // 圆内：填充颜色
                let alpha = if dist > radius - 1.0 {
                    // 边缘抗锯齿
                    (255.0 * (radius - dist)).clamp(0.0, 255.0) as u8
                } else {
                    255
                };
                rgba.push(r);
                rgba.push(g);
                rgba.push(b);
                rgba.push(alpha);
            } else {
                // 圆外：透明
                rgba.push(0);
                rgba.push(0);
                rgba.push(0);
                rgba.push(0);
            }
        }
    }

    // 在圆中间画一个白色的扫帚形状（简化为白色圆点）
    let inner_radius = radius * 0.4;
    for y in 0..size {
        for x in 0..size {
            let dx = x as f32 + 0.5 - center;
            let dy = y as f32 + 0.5 - center;
            let dist = (dx * dx + dy * dy).sqrt();

            if dist <= inner_radius {
                let idx = ((y * size + x) * 4) as usize;
                rgba[idx] = 255; // R
                rgba[idx + 1] = 255; // G
                rgba[idx + 2] = 255; // B
                                     // alpha 保持不变
            }
        }
    }

    Icon::from_rgba(rgba, size, size).unwrap_or_else(|_| {
        // 如果创建失败，返回一个 1x1 的透明图标
        Icon::from_rgba(vec![0, 0, 0, 0], 1, 1).unwrap()
    })
}

/// 日志辅助
mod log {
    pub fn log(msg: &str) {
        let home = dirs::home_dir().unwrap_or_else(|| std::path::PathBuf::from("/tmp"));
        let log_path = home.join(".maclean/logs/menubar.log");
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .append(true)
            .create(true)
            .open(&log_path)
        {
            use std::io::Write;
            let ts = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            let _ = writeln!(f, "[{}] {}", ts, msg);
        }
    }
}
