//! Terminal themes and shared output colors.

use std::{
    fmt::Display,
    io::{self, IsTerminal},
};

use clap::builder::styling::{Color as ClapColor, RgbColor as ClapRgb, Styles};
use crossterm::style::{Color as TerminalColor, Stylize};
use ratatui::style::{Color as TuiColor, Style as TuiStyle};
use spaceprune::api::DEFAULT_THEME_ID;

/// A semantic text color.
#[derive(Debug, Clone, Copy)]
pub(crate) enum Tone {
    /// Primary accent color.
    Accent,
    /// Secondary accent color.
    Secondary,
    /// Successful state.
    Success,
    /// Warning state.
    Warning,
    /// Error state.
    Error,
    /// Secondary information.
    Muted,
}

/// A 24-bit RGB color.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Rgb(u8, u8, u8);

impl Rgb {
    const fn new(red: u8, green: u8, blue: u8) -> Self {
        Self(red, green, blue)
    }

    fn clap(self) -> ClapColor {
        ClapColor::Rgb(ClapRgb(self.0, self.1, self.2))
    }

    fn terminal(self) -> TerminalColor {
        TerminalColor::Rgb {
            r: self.0,
            g: self.1,
            b: self.2,
        }
    }

    /// Convert to a Ratatui color.
    pub(crate) fn tui(self) -> TuiColor {
        TuiColor::Rgb(self.0, self.1, self.2)
    }

    /// Blend toward another palette color without introducing a separate color scheme.
    pub(crate) fn mix(self, other: Self, percent: u8) -> Self {
        let channel = |from: u8, to: u8| {
            ((u16::from(from) * (100 - u16::from(percent)) + u16::from(to) * u16::from(percent))
                / 100) as u8
        };
        Self(
            channel(self.0, other.0),
            channel(self.1, other.1),
            channel(self.2, other.2),
        )
    }
}

/// A terminal theme palette.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Palette {
    /// Main background color.
    pub(crate) background: Rgb,
    /// Panel and selection background color.
    pub(crate) surface: Rgb,
    /// Main foreground color.
    pub(crate) foreground: Rgb,
    /// Secondary text and border color.
    pub(crate) muted: Rgb,
    /// Primary accent color.
    pub(crate) accent: Rgb,
    /// Secondary accent color.
    pub(crate) secondary: Rgb,
    /// Success color.
    pub(crate) success: Rgb,
    /// Warning color.
    pub(crate) warning: Rgb,
    /// Error color.
    pub(crate) error: Rgb,
}

/// A built-in terminal theme.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Theme {
    id: &'static str,
    name: &'static str,
    /// The theme palette.
    pub(crate) palette: Palette,
}

impl Theme {
    /// Return all built-in themes.
    pub(crate) const fn all() -> &'static [Self] {
        &THEMES
    }

    /// Look up a theme by its stable ID.
    pub(crate) fn named(id: &str) -> Option<Self> {
        THEMES.iter().copied().find(|theme| theme.id == id)
    }

    /// Return the stable theme ID.
    pub(crate) const fn id(self) -> &'static str {
        self.id
    }

    /// Return the theme display name.
    pub(crate) const fn name(self) -> &'static str {
        self.name
    }

    /// Localized theme name for human-facing surfaces; machine metadata keeps `name`.
    pub(crate) fn display_name(self) -> &'static str {
        crate::i18n::text(match self.id {
            "aurora" => "terminal.theme_aurora",
            "orbit" => "terminal.theme_orbit",
            "graphite" => "terminal.theme_graphite",
            "forest" => "terminal.theme_forest",
            _ => unreachable!("theme IDs are built in"),
        })
    }

    /// Emphasize module names without filling their row or disclosure marker.
    pub(crate) fn module_color(self) -> Rgb {
        if self.id == "graphite" {
            Rgb::new(0x88, 0xb8, 0xf2)
        } else {
            self.palette.accent
        }
    }

    /// Hierarchy color for projects and directories, separate from selection states.
    pub(crate) fn project_color(self) -> Rgb {
        match self.id {
            "graphite" => Rgb::new(0x7a, 0xb8, 0xc8),
            "forest" => Rgb::new(0x8b, 0xbf, 0xd1),
            _ => self.palette.secondary.mix(self.palette.foreground, 35),
        }
    }

    /// Readable leaf and action text with a restrained theme tint.
    pub(crate) fn body_color(self) -> Rgb {
        self.palette.foreground.mix(self.project_color(), 22)
    }

    /// Return styles for Clap help and error output.
    pub(crate) fn clap_styles(self) -> Styles {
        Styles::styled()
            .header(self.palette.accent.clap().on_default().bold())
            .usage(self.palette.accent.clap().on_default().bold())
            .literal(self.palette.secondary.clap().on_default().bold())
            .placeholder(self.palette.muted.clap().on_default())
            .error(self.palette.error.clap().on_default().bold())
            .valid(self.palette.success.clap().on_default())
            .invalid(self.palette.warning.clap().on_default())
            .context(self.palette.muted.clap().on_default())
            .context_value(self.palette.foreground.clap().on_default())
    }

    /// Return the base foreground and background for terminal interfaces.
    pub(crate) fn base_style(self) -> TuiStyle {
        TuiStyle::new()
            .fg(self.palette.foreground.tui())
            .bg(self.palette.background.tui())
    }

    /// Style stdout text, leaving redirected output as plain text.
    pub(crate) fn stdout(self, value: impl Display, tone: Tone) -> String {
        self.paint(value, tone, io::stdout().is_terminal())
    }

    /// Style stderr text, leaving redirected output as plain text.
    pub(crate) fn stderr(self, value: impl Display, tone: Tone) -> String {
        self.paint(value, tone, io::stderr().is_terminal())
    }

    fn paint(self, value: impl Display, tone: Tone, enabled: bool) -> String {
        let value = value.to_string();
        if !enabled {
            return value;
        }
        let color = match tone {
            Tone::Accent => self.palette.accent,
            Tone::Secondary => self.palette.secondary,
            Tone::Success => self.palette.success,
            Tone::Warning => self.palette.warning,
            Tone::Error => self.palette.error,
            Tone::Muted => self.palette.muted,
        };
        let styled = value.with(color.terminal());
        if matches!(tone, Tone::Accent | Tone::Success | Tone::Error) {
            format!("{}", styled.bold())
        } else {
            format!("{styled}")
        }
    }
}

