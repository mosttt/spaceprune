//! Shared section headings and display-width-aware columns for human output.

use std::time::Duration;
use unicode_width::UnicodeWidthStr;

use crate::theme::{Theme, Tone};

pub(crate) struct Receipt {
    theme: Theme,
    stderr: bool,
    width: usize,
}

impl Receipt {
    pub(crate) fn stdout(theme: Theme) -> Self {
        Self::new(theme, false)
    }

    pub(crate) fn stderr(theme: Theme) -> Self {
        Self::new(theme, true)
    }

    fn new(theme: Theme, stderr: bool) -> Self {
        Self {
            theme,
            stderr,
            width: crossterm::terminal::size().map_or(72, |(columns, _)| {
                usize::from(columns.saturating_sub(2)).min(72)
            }),
        }
    }

    fn paint(&self, value: &str, tone: Tone) -> String {
        if self.stderr {
            self.theme.stderr(value, tone)
        } else {
            self.theme.stdout(value, tone)
        }
    }

    pub(crate) fn heading(&self, title: &str, tone: Tone, elapsed: Option<Duration>) -> String {
        let elapsed = elapsed.map_or_else(String::new, |value| {
            format!(" · {}", crate::progress::duration(value))
        });
        let remaining = self
            .width
            .saturating_sub(title.width() + elapsed.width() + 4);
        format!(
            "\n{} {}{} {}\n",
            self.paint("──", Tone::Muted),
            self.paint(title, tone),
            self.paint(&elapsed, Tone::Muted),
            self.paint(&"─".repeat(remaining), Tone::Muted),
        )
    }

    pub(crate) fn rule(&self) -> String {
        format!(
            "  {}\n",
            self.paint(&"─".repeat(self.width.saturating_sub(2)), Tone::Muted)
        )
    }

    pub(crate) fn row(&self, label: &str, value: &str, tone: Tone) -> String {
        format!("{}{}", field_label(label), self.paint(value, tone))
    }
}

pub(crate) fn field_label(label: &str) -> String {
    format!(
        "  {label}{}",
        " ".repeat(16_usize.saturating_sub(label.width()).max(2))
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chinese_and_english_values_start_at_the_same_terminal_column() {
        for label in [
            "成功",
            "处理大小",
            "卷空间变化",
            "Processed size",
            "Service issues",
        ] {
            assert_eq!(field_label(label).width(), 18, "{label}");
        }
        assert!(field_label("a label longer than sixteen cells").ends_with("  "));
    }
}
