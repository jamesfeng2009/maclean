//! TUI 渲染逻辑
//!
//! 使用 ratatui 渲染磁盘概览、Tab 栏、扫描结果列表和快捷键提示。

use ratatui::prelude::*;
use ratatui::widgets::*;

use crate::app::{App, ConfirmState, ScanState, Tab};
use crate::scanner::{format_size, ScanItem};

/// 渲染整个 UI
pub fn render(frame: &mut Frame, app: &mut App) {
    let chunks = Layout::vertical([
        Constraint::Length(3),  // 磁盘概览
        Constraint::Length(3),  // Tab 栏
        Constraint::Min(10),   // 列表区域
        Constraint::Length(1),  // 快捷键提示
    ])
    .split(frame.area());

    render_disk_overview(frame, app, chunks[0]);
    render_tabs(frame, app, chunks[1]);
    render_list(frame, app, chunks[2]);
    render_help_bar(frame, chunks[3]);

    // 删除确认弹窗
    if matches!(app.confirm, ConfirmState::Pending) {
        render_confirm_popup(frame, app);
    }

    // 删除中弹窗
    if matches!(app.confirm, ConfirmState::Deleting) {
        render_deleting_popup(frame, app);
    }
}

/// 渲染磁盘概览
fn render_disk_overview(frame: &mut Frame, app: &App, area: Rect) {
    let used = app.disk_total.saturating_sub(app.disk_free);
    let used_pct = if app.disk_total > 0 {
        (used as f64 / app.disk_total as f64 * 100.0) as u64
    } else {
        0
    };

    let gauge_color = if used_pct > 85 {
        Color::Red
    } else if used_pct > 70 {
        Color::Yellow
    } else {
        Color::Green
    };

    let gauge = Gauge::default()
        .block(
            Block::new()
                .borders(Borders::ALL)
                .title(format!(
                    " 磁盘: {} 已用 / {} 总计 / {} 可用 ({}) ",
                    format_size(used),
                    format_size(app.disk_total),
                    format_size(app.disk_free),
                    app.tab.title(),
                )),
        )
        .gauge_style(Style::new().fg(gauge_color))
        .percent(used_pct as u16);

    frame.render_widget(gauge, area);
}

/// 渲染 Tab 栏
fn render_tabs(frame: &mut Frame, app: &App, area: Rect) {
    let tabs: Vec<Line> = Tab::all()
        .iter()
        .map(|t| {
            let idx = match t {
                Tab::DevCache => 0,
                Tab::LargeFiles => 1,
                Tab::AppCache => 2,
                Tab::Apfs => 3,
            };
            let count = app.results[idx].len();
            let title = if count > 0 {
                format!(" {} ({}) ", t.title(), count)
            } else {
                format!(" {} ", t.title())
            };

            if *t == app.tab {
                Line::styled(title, Style::new().fg(Color::Black).bg(Color::Cyan).bold())
            } else {
                Line::styled(title, Style::new().fg(Color::DarkGray))
            }
        })
        .collect();

    let tabs_widget = Tabs::new(tabs)
        .block(Block::new().borders(Borders::ALL))
        .divider("│");

    frame.render_widget(tabs_widget, area);
}

/// 渲染扫描结果列表
fn render_list(frame: &mut Frame, app: &App, area: Rect) {
    let items = app.current_items();
    let state = app.current_scan_state();

    let title = match state {
        ScanState::Idle => format!(" {} - 按 r 开始扫描 ", app.tab.title()),
        ScanState::Scanning => format!(" {} - 扫描中... ", app.tab.title()),
        ScanState::Done => {
            let total: u64 = items.iter().map(|i| i.size_bytes).sum();
            format!(
                " {} - {} 项, 共 {} ({}ms) ",
                app.tab.title(),
                items.len(),
                format_size(total),
                app.scan_time_ms[match app.tab {
                    Tab::DevCache => 0,
                    Tab::LargeFiles => 1,
                    Tab::AppCache => 2,
                    Tab::Apfs => 3,
                }],
            )
        }
    };

    if items.is_empty() {
        let msg = match state {
            ScanState::Idle => "按 r 键开始扫描",
            ScanState::Scanning => "正在扫描，请稍候...",
            ScanState::Done => "扫描完成，未发现可清理项目",
        };
        let paragraph = Paragraph::new(msg)
            .alignment(Alignment::Center)
            .block(Block::new().borders(Borders::ALL).title(format!(" {} ", title)));
        frame.render_widget(paragraph, area);
        return;
    }

    // 构建列表项
    let list_items: Vec<ListItem> = items
        .iter()
        .enumerate()
        .skip(app.scroll_offset)
        .take(20) // 最多显示 20 行
        .map(|(i, item)| {
            let checkbox = if item.selected {
                "[x]"
            } else if item.deletable {
                "[ ]"
            } else {
                "[!]"
            };

            let size_str = if item.size_bytes == 0 {
                "未知".to_string()
            } else {
                format_size(item.size_bytes)
            };

            let path_display = truncate_path(&item.path, 50);

            let line = Line::from(vec![
                Span::styled(format!("{:>8} ", i + 1), Style::new().fg(Color::DarkGray)),
                Span::styled(format!("{} ", checkbox), Style::new().fg(
                    if item.selected {
                        Color::Green
                    } else if !item.deletable {
                        Color::Red
                    } else {
                        Color::Gray
                    },
                )),
                Span::styled(format!("{:<10} ", item.category), Style::new().fg(Color::Cyan)),
                Span::styled(format!("{:>10} ", size_str), Style::new().fg(Color::Yellow)),
                Span::raw(path_display),
            ]);

            if i == app.list_index {
                ListItem::new(line).style(Style::new().bg(Color::DarkGray))
            } else {
                ListItem::new(line)
            }
        })
        .collect();

    let list = List::new(list_items)
        .block(Block::new().borders(Borders::ALL).title(format!(" {} ", title)))
        .highlight_style(Style::new().bg(Color::DarkGray));

    frame.render_widget(list, area);

    // 选中项统计
    let selected_count = app.selected_count();
    let selected_size = app.selected_selected_size_display();
    if selected_count > 0 {
        let info = format!(
            " 已选 {} 项, 共 {} ",
            selected_count,
            selected_size,
        );
        let info_area = Rect {
            x: area.x + 1,
            y: area.bottom() - 1,
            width: info.len() as u16 + 2,
            height: 1,
        };
        let info_span = Span::styled(info, Style::new().fg(Color::Black).bg(Color::Green));
        frame.render_widget(info_span, info_area);
    }
}

