//! Theme-aware details for the focused cleanup row.

use super::*;

impl SelectionApp<'_> {
    pub(super) fn render_details(&self, frame: &mut Frame, area: Rect) {
        let palette = self.theme.palette;
        let accent = Style::new().fg(self.theme.module_color().tui()).bold();
        let muted = Style::new().fg(palette.muted.tui());
        let warning = Style::new().fg(palette.warning.tui());
        let text = self
            .visible
            .get(self.cursor)
            .map(|index| &self.rows[*index])
            .map_or_else(
                || Text::styled(crate::i18n::text("terminal.no_matching"), muted),
                |row| {
                    let value = row_identity_style(row, self.theme);
                    let not_applicable =
                        row.logical_size_applicability == SizeApplicability::NotApplicable;
                    let size_style = row_size_style(row, self.theme);
                    if let Some(item) = row.item {
                        let reason_style = row_status_style(row, self.theme);
                        let mut lines = vec![
                            location_line(&item.location, value, muted),
                            Line::from(vec![
                                Span::styled(crate::i18n::text("terminal.reason_label"), muted),
                                Span::styled(&item.reason, reason_style),
                            ]),
                        ];
                        if not_applicable {
                            lines.push(Line::from(vec![
                                Span::styled(crate::i18n::text("terminal.logical_label"), muted),
                                Span::styled(crate::i18n::text("terminal.not_applicable"), muted),
                            ]));
                        } else if row.item_accounting == Some(CleanupSizeAccounting::SnapshotGroups)
                        {
                            lines.push(Line::from(vec![
                                Span::styled(crate::i18n::text("terminal.logical_label"), muted),
                                Span::styled(
                                    format_logical_size(
                                        row.logical_bytes,
                                        row.unknown_logical_items,
                                    ),
                                    size_style,
                                ),
                                Span::styled(" · ", muted),
                                Span::styled(
                                    crate::i18n::text("terminal.unknown_reclaimable"),
                                    warning,
                                ),
                            ]));
                            lines.push(Line::styled(
                                crate::i18n::text("terminal.snapshot_shared"),
                                muted,
                            ));
                        }
                        if !not_applicable
                            && row.item_accounting == Some(CleanupSizeAccounting::ProcessedBytes)
                        {
                            lines.push(Line::from(vec![
                                Span::styled(crate::i18n::text("terminal.processed_label"), muted),
                                Span::styled(
                                    item.reclaimable_bytes.map_or_else(
                                        || crate::i18n::text("terminal.unknown").to_owned(),
                                        format_bytes,
                                    ),
                                    if item.reclaimable_bytes.is_some() {
                                        value
                                    } else {
                                        warning.bold()
                                    },
                                ),
                            ]));
                        }
                        Text::from(lines)
                    } else {
                        let selection = row_selection_style(row, self.theme);
                        let unknown = if row.unknown_logical_items == 0 {
                            muted
                        } else {
                            warning
                        };
                        let mut lines = row
                            .locations
                            .iter()
                            .map(|location| location_line(location, value, muted))
                            .collect::<Vec<_>>();
                        lines.extend(
                            row.diagnostics
                                .iter()
                                .map(|message| Line::styled(message, warning)),
                        );
                        let mut logical = vec![
                            Span::styled(crate::i18n::text("terminal.logical_label"), muted),
                            Span::styled(
                                format_applicable_logical_size(
                                    row.logical_bytes,
                                    row.unknown_logical_items,
                                    row.logical_size_applicability,
                                ),
                                size_style,
                            ),
                        ];
                        if !not_applicable {
                            logical.extend([
                                Span::styled(" · ", muted),
                                Span::styled(
                                    crate::i18n::message(
                                        "terminal.items_unknown_size",
                                        &[("p0", row.unknown_logical_items.to_string())],
                                    ),
                                    unknown,
                                ),
                            ]);
                        }
                        lines.extend([
                            if matches!(row.kind, RowKind::Task { .. }) {
                                Line::from(vec![
                                    Span::styled(
                                        crate::i18n::text("terminal.handler_prefix"),
                                        muted,
                                    ),
                                    Span::styled(row.description.clone(), value),
                                ])
                            } else {
                                Line::styled(row.description.clone(), muted)
                            },
                            Line::from(vec![
                                Span::styled(
                                    crate::i18n::text("terminal.selected_label"),
                                    selection,
                                ),
                                Span::styled(row.selected.to_string(), value),
                                Span::styled(crate::i18n::text("terminal.of"), muted),
                                Span::styled(row.selectable.to_string(), value),
                                Span::styled(crate::i18n::text("terminal.selectable_items"), muted),
                            ]),
                            Line::from(logical),
                        ]);
                        if !not_applicable {
                            lines.push(Line::styled(
                                crate::i18n::text("terminal.logical_separate"),
                                muted,
                            ));
                        }
                        Text::from(lines)
                    }
                },
            );
        frame.render_widget(
            Paragraph::new(text).wrap(Wrap { trim: true }).block(
                Block::new()
                    .title(Span::styled(crate::i18n::text("terminal.details"), accent))
                    .borders(Borders::ALL)
                    .border_type(BorderType::Rounded)
                    .border_style(palette.secondary.tui()),
            ),
            area,
        );
    }
}

fn location_line(location: &str, value: Style, label: Style) -> Line<'_> {
    Line::from(vec![
        Span::styled(crate::i18n::text("terminal.location_label"), label),
        Span::styled(crate::progress::display_message(location), value),
    ])
}
