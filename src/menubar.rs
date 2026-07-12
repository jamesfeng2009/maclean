//! 菜单栏 HUD
//!
//! 在 macOS 菜单栏（右上角）显示常驻图标，
//! 实时显示磁盘使用率，点击展开快捷操作菜单。

use std::sync::mpsc;
use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, TrayIcon, TrayIconBuilder};

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

/// 菜单栏 HUD 管理器
pub struct MenuBarHud {
    tray_icon: Option<TrayIcon>,
    /// 菜单项 ID → 操作 的映射
    menu_items: Vec<(String, TrayAction)>,
    /// 上次更新的磁盘使用率
    last_disk_pct: f32,
}

impl MenuBarHud {
    /// 创建菜单栏 HUD
    pub fn new() -> Self {
        Self {
            tray_icon: None,
            menu_items: Vec::new(),
            last_disk_pct: -1.0,
        }
    }

    /// 初始化菜单栏图标
    pub fn init(&mut self) {
        let icon = create_icon(0.0);

        let menu = build_menu(&mut self.menu_items);

        let tray = TrayIconBuilder::new()
            .with_menu(Box::new(menu))
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

    /// 轮询菜单事件
    pub fn poll_events(&self) -> Vec<TrayAction> {
        let receiver = MenuEvent::receiver();
        let mut actions = Vec::new();

        while let Ok(event) = receiver.try_recv() {
            let id: &str = event.id().0.as_ref();
            for (menu_id, action) in &self.menu_items {
                if menu_id.as_str() == id {
                    actions.push(action.clone());
                    break;
                }
            }
        }

        actions
    }
}

/// 构建菜单
fn build_menu(menu_items: &mut Vec<(String, TrayAction)>) -> Menu {
    let mut menu = Menu::new();

    // 标题项（不可点击）
    let title = MenuItem::new("🧹 Maclean", false, None);
    menu.append(&title).ok();

    let separator1 = PredefinedMenuItem::separator();
    menu.append(&separator1).ok();

    // 快速扫描
    let quick_scan = MenuItem::new("⚡ 快速扫描", true, None);
    menu_items.push((quick_scan.id().0.to_string(), TrayAction::QuickScan));
    menu.append(&quick_scan).ok();

    // 一键清理
    let quick_clean = MenuItem::new("🗑️ 一键清理 (安全项)", true, None);
    menu_items.push((quick_clean.id().0.to_string(), TrayAction::QuickClean));
    menu.append(&quick_clean).ok();

    let separator2 = PredefinedMenuItem::separator();
    menu.append(&separator2).ok();

    // 打开主窗口
    let show_window = MenuItem::new("📊 打开主窗口", true, None);
    menu_items.push((show_window.id().0.to_string(), TrayAction::ShowWindow));
    menu.append(&show_window).ok();

    let separator3 = PredefinedMenuItem::separator();
    menu.append(&separator3).ok();

    // 退出
    let quit = MenuItem::new("退出 Maclean", true, None);
    menu_items.push((quit.id().0.to_string(), TrayAction::Quit));
    menu.append(&quit).ok();

    menu
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

    // 在圆中间画百分比数字（简化版：用白色小方块表示）
    // 实际使用文字渲染需要更复杂的处理，这里用简化图标
    // 在中心画一个白色的扫帚形状（简化为白色圆点）
    let inner_radius = radius * 0.4;
    for y in 0..size {
        for x in 0..size {
            let dx = x as f32 + 0.5 - center;
            let dy = y as f32 + 0.5 - center;
            let dist = (dx * dx + dy * dy).sqrt();

            if dist <= inner_radius {
                let idx = ((y * size + x) * 4) as usize;
                rgba[idx] = 255;     // R
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
        let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
        let log_path = std::path::PathBuf::from(&home).join(".maclean/logs/menubar.log");
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
