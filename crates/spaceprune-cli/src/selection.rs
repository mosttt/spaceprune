//! 基于 Ratatui 的终端树形选择和最终确认界面。

mod details;
mod summary;
mod tree;

use std::{
    collections::{BTreeMap, BTreeSet},
    io::{self, Write},
    sync::mpsc,
    thread,
    time::Duration,
};

use anyhow::Result;
use crossterm::event::{
    self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseEventKind,
};
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::{Style, Stylize},
    text::{Line, Span, Text},
    widgets::{Block, BorderType, Borders, Cell, Clear, Paragraph, Row, Table, TableState, Wrap},
};
use spaceprune::api::{
    CancellationToken, CleanupItem, CleanupPlan, CleanupSizeAccounting, HandlerDescriptor, ItemKey,
    LogicalSizeSummary, Recommendation, ScanReport, SizeApplicability, fs::format_bytes,
};

use crate::{
    terminal::{AppTerminal, with_terminal},
    theme::{Theme, Tone},
};

/// 交互编辑推荐集合并生成最终计划；取消时返回 `None`。
pub(crate) fn select_plan(
    report: &ScanReport,
    cancellation: &CancellationToken,
    theme: Theme,
    descriptors: &[HandlerDescriptor],
) -> Result<Option<CleanupPlan>> {
    with_terminal(crate::i18n::text("terminal.selection_title"), |terminal| {
        SelectionApp::with_accounting(
            report,
            cancellation,
            theme,
            &descriptors
                .iter()
                .map(|descriptor| (descriptor.id.clone(), descriptor.size_accounting))
                .collect(),
        )
        .run(terminal)
    })
}

#[derive(Debug, Clone)]
enum RowKind {
    Module(String),
    Task { module_id: String, task_id: String },
    Item(ItemKey),
}

#[derive(Debug)]
struct ViewRow<'a> {
    kind: RowKind,
    parent: Option<usize>,
    subtree_end: usize,
    label: String,
    logical_bytes: Option<u64>,
    logical_size_applicability: SizeApplicability,
    unknown_logical_items: usize,
    selected: usize,
    selected_logical_bytes: u128,
    selected_unknown_logical: usize,
    selected_not_applicable_logical: usize,
    selectable: usize,
    item: Option<&'a CleanupItem>,
    item_accounting: Option<CleanupSizeAccounting>,
    locations: Vec<String>,
    description: String,
    diagnostics: Vec<String>,
    search_text: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Decision {
    Continue,
    Accept,
    Cancel,
}

#[derive(Debug)]
struct SelectionApp<'a> {
    report: &'a ScanReport,
    cancellation: &'a CancellationToken,
    theme: Theme,
    selected: BTreeSet<ItemKey>,
    selectable: BTreeSet<ItemKey>,
    recommended: BTreeSet<ItemKey>,
    rows: Vec<ViewRow<'a>>,
    visible: Vec<usize>,
    view_dirty: bool,
    estimate: (u128, usize),
    snapshot_groups: usize,
    collapsed_modules: BTreeSet<String>,
    collapsed_tasks: BTreeSet<(String, String)>,
    cursor: usize,
    table_offset: usize,
    search: String,
    searching: bool,
    reveal_search_matches: bool,
    confirming: bool,
    help: bool,
}

impl<'a> SelectionApp<'a> {
    fn with_accounting(
        report: &'a ScanReport,
        cancellation: &'a CancellationToken,
        theme: Theme,
        accounting: &BTreeMap<String, CleanupSizeAccounting>,
    ) -> Self {
        let recommended = report.recommended_keys();
        let selectable = report.selectable_keys();
        let mut app = Self {
            report,
            cancellation,
            theme,
            selected: recommended.clone(),
            selectable,
            recommended,
            rows: Vec::new(),
            visible: Vec::new(),
            view_dirty: true,
            estimate: (0, 0),
            snapshot_groups: 0,
            collapsed_modules: BTreeSet::new(),
            collapsed_tasks: BTreeSet::new(),
            cursor: 0,
            table_offset: 0,
            search: String::new(),
            searching: false,
            reveal_search_matches: true,
            confirming: false,
            help: false,
        };
        app.rows = app.build_rows(accounting);
        app.fold_projects();
        app.refresh_selection(|item| item.recommendation == Recommendation::Recommended);
        app.refresh_visible_rows();
        app
    }

    fn run(&mut self, terminal: &mut AppTerminal) -> Result<Option<CleanupPlan>> {
        loop {
            if self.cancellation.is_cancelled() {
                return Ok(None);
            }
            terminal.draw(|frame| self.render(frame))?;
            match self.read_event()? {
                Decision::Continue => {}
                Decision::Accept => {
                    return self
                        .report
                        .plan_selected(&self.selected)
                        .map(Some)
                        .map_err(Into::into);
                }
                Decision::Cancel => return Ok(None),
            }
        }
    }

    fn read_event(&mut self) -> Result<Decision> {
        while !event::poll(Duration::from_millis(100))? {
            if self.cancellation.is_cancelled() {
                return Ok(Decision::Cancel);
            }
        }
        match event::read()? {
            Event::Key(key) if key.kind == KeyEventKind::Press => self.handle_key(key),
            Event::Mouse(mouse) => {
                match mouse.kind {
                    MouseEventKind::ScrollDown => self.move_cursor(3),
                    MouseEventKind::ScrollUp => self.move_cursor(-3),
                    _ => {}
                }
                Ok(Decision::Continue)
            }
            _ => Ok(Decision::Continue),
        }
    }

    fn handle_key(&mut self, key: KeyEvent) -> Result<Decision> {
        if key.modifiers.contains(KeyModifiers::CONTROL)
            && matches!(key.code, KeyCode::Char('c' | 'C'))
        {
            self.cancellation.cancel();
            return Ok(Decision::Cancel);
        }
        if self.help {
            self.help = false;
            return Ok(Decision::Continue);
        }
        if self.confirming {
            return Ok(match key.code {
                KeyCode::Enter => Decision::Accept,
                KeyCode::Esc | KeyCode::Backspace => {
                    self.confirming = false;
                    Decision::Continue
                }
                KeyCode::Char('q' | 'Q') => Decision::Cancel,
                _ => Decision::Continue,
            });
        }
        if self.searching {
            match key.code {
                KeyCode::Enter => self.searching = false,
                KeyCode::Esc => {
                    self.searching = false;
                    self.search.clear();
                    self.view_dirty = true;
                }
                KeyCode::Backspace => {
                    self.reveal_search_matches = true;
                    self.search.pop();
                    self.cursor = 0;
                    self.view_dirty = true;
                }
                KeyCode::Char(character) => {
                    self.reveal_search_matches = true;
                    self.search.push(character);
                    self.cursor = 0;
                    self.view_dirty = true;
                }
                _ => {}
            }
            return Ok(Decision::Continue);
        }

        match key.code {
            KeyCode::Up | KeyCode::Char('k') => self.move_cursor(-1),
            KeyCode::Down | KeyCode::Char('j') => self.move_cursor(1),
            KeyCode::PageUp => self.move_cursor(-10),
            KeyCode::PageDown => self.move_cursor(10),
            KeyCode::Home => self.cursor = 0,
            KeyCode::End => self.cursor = self.visible.len().saturating_sub(1),
            KeyCode::Char('-') => self.collapse_all(),
            KeyCode::Char('+' | '=') => self.expand_all(),
            KeyCode::Left | KeyCode::Char('h') => self.collapse_current(),
            KeyCode::Right | KeyCode::Char('l') => self.expand_current(),
            KeyCode::Char(' ') => self.toggle_current(),
            KeyCode::Char('a' | 'A') => {
                self.selected.clone_from(&self.selectable);
                self.refresh_selection(|item| item.recommendation != Recommendation::Unavailable);
            }
            KeyCode::Char('r' | 'R') => {
                self.selected.clone_from(&self.recommended);
                self.refresh_selection(|item| item.recommendation == Recommendation::Recommended);
            }
            KeyCode::Char('c' | 'C') => {
                self.selected.clear();
                self.refresh_selection(|_| false);
            }
            KeyCode::Char('u' | 'U') => self.focus_next_unknown(),
            KeyCode::Char('/') => self.searching = true,
            KeyCode::Char('?') => self.help = true,
            KeyCode::Enter => {
                if self.selected.is_empty() {
                    return Ok(Decision::Accept);
                }
                self.confirming = true;
            }
            KeyCode::Esc | KeyCode::Char('q' | 'Q') => return Ok(Decision::Cancel),
            _ => {}
        }
        Ok(Decision::Continue)
    }

