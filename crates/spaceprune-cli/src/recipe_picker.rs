//! 本地清理方案的交互式选择器。

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
    style::{Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, BorderType, Borders, List, ListItem, Padding, Paragraph, Wrap},
};
use spaceprune::api::RecipeInfo;

use crate::{
    terminal::{AppTerminal, with_terminal},
    theme::Theme,
};

/// 交互选择一个本地清理方案；取消时返回 `None`。
pub(crate) fn pick_recipe(
    recipes: &[RecipeInfo],
    active_recipe: Option<&str>,
    theme: Theme,
) -> Result<Option<String>> {
    if !io::stdin().is_terminal() || !io::stderr().is_terminal() {
        bail!("{}", crate::i18n::text("terminal.recipe_tty"));
    }
    if recipes.is_empty() {
        bail!("{}", crate::i18n::text("terminal.no_recipes"));
    }
    with_terminal(crate::i18n::text("terminal.recipe_title"), |terminal| {
        RecipePicker::new(recipes, active_recipe).run(terminal, theme)
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Decision {
    Continue,
    Accept,
    Cancel,
}

#[derive(Debug)]
struct RecipePicker<'a> {
    recipes: &'a [RecipeInfo],
    active_recipe: Option<String>,
    selected: usize,
}

impl<'a> RecipePicker<'a> {
    fn new(recipes: &'a [RecipeInfo], active_recipe: Option<&str>) -> Self {
        let selected = active_recipe
            .and_then(|active| recipes.iter().position(|recipe| recipe.id == active))
            .unwrap_or_default();
        Self {
            recipes,
            active_recipe: active_recipe.map(str::to_owned),
            selected,
        }
    }

    fn run(&mut self, terminal: &mut AppTerminal, theme: Theme) -> Result<Option<String>> {
        loop {
            terminal.draw(|frame| self.render(frame, theme))?;
            match self.read_event()? {
                Decision::Continue => {}
                Decision::Accept => return Ok(Some(self.selected_recipe().id.clone())),
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
                self.selected = self.recipes.len().saturating_sub(1);
                Decision::Continue
            }
            KeyCode::Char(value) if value.is_ascii_digit() => {
                let index = value.to_digit(10).unwrap() as usize;
                if (1..=self.recipes.len().min(9)).contains(&index) {
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
            .unwrap_or_else(|| self.recipes.len().saturating_sub(1));
    }

    fn next(&mut self) {
        self.selected = (self.selected + 1) % self.recipes.len();
    }

    fn selected_recipe(&self) -> &RecipeInfo {
        &self.recipes[self.selected]
    }

    fn render(&self, frame: &mut Frame, theme: Theme) {
        frame.render_widget(Block::new().style(theme.base_style()), frame.area());
        let areas = Layout::vertical([
            Constraint::Length(3),
            Constraint::Min(10),
            Constraint::Length(3),
        ])
        .split(frame.area());
        self.render_header(frame, areas[0], theme);
        let body = Layout::horizontal([Constraint::Percentage(44), Constraint::Percentage(56)])
            .spacing(1)
            .split(areas[1]);
        self.render_recipe_list(frame, body[0], theme);
        self.render_preview(frame, body[1], theme);
        self.render_footer(frame, areas[2], theme);
    }

    fn render_header(&self, frame: &mut Frame, area: Rect, theme: Theme) {
        let palette = theme.palette;
        let title = Line::from(vec![
            Span::styled(
                crate::i18n::text("terminal.recipe_header"),
                Style::new()
                    .fg(palette.background.tui())
                    .bg(palette.accent.tui())
                    .bold(),
            ),
            Span::raw(crate::i18n::text("terminal.recipe_instruction")),
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

    fn render_recipe_list(&self, frame: &mut Frame, area: Rect, theme: Theme) {
        let palette = theme.palette;
        let capacity = usize::from(area.height.saturating_sub(4)).max(1);
        let start = self
            .selected
            .saturating_add(1)
            .saturating_sub(capacity)
            .min(self.recipes.len().saturating_sub(capacity));
        let items = self
            .recipes
            .iter()
            .enumerate()
            .skip(start)
            .take(capacity)
            .map(|(index, recipe)| {
                let selected = index == self.selected;
                let active = self.active_recipe.as_deref() == Some(recipe.id.as_str());
                let shortcut = if index < 9 {
                    format!("{} ", index + 1)
                } else {
                    "  ".to_owned()
                };
                let line = Line::from(vec![
                    Span::raw(if selected { "▶ " } else { "  " }),
                    Span::styled(shortcut, Style::new().fg(palette.muted.tui())),
                    Span::raw(&recipe.id),
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
                    .title(crate::i18n::text("terminal.recipe_list_title"))
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
        let recipe = self.selected_recipe();
        let active = self.active_recipe.as_deref() == Some(recipe.id.as_str());
        let status = if active {
            crate::i18n::text("terminal.active")
        } else {
            crate::i18n::text("terminal.available")
        };
        let status_color = if active {
            palette.success.tui()
        } else {
            palette.warning.tui()
        };
        let text = Text::from(vec![
            Line::from(Span::styled(
                &recipe.name,
                Style::new().fg(palette.accent.tui()).bold(),
            )),
            Line::from(""),
            Line::from(vec![
                Span::styled("ID      ", Style::new().fg(palette.muted.tui())),
                Span::styled(&recipe.id, Style::new().fg(palette.secondary.tui()).bold()),
            ]),
            Line::from(vec![
                Span::styled(
                    crate::i18n::text("terminal.status_label"),
                    Style::new().fg(palette.muted.tui()),
                ),
                Span::styled(status, Style::new().fg(status_color).bold()),
            ]),
            Line::from(""),
            Line::from(Span::styled(
                crate::i18n::text("terminal.entry_file"),
                Style::new().fg(palette.muted.tui()),
            )),
            Line::from(recipe.path.display().to_string()),
            Line::from(""),
            Line::from(Span::styled(
                crate::i18n::text("terminal.recipe_controls"),
                Style::new().fg(palette.foreground.tui()),
            )),
        ]);
        frame.render_widget(
            Paragraph::new(text)
                .wrap(Wrap { trim: false })
                .style(
                    Style::new()
                        .fg(palette.foreground.tui())
                        .bg(palette.surface.tui()),
                )
                .block(
                    Block::new()
                        .title(crate::i18n::text("terminal.recipe_preview"))
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
        let jump_limit = self.recipes.len().min(9);
        let status = if self.active_recipe.as_deref() == Some(self.selected_recipe().id.as_str()) {
            Span::styled(
                crate::i18n::text("terminal.current_recipe"),
                Style::new().fg(palette.success.tui()).bold(),
            )
        } else {
            Span::styled(
                crate::i18n::text("terminal.recipe_enter"),
                Style::new().fg(palette.warning.tui()),
            )
        };
        let mut keys = vec![key("↑/↓", crate::i18n::text("terminal.move"), theme)];
        if jump_limit > 1 {
            keys.push(key(
                &format!("1-{jump_limit}"),
                crate::i18n::text("terminal.jump"),
                theme,
            ));
        }
        keys.push(key("Enter", crate::i18n::text("terminal.use"), theme));
        keys.push(key("Esc", crate::i18n::text("terminal.cancel"), theme));
        let text = Text::from(vec![
            Line::from(keys.into_iter().flatten().collect::<Vec<_>>()),
            Line::from(vec![Span::raw("  "), status]),
        ]);
        frame.render_widget(Paragraph::new(text), area);
    }
}

fn key(shortcut: &str, label: &str, theme: Theme) -> Vec<Span<'static>> {
    crate::terminal::key_hint(shortcut, label, theme)
        .into_iter()
        .map(|span| Span::styled(span.content.into_owned(), span.style))
        .collect()
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    fn recipes() -> Vec<RecipeInfo> {
        ["default", "personal", "work"]
            .into_iter()
            .map(|id| RecipeInfo {
                id: id.to_owned(),
                name: format!("{id} recipe"),
                path: PathBuf::from(format!("recipes/{id}/{id}.toml")),
            })
            .collect()
    }

    #[test]
    fn starts_at_active_recipe_and_wraps() {
        let recipes = recipes();
        let mut picker = RecipePicker::new(&recipes, Some("personal"));
        assert_eq!(picker.selected_recipe().id, "personal");

        picker.handle_key(KeyEvent::new(KeyCode::Home, KeyModifiers::NONE));
        picker.handle_key(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));
        assert_eq!(picker.selected_recipe().id, "work");

        picker.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        assert_eq!(picker.selected_recipe().id, "default");
    }

    #[test]
    fn number_keys_select_recipes() {
        let recipes = recipes();
        let mut picker = RecipePicker::new(&recipes, None);
        picker.handle_key(KeyEvent::new(KeyCode::Char('2'), KeyModifiers::NONE));
        assert_eq!(picker.selected_recipe().id, "personal");
    }
}
