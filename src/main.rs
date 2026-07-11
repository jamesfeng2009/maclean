//! maclean - macOS 磁盘清理 TUI 工具
//!
//! 专注开发者缓存与深度清理，扫描并清理 Rust/Xcode/Node.js/Go 等
//! 开发者工具产生的缓存、构建产物，以及 APFS 快照等。

mod app;
mod i18n;
mod safety;
mod scanner;
mod ui;

// 初始化国际化，加载 locales/ 目录下的翻译文件
rust_i18n::i18n!("locales");

use std::io::{self, stdout};

use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use crossterm::ExecutableCommand;
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;

use app::{App, ConfirmState, ScanState};

fn main() -> io::Result<()> {
    // 初始化终端
    enable_raw_mode()?;
    stdout().execute(EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout());
    let mut terminal = Terminal::new(backend)?;

    // 创建 App 状态
    let mut app = App::new();

    // 首次自动扫描当前 Tab
    app.scan_current();

    // 事件循环
    run_event_loop(&mut terminal, &mut app)?;

    // 恢复终端
    disable_raw_mode()?;
    stdout().execute(LeaveAlternateScreen)?;

    Ok(())
}

/// 主事件循环
fn run_event_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut App,
) -> io::Result<()> {
    loop {
        // 渲染
        terminal.draw(|frame| ui::render(frame, app))?;

        // 处理输入
        if !event::poll(std::time::Duration::from_millis(100))? {
            continue;
        }

        let Event::Key(key) = event::read()? else {
            continue;
        };

        if key.kind != KeyEventKind::Press {
            continue;
        }

        // 处理确认状态下的按键
        match &app.confirm {
            ConfirmState::Pending => {
                match key.code {
                    KeyCode::Char('y') | KeyCode::Char('Y') => {
                        app.confirm_delete();
                    }
                    KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                        app.cancel_delete();
                    }
                    _ => {}
                }
                continue;
            }
            ConfirmState::Deleting => {
                // 删除中不接受输入，等待完成
                continue;
            }
            ConfirmState::None => {}
        }

        // 正常状态下的按键处理
        match key.code {
            // 退出
            KeyCode::Char('q') | KeyCode::Esc => {
                app.quit();
            }
            // Tab 切换
            KeyCode::Tab | KeyCode::BackTab => {
                if key.code == KeyCode::Tab {
                    app.next_tab();
                } else {
                    app.prev_tab();
                }
            }
            // 上一个 Tab (Shift+Tab 在某些终端是 BackTab)
            KeyCode::Char('h') => {
                app.prev_tab();
            }
            // 下一个 Tab
            KeyCode::Char('l') => {
                app.next_tab();
            }
            // 列表移动
            KeyCode::Up | KeyCode::Char('k') => {
                app.move_up();
            }
            KeyCode::Down | KeyCode::Char('j') => {
                app.move_down();
            }
            // 勾选/取消勾选
            KeyCode::Char(' ') => {
                app.toggle_select();
            }
            // 全选
            KeyCode::Char('a') => {
                app.select_all();
            }
            // 取消全选
            KeyCode::Char('n') => {
                app.deselect_all();
            }
            // 删除
            KeyCode::Char('d') | KeyCode::Char('D') => {
                app.prepare_delete();
            }
            // 重新扫描
            KeyCode::Char('r') | KeyCode::Char('R') => {
                app.scan_current();
            }
            // 切换语言
            KeyCode::Char('L') => {
                i18n::toggle_language();
            }
            _ => {}
        }

        if app.should_quit {
            break;
        }
    }

    Ok(())
}