    fn move_cursor(&mut self, delta: isize) {
        let last = self.visible.len().saturating_sub(1);
        self.cursor = self.cursor.saturating_add_signed(delta).min(last);
    }

    fn current_kind(&self) -> Option<RowKind> {
        self.visible
            .get(self.cursor)
            .map(|index| self.rows[*index].kind.clone())
    }

    fn toggle_current(&mut self) {
        let Some(&start) = self.visible.get(self.cursor) else {
            return;
        };
        let row = &self.rows[start];
        let remove = row.selected == row.selectable;
        let end = row.subtree_end;
        for index in start..end {
            let row = &self.rows[index];
            if row.selectable == 0 || (row.selected > 0) == !remove {
                continue;
            }
            let RowKind::Item(key) = &row.kind else {
                continue;
            };
            if remove {
                self.selected.remove(key);
            } else {
                self.selected.insert(key.clone());
            }
            let item = row.item.expect("Item rows retain their scan result");
            if row.item_accounting == Some(CleanupSizeAccounting::SnapshotGroups) {
                if remove {
                    self.snapshot_groups -= 1;
                } else {
                    self.snapshot_groups += 1;
                }
            } else {
                match (item.reclaimable_bytes, remove) {
                    (Some(bytes), false) => self.estimate.0 += u128::from(bytes),
                    (Some(bytes), true) => self.estimate.0 -= u128::from(bytes),
                    (None, false) => self.estimate.1 += 1,
                    (None, true) => self.estimate.1 -= 1,
                }
            }
            let mut current = Some(index);
            while let Some(index) = current {
                let row = &mut self.rows[index];
                if remove {
                    row.selected -= 1;
                    row.selected_logical_bytes -=
                        u128::from(item.applicable_logical_bytes().unwrap_or(0));
                    row.selected_unknown_logical -= usize::from(item.has_unknown_logical_size());
                    row.selected_not_applicable_logical -= usize::from(
                        item.logical_size_applicability == SizeApplicability::NotApplicable,
                    );
                } else {
                    row.selected += 1;
                    row.selected_logical_bytes +=
                        u128::from(item.applicable_logical_bytes().unwrap_or(0));
                    row.selected_unknown_logical += usize::from(item.has_unknown_logical_size());
                    row.selected_not_applicable_logical += usize::from(
                        item.logical_size_applicability == SizeApplicability::NotApplicable,
                    );
                }
                current = row.parent;
            }
        }
    }

    fn collapse_current(&mut self) {
        self.reveal_search_matches = false;
        self.view_dirty = true;
        match self.current_kind() {
            Some(RowKind::Module(module)) => {
                self.collapsed_modules.insert(module);
            }
            Some(RowKind::Task { module_id, task_id }) => {
                self.collapsed_tasks.insert((module_id, task_id));
            }
            Some(RowKind::Item(_)) => {
                if let Some(parent) = self.rows[self.visible[self.cursor]].parent {
                    let RowKind::Task { module_id, task_id } = self.rows[parent].kind.clone()
                    else {
                        return;
                    };
                    self.collapsed_tasks
                        .insert((module_id.clone(), task_id.clone()));
                    self.focus_task(&module_id, &task_id);
                }
            }
            None => {}
        }
    }

    fn expand_current(&mut self) {
        self.view_dirty = true;
        match self.current_kind() {
            Some(RowKind::Module(module)) => {
                self.collapsed_modules.remove(&module);
            }
            Some(RowKind::Task { module_id, task_id }) => {
                self.collapsed_tasks.remove(&(module_id, task_id));
            }
            _ => {}
        }
    }

    fn collapse_all(&mut self) {
        self.reveal_search_matches = false;
        self.view_dirty = true;
        self.collapsed_modules = self
            .report
            .modules
            .iter()
            .map(|module| module.id.clone())
            .collect();
        self.fold_projects();
        self.cursor = 0;
    }

    fn expand_all(&mut self) {
        self.collapsed_modules.clear();
        self.collapsed_tasks.clear();
        self.view_dirty = true;
    }

    fn fold_projects(&mut self) {
        self.collapsed_tasks = self
            .rows
            .iter()
            .filter_map(|row| match &row.kind {
                RowKind::Task { module_id, task_id } => Some((module_id.clone(), task_id.clone())),
                _ => None,
            })
            .collect();
    }

    fn focus_task(&mut self, module_id: &str, task_id: &str) {
        if let Some(index) = self.visible.iter().position(|index| {
            matches!(
                &self.rows[*index].kind,
                RowKind::Task { module_id: module, task_id: task }
                    if module == module_id && task == task_id
            )
        }) {
            self.cursor = index;
        }
    }

    fn refresh_selection(&mut self, selected: impl Fn(&CleanupItem) -> bool) {
        self.estimate = (0, 0);
        self.snapshot_groups = 0;
        for row in &mut self.rows {
            row.selected = usize::from(row.item.is_some_and(&selected));
            row.selected_logical_bytes = 0;
            row.selected_unknown_logical = 0;
            row.selected_not_applicable_logical = 0;
            if row.selected > 0 {
                let item = row.item.expect("Only item rows have a selected value here");
                row.selected_logical_bytes =
                    u128::from(item.applicable_logical_bytes().unwrap_or(0));
                row.selected_unknown_logical = usize::from(item.has_unknown_logical_size());
                row.selected_not_applicable_logical = usize::from(
                    item.logical_size_applicability == SizeApplicability::NotApplicable,
                );
                if row.item_accounting == Some(CleanupSizeAccounting::SnapshotGroups) {
                    self.snapshot_groups += 1;
                } else if let Some(bytes) = item.reclaimable_bytes {
                    self.estimate.0 += u128::from(bytes);
                } else {
                    self.estimate.1 += 1;
                }
            }
        }
        for index in (0..self.rows.len()).rev() {
            if let Some(parent) = self.rows[index].parent {
                self.rows[parent].selected += self.rows[index].selected;
                self.rows[parent].selected_logical_bytes += self.rows[index].selected_logical_bytes;
                self.rows[parent].selected_unknown_logical +=
                    self.rows[index].selected_unknown_logical;
                self.rows[parent].selected_not_applicable_logical +=
                    self.rows[index].selected_not_applicable_logical;
            }
        }
    }

    fn render(&mut self, frame: &mut Frame) {
        frame.render_widget(Block::new().style(self.theme.base_style()), frame.area());
        if self.view_dirty {
            self.refresh_visible_rows();
        }
        let detail_height = self
            .visible
            .get(self.cursor)
            .map_or(7, |index| {
                7 + self.rows[*index].locations.len() as u16
                    + self.rows[*index].diagnostics.len().min(4) as u16 * 2
            })
            .min((frame.area().height / 2).max(7));
        let areas = Layout::vertical([
            Constraint::Length(3),
            Constraint::Min(8),
            Constraint::Length(detail_height),
            Constraint::Length(2),
        ])
        .split(frame.area());
        self.render_header(frame, areas[0]);
        self.render_table(frame, areas[1]);
        self.render_details(frame, areas[2]);
        self.render_footer(frame, areas[3]);
        if self.confirming {
            self.render_confirmation(frame);
        } else if self.help {
            self.render_help(frame);
        }
    }

