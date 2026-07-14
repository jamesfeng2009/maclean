//! 菜单栏 HUD
//!
//! 在 macOS 菜单栏（右上角）显示常驻图标，
//! 实时显示磁盘使用率，点击展开快捷操作菜单。

use std::collections::HashMap;
use tray_icon::menu::{MenuEvent, MenuItem, PredefinedMenuItem, Submenu};
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
    action_map: HashMap<String, TrayAction>,
    /// 保留 MenuItem 实例，防止它们被提前 drop。
    /// Submenu::append 内部会 clone，但显式持有可避免生命周期歧义。
    _menu_items: Vec<MenuItem>,
    /// 上次更新的磁盘使用率
    last_disk_pct: f32,
}

impl MenuBarHud {
    /// 创建菜单栏 HUD
    pub fn new() -> Self {
        Self {
            tray_icon: None,
            action_map: HashMap::new(),
            _menu_items: Vec::new(),
            last_disk_pct: -1.0,
        }
    }

    /// 初始化菜单栏图标
    pub fn init(&mut self) {
        let icon = create_icon(0.0);

        // macOS 上必须用 Submenu 作为托盘右键菜单的根，
        // Menu::append 在 macOS 只允许添加 Submenu。
        let menu = Submenu::new("Maclean", true);

        let mut items = Vec::new();
        add_title(&menu, &mut items);
        add_action(
            &menu,
            &mut items,
            &mut self.action_map,
            "quick_scan",
            "⚡ 快速扫描",
            TrayAction::QuickScan,
        );
        add_action(
            &menu,
            &mut items,
            &mut self.action_map,
            "quick_clean",
            "🗑️ 一键清理 (安全项)",
            TrayAction::QuickClean,
        );
        add_action(
            &menu,
            &mut items,
            &mut self.action_map,
            "show_window",
            "📊 打开主窗口",
            TrayAction::ShowWindow,
        );
        add_action(
            &menu,
            &mut items,
            &mut self.action_map,
            "quit",
            "退出 Maclean",
            TrayAction::Quit,
        );

        self._menu_items = items;

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
            log::log(&format!("收到菜单事件 id={}", id));

            if let Some(action) = self.action_map.get(id) {
                log::log(&format!("匹配到操作: {:?}", action));
                actions.push(action.clone());
            } else {
                log::log("未匹配到任何操作");
            }
        }

        actions
    }
}

/// 添加不可点击的标题项
fn add_title(menu: &Submenu, items: &mut Vec<MenuItem>) {
    let title = MenuItem::with_id("maclean.title", "🧹 Maclean", false, None);
    let _ = menu.append(&title);
    let _ = menu.append(&PredefinedMenuItem::separator());
    items.push(title);
}

/// 添加一个可点击菜单项，并注册 ID → 操作映射
fn add_action(
    menu: &Submenu,
    items: &mut Vec<MenuItem>,
    map: &mut HashMap<String, TrayAction>,
    id: &str,
    text: &str,
    action: TrayAction,
) {
    let item = MenuItem::with_id(id, text, true, None);
    map.insert(item.id().0.to_string(), action);
    let _ = menu.append(&item);
    items.push(item);
    let _ = menu.append(&PredefinedMenuItem::separator());
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
