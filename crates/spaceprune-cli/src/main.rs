#![forbid(unsafe_code)]
#![deny(missing_docs)]
#![deny(rustdoc::broken_intra_doc_links)]

//! SpacePrune 命令行入口。

mod application;
mod authorization;
mod cli;
mod commands;
mod i18n;
mod output;
mod progress;
mod recipe_picker;
mod runtime;
mod selection;
mod terminal;
mod theme;
mod theme_picker;

use std::{env, process::ExitCode};

const EXIT_CANCELLED: u8 = 130;

fn main() -> ExitCode {
    if let Some(code) = spaceprune::run_go_cache_worker_if_requested() {
        return ExitCode::from(u8::try_from(code).unwrap_or(1));
    }
    if let Some(code) = spaceprune::run_native_installer_if_requested() {
        return ExitCode::from(u8::try_from(code).unwrap_or(1));
    }
    application::run_cli(env::args_os())
}