    fn render_header(&self, frame: &mut Frame, area: Rect) {
        let palette = self.theme.palette;
        let title = vec![
            Span::styled(
                " SPACEPRUNE ",
                Style::new()
                    .fg(palette.background.tui())
                    .bg(palette.accent.tui())
                    .bold(),
            ),
            Span::raw("  "),
            Span::styled(
                &self.report.config_name,
                Style::new().fg(palette.foreground.tui()).bold(),
            ),
            Span::raw("    "),
            Span::styled(
                crate::i18n::message(
                    "terminal.selection_count",
                    &[("count", self.selected.len().to_string())],
                ),
                Style::new().fg(palette.success.tui()).bold(),
            ),
        ];
        let summary = self.selected_summary_spans();
        frame.render_widget(
            Paragraph::new(vec![Line::from(title), Line::from(summary)]).block(
                Block::new()
                    .borders(Borders::BOTTOM)
                    .border_style(palette.muted.tui()),
            ),
            area,
        );
    }

    fn render_table(&mut self, frame: &mut Frame, area: Rect) {
        let palette = self.theme.palette;
        // Rows have height one; borders and the header occupy four terminal lines.
        let capacity = usize::from(area.height.saturating_sub(4)).max(1);
        self.table_offset = self
            .table_offset
            .min(self.visible.len().saturating_sub(capacity));
        if self.cursor < self.table_offset {
            self.table_offset = self.cursor;
        } else if self.cursor >= self.table_offset + capacity {
            self.table_offset = self.cursor + 1 - capacity;
        }
        let end = (self.table_offset + capacity).min(self.visible.len());
        let table_rows =
            self.visible[self.table_offset..end]
                .iter()
                .enumerate()
                .map(|(offset, index)| {
                    let row = &self.rows[*index];
                    let background = if self.table_offset + offset == self.cursor {
                        palette.background.mix(self.theme.module_color(), 16)
                    } else if matches!(row.kind, RowKind::Module(_)) {
                        palette.background.mix(self.theme.module_color(), 5)
                    } else {
                        palette.background
                    };
                    let name_style = row_identity_style(row, self.theme);
                    let marker = if matches!(row.kind, RowKind::Item(_)) {
                        Span::styled(
                            item_checkbox(row.selectable > 0, row.selected > 0),
                            Style::new().fg(if row.selected > 0 {
                                palette.success.tui()
                            } else {
                                palette.muted.tui()
                            }),
                        )
                    } else {
                        let arrow = disclosure(
                            self.row_collapsed(row),
                            !self.search.is_empty() && self.reveal_search_matches,
                        );
                        Span::styled(arrow, name_style)
                    };
                    Row::new([
                        Cell::from(Line::from(vec![
                            Span::styled(
                                self.tree_prefix(*index),
                                Style::new().fg(palette.muted.tui()),
                            ),
                            marker,
                            Span::raw(" "),
                            Span::styled(&row.label, name_style),
                        ])),
                        Cell::from(
                            Line::from(format_applicable_logical_size(
                                row.logical_bytes,
                                row.unknown_logical_items,
                                row.logical_size_applicability,
                            ))
                            .right_aligned(),
                        )
                        .style(row_size_style(row, self.theme)),
                        Cell::from(row_status(row, self.theme)),
                    ])
                    .style(Style::new().bg(background.tui()))
                });
        let header = Row::new([
            Cell::from(crate::i18n::text("terminal.cleanup_item"))
                .fg(self.theme.module_color().tui()),
            Cell::from(Line::from(crate::i18n::text("terminal.logical_size")).right_aligned())
                .fg(self.theme.project_color().tui()),
            Cell::from(crate::i18n::text("terminal.status")).fg(palette.foreground.tui()),
        ])
        .bold()
        .bottom_margin(1);
        let table = Table::new(
            table_rows,
            [
                Constraint::Min(24),
                Constraint::Length(12),
                Constraint::Length(26),
            ],
        )
        .header(header)
        .column_spacing(2)
        .block(
            Block::new()
                .title(crate::i18n::text("terminal.cleanup_selection"))
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .border_style(palette.muted.tui()),
        )
        // Row backgrounds mark navigation; highlighting must preserve column colors and badges.
        .row_highlight_style(Style::new())
        .highlight_symbol(Span::styled(
            "▶ ",
            Style::new().fg(palette.accent.tui()).bold(),
        ));
        let mut state = TableState::default().with_selected(self.cursor - self.table_offset);
        frame.render_stateful_widget(table, area, &mut state);
    }

    fn render_footer(&self, frame: &mut Frame, area: Rect) {
        let palette = self.theme.palette;
        let text = if self.searching {
            Text::from(Line::from(vec![
                Span::styled(
                    crate::i18n::text("terminal.search_label"),
                    Style::new().fg(palette.accent.tui()).bold(),
                ),
                Span::raw(&self.search),
                Span::styled("█", Style::new().fg(palette.accent.tui())),
                Span::styled(
                    crate::i18n::text("terminal.search_keys"),
                    Style::new().fg(palette.muted.tui()),
                ),
            ]))
        } else {
            let mut navigation = vec![
                ("↑↓", crate::i18n::text("terminal.move")),
                ("Space", crate::i18n::text("terminal.toggle")),
                ("←→", crate::i18n::text("terminal.fold")),
            ];
            if self.estimate.1 > 0 {
                navigation.push(("U", crate::i18n::text("terminal.locate_unknown")));
            }
            if area.width >= 100 {
                navigation.extend([
                    ("A", crate::i18n::text("terminal.all")),
                    ("C", crate::i18n::text("terminal.clear")),
                    ("R", crate::i18n::text("terminal.recommended_key")),
                ]);
            }
            let mut actions = vec![
                ("Enter", crate::i18n::text("terminal.continue")),
                ("Esc", crate::i18n::text("terminal.cancel")),
                ("?", crate::i18n::text("terminal.help")),
            ];
            if area.width >= 100 {
                actions.extend([
                    ("/", crate::i18n::text("terminal.search")),
                    ("-/+", crate::i18n::text("terminal.fold_all")),
                ]);
            }
            Text::from(vec![
                Line::from(
                    navigation
                        .into_iter()
                        .flat_map(|(key, label)| crate::terminal::key_hint(key, label, self.theme))
                        .collect::<Vec<_>>(),
                ),
                Line::from(
                    actions
                        .into_iter()
                        .flat_map(|(key, label)| crate::terminal::key_hint(key, label, self.theme))
                        .collect::<Vec<_>>(),
                ),
            ])
        };
        frame.render_widget(Paragraph::new(text), area);
    }

    fn render_confirmation(&self, frame: &mut Frame) {
        let palette = self.theme.palette;
        let area = centered(frame.area(), 76, 10);
        frame.render_widget(Clear, area);
        let content = Text::from(vec![
            Line::from(""),
            Line::from(crate::i18n::message(
                "terminal.selection_count",
                &[("count", self.selected.len().to_string())],
            ))
            .centered()
            .bold(),
            Line::from(self.selected_summary_spans()).centered(),
            Line::from(""),
            Line::from(vec![
                Span::styled(
                    crate::i18n::text("terminal.confirm_key"),
                    Style::new()
                        .fg(palette.background.tui())
                        .bg(palette.success.tui())
                        .bold(),
                ),
                Span::raw("    "),
                Span::styled(
                    crate::i18n::text("terminal.return_key"),
                    Style::new()
                        .fg(palette.foreground.tui())
                        .bg(palette.surface.tui()),
                ),
            ])
            .centered(),
        ]);
        frame.render_widget(
            Paragraph::new(content)
                .wrap(Wrap { trim: true })
                .style(
                    Style::new()
                        .fg(palette.foreground.tui())
                        .bg(palette.surface.tui()),
                )
                .block(
                    Block::new()
                        .title(crate::i18n::text("terminal.confirm_cleanup"))
                        .borders(Borders::ALL)
                        .border_type(BorderType::Rounded)
                        .border_style(palette.success.tui()),
                ),
            area,
        );
    }

