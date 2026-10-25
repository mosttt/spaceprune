//! 所见即所得的交互式终端主题选择器。

use std::{
    io::{self, IsTerminal},
    time::Duration,
};

use anyhow::{Result, bail};
use crossterm::event::{
    self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseEventKind,
};
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::{Modifier, Style, Stylize},
    text::{Line, Span, Text},
    widgets::{Block, BorderType, Borders, List, ListItem, Padding, Paragraph},
};
use spaceprune::api::THEME_IDS;

use crate::{
    terminal::{AppTerminal, with_terminal},
    theme::Theme,
};

/// 交互预览并选择主题；取消时返回 `None`。
pub(crate) fn pick_theme(active_theme: Theme) -> Result<Option<Theme>> {
    if !io::stdin().is_terminal() || !io::stderr().is_terminal() {
        bail!(
            "{}",
            crate::i18n::message(
                "terminal.theme_tty",
                &[("p0", (THEME_IDS.join(", ")).to_string())]
            )
        );
    }
    with_terminal(crate::i18n::text("terminal.theme_title"), |terminal| {
        ThemePicker::new(active_theme).run(terminal)
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Decision {
    Continue,
    Accept,
    Cancel,
}

#[derive(Debug)]
struct ThemePicker {
    active_theme: Theme,
    selected: usize,
}

impl ThemePicker {
    fn new(active_theme: Theme) -> Self {
        let selected = Theme::all()
            .iter()
            .position(|theme| *theme == active_theme)
            .unwrap_or_default();
        Self {
            active_theme,
            selected,
        }
    }

    fn run(&mut self, terminal: &mut AppTerminal) -> Result<Option<Theme>> {
        loop {
            terminal.draw(|frame| self.render(frame))?;
            match self.read_event()? {
                Decision::Continue => {}
                Decision::Accept => return Ok(Some(self.selected_theme())),
                Decision::Cancel => return Ok(None),
            }
        }
    }

    fn read_event(&mut self) -> Result<Decision> {
        if !event::poll(Duration::from_millis(100))? {
            return Ok(Decision::Continue);
        }
        Ok(match event::read()? {
            Event::Key(key) if key.kind == KeyEventKind::Press => self.handle_key(key),
            Event::Mouse(mouse) => {
                match mouse.kind {
                    MouseEventKind::ScrollUp => self.previous(),
                    MouseEventKind::ScrollDown => self.next(),
                    _ => {}
                }
                Decision::Continue
            }
            _ => Decision::Continue,
        })
    }

    fn handle_key(&mut self, key: KeyEvent) -> Decision {
        if key.modifiers.contains(KeyModifiers::CONTROL)
            && matches!(key.code, KeyCode::Char('c' | 'C'))
        {
            return Decision::Cancel;
        }
        match key.code {
            KeyCode::Up | KeyCode::Left | KeyCode::Char('h' | 'H' | 'k' | 'K') => {
                self.previous();
                Decision::Continue
            }
            KeyCode::Down | KeyCode::Right | KeyCode::Char('j' | 'J' | 'l' | 'L') => {
                self.next();
                Decision::Continue
            }
            KeyCode::Home => {
                self.selected = 0;
                Decision::Continue
            }
            KeyCode::End => {
                self.selected = Theme::all().len().saturating_sub(1);
                Decision::Continue
            }
            KeyCode::Char(value) if value.is_ascii_digit() => {
                let index = value.to_digit(10).unwrap() as usize;
                if (1..=Theme::all().len()).contains(&index) {
                    self.selected = index - 1;
                }
                Decision::Continue
            }
            KeyCode::Enter => Decision::Accept,
            KeyCode::Esc | KeyCode::Char('q' | 'Q') => Decision::Cancel,
            _ => Decision::Continue,
        }
    }

    fn previous(&mut self) {
        self.selected = self
            .selected
            .checked_sub(1)
            .unwrap_or_else(|| Theme::all().len().saturating_sub(1));
    }

    fn next(&mut self) {
        self.selected = (self.selected + 1) % Theme::all().len();
    }

    fn selected_theme(&self) -> Theme {
        Theme::all()[self.selected]
    }

    fn render(&self, frame: &mut Frame) {
        let theme = self.selected_theme();
        frame.render_widget(Block::new().style(theme.base_style()), frame.area());
        let areas = Layout::vertical([
            Constraint::Length(3),
            Constraint::Min(14),
            Constraint::Length(3),
        ])
        .split(frame.area());
        self.render_header(frame, areas[0], theme);
        let body = Layout::horizontal([Constraint::Length(29), Constraint::Min(36)])
            .spacing(1)
            .split(areas[1]);
        self.render_theme_list(frame, body[0], theme);
        self.render_preview(frame, body[1], theme);
        self.render_footer(frame, areas[2], theme);
    }

    fn render_header(&self, frame: &mut Frame, area: Rect, theme: Theme) {
        let palette = theme.palette;
        let title = Line::from(vec![
            Span::styled(
                crate::i18n::text("terminal.theme_header"),
                Style::new()
                    .fg(palette.background.tui())
                    .bg(palette.accent.tui())
                    .bold(),
            ),
            Span::raw("  "),
            Span::styled(
                theme.display_name(),
                Style::new().fg(palette.foreground.tui()).bold(),
            ),
            Span::raw("  "),
            Span::styled(theme.id(), Style::new().fg(palette.muted.tui())),
        ]);
        frame.render_widget(
            Paragraph::new(title).block(
                Block::new()
                    .borders(Borders::BOTTOM)
                    .border_style(palette.muted.tui()),
            ),
            area,
        );
    }

    fn render_theme_list(&self, frame: &mut Frame, area: Rect, preview_theme: Theme) {
        let palette = preview_theme.palette;
        let items = Theme::all()
            .iter()
            .enumerate()
            .map(|(index, theme)| {
                let selected = index == self.selected;
                let active = *theme == self.active_theme;
                let line = Line::from(vec![
                    Span::raw(if selected { "▶ " } else { "  " }),
                    Span::styled(
                        format!("{} ", index + 1),
                        Style::new().fg(palette.muted.tui()),
                    ),
                    Span::raw(theme.display_name()),
                    Span::raw(if active { "  ●" } else { "" }),
                ]);
                let style = if selected {
                    Style::new()
                        .fg(palette.background.tui())
                        .bg(palette.secondary.tui())
                        .add_modifier(Modifier::BOLD)
                } else if active {
                    Style::new().fg(palette.success.tui())
                } else {
                    Style::new().fg(palette.foreground.tui())
                };
                ListItem::new(line).style(style)
            })
            .collect::<Vec<_>>();
        frame.render_widget(
            List::new(items).block(
                Block::new()
                    .title(crate::i18n::text("terminal.theme_list_title"))
                    .borders(Borders::ALL)
                    .border_type(BorderType::Rounded)
                    .border_style(palette.muted.tui())
                    .padding(Padding::vertical(1)),
            ),
            area,
        );
    }

    fn render_preview(&self, frame: &mut Frame, area: Rect, theme: Theme) {
        let palette = theme.palette;
        let text = Text::from(vec![
            Line::from(vec![
                Span::styled(
                    crate::i18n::text("terminal.configuration_preview"),
                    Style::new().fg(palette.accent.tui()).bold(),
                ),
                Span::raw(crate::i18n::text("terminal.personal_preview")),
                Span::styled(
                    crate::i18n::text("terminal.live_preview"),
                    Style::new().fg(palette.muted.tui()),
                ),
            ]),
            Line::from(""),
            Line::from(vec![
                Span::styled(
                    crate::i18n::text("terminal.developer_preview"),
                    Style::new().fg(palette.accent.tui()).bold(),
                ),
                Span::styled("(cache)", Style::new().fg(palette.secondary.tui())),
                Span::raw(crate::i18n::text("terminal.used_preview")),
            ]),
            Line::from(vec![
                Span::styled(
                    crate::i18n::text("terminal.recommended_preview"),
                    Style::new().fg(palette.success.tui()).bold(),
                ),
                Span::raw(crate::i18n::text("terminal.browser_preview")),
            ]),
            Line::from(vec![
                Span::styled(
                    crate::i18n::text("terminal.optional_preview"),
                    Style::new().fg(palette.warning.tui()),
                ),
                Span::raw(crate::i18n::text("terminal.build_preview")),
            ]),
            Line::from(vec![
                Span::styled(
                    crate::i18n::text("terminal.unavailable_preview"),
                    Style::new().fg(palette.muted.tui()),
                ),
                Span::raw(crate::i18n::text("terminal.protected_preview")),
            ]),
            Line::from(vec![
                Span::styled(
                    crate::i18n::text("terminal.error_preview"),
                    Style::new().fg(palette.error.tui()).bold(),
                ),
                Span::raw(crate::i18n::text("terminal.denied_preview")),
            ]),
            Line::from(""),
            Line::from(vec![
                badge(
                    crate::i18n::text("terminal.accent_badge"),
                    palette.background.tui(),
                    palette.accent.tui(),
                ),
                Span::raw(" "),
                badge(
                    crate::i18n::text("terminal.secondary_badge"),
                    palette.background.tui(),
                    palette.secondary.tui(),
                ),
            ]),
            Line::from(vec![
                badge(
                    crate::i18n::text("terminal.success_badge"),
                    palette.background.tui(),
                    palette.success.tui(),
                ),
                Span::raw(" "),
                badge(
                    crate::i18n::text("terminal.warning_badge"),
                    palette.background.tui(),
                    palette.warning.tui(),
                ),
                Span::raw(" "),
                badge(
                    crate::i18n::text("terminal.error_badge"),
                    palette.background.tui(),
                    palette.error.tui(),
                ),
            ]),
            Line::from(""),
            Line::from(crate::i18n::text("terminal.cleanup_preview"))
                .fg(palette.success.tui())
                .bold(),
        ]);
        frame.render_widget(
            Paragraph::new(text)
                .style(
                    Style::new()
                        .fg(palette.foreground.tui())
                        .bg(palette.surface.tui()),
                )
                .block(
                    Block::new()
                        .title(crate::i18n::text("terminal.interface_preview"))
                        .borders(Borders::ALL)
                        .border_type(BorderType::Rounded)
                        .border_style(palette.accent.tui())
                        .padding(Padding::uniform(1)),
                ),
            area,
        );
    }

    fn render_footer(&self, frame: &mut Frame, area: Rect, theme: Theme) {
        let palette = theme.palette;
        let jump_range = format!("1-{}", Theme::all().len());
        let status = if theme == self.active_theme {
            Span::styled(
                crate::i18n::text("terminal.current_theme"),
                Style::new().fg(palette.success.tui()).bold(),
            )
        } else {
            Span::styled(
                crate::i18n::text("terminal.theme_save"),
                Style::new().fg(palette.warning.tui()),
            )
        };
        let text = Text::from(vec![
            Line::from(
                vec![
                    key("←/→", crate::i18n::text("terminal.switch"), theme),
                    key(&jump_range, crate::i18n::text("terminal.jump"), theme),
                    key("Enter", crate::i18n::text("terminal.save"), theme),
                    key("Esc", crate::i18n::text("terminal.cancel"), theme),
                ]
                .into_iter()
                .flatten()
                .collect::<Vec<_>>(),
            ),
            Line::from(vec![Span::raw("  "), status]),
        ]);
        frame.render_widget(Paragraph::new(text), area);
    }
}

fn badge(
    label: &'static str,
    foreground: ratatui::style::Color,
    background: ratatui::style::Color,
) -> Span<'static> {
    Span::styled(
        label,
        Style::new()
            .fg(foreground)
            .bg(background)
            .add_modifier(Modifier::BOLD),
    )
}

fn key(shortcut: &str, label: &str, theme: Theme) -> Vec<Span<'static>> {
    crate::terminal::key_hint(shortcut, label, theme)
        .into_iter()
        .map(|span| Span::styled(span.content.into_owned(), span.style))
        .collect()
}

#[cfg(test)]
mod tests {
    use ratatui::{Terminal, backend::TestBackend};

    use super::*;

    #[test]
    fn navigation_wraps_and_number_keys_jump() {
        let mut picker = ThemePicker::new(Theme::default());
        picker.handle_key(KeyEvent::new(KeyCode::Left, KeyModifiers::NONE));
        assert_eq!(picker.selected_theme().id(), "forest");

        picker.handle_key(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE));
        assert_eq!(picker.selected_theme(), Theme::default());

        picker.handle_key(KeyEvent::new(KeyCode::Char('3'), KeyModifiers::NONE));
        assert_eq!(picker.selected_theme().id(), "graphite");

        picker.handle_key(KeyEvent::new(KeyCode::Char('4'), KeyModifiers::NONE));
        assert_eq!(picker.selected_theme().id(), "forest");

        picker.handle_key(KeyEvent::new(KeyCode::Char('9'), KeyModifiers::NONE));
        assert_eq!(picker.selected_theme().id(), "forest");
    }

    #[test]
    fn preview_repaints_with_the_selected_theme() {
        let active = Theme::default();
        let mut picker = ThemePicker::new(active);
        picker.handle_key(KeyEvent::new(KeyCode::Char('4'), KeyModifiers::NONE));
        let selected = picker.selected_theme();
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();

        terminal.draw(|frame| picker.render(frame)).unwrap();

        let buffer = terminal.backend().buffer();
        assert_eq!(
            buffer.cell((79, 23)).unwrap().bg,
            selected.palette.background.tui()
        );
        assert_eq!(
            buffer.cell((0, 0)).unwrap().bg,
            selected.palette.accent.tui()
        );
    }
}
