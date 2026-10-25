//! Management command adapters; configuration and platform operations stay in Core.

mod automations;
mod language;
mod modules;
mod plugins;
mod recipes;
mod themes;

pub(crate) use automations::run_automation_command;
pub(crate) use language::run_language_command;
pub(crate) use modules::run_module_command;
pub(crate) use plugins::run_plugin_command;
pub(crate) use recipes::run_recipe_command;
pub(crate) use themes::run_theme_command;

use unicode_width::UnicodeWidthStr;

fn pad_cell(value: &str, width: usize) -> String {
    format!("{value}{}", " ".repeat(width - value.width()))
}