    fn render_help(&self, frame: &mut Frame) {
        let palette = self.theme.palette;
        let area = centered(frame.area(), 78, 19);
        frame.render_widget(Clear, area);
        let text = Text::from(vec![
            Line::from(crate::i18n::text("terminal.help_move")),
            Line::from(crate::i18n::text("terminal.help_toggle")),
            Line::from(crate::i18n::text("terminal.help_fold")),
            Line::from(crate::i18n::text("terminal.help_fold_all")),
            Line::from(crate::i18n::text("terminal.help_all")),
            Line::from(crate::i18n::text("terminal.help_recommend")),
            Line::from(crate::i18n::text("terminal.help_clear")),
            Line::from(crate::i18n::text("terminal.help_search")),
            Line::from(crate::i18n::text("terminal.help_unknown")),
            Line::from(crate::i18n::text("terminal.help_confirm")),
            Line::from(crate::i18n::text("terminal.help_cancel")),
            Line::from(""),
            Line::from(crate::i18n::text("terminal.help_close"))
                .centered()
                .fg(palette.muted.tui()),
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
                        .title(crate::i18n::text("terminal.keyboard_shortcuts"))
                        .borders(Borders::ALL)
                        .border_type(BorderType::Rounded)
                        .border_style(palette.accent.tui())
                        .padding(ratatui::widgets::Padding::uniform(1)),
                ),
            area,
        );
    }

    fn selected_estimate(&self) -> (u64, usize) {
        (
            self.estimate.0.min(u128::from(u64::MAX)) as u64,
            self.estimate.1,
        )
    }
}

fn filesystem_task_locations(task: &spaceprune::api::TaskReport) -> Vec<String> {
    if task.handler_id != "builtin.filesystem/paths" {
        return Vec::new();
    }
    task.request
        .options
        .get("roots")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|root| root.get("path").and_then(serde_json::Value::as_str))
        .map(|path| {
            std::path::absolute(path).map_or_else(
                |_| path.to_owned(),
                |absolute| absolute.display().to_string(),
            )
        })
        .collect()
}

fn disclosure(collapsed: bool, searching: bool) -> &'static str {
    if searching || !collapsed {
        "▾"
    } else {
        "▸"
    }
}

fn item_checkbox(selectable: bool, selected: bool) -> &'static str {
    if !selectable {
        "[—]"
    } else if selected {
        "[✓]"
    } else {
        "[ ]"
    }
}

fn format_logical_size(bytes: Option<u64>, unknown_items: usize) -> String {
    match (bytes, unknown_items) {
        (None, _) | (Some(0), 1..) => crate::i18n::text("terminal.unknown").to_owned(),
        (Some(bytes), 0) => format_bytes(bytes),
        (Some(bytes), _) => format!("{}+", format_bytes(bytes)),
    }
}

fn format_applicable_logical_size(
    bytes: Option<u64>,
    unknown_items: usize,
    applicability: SizeApplicability,
) -> String {
    if applicability == SizeApplicability::NotApplicable {
        crate::i18n::text("terminal.not_applicable").to_owned()
    } else {
        format_logical_size(bytes, unknown_items)
    }
}

fn format_estimate(known_bytes: u64, unknown_items: usize) -> String {
    match (known_bytes, unknown_items) {
        (known, 0) => crate::i18n::message(
            "terminal.selection_processed",
            &[("size", format_bytes(known))],
        ),
        (0, unknown) => crate::i18n::message(
            "terminal.unknown_count",
            &[("unknown", (unknown).to_string())],
        ),
        (known, unknown) => crate::i18n::message(
            "terminal.estimated_unknown",
            &[
                ("p0", (format_bytes(known)).to_string()),
                ("unknown", (unknown).to_string()),
            ],
        ),
    }
}

fn estimate_spans(known_bytes: u64, unknown_items: usize, theme: Theme) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    if known_bytes > 0 || unknown_items == 0 {
        spans.push(Span::styled(
            format_estimate(known_bytes, 0),
            Style::new().fg(theme.project_color().tui()).bold(),
        ));
    }
    if unknown_items > 0 {
        if known_bytes > 0 {
            spans.push(Span::styled(
                " + ",
                Style::new().fg(theme.palette.muted.tui()),
            ));
        }
        spans.push(Span::styled(
            format_estimate(0, unknown_items),
            Style::new().fg(theme.palette.warning.tui()).bold(),
        ));
    }
    spans
}

fn row_status(row: &ViewRow<'_>, theme: Theme) -> Line<'static> {
    let text = if let Some(item) = row.item {
        match item.recommendation {
            Recommendation::Recommended => crate::i18n::text("terminal.recommended"),
            Recommendation::Optional => crate::i18n::text("terminal.optional"),
            Recommendation::Unavailable => crate::i18n::text("terminal.unavailable"),
        }
    } else if matches!(row.kind, RowKind::Task { .. }) && !row.diagnostics.is_empty() {
        crate::i18n::text("terminal.row_scan_issues")
    } else if row.selectable == 0 {
        crate::i18n::text("terminal.nothing_selectable")
    } else if row.selected == row.selectable {
        crate::i18n::text("terminal.all_selected")
    } else if row.selected == 0 {
        crate::i18n::text("terminal.not_selected")
    } else {
        let identity = row_identity_style(row, theme);
        let not_applicable = row.selected == row.selected_not_applicable_logical;
        let size_style = if not_applicable {
            identity.fg(theme.palette.muted.tui())
        } else if row.selected_unknown_logical > 0 {
            identity.fg(theme.palette.warning.tui())
        } else {
            identity
        };
        return Line::from(vec![
            Span::styled("◐ ", row_selection_style(row, theme)),
            Span::styled(format!("{}/{}", row.selected, row.selectable), identity),
            Span::styled(" · ", Style::new().fg(theme.palette.muted.tui())),
            Span::styled(
                format_applicable_logical_size(
                    Some(row.selected_logical_bytes.min(u128::from(u64::MAX)) as u64),
                    row.selected_unknown_logical,
                    if not_applicable {
                        SizeApplicability::NotApplicable
                    } else {
                        SizeApplicability::Applicable
                    },
                ),
                size_style,
            ),
        ]);
    };
    Line::styled(text, row_status_style(row, theme))
}

// Identity colors stay stable when selection and focus change.
fn row_identity_style(row: &ViewRow<'_>, theme: Theme) -> Style {
    match row.kind {
        RowKind::Module(_) => Style::new().fg(theme.module_color().tui()).bold(),
        RowKind::Task { .. } => Style::new().fg(theme.project_color().tui()),
        RowKind::Item(_) => Style::new().fg(theme.body_color().tui()),
    }
}

fn row_size_style(row: &ViewRow<'_>, theme: Theme) -> Style {
    let style = row_identity_style(row, theme);
    if row.logical_size_applicability == SizeApplicability::NotApplicable {
        style.fg(theme.palette.muted.tui())
    } else if row.logical_bytes.is_none() || row.unknown_logical_items > 0 {
        style.fg(theme.palette.warning.tui())
    } else {
        style
    }
}

fn row_selection_style(row: &ViewRow<'_>, theme: Theme) -> Style {
    let palette = theme.palette;
    if row.selectable == 0 || row.selected == 0 {
        Style::new().fg(palette.muted.tui())
    } else if row.selected == row.selectable {
        Style::new().fg(palette.success.tui()).bold()
    } else {
        Style::new().fg(palette.warning.tui()).bold()
    }
}

fn row_status_style(row: &ViewRow<'_>, theme: Theme) -> Style {
    let palette = theme.palette;
    if let Some(item) = row.item {
        return Style::new().fg(match item.recommendation {
            Recommendation::Recommended => palette.success.tui(),
            Recommendation::Optional => palette.warning.tui(),
            Recommendation::Unavailable => palette.muted.tui(),
        });
    }
    if matches!(row.kind, RowKind::Task { .. }) && !row.diagnostics.is_empty() {
        Style::new().fg(palette.warning.tui()).bold()
    } else {
        row_selection_style(row, theme)
    }
}

fn task_diagnostics(task: &spaceprune::api::TaskReport) -> Vec<String> {
    task.error
        .iter()
        .cloned()
        .chain(task.issues.iter().map(|issue| match &issue.location {
            Some(location) => format!("{}: {}", location, issue.message),
            None => issue.message.clone(),
        }))
        .collect()
}

fn centered(area: Rect, width: u16, height: u16) -> Rect {
    let width = width.min(area.width.saturating_sub(2));
    let height = height.min(area.height.saturating_sub(2));
    Rect {
        x: area.x + area.width.saturating_sub(width) / 2,
        y: area.y + area.height.saturating_sub(height) / 2,
        width,
        height,
    }
}

/// 读取一次简洁的是/否确认。
pub(crate) fn confirm(
    message: &str,
    cancellation: &CancellationToken,
    theme: Theme,
) -> Result<bool> {
    eprint!(
        "{} {} ",
        theme.stderr(message, Tone::Accent),
        theme.stderr(crate::i18n::text("terminal.yes_no"), Tone::Muted)
    );
    io::stderr().flush()?;
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        let mut input = String::new();
        let result = io::stdin().read_line(&mut input).map(|_| input);
        let _ = sender.send(result);
    });
    let input = loop {
        if cancellation.is_cancelled() {
            return Ok(false);
        }
        match receiver.recv_timeout(Duration::from_millis(100)) {
            Ok(result) => break result?,
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                return Err(anyhow::anyhow!(
                    "{}",
                    crate::i18n::text("terminal.input_stopped")
                ));
            }
        }
    };
    Ok(matches!(
        input.trim().to_ascii_lowercase().as_str(),
        "y" | "yes"
    ))
}

