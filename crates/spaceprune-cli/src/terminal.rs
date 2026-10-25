//! 交互界面共用的终端会话管理。

use std::{
    fmt,
    io::{self, Stderr},
};

use anyhow::{Context, Result};
use crossterm::{
    Command,
    cursor::Show,
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{Terminal, backend::CrosstermBackend};

/// Distinguish shortcut keys from their actions without adding filled keycaps.
pub(crate) fn key_hint<'a>(
    shortcut: &'a str,
    label: &'a str,
    theme: crate::theme::Theme,
) -> Vec<ratatui::text::Span<'a>> {
    use ratatui::{style::Style, text::Span};
    let palette = theme.palette;
    let color = match shortcut {
        "Enter" => palette.success,
        "Esc" => palette.warning,
        _ => theme.module_color(),
    };
    vec![
        Span::raw(" "),
        Span::styled(shortcut, Style::new().fg(color.tui()).bold()),
        Span::raw(" "),
        Span::styled(label, Style::new().fg(theme.body_color().tui())),
        Span::raw("  "),
    ]
}

/// SpacePrune 交互界面使用的 stderr 终端。
pub(crate) type AppTerminal = Terminal<CrosstermBackend<Stderr>>;

/// 在备用屏幕和原始输入模式中运行一个交互界面。
pub(crate) fn with_terminal<T>(
    title: &'static str,
    run: impl FnOnce(&mut AppTerminal) -> Result<T>,
) -> Result<T> {
    enable_raw_mode().context(crate::i18n::text("terminal.raw_mode_failed"))?;
    let mut restore = TerminalRestore {
        alternate_screen: false,
        title: false,
    };
    execute!(io::stderr(), SetScopedTitle(title))
        .context(crate::i18n::text("terminal.title_failed"))?;
    restore.title = true;
    execute!(io::stderr(), EnterAlternateScreen)
        .context(crate::i18n::text("terminal.alternate_failed"))?;
    restore.alternate_screen = true;
    let backend = CrosstermBackend::new(io::stderr());
    let mut terminal = Terminal::new(backend).context(crate::i18n::text("terminal.ui_failed"))?;
    run(&mut terminal)
}

struct TerminalRestore {
    alternate_screen: bool,
    title: bool,
}

impl Drop for TerminalRestore {
    fn drop(&mut self) {
        if self.alternate_screen {
            let _ = execute!(io::stderr(), LeaveAlternateScreen, Show);
        }
        if self.title {
            let _ = execute!(io::stderr(), RestoreScopedTitle);
        }
        let _ = disable_raw_mode();
    }
}

struct SetScopedTitle(&'static str);

impl Command for SetScopedTitle {
    fn write_ansi(&self, formatter: &mut impl fmt::Write) -> fmt::Result {
        write!(formatter, "\x1b[22;2t\x1b]2;{}\x1b\\", self.0)
    }

    #[cfg(windows)]
    fn execute_winapi(&self) -> io::Result<()> {
        Ok(())
    }
}

struct RestoreScopedTitle;

impl Command for RestoreScopedTitle {
    fn write_ansi(&self, formatter: &mut impl fmt::Write) -> fmt::Result {
        formatter.write_str("\x1b[23;2t")
    }

    #[cfg(windows)]
    fn execute_winapi(&self) -> io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scoped_title_uses_save_and_restore_sequences() {
        let mut output = String::new();
        SetScopedTitle("SpacePrune — Interactive cleanup")
            .write_ansi(&mut output)
            .unwrap();
        assert_eq!(
            output,
            "\x1b[22;2t\x1b]2;SpacePrune — Interactive cleanup\x1b\\"
        );

        output.clear();
        RestoreScopedTitle.write_ansi(&mut output).unwrap();
        assert_eq!(output, "\x1b[23;2t");
    }
}