impl Default for Theme {
    fn default() -> Self {
        Theme::named(DEFAULT_THEME_ID).expect("the default theme is built in")
    }
}

// Match the desktop dark palettes; terminal preferences remain independent.
const THEMES: [Theme; 4] = [
    Theme {
        id: "aurora",
        name: "Aurora",
        palette: Palette {
            background: Rgb::new(0x0c, 0x0f, 0x18),
            surface: Rgb::new(0x14, 0x18, 0x24),
            foreground: Rgb::new(0xe9, 0xee, 0xf7),
            muted: Rgb::new(0xa4, 0xac, 0xc1),
            accent: Rgb::new(0x5a, 0xe7, 0xf2),
            secondary: Rgb::new(0x76, 0x78, 0xf9),
            success: Rgb::new(0x76, 0xc1, 0x9f),
            warning: Rgb::new(0xed, 0xc2, 0x7b),
            error: Rgb::new(0xf4, 0x7f, 0x76),
        },
    },
    Theme {
        id: "orbit",
        name: "Orbit",
        palette: Palette {
            background: Rgb::new(0x12, 0x0f, 0x1a),
            surface: Rgb::new(0x1a, 0x17, 0x26),
            foreground: Rgb::new(0xe9, 0xee, 0xf7),
            muted: Rgb::new(0xa4, 0xac, 0xc1),
            accent: Rgb::new(0xc7, 0x9c, 0xfc),
            secondary: Rgb::new(0xf2, 0x63, 0xb4),
            success: Rgb::new(0x76, 0xc1, 0x9f),
            warning: Rgb::new(0xed, 0xc2, 0x7b),
            error: Rgb::new(0xf4, 0x7f, 0x76),
        },
    },
    Theme {
        id: "graphite",
        name: "Graphite",
        palette: Palette {
            background: Rgb::new(0x12, 0x13, 0x17),
            surface: Rgb::new(0x1b, 0x1d, 0x22),
            foreground: Rgb::new(0xe9, 0xee, 0xf7),
            muted: Rgb::new(0xa4, 0xac, 0xc1),
            accent: Rgb::new(0xd0, 0xd8, 0xe1),
            secondary: Rgb::new(0x75, 0x85, 0xa3),
            success: Rgb::new(0x76, 0xc1, 0x9f),
            warning: Rgb::new(0xed, 0xc2, 0x7b),
            error: Rgb::new(0xf4, 0x7f, 0x76),
        },
    },
    Theme {
        id: "forest",
        name: "Forest",
        palette: Palette {
            background: Rgb::new(0x17, 0x1c, 0x1a),
            surface: Rgb::new(0x1e, 0x24, 0x22),
            foreground: Rgb::new(0xe6, 0xea, 0xe7),
            muted: Rgb::new(0x9b, 0xa6, 0xa1),
            accent: Rgb::new(0x76, 0xc1, 0x9f),
            secondary: Rgb::new(0x43, 0xa3, 0x89),
            success: Rgb::new(0x76, 0xc1, 0x9f),
            warning: Rgb::new(0xed, 0xc2, 0x7b),
            error: Rgb::new(0xe2, 0x63, 0x5a),
        },
    },
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn theme_ids_match_the_builtin_palettes() {
        assert_eq!(
            Theme::all()
                .iter()
                .map(|theme| theme.id())
                .collect::<Vec<_>>(),
            spaceprune::api::THEME_IDS
        );
    }
}