#[cfg(test)]
mod tests {
    use ratatui::{Terminal, backend::TestBackend, style::Modifier};
    use spaceprune::api::{ModuleReport, ScanReportData, TaskReport};

    use super::*;

    fn report_with_items(count: usize) -> ScanReport {
        ScanReport::from_report(ScanReportData {
            scan_id: ScanReport::new_scan_id(),
            config_name: "Selection fixture".to_owned(),
            config_source: "test".to_owned(),
            cancelled: false,
            stop_reason: None,
            modules: vec![ModuleReport {
                id: "cache".to_owned(),
                name: "Application cache".to_owned(),
                description: "Synthetic scan results".to_owned(),
                tasks: vec![TaskReport {
                    module_id: "cache".to_owned(),
                    task_id: "files".to_owned(),
                    name: "Cached files".to_owned(),
                    handler_id: "test/cache".to_owned(),
                    request: serde_json::from_value(serde_json::json!({
                        "module_id": "cache", "task_id": "files", "name": "Cached files",
                        "handler_id": "test/cache", "options": {}, "recommend": {}, "tags": [],
                    }))
                    .unwrap(),
                    items: (0..count)
                        .map(|index| CleanupItem {
                            logical_size_applicability: Default::default(),
                            id: format!("item-{index}"),
                            parent_id: None,
                            name: format!("file-{index:05}"),
                            location: format!("/fixture/cache/file-{index:05}"),
                            logical_bytes: Some((index as u64 + 1) * 10),
                            reclaimable_bytes: (index != 1).then_some((index as u64 + 1) * 10),
                            modified_at: None,
                            recommendation: if index == 0 {
                                Recommendation::Recommended
                            } else if index == count - 1 {
                                Recommendation::Unavailable
                            } else {
                                Recommendation::Optional
                            },
                            reason: "Fixture item".to_owned(),
                            tags: Vec::new(),
                            opaque: serde_json::json!({}),
                        })
                        .collect(),
                    issues: Vec::new(),
                    error: None,
                }],
            }],
        })
    }

    impl<'a> SelectionApp<'a> {
        fn new(report: &'a ScanReport, cancellation: &'a CancellationToken, theme: Theme) -> Self {
            let accounting = report
                .modules
                .iter()
                .flat_map(|module| &module.tasks)
                .map(|task| {
                    (
                        task.handler_id.clone(),
                        CleanupSizeAccounting::ProcessedBytes,
                    )
                })
                .collect();
            Self::with_accounting(report, cancellation, theme, &accounting)
        }
    }

    fn press(app: &mut SelectionApp<'_>, code: KeyCode) {
        app.handle_key(KeyEvent::new(code, KeyModifiers::NONE))
            .unwrap();
        if app.view_dirty {
            app.refresh_visible_rows();
        }
    }

    fn screen_text(terminal: &Terminal<TestBackend>) -> String {
        terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect()
    }

    #[test]
    fn cached_selection_keeps_item_group_and_global_totals_in_sync() {
        let report = report_with_items(4);
        let cancellation = CancellationToken::default();
        let mut app = SelectionApp::new(&report, &cancellation, Theme::default());
        press(&mut app, KeyCode::Char('+'));
        assert_eq!(app.selected_estimate(), (10, 0));
        assert_eq!(
            row_status(&app.rows[0], app.theme).to_string(),
            "◐ 1/3 · 10 B"
        );

        press(&mut app, KeyCode::Char(' '));
        assert_eq!(app.selected.len(), 3);
        assert_eq!(app.selected_estimate(), (40, 1));
        assert_eq!(app.rows[0].selected, 3);
        assert_eq!(app.rows[1].selected, 3);

        press(&mut app, KeyCode::End);
        press(&mut app, KeyCode::Char(' '));
        assert_eq!(
            app.selected.len(),
            3,
            "Unavailable items cannot be selected"
        );
        press(&mut app, KeyCode::Up);
        press(&mut app, KeyCode::Char(' '));
        assert_eq!(app.selected_estimate(), (10, 1));
        assert_eq!(app.rows[0].selected, 2);
        assert_eq!(app.rows[1].selected, 2);
        assert_eq!(
            row_status(&app.rows[0], app.theme).to_string(),
            "◐ 2/3 · 30 B"
        );

        press(&mut app, KeyCode::Home);
        press(&mut app, KeyCode::Char(' '));
        assert_eq!(app.selected_estimate(), (40, 1));
        press(&mut app, KeyCode::Char(' '));
        assert!(app.selected.is_empty());
        assert_eq!(app.selected_estimate(), (0, 0));
        press(&mut app, KeyCode::Char('R'));
        assert_eq!(app.selected, report.recommended_keys());
        assert_eq!(app.selected_estimate(), (10, 0));
        press(&mut app, KeyCode::Char('A'));
        assert_eq!(app.selected, report.selectable_keys());
        assert_eq!(app.selected_estimate(), (40, 1));
        press(&mut app, KeyCode::Char('C'));
        assert_eq!(app.rows[0].selected, 0);
        assert_eq!(app.rows[1].selected, 0);
        assert_eq!(app.selected_estimate(), (0, 0));
    }