/// 渲染快捷键提示栏
fn render_help_bar(frame: &mut Frame, area: Rect) {
    let help = if matches!(frame.area().width, w if w < 80) {
        "Tab切换 | ↑↓选择 | Space勾选 | d删除 | a全选 | n取消 | r扫描 | q退出"
    } else {
        " Tab切换 | ↑↓/jk选择 | Space勾选 | d删除 | a全选 | n取消全选 | r重新扫描 | q退出 "
    };

    let bar = Paragraph::new(help)
        .style(Style::new().fg(Color::DarkGray))
        .alignment(Alignment::Center);

    frame.render_widget(bar, area);
}

/// 渲染删除确认弹窗
fn render_confirm_popup(frame: &mut Frame, app: &App) {
    let count = app.selected_count();
    let size = app.selected_total_size();

    let popup_area = centered_rect(50, 7, frame.area());

    let popup = Clear;
    frame.render_widget(popup, popup_area);

    let text = vec![
        Line::from(""),
        Line::from(vec![
            Span::raw("  即将删除 "),
            Span::styled(format!("{}", count), Style::new().fg(Color::Yellow).bold()),
            Span::raw(" 项, 共 "),
            Span::styled(format!("{}", format_size(size)), Style::new().fg(Color::Red).bold()),
        ]),
        Line::from(""),
        Line::from("  此操作不可撤销!"),
        Line::from(""),
        Line::from(vec![
            Span::styled("  [y] ", Style::new().fg(Color::Green).bold()),
            Span::raw("确认删除  "),
            Span::styled("[n/Esc] ", Style::new().fg(Color::Red).bold()),
            Span::raw("取消"),
        ]),
    ];

    let block = Block::new()
        .borders(Borders::ALL)
        .title(" 确认删除 ")
        .border_style(Style::new().fg(Color::Red));

    let paragraph = Paragraph::new(text).block(block);
    frame.render_widget(paragraph, popup_area);
}

/// 渲染删除中弹窗
fn render_deleting_popup(frame: &mut Frame, app: &App) {
    let popup_area = centered_rect(60, 12, frame.area());

    let popup = Clear;
    frame.render_widget(popup, popup_area);

    // 显示最近 8 条日志
    let logs: Vec<Line> = app
        .logs
        .iter()
        .rev()
        .take(8)
        .map(|log| {
            let color = if log.starts_with('✓') {
                Color::Green
            } else if log.starts_with('✗') {
                Color::Red
            } else {
                Color::Yellow
            };
            Line::from(Span::styled(log.as_str(), Style::new().fg(color)))
        })
        .collect();

    let block = Block::new()
        .borders(Borders::ALL)
        .title(" 正在清理... ")
        .border_style(Style::new().fg(Color::Cyan));

    let paragraph = Paragraph::new(logs).block(block);
    frame.render_widget(paragraph, popup_area);
}

/// 居中弹窗辅助函数
fn centered_rect(percent_x: u16, height: u16, area: Rect) -> Rect {
    let popup_width = area.width * percent_x / 100;
    let popup_x = (area.width - popup_width) / 2;
    let popup_y = (area.height - height) / 2;

    Rect {
        x: popup_x,
        y: popup_y,
        width: popup_width,
        height,
    }
}

/// 截断路径，保留末尾部分
fn truncate_path(path: &str, max_len: usize) -> String {
    if path.len() <= max_len {
        return path.to_string();
    }

    let suffix = &path[path.len() - max_len + 3..];
    format!("...{}", suffix)
}

/// App 的辅助 trait（在 ui 中扩展）
impl App {
    /// 获取选中项总大小的格式化字符串
    fn selected_selected_size_display(&self) -> String {
        format_size(self.selected_total_size())
    }
}