    #[test]
    fn group_toggles_stay_within_their_task_and_module() {
        let mut report = report_with_items(4);
        let mut other = report.modules[0].clone();
        other.id = "other".to_owned();
        other.tasks[0].module_id = other.id.clone();
        other.tasks[0].request.module_id = other.id.clone();
        let mut logs = report.modules[0].tasks[0].clone();
        logs.task_id = "logs".to_owned();
        logs.request.task_id = logs.task_id.clone();
        report.modules[0].tasks.push(logs);
        report.modules.push(other);
        let cancellation = CancellationToken::default();
        let mut app = SelectionApp::new(&report, &cancellation, Theme::default());
        press(&mut app, KeyCode::Char('+'));
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Char(' '));
        assert_eq!(app.selected.len(), 5);
        assert_eq!(app.selected_estimate(), (60, 1));
        press(&mut app, KeyCode::Home);
        press(&mut app, KeyCode::Char(' '));
        assert_eq!(
            app.selected
                .iter()
                .filter(|key| key.module_id == "cache")
                .count(),
            6
        );
        assert_eq!(
            app.selected
                .iter()
                .filter(|key| key.module_id == "other")
                .count(),
            1
        );
        assert_eq!(app.selected_estimate(), (90, 2));
        press(&mut app, KeyCode::Char(' '));
        assert_eq!(app.selected.len(), 1);
        assert_eq!(app.selected.first().unwrap().module_id, "other");
        assert_eq!(app.selected_estimate(), (10, 0));
    }

    #[test]
    fn deselection_keeps_the_same_saturated_size_as_a_fresh_sum() {
        let mut report = report_with_items(3);
        report.modules[0].tasks[0].items[0].reclaimable_bytes = Some(u64::MAX);
        report.modules[0].tasks[0].items[1].reclaimable_bytes = Some(10);
        let cancellation = CancellationToken::default();
        let mut app = SelectionApp::new(&report, &cancellation, Theme::default());
        press(&mut app, KeyCode::Char('+'));
        press(&mut app, KeyCode::Char(' '));
        assert_eq!(app.selected_estimate(), (u64::MAX, 0));
        press(&mut app, KeyCode::End);
        press(&mut app, KeyCode::Up);
        press(&mut app, KeyCode::Char(' '));
        assert_eq!(app.selected_estimate(), (u64::MAX, 0));
    }

    #[test]
    fn large_report_navigation_search_and_folding_keep_the_focused_row_visible() {
        let report = report_with_items(20_000);
        let cancellation = CancellationToken::default();
        let mut app = SelectionApp::new(&report, &cancellation, Theme::default());
        press(&mut app, KeyCode::Char('+'));
        let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
        terminal.draw(|frame| app.render(frame)).unwrap();
        press(&mut app, KeyCode::End);
        terminal.draw(|frame| app.render(frame)).unwrap();
        assert!(screen_text(&terminal).contains("/fixture/cache/file-19999"));
        for _ in 0..20 {
            press(&mut app, KeyCode::Up);
            terminal.draw(|frame| app.render(frame)).unwrap();
            assert!(screen_text(&terminal).contains("/fixture/cache/file-19998"));
            press(&mut app, KeyCode::Down);
            terminal.draw(|frame| app.render(frame)).unwrap();
        }
        press(&mut app, KeyCode::Left);
        terminal.draw(|frame| app.render(frame)).unwrap();
        assert_eq!(app.visible.len(), 2);
        assert!(screen_text(&terminal).contains("Handler: test/cache"));

        press(&mut app, KeyCode::Char('/'));
        for character in "FILE-19999".chars() {
            press(&mut app, KeyCode::Char(character));
            terminal.draw(|frame| app.render(frame)).unwrap();
        }
        assert_eq!(
            app.visible.len(),
            3,
            "Search includes matching leaves and their ancestors"
        );
        press(&mut app, KeyCode::Enter);
        press(&mut app, KeyCode::End);
        terminal.draw(|frame| app.render(frame)).unwrap();
        assert!(screen_text(&terminal).contains("/fixture/cache/file-19999"));

        press(&mut app, KeyCode::Char('/'));
        press(&mut app, KeyCode::Esc);
        terminal.draw(|frame| app.render(frame)).unwrap();
        assert_eq!(
            app.visible.len(),
            2,
            "Clearing search restores the folded task"
        );
        press(&mut app, KeyCode::Right);
        terminal.draw(|frame| app.render(frame)).unwrap();
        assert_eq!(app.visible.len(), 20_002);
        press(&mut app, KeyCode::End);
        terminal.backend_mut().resize(80, 24);
        terminal.resize(Rect::new(0, 0, 80, 24)).unwrap();
        terminal.draw(|frame| app.render(frame)).unwrap();
        assert!(screen_text(&terminal).contains("/fixture/cache/file-19999"));
    }

    #[test]
    fn scan_initially_expands_modules_and_folds_project_contents() {
        let report = report_with_items(3);
        let cancellation = CancellationToken::default();
        let mut app = SelectionApp::new(&report, &cancellation, Theme::default());
        assert_eq!(app.visible.len(), 2);
        assert!(matches!(app.current_kind(), Some(RowKind::Module(_))));
        press(&mut app, KeyCode::Down);
        assert!(matches!(app.current_kind(), Some(RowKind::Task { .. })));
        press(&mut app, KeyCode::Right);
        assert_eq!(app.visible.len(), 5);
        press(&mut app, KeyCode::Char('/'));
        for c in "file".chars() {
            press(&mut app, KeyCode::Char(c));
        }
        press(&mut app, KeyCode::Enter);
        press(&mut app, KeyCode::Char('-'));
        assert_eq!(app.visible.len(), 1);
        press(&mut app, KeyCode::Char('+'));
        assert_eq!(app.visible.len(), 5);
    }

    #[test]
    fn fold_all_expands_nested_tasks_and_preserves_selection() {
        let mut report = report_with_items(3);
        let mut other = report.modules[0].clone();
        other.id = "other".to_owned();
        report.modules.push(other);
        let cancellation = CancellationToken::default();
        let mut app = SelectionApp::new(&report, &cancellation, Theme::default());
        press(&mut app, KeyCode::Char('+'));
        let selected = app.selected.clone();
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Left);
        assert_eq!(app.visible.len(), 7);
        press(&mut app, KeyCode::Char('-'));
        assert_eq!(app.visible.len(), 2);
        press(&mut app, KeyCode::Right);
        assert_eq!(
            app.visible.len(),
            3,
            "Collapse all also folds task children"
        );
        press(&mut app, KeyCode::Char('+'));
        assert_eq!(app.visible.len(), 10, "Expand all opens every task");
        press(&mut app, KeyCode::Char('-'));
        press(&mut app, KeyCode::Char('='));
        assert_eq!(app.visible.len(), 10);
        assert_eq!(app.selected, selected);
    }

    #[test]
    fn empty_failed_tasks_remain_visible_with_their_diagnostics() {
        let mut report = report_with_items(0);
        report.modules[0].tasks[0].error =
            Some("Snapshot identity changed; inspect the saved rule".to_owned());
        let cancellation = CancellationToken::default();
        let mut app = SelectionApp::new(&report, &cancellation, Theme::default());
        press(&mut app, KeyCode::Char('+'));
        assert_eq!(app.visible.len(), 2);
        let mut terminal = Terminal::new(TestBackend::new(120, 30)).unwrap();
        press(&mut app, KeyCode::Down);
        terminal.draw(|frame| app.render(frame)).unwrap();
        let text = screen_text(&terminal);
        assert!(text.contains("scan issues"));
        assert!(text.contains("Snapshot identity changed"));
        press(&mut app, KeyCode::Char(' '));
        assert!(app.selected.is_empty());
    }

    #[test]
    fn render_uses_the_active_theme_palette() {
        let report = ScanReport::from_report(ScanReportData {
            scan_id: ScanReport::new_scan_id(),
            config_name: "test".to_owned(),
            config_source: "test".to_owned(),
            modules: Vec::new(),
            cancelled: false,
            stop_reason: None,
        });
        let cancellation = CancellationToken::default();
        let theme = Theme::named("orbit").unwrap();
        let mut app = SelectionApp::new(&report, &cancellation, theme);
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();

        terminal.draw(|frame| app.render(frame)).unwrap();

        let buffer = terminal.backend().buffer();
        assert_eq!(buffer.cell((0, 0)).unwrap().bg, theme.palette.accent.tui());
        assert_eq!(
            buffer.cell((79, 23)).unwrap().bg,
            theme.palette.background.tui()
        );
    }

    #[test]
    fn focus_preserves_column_styles_and_selection_states_in_every_theme() {
        let report = report_with_items(4);
        let cancellation = CancellationToken::default();
        for theme in Theme::all() {
            let palette = theme.palette;
            let mut app = SelectionApp::new(&report, &cancellation, *theme);
            app.expand_all();
            let mut terminal = Terminal::new(TestBackend::new(110, 30)).unwrap();
            for (key, status, color, bold) in [
                ('R', "◐ 1/3 · 10 B", palette.warning, true),
                ('A', "✓ all selected", palette.success, true),
                ('C', "○ not selected", palette.muted, false),
            ] {
                press(&mut app, KeyCode::Char(key));
                for cursor in [0, 1, 2] {
                    app.cursor = cursor;
                    terminal.draw(|frame| app.render(frame)).unwrap();
                    let buffer = terminal.backend().buffer();
                    let row = buffer
                        .content
                        .chunks(110)
                        .find(|cells| {
                            cells
                                .iter()
                                .map(|cell| cell.symbol())
                                .collect::<String>()
                                .contains("Application cache")
                        })
                        .unwrap();
                    let text = row.iter().map(|cell| cell.symbol()).collect::<String>();
                    for (label, foreground, bold) in [
                        ("Application cache", theme.module_color(), true),
                        ("100 B", theme.module_color(), true),
                        (status, color, bold),
                    ] {
                        let x = text[..text.find(label).unwrap()].chars().count();
                        let cell = &row[x];
                        assert_eq!(cell.fg, foreground.tui(), "{}: {label}", theme.id());
                        assert_eq!(cell.modifier.contains(Modifier::BOLD), bold);
                        assert_eq!(
                            cell.bg,
                            if cursor == 0 {
                                palette.background.mix(theme.module_color(), 16)
                            } else {
                                palette.background.mix(theme.module_color(), 5)
                            }
                            .tui()
                        );
                        assert!(
                            text_contrast(cell.fg, cell.bg) >= 4.5,
                            "{}: {label} must remain readable when focused",
                            theme.id()
                        );
                    }
                    if key == 'R' {
                        for label in ["1/3", "10 B"] {
                            let x = text[..text.rfind(label).unwrap()].chars().count();
                            assert_eq!(row[x].fg, theme.module_color().tui());
                        }
                    }
                    for (name, size, identity) in [
                        ("Cached files", "100 B", theme.project_color()),
                        ("file-00000", "10 B", theme.body_color()),
                        ("file-00003", "40 B", theme.body_color()),
                    ] {
                        let row = buffer
                            .content
                            .chunks(110)
                            .find(|cells| {
                                cells
                                    .iter()
                                    .map(|cell| cell.symbol())
                                    .collect::<String>()
                                    .contains(name)
                            })
                            .unwrap();
                        let text = row.iter().map(|cell| cell.symbol()).collect::<String>();
                        for label in [name, size] {
                            let x = text[..text.find(label).unwrap()].chars().count();
                            assert_eq!(row[x].fg, identity.tui(), "{}: {label}", theme.id());
                            assert!(text_contrast(row[x].fg, row[x].bg) >= 4.5);
                        }
                    }
                    if key == 'R' {
                        let project = buffer
                            .content
                            .chunks(110)
                            .find(|cells| {
                                cells
                                    .iter()
                                    .map(|cell| cell.symbol())
                                    .collect::<String>()
                                    .contains("Cached files")
                            })
                            .unwrap();
                        let text = project.iter().map(|cell| cell.symbol()).collect::<String>();
                        for label in ["1/3", "10 B"] {
                            let x = text[..text.rfind(label).unwrap()].chars().count();
                            assert_eq!(project[x].fg, theme.project_color().tui());
                            assert!(!project[x].modifier.contains(Modifier::BOLD));
                        }
                        let marker = project.iter().find(|cell| cell.symbol() == "◐").unwrap();
                        assert_eq!(marker.fg, palette.warning.tui());
                    }
                    if cursor == 2 {
                        let location = buffer
                            .content
                            .chunks(110)
                            .find(|cells| {
                                cells
                                    .iter()
                                    .map(|cell| cell.symbol())
                                    .collect::<String>()
                                    .contains("/fixture/cache/file-00000")
                            })
                            .unwrap();
                        let text = location
                            .iter()
                            .map(|cell| cell.symbol())
                            .collect::<String>();
                        let x = text.find("/fixture/cache/file-00000").unwrap();
                        assert_eq!(location[x].fg, theme.body_color().tui());
                    }
                    let arrow = row.iter().find(|cell| cell.symbol() == "▾").unwrap();
                    assert_eq!(arrow.bg, row[10].bg);
                    assert_eq!(arrow.fg, theme.module_color().tui());
                    let size_end = text[..text.find("100 B").unwrap()].chars().count() + 5;
                    let header = buffer
                        .content
                        .chunks(110)
                        .map(|cells| cells.iter().map(|cell| cell.symbol()).collect::<String>())
                        .find(|text| text.contains("Logical size"))
                        .unwrap();
                    let header_end = header[..header.find("Logical size").unwrap()]
                        .chars()
                        .count()
                        + 12;
                    assert_eq!(
                        size_end, header_end,
                        "Sizes align with their column's right edge"
                    );
                }
            }
        }
    }

    fn text_contrast(foreground: ratatui::style::Color, background: ratatui::style::Color) -> f64 {
        let luminance = |color| {
            let ratatui::style::Color::Rgb(r, g, b) = color else {
                panic!("Expected an RGB theme color")
            };
            [r, g, b]
                .into_iter()
                .zip([0.2126, 0.7152, 0.0722])
                .map(|(channel, weight)| {
                    let channel = f64::from(channel) / 255.0;
                    weight
                        * if channel <= 0.04045 {
                            channel / 12.92
                        } else {
                            ((channel + 0.055) / 1.055).powf(2.4)
                        }
                })
                .sum::<f64>()
        };
        let foreground = luminance(foreground);
        let background = luminance(background);
        (foreground.max(background) + 0.05) / (foreground.min(background) + 0.05)
    }

    #[test]
    fn focused_filesystem_group_displays_its_roots_without_changing_selection() {
        let roots = if cfg!(windows) {
            [
                r"C:\fixture\project-a\target",
                r"C:\fixture\project-b\target",
            ]
        } else {
            ["/fixture/project-a/target", "/fixture/project-b/target"]
        };
        let mut report = report_with_items(3);
        let task = &mut report.modules[0].tasks[0];
        task.name = "target".to_owned();
        task.handler_id = "builtin.filesystem/paths".to_owned();
        task.request.options = serde_json::json!({"roots": [
            {"path": roots[0]},
            {"path": roots[1]},
        ]});
        let cancellation = CancellationToken::default();
        let mut app = SelectionApp::new(&report, &cancellation, Theme::default());
        press(&mut app, KeyCode::Char('+'));
        let selected = app.selected.clone();
        press(&mut app, KeyCode::Down);
        let mut terminal = Terminal::new(TestBackend::new(110, 32)).unwrap();
        terminal.draw(|frame| app.render(frame)).unwrap();
        let text = screen_text(&terminal);
        assert!(text.contains(roots[0]));
        assert!(text.contains(roots[1]));
        assert!(text.contains("Handler: builtin.filesystem/paths"));
        assert!(text.contains("Selected 1 of 2 selectable items"));
        assert_eq!(app.selected, selected);
    }

    #[test]
    fn chinese_space_toggles_keep_mixed_and_unknown_estimates_renderable() {
        const CHILD: &str = "SPACEPRUNE_TEST_CHINESE_SELECTION";
        if std::env::var_os(CHILD).is_none() {
            let output = std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "selection::tests::chinese_space_toggles_keep_mixed_and_unknown_estimates_renderable"])
                .env(CHILD, "1").output().unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stdout)
            );
            return;
        }
        crate::i18n::set_language(spaceprune::api::Language::SimplifiedChinese);
        assert_eq!(format_estimate(0, 2), "2 项未知");
        assert_eq!(format_estimate(10, 2), "预计处理 10 B + 2 项未知");
        let report = report_with_items(4);
        let cancellation = CancellationToken::default();
        let mut app = SelectionApp::new(&report, &cancellation, Theme::default());
        let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
        press(&mut app, KeyCode::Char('+'));
        // A module, task and leaf toggle each recompute the same mixed-size summary.
        for code in [
            KeyCode::Char(' '),
            KeyCode::Char(' '),
            KeyCode::Down,
            KeyCode::Char(' '),
            KeyCode::Down,
            KeyCode::Char(' '),
        ] {
            press(&mut app, code);
            terminal.draw(|frame| app.render(frame)).unwrap();
        }
        assert!(
            screen_text(&terminal)
                .split_whitespace()
                .collect::<String>()
                .contains("项未知")
        );
        assert_eq!(app.selected_estimate(), (30, 1));
        assert_eq!(
            app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE))
                .unwrap(),
            Decision::Cancel
        );
    }

    #[test]
    fn selection_estimate_exposes_unknown_reclaimable_items() {
        assert_eq!(
            format_estimate(636 * 1024 * 1024, 0),
            "Processed estimate 636 MiB"
        );
        assert_eq!(
            format_estimate(636 * 1024 * 1024, 272),
            "Processed estimate 636 MiB + 272 unknown"
        );
        assert_eq!(format_estimate(0, 272), "272 unknown");
    }

    #[test]
    fn not_applicable_rows_use_the_public_contract_and_do_not_enter_selection() {
        let mut report = report_with_items(1);
        let item = &mut report.modules[0].tasks[0].items[0];
        item.logical_bytes = None;
        item.reclaimable_bytes = None;
        item.logical_size_applicability = SizeApplicability::NotApplicable;
        item.recommendation = Recommendation::Unavailable;
        // No handler ID, item ID or private opaque field is needed for this projection.
        let cancellation = CancellationToken::default();
        let mut app = SelectionApp::new(&report, &cancellation, Theme::default());
        press(&mut app, KeyCode::Char('+'));
        assert!(
            app.rows
                .iter()
                .all(|row| row.logical_size_applicability == SizeApplicability::NotApplicable)
        );
        assert!(app.rows.iter().all(|row| row.unknown_logical_items == 0));
        let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
        for cursor in 0..app.rows.len() {
            app.cursor = cursor;
            press(&mut app, KeyCode::Char(' '));
            terminal.draw(|frame| app.render(frame)).unwrap();
            let text = screen_text(&terminal);
            assert!(text.contains("N/A"));
            assert!(!text.contains("unknown"));
            assert!(app.selected.is_empty());
        }
    }

    #[test]
    fn snapshot_logical_sizes_do_not_become_reclaimable_estimates_or_false_zero_totals() {
        let mut report = report_with_items(4);
        let items = &mut report.modules[0].tasks[0].items;
        for item in items.iter_mut() {
            item.logical_bytes = None;
            item.reclaimable_bytes = None;
            item.tags.clear();
        }
        items[0].logical_bytes = Some(4 * 1_048_576);
        let cancellation = CancellationToken::default();
        let mut app = SelectionApp::with_accounting(
            &report,
            &cancellation,
            Theme::default(),
            &BTreeMap::from([("test/cache".into(), CleanupSizeAccounting::SnapshotGroups)]),
        );
        press(&mut app, KeyCode::Char('+'));
        assert_eq!(app.selected_estimate(), (0, 0));
        assert_eq!(app.snapshot_groups, 1);
        let summary = app
            .selected_summary_spans()
            .into_iter()
            .map(|span| span.content.into_owned())
            .collect::<String>();
        assert_eq!(summary, "Snapshot groups 1");
        assert_eq!(
            format_logical_size(app.rows[0].logical_bytes, app.rows[0].unknown_logical_items),
            "4.00 MiB+"
        );
        let mut terminal = Terminal::new(TestBackend::new(110, 32)).unwrap();
        app.cursor = 2;
        terminal.draw(|frame| app.render(frame)).unwrap();
        let text = screen_text(&terminal);
        assert!(text.contains("Logical size"));
        assert!(text.contains("4.00 MiB+"));
        assert!(text.contains("Snapshot data can be shared"));
        assert!(text.contains("reclaimable space unknown"));
        assert_eq!(format_logical_size(Some(0), 2), "unknown");
        assert_eq!(format_logical_size(Some(0), 0), "0 B");
        // Select only an unknown-size item while the module also has known-size siblings.
        press(&mut app, KeyCode::Char(' '));
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Char(' '));
        terminal.draw(|frame| app.render(frame)).unwrap();
        assert!(screen_text(&terminal).contains("◐ 1/3 · unknown"));
        press(&mut app, KeyCode::Up);
        press(&mut app, KeyCode::Char(' '));
        terminal.draw(|frame| app.render(frame)).unwrap();
        assert!(screen_text(&terminal).contains("◐ 2/3 · 4.00 MiB+"));
    }
    #[test]
    fn mixed_accounting_separates_snapshots_and_locates_only_selected_unknown_content() {
        let mut report = report_with_items(4);
        let mut snapshots = report.modules[0].tasks[0].clone();
        snapshots.task_id = "snapshots".into();
        snapshots.handler_id = "third-party.history/groups".into();
        snapshots.request.task_id = snapshots.task_id.clone();
        snapshots.request.handler_id = snapshots.handler_id.clone();
        snapshots.name = "Historical groups".into();
        snapshots.items.truncate(1);
        snapshots.items[0].logical_bytes = Some(131 * 1024 * 1024 * 1024);
        snapshots.items[0].reclaimable_bytes = None;
        snapshots.items[0].tags.clear();
        report.modules[0].tasks.push(snapshots);
        let cancellation = CancellationToken::default();
        let mut app = SelectionApp::with_accounting(
            &report,
            &cancellation,
            Theme::default(),
            &BTreeMap::from([
                ("test/cache".into(), CleanupSizeAccounting::ProcessedBytes),
                (
                    "third-party.history/groups".into(),
                    CleanupSizeAccounting::SnapshotGroups,
                ),
            ]),
        );
        assert_eq!(app.selected_estimate(), (10, 0));
        assert_eq!(app.snapshot_groups, 1);
        let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
        terminal.draw(|frame| app.render(frame)).unwrap();
        let text = screen_text(&terminal);
        assert!(text.contains("Processed estimate 10 B"));
        assert!(text.contains("Snapshot groups 1"));
        assert!(!text.contains("1 unknown"));
        press(&mut app, KeyCode::Char('A'));
        assert_eq!(app.selected_estimate(), (40, 1));
        assert_eq!(app.snapshot_groups, 1);
        let selection = app.selected.clone();
        press(&mut app, KeyCode::Char('-'));
        app.search = "Historical".into();
        app.refresh_visible_rows();
        press(&mut app, KeyCode::Char('U'));
        assert!(app.search.is_empty());
        let focused = &app.rows[app.visible[app.cursor]];
        assert_eq!(focused.item.unwrap().id, "item-1");
        assert_eq!(
            focused.item_accounting,
            Some(CleanupSizeAccounting::ProcessedBytes)
        );
        assert_eq!(app.selected, selection);
        terminal.draw(|frame| app.render(frame)).unwrap();
        let text = screen_text(&terminal);
        assert!(text.contains("/fixture/cache/file-00001"));
        assert!(text.contains("Processed estimate: unknown"));
        press(&mut app, KeyCode::Char('U'));
        assert_eq!(app.rows[app.visible[app.cursor]].item.unwrap().id, "item-1");
        assert_eq!(app.selected, selection);
        press(&mut app, KeyCode::Enter);
        terminal.draw(|frame| app.render(frame)).unwrap();
        let text = screen_text(&terminal);
        assert!(text.contains("Processed estimate 40 B + 1 unknown  ·  Snapshot groups 1"));
        press(&mut app, KeyCode::Esc);
        press(&mut app, KeyCode::Char('C'));
        assert_eq!(app.snapshot_groups, 0);
        let cursor = app.cursor;
        press(&mut app, KeyCode::Char('U'));
        assert_eq!(app.cursor, cursor);
        press(&mut app, KeyCode::Char('R'));
        assert_eq!(app.selected_estimate(), (10, 0));
        assert_eq!(app.snapshot_groups, 1);
    }
}
