//! 基于配置动态构造的命令行接口。

use std::{
    collections::BTreeSet,
    ffi::{OsStr, OsString},
    path::PathBuf,
};

use anyhow::{Result, bail};
use clap::{Arg, ArgAction, ArgGroup, ArgMatches, Command, error::ErrorKind, value_parser};
use spaceprune::api::{
    AUTHORIZATION_COMMAND_ID, AUTOMATION_COMMAND_ID, CLEAN_COMMAND_ID, LANGUAGE_COMMAND_ID,
    LanguagePreference, MODULE_COMMAND_ID, PLUGIN_COMMAND_ID, RECIPE_COMMAND_ID, ResolvedConfig,
    SCAN_COMMAND_ID, THEME_COMMAND_ID, THEME_IDS, current_platform,
};

use crate::theme::Theme;

/// SpacePrune 固定运行方式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Action {
    /// 只扫描。
    Scan,
    /// 清理推荐项。
    Clean,
    /// 交互选择。
    Select,
}

/// 命令输出格式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OutputMode {
    /// 面向终端用户的文本。
    Human,
    /// 面向程序的 JSON。
    Json,
}

/// 已解析的顶层命令。
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum CliCommand {
    /// Inspect or explicitly install the native authorization components.
    Authorization(AuthorizationCommand),
    /// Inspect or change the shared application language.
    Language(LanguageCommand),
    /// 执行全部模块或一个配置模块。
    Run {
        /// 指定模块；为空时执行全部已启用模块。
        module: Option<String>,
        /// 模块的动态选择器。
        selectors: BTreeSet<String>,
        /// 固定脚本接口选择的任务；空集合表示全部。
        tasks: BTreeSet<String>,
    },
    /// 列出配置模块。
    ModuleList {
        /// 是否包含禁用模块。
        all: bool,
    },
    /// 管理外部插件。
    Plugin(PluginCommand),
    /// 管理自动化。
    Automation(AutomationCommand),
    /// 管理终端主题。
    Theme(ThemeCommand),
    /// 管理清理方案。
    Recipe(RecipeCommand),
}

/// Shared-language management does not require an active recipe.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LanguageCommand {
    /// Show the current preference and resolved language.
    Show,
    /// List supported languages and the system-following mode.
    List,
    /// Save a manual preference.
    Set(LanguagePreference),
}

/// Native authorization setup commands do not require an active recipe.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum AuthorizationCommand {
    /// Inspect the installation without requesting system authorization.
    Status,
    /// Install or update the bundled helper through the operating system.
    Install { graphical: bool },
}

/// 外部插件管理命令。
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum PluginCommand {
    /// List all capability groups and device-local enablement.
    Catalog,
    /// Enable a built-in or installed plugin.
    Enable(String),
    /// Disable a built-in or installed plugin.
    Disable(String),
    /// 列出已安装插件。
    List,
    /// 查看一个插件。
    Inspect(String),
    /// 安装本地插件目录。
    Install(PathBuf),
    /// 删除一个插件。
    Remove(String),
}

/// 自动化管理命令。
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum AutomationCommand {
    /// 列出自动化。
    List,
    /// 安装一个自动化。
    Install(String),
    /// 卸载一个自动化。
    Uninstall(String),
    /// 运行一个自动化。
    Run(String),
}

/// 终端主题管理命令。
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum ThemeCommand {
    /// 交互选择主题。
    Pick,
    /// 列出内置主题。
    List,
    /// 按 ID 设置当前主题。
    Set(String),
}

/// 清理方案管理命令。
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum RecipeCommand {
    /// Resume following the graphical application's current recipe.
    Follow,
    /// 交互选择清理方案。
    Pick,
    /// 列出本地清理方案。
    List,
    /// 按 ID 设置当前清理方案。
    Set(String),
}

/// Clap 解析得到的执行参数。
#[derive(Debug)]
pub(crate) struct ParsedArgs {
    pub(crate) config: Option<PathBuf>,
    pub(crate) recipe: Option<String>,
    pub(crate) action: Option<Action>,
    pub(crate) yes: bool,
    pub(crate) output: OutputMode,
    pub(crate) no_progress: bool,
    pub(crate) command: CliCommand,
}

/// 命令行解析结果。
pub(crate) enum ParseResult {
    /// 参数有效，可以继续执行。
    Parsed(ParsedArgs),
    /// Clap 已经输出帮助、版本或参数错误。
    Exit(u8),
}

/// 解析命令行；未提供配置时，未知子命令会留给配置加载后的第二阶段。
pub(crate) fn parse(
    arguments: &[OsString],
    config: Option<&ResolvedConfig>,
    theme: Theme,
) -> Result<ParseResult> {
    match try_parse(arguments, config, theme) {
        Ok(parsed) => Ok(ParseResult::Parsed(parsed)),
        Err(error) => {
            let code = error.exit_code() as u8;
            print_parse_error(&error)?;
            Ok(ParseResult::Exit(code))
        }
    }
}

/// Recognize setup commands before reading possibly broken application preferences.
pub(crate) fn authorization_args(arguments: &[OsString]) -> Option<ParsedArgs> {
    try_parse(arguments, None, Theme::default())
        .ok()
        .filter(|parsed| matches!(parsed.command, CliCommand::Authorization(_)))
}

/// 在构造动态命令树前发现显式配置路径。
pub(crate) fn explicit_config_path(arguments: &[OsString]) -> Result<Option<PathBuf>> {
    let mut path = None;
    let mut arguments = arguments.iter().skip(1);
    while let Some(argument) = arguments.next() {
        if argument == OsStr::new("--config") {
            if path.is_some() {
                bail!("{}", crate::i18n::text("commands.config_once"));
            }
            path = Some(PathBuf::from(arguments.next().ok_or_else(|| {
                anyhow::anyhow!("{}", crate::i18n::text("commands.config_path"))
            })?));
        } else if let Some(value) = argument
            .to_str()
            .and_then(|value| value.strip_prefix("--config="))
        {
            if path.is_some() {
                bail!("{}", crate::i18n::text("commands.config_once"));
            }
            if value.is_empty() {
                bail!("{}", crate::i18n::text("commands.config_path"));
            }
            path = Some(PathBuf::from(value));
        }
    }
    Ok(path)
}

fn try_parse(
    arguments: &[OsString],
    config: Option<&ResolvedConfig>,
    theme: Theme,
) -> std::result::Result<ParsedArgs, clap::Error> {
    let mut command = command(config, theme);
    let matches = command.clone().try_get_matches_from(arguments)?;
    if matches
        .subcommand_name()
        .is_some_and(|name| matches!(name, SCAN_COMMAND_ID | CLEAN_COMMAND_ID))
        && (matches.get_flag("scan") || matches.get_flag("clean") || matches.get_flag("select"))
    {
        return Err(command.error(
            ErrorKind::ArgumentConflict,
            crate::i18n::text("commands.mode_conflict"),
        ));
    }
    Ok(parsed_args(&matches, config))
}

fn command(config: Option<&ResolvedConfig>, theme: Theme) -> Command {
    let mut command = Command::new("spaceprune")
        .bin_name("spaceprune")
        .about(crate::i18n::text("commands.about"))
        .version(env!("CARGO_PKG_VERSION"))
        .styles(theme.clap_styles())
        .propagate_version(true)
        .disable_help_subcommand(true)
        .after_help(crate::i18n::text("commands.root_examples"))
        .arg(
            Arg::new("recipe")
                .long("recipe")
                .value_name("ID")
                .help(crate::i18n::text("commands.help_recipe_override"))
                .global(true),
        )
        .arg(
            Arg::new("config")
                .long("config")
                .value_name(crate::i18n::text("commands.path_header"))
                .help(crate::i18n::text("commands.help_config"))
                .value_parser(value_parser!(PathBuf))
                .global(true),
        )
        .arg(
            Arg::new("scan")
                .long("scan")
                .help(crate::i18n::text("commands.help_scan"))
                .action(ArgAction::SetTrue)
                .global(true),
        )
        .arg(
            Arg::new("clean")
                .long("clean")
                .help(crate::i18n::text("commands.help_clean"))
                .action(ArgAction::SetTrue)
                .global(true),
        )
        .arg(
            Arg::new("select")
                .long("select")
                .help(crate::i18n::text("commands.help_select"))
                .action(ArgAction::SetTrue)
                .global(true),
        )
        .group(
            ArgGroup::new("mode")
                .args(["scan", "clean", "select"])
                .multiple(false),
        )
        .arg(
            Arg::new("yes")
                .long("yes")
                .short('y')
                .help(crate::i18n::text("commands.help_yes"))
                .action(ArgAction::SetTrue)
                .global(true),
        )
        .arg(
            Arg::new("output")
                .long("output")
                .value_name("FORMAT")
                .help(crate::i18n::text("commands.help_output"))
                .value_parser(["human", "json"])
                .default_value("human")
                .global(true),
        )
        .arg(
            Arg::new("no-progress")
                .long("no-progress")
                .help(crate::i18n::text("commands.help_no_progress"))
                .action(ArgAction::SetTrue)
                .global(true),
        )
        .subcommand(language_command())
        .subcommand(module_command())
        .subcommand(authorization_command())
        .subcommand(plugin_command())
        .subcommand(automation_command())
        .subcommand(recipe_command())
        .subcommand(theme_command())
        .subcommand(fixed_run_command(
            SCAN_COMMAND_ID,
            crate::i18n::text("commands.help_scan"),
        ))
        .subcommand(fixed_run_command(
            CLEAN_COMMAND_ID,
            crate::i18n::text("commands.help_clean_fixed"),
        ));

    if let Some(config) = config {
        for (id, module) in config.enabled_modules() {
            let about = if module.description.is_empty() {
                module.name.clone()
            } else {
                module.description.clone()
            };
            let mut module_command = Command::new(id.to_owned()).about(about);
            for (selector_id, selector) in module
                .selectors
                .iter()
                .filter(|(_, selector)| selector.enabled)
            {
                module_command = module_command.arg(
                    Arg::new(selector_id.to_owned())
                        .long(selector_id.to_owned())
                        .help(selector.name.clone())
                        .action(ArgAction::SetTrue),
                );
            }
            let tasks = module
                .tasks
                .iter()
                .filter(|(_, task)| {
                    task.enabled
                        && (task.platforms.is_empty()
                            || task
                                .platforms
                                .iter()
                                .any(|platform| platform == current_platform()))
                })
                .map(|(id, task)| format!("  {id:<20} {}", task.name))
                .collect::<Vec<_>>();
            if !tasks.is_empty() {
                module_command = module_command.after_help(crate::i18n::message(
                    "commands.tasks",
                    &[("p0", (tasks.join("\n")).to_string())],
                ));
            }
            command = command.subcommand(module_command);
        }
    } else {
        command = command.allow_external_subcommands(true);
    }

    localize_command(scope_help_options(command))
}

fn scope_help_options(mut command: Command) -> Command {
    let globals = command.get_arguments().cloned().collect::<Vec<_>>();
    let hidden = |ids: &[&str]| {
        globals
            .iter()
            .filter(|arg| ids.contains(&arg.get_id().as_str()))
            .cloned()
            .map(|arg| arg.hide(true))
            .collect::<Vec<_>>()
    };
    // Shadow inherited arguments only in help. Existing parse/validation behavior is retained.
    for id in [
        LANGUAGE_COMMAND_ID,
        MODULE_COMMAND_ID,
        RECIPE_COMMAND_ID,
        THEME_COMMAND_ID,
        PLUGIN_COMMAND_ID,
        AUTOMATION_COMMAND_ID,
        AUTHORIZATION_COMMAND_ID,
    ] {
        let mut ids = vec!["scan", "clean", "select", "yes", "no-progress"];
        if matches!(id, PLUGIN_COMMAND_ID | AUTHORIZATION_COMMAND_ID) {
            ids.push("config");
        }
        command = command.mut_subcommand(id, |subcommand| subcommand.args(hidden(&ids)));
    }
    let no_progress = globals
        .iter()
        .find(|arg| arg.get_id() == "no-progress")
        .expect("root progress option")
        .clone();
    command = command.mut_subcommand(AUTOMATION_COMMAND_ID, |automation| {
        automation.mut_subcommand("run", |run| run.arg(no_progress))
    });
    command = command.mut_subcommand(SCAN_COMMAND_ID, |scan| {
        scan.args(hidden(&["scan", "clean", "select", "yes"]))
    });
    command.mut_subcommand(CLEAN_COMMAND_ID, |clean| {
        clean.args(hidden(&["scan", "clean", "select"]))
    })
}

fn localize_command(command: Command) -> Command {
    command
        .disable_help_flag(true)
        .disable_version_flag(true)
        .arg(
            Arg::new("help")
                .long("help")
                .short('h')
                .action(ArgAction::Help)
                .help(crate::i18n::text("commands.print_help")),
        )
        .arg(
            Arg::new("version")
                .long("version")
                .short('V')
                .action(ArgAction::Version)
                .help(crate::i18n::text("commands.print_version")),
        )
        .help_template(crate::i18n::text("commands.help_template"))
        .subcommand_help_heading(crate::i18n::text("commands.commands"))
        .mut_args(|arg| {
            let heading = if arg.is_positional() {
                "commands.arguments"
            } else {
                "commands.options"
            };
            let arg = if crate::i18n::is_chinese() && arg.get_action().takes_values() {
                arg.hide_default_value(true).hide_possible_values(true)
            } else {
                arg
            };
            arg.help_heading(crate::i18n::text(heading))
        })
        .mut_subcommands(localize_command)
}

fn print_parse_error(error: &clap::Error) -> std::io::Result<()> {
    use clap::error::ContextKind;
    if !crate::i18n::is_chinese()
        || matches!(
            error.kind(),
            ErrorKind::DisplayHelp
                | ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand
                | ErrorKind::DisplayVersion
        )
    {
        return error.print();
    }
    let key = match error.kind() {
        ErrorKind::InvalidValue | ErrorKind::ValueValidation => "commands.invalid_value",
        ErrorKind::UnknownArgument | ErrorKind::InvalidSubcommand => "commands.unknown_argument",
        ErrorKind::ArgumentConflict => "commands.argument_conflict",
        ErrorKind::MissingRequiredArgument | ErrorKind::MissingSubcommand => {
            "commands.missing_argument"
        }
        ErrorKind::TooManyValues | ErrorKind::TooFewValues | ErrorKind::WrongNumberOfValues => {
            "commands.value_count"
        }
        ErrorKind::InvalidUtf8 => "commands.invalid_utf8",
        ErrorKind::NoEquals => "commands.requires_equals",
        _ => "commands.invalid_arguments",
    };
    eprintln!(
        "{} {}",
        crate::i18n::text("commands.error_label"),
        crate::i18n::text(key)
    );
    for (kind, value) in error.context() {
        let label = match kind {
            ContextKind::InvalidArg => "commands.argument_label",
            ContextKind::InvalidSubcommand => "commands.subcommand_label",
            ContextKind::InvalidValue => "commands.value_label",
            ContextKind::PriorArg => "commands.prior_argument_label",
            ContextKind::ValidValue => "commands.valid_values_label",
            ContextKind::ValidSubcommand => "commands.valid_commands_label",
            ContextKind::ExpectedNumValues => "commands.expected_count_label",
            ContextKind::ActualNumValues => "commands.actual_count_label",
            ContextKind::MinValues => "commands.minimum_count_label",
            ContextKind::SuggestedArg
            | ContextKind::SuggestedSubcommand
            | ContextKind::SuggestedValue
            | ContextKind::SuggestedCommand => "commands.suggestion_label",
            ContextKind::TrailingArg => "commands.trailing_argument_label",
            ContextKind::Custom => "commands.detail_label",
            _ => continue,
        };
        eprintln!("  {} {value}", crate::i18n::text(label));
    }
    eprintln!("\n{}", crate::i18n::text("commands.parse_hint"));
    Ok(())
}

fn language_command() -> Command {
    Command::new(LANGUAGE_COMMAND_ID)
        .about(crate::i18n::text("commands.language_about"))
        .subcommand(Command::new("list").about(crate::i18n::text("commands.language_list_about")))
        .subcommand(
            Command::new("set")
                .about(crate::i18n::text("commands.language_set_about"))
                .arg(
                    Arg::new("language-id")
                        .help(crate::i18n::text("commands.language_values"))
                        .value_parser(["system", "en", "zh-CN"])
                        .required(true),
                ),
        )
}

fn authorization_command() -> Command {
    Command::new(AUTHORIZATION_COMMAND_ID)
        .about(crate::i18n::text("commands.help_authorization"))
        .subcommand_required(true)
        .subcommand(Command::new("status").about(crate::i18n::text("commands.help_auth_status")))
        .subcommand(
            Command::new("install")
                .about(crate::i18n::text("commands.help_auth_install"))
                .arg(
                    Arg::new("graphical")
                        .long("graphical")
                        .help(crate::i18n::text("commands.help_graphical"))
                        .action(ArgAction::SetTrue),
                ),
        )
}

fn module_command() -> Command {
    Command::new(MODULE_COMMAND_ID)
        .about(crate::i18n::text("commands.help_module"))
        .after_help(crate::i18n::text("commands.help_module_example"))
        .subcommand_required(true)
        .arg_required_else_help(true)
        .subcommand(
            Command::new("list")
                .about(crate::i18n::text("commands.help_module_list"))
                .arg(
                    Arg::new("all")
                        .long("all")
                        .help(crate::i18n::text("commands.help_all"))
                        .action(ArgAction::SetTrue),
                ),
        )
}

fn plugin_command() -> Command {
    Command::new(PLUGIN_COMMAND_ID)
        .about(crate::i18n::text("commands.help_plugin"))
        .after_help(crate::i18n::text("commands.help_plugin_usage"))
        .subcommand_required(true)
        .arg_required_else_help(true)
        .subcommand(Command::new("list").about(crate::i18n::text("commands.help_plugin_list")))
        .subcommand(Command::new("catalog").about(crate::i18n::text("commands.help_catalog")))
        .subcommand(
            Command::new("enable")
                .about(crate::i18n::text("commands.help_enable"))
                .arg(Arg::new("plugin-id").required(true)),
        )
        .subcommand(
            Command::new("disable")
                .about(crate::i18n::text("commands.help_disable"))
                .arg(Arg::new("plugin-id").required(true)),
        )
        .subcommand(
            Command::new("inspect")
                .about(crate::i18n::text("commands.help_inspect"))
                .arg(Arg::new("plugin-id").required(true)),
        )
        .subcommand(
            Command::new("install")
                .about(crate::i18n::text("commands.help_install"))
                .arg(
                    Arg::new("plugin-path")
                        .value_parser(value_parser!(PathBuf))
                        .required(true),
                ),
        )
        .subcommand(
            Command::new("remove")
                .about(crate::i18n::text("commands.help_remove"))
                .arg(Arg::new("plugin-id").required(true)),
        )
}

fn automation_command() -> Command {
    Command::new(AUTOMATION_COMMAND_ID)
        .about(crate::i18n::text("commands.help_automation"))
        .after_help(crate::i18n::text("commands.help_automation_usage"))
        .subcommand_required(true)
        .arg_required_else_help(true)
        .subcommand(Command::new("list").about(crate::i18n::text("commands.help_automation_list")))
        .subcommand(
            Command::new("install")
                .about(crate::i18n::text("commands.help_automation_install"))
                .arg(Arg::new("automation-id").required(true)),
        )
        .subcommand(
            Command::new("uninstall")
                .about(crate::i18n::text("commands.help_automation_uninstall"))
                .arg(Arg::new("automation-id").required(true)),
        )
        .subcommand(
            Command::new("run")
                .about(crate::i18n::text("commands.help_automation_run"))
                .arg(Arg::new("automation-id").required(true)),
        )
}

fn theme_command() -> Command {
    Command::new(THEME_COMMAND_ID)
        .about(crate::i18n::text("commands.help_theme"))
        .after_help(crate::i18n::text("commands.help_theme_usage"))
        .subcommand(Command::new("list").about(crate::i18n::text("commands.help_theme_list")))
        .subcommand(
            Command::new("set")
                .about(crate::i18n::text("commands.help_theme_set"))
                .arg(
                    Arg::new("theme-id")
                        .help(crate::i18n::text("commands.help_theme_id"))
                        .value_parser(THEME_IDS)
                        .required(true),
                ),
        )
}

fn recipe_command() -> Command {
    Command::new(RECIPE_COMMAND_ID)
        .about(crate::i18n::text("commands.help_recipe"))
        .after_help(crate::i18n::text("commands.help_recipe_usage"))
        .subcommand(Command::new("list").about(crate::i18n::text("commands.help_recipe_list")))
        .subcommand(Command::new("follow").about(crate::i18n::text("commands.help_recipe_follow")))
        .subcommand(
            Command::new("set")
                .about(crate::i18n::text("commands.help_recipe_set"))
                .arg(
                    Arg::new("recipe-id")
                        .help(crate::i18n::text("commands.help_recipe_id"))
                        .required(true),
                ),
        )
}

fn fixed_run_command(id: &'static str, about: &'static str) -> Command {
    Command::new(id)
        .about(about)
        .arg(
            Arg::new("script-module")
                .long("module")
                .value_name("ID")
                .help(crate::i18n::text("commands.help_script_module")),
        )
        .arg(
            Arg::new("script-selector")
                .long("selector")
                .value_name("ID")
                .help(crate::i18n::text("commands.help_script_selector"))
                .action(ArgAction::Append)
                .requires("script-module"),
        )
        .arg(
            Arg::new("script-task")
                .long("task")
                .value_name("ID")
                .help(crate::i18n::text("commands.help_script_task"))
                .action(ArgAction::Append)
                .requires("script-module"),
        )
}

fn parsed_args(matches: &ArgMatches, config: Option<&ResolvedConfig>) -> ParsedArgs {
    let fixed_action = match matches.subcommand_name() {
        Some(SCAN_COMMAND_ID) => Some(Action::Scan),
        Some(CLEAN_COMMAND_ID) => Some(Action::Clean),
        _ => None,
    };
    let action = if matches.get_flag("scan") {
        Some(Action::Scan)
    } else if matches.get_flag("clean") {
        Some(Action::Clean)
    } else if matches.get_flag("select") {
        Some(Action::Select)
    } else {
        fixed_action
    };
    let output = match matches
        .get_one::<String>("output")
        .expect("output has a default")
        .as_str()
    {
        "human" => OutputMode::Human,
        "json" => OutputMode::Json,
        _ => unreachable!("output values are restricted by clap"),
    };
    ParsedArgs {
        config: matches.get_one::<PathBuf>("config").cloned(),
        recipe: matches.get_one::<String>("recipe").cloned(),
        action,
        yes: matches.get_flag("yes"),
        output,
        no_progress: matches.get_flag("no-progress"),
        command: parsed_command(matches, config),
    }
}

fn parsed_command(matches: &ArgMatches, config: Option<&ResolvedConfig>) -> CliCommand {
    let Some((command, command_matches)) = matches.subcommand() else {
        return CliCommand::Run {
            module: None,
            selectors: BTreeSet::new(),
            tasks: BTreeSet::new(),
        };
    };
    match command {
        LANGUAGE_COMMAND_ID => CliCommand::Language(match command_matches.subcommand() {
            Some(("list", _)) => LanguageCommand::List,
            Some(("set", args)) => LanguageCommand::Set(
                LanguagePreference::parse(
                    args.get_one::<String>("language-id")
                        .expect("language ID is required"),
                )
                .expect("language IDs are restricted by clap"),
            ),
            _ => LanguageCommand::Show,
        }),
        MODULE_COMMAND_ID => {
            let (_, subcommand_matches) = command_matches
                .subcommand()
                .expect("module requires a subcommand");
            CliCommand::ModuleList {
                all: subcommand_matches.get_flag("all"),
            }
        }
        AUTHORIZATION_COMMAND_ID => CliCommand::Authorization(match command_matches.subcommand() {
            Some(("install", args)) => AuthorizationCommand::Install {
                graphical: args.get_flag("graphical"),
            },
            _ => AuthorizationCommand::Status,
        }),
        PLUGIN_COMMAND_ID => CliCommand::Plugin(parsed_plugin_command(command_matches)),
        AUTOMATION_COMMAND_ID => CliCommand::Automation(parsed_automation_command(command_matches)),
        RECIPE_COMMAND_ID => CliCommand::Recipe(parsed_recipe_command(command_matches)),
        THEME_COMMAND_ID => CliCommand::Theme(parsed_theme_command(command_matches)),
        SCAN_COMMAND_ID | CLEAN_COMMAND_ID => CliCommand::Run {
            module: command_matches.get_one::<String>("script-module").cloned(),
            selectors: command_matches
                .get_many::<String>("script-selector")
                .into_iter()
                .flatten()
                .cloned()
                .collect(),
            tasks: command_matches
                .get_many::<String>("script-task")
                .into_iter()
                .flatten()
                .cloned()
                .collect(),
        },
        module_id => {
            let mut selectors = BTreeSet::new();
            if let Some(module) = config.and_then(|config| config.modules.get(module_id)) {
                selectors.extend(
                    module
                        .selectors
                        .iter()
                        .filter(|(_, selector)| selector.enabled)
                        .filter(|(id, _)| command_matches.get_flag(id.as_str()))
                        .map(|(id, _)| id.clone()),
                );
            }
            CliCommand::Run {
                module: Some(module_id.to_owned()),
                selectors,
                tasks: BTreeSet::new(),
            }
        }
    }
}

fn parsed_plugin_command(matches: &ArgMatches) -> PluginCommand {
    let (subcommand, subcommand_matches) =
        matches.subcommand().expect("plugin requires a subcommand");
    match subcommand {
        "list" => PluginCommand::List,
        "catalog" => PluginCommand::Catalog,
        "enable" => PluginCommand::Enable(
            subcommand_matches
                .get_one::<String>("plugin-id")
                .expect("plugin ID is required")
                .clone(),
        ),
        "disable" => PluginCommand::Disable(
            subcommand_matches
                .get_one::<String>("plugin-id")
                .expect("plugin ID is required")
                .clone(),
        ),
        "inspect" => PluginCommand::Inspect(
            subcommand_matches
                .get_one::<String>("plugin-id")
                .expect("plugin ID is required")
                .clone(),
        ),
        "install" => PluginCommand::Install(
            subcommand_matches
                .get_one::<PathBuf>("plugin-path")
                .expect("plugin path is required")
                .clone(),
        ),
        "remove" => PluginCommand::Remove(
            subcommand_matches
                .get_one::<String>("plugin-id")
                .expect("plugin ID is required")
                .clone(),
        ),
        _ => unreachable!("plugin subcommands are restricted by clap"),
    }
}

fn parsed_automation_command(matches: &ArgMatches) -> AutomationCommand {
    let (subcommand, subcommand_matches) = matches
        .subcommand()
        .expect("automation requires a subcommand");
    let id = || {
        subcommand_matches
            .get_one::<String>("automation-id")
            .expect("automation ID is required")
            .clone()
    };
    match subcommand {
        "list" => AutomationCommand::List,
        "install" => AutomationCommand::Install(id()),
        "uninstall" => AutomationCommand::Uninstall(id()),
        "run" => AutomationCommand::Run(id()),
        _ => unreachable!("automation subcommands are restricted by clap"),
    }
}

fn parsed_theme_command(matches: &ArgMatches) -> ThemeCommand {
    match matches.subcommand() {
        None => ThemeCommand::Pick,
        Some(("list", _)) => ThemeCommand::List,
        Some(("set", subcommand_matches)) => ThemeCommand::Set(
            subcommand_matches
                .get_one::<String>("theme-id")
                .expect("theme ID is required")
                .clone(),
        ),
        Some(_) => unreachable!("theme subcommands are restricted by clap"),
    }
}

fn parsed_recipe_command(matches: &ArgMatches) -> RecipeCommand {
    match matches.subcommand() {
        None => RecipeCommand::Pick,
        Some(("follow", _)) => RecipeCommand::Follow,
        Some(("list", _)) => RecipeCommand::List,
        Some(("set", subcommand_matches)) => RecipeCommand::Set(
            subcommand_matches
                .get_one::<String>("recipe-id")
                .expect("recipe ID is required")
                .clone(),
        ),
        Some(_) => unreachable!("recipe subcommands are restricted by clap"),
    }
}

#[cfg(test)]
mod tests {
    use std::{collections::BTreeSet, ffi::OsString};

    use crate::theme::Theme;

    use super::{Action, CliCommand, OutputMode, RecipeCommand, ThemeCommand, try_parse};

    fn arguments(values: &[&str]) -> Vec<OsString> {
        std::iter::once("spaceprune")
            .chain(values.iter().copied())
            .map(OsString::from)
            .collect()
    }

    #[test]
    fn schedule_recipe_argument_is_parsed_without_changing_preferences() {
        let parsed = try_parse(
            &arguments(&[
                "--config",
                "config.toml",
                "--recipe",
                "chosen",
                "--no-progress",
                "automation",
                "run",
                "daily",
            ]),
            None,
            Theme::default(),
        )
        .unwrap();
        assert_eq!(parsed.recipe.as_deref(), Some("chosen"));
        assert_eq!(
            parsed.command,
            CliCommand::Automation(super::AutomationCommand::Run("daily".into()))
        );
    }

    #[test]
    fn actions_are_mutually_exclusive() {
        assert!(try_parse(&arguments(&["--scan", "--clean"]), None, Theme::default()).is_err());
    }

    #[test]
    fn singleton_options_are_rejected_when_repeated() {
        assert!(
            try_parse(
                &arguments(&["--config=a", "--config", "b", "--scan"]),
                None,
                Theme::default()
            )
            .is_err()
        );
        assert!(
            try_parse(
                &arguments(&["--output=json", "--output", "human", "--scan"]),
                None,
                Theme::default()
            )
            .is_err()
        );
    }

    #[test]
    fn parses_module_list_all() {
        let parsed = try_parse(
            &arguments(&["module", "list", "--all", "--output=json"]),
            None,
            Theme::default(),
        )
        .unwrap();
        assert_eq!(parsed.output, OutputMode::Json);
        assert_eq!(parsed.command, CliCommand::ModuleList { all: true });
    }

    #[test]
    fn all_is_not_a_global_option() {
        let error =
            try_parse(&arguments(&["--all", "--scan"]), None, Theme::default()).unwrap_err();
        assert_eq!(error.kind(), clap::error::ErrorKind::UnknownArgument);
    }

    #[test]
    fn parses_root_scan() {
        let parsed = try_parse(
            &arguments(&["--scan", "--no-progress"]),
            None,
            Theme::default(),
        )
        .unwrap();
        assert_eq!(parsed.action, Some(Action::Scan));
        assert!(parsed.no_progress);
        assert_eq!(
            parsed.command,
            CliCommand::Run {
                module: None,
                selectors: BTreeSet::new(),
                tasks: BTreeSet::new(),
            }
        );
    }

    #[test]
    fn parses_stable_scan_and_clean_commands() {
        let parsed = try_parse(
            &arguments(&[
                "scan",
                "--module",
                "personal",
                "--selector",
                "logs",
                "--selector",
                "large",
                "--task",
                "cache",
            ]),
            None,
            Theme::default(),
        )
        .unwrap();
        assert_eq!(parsed.action, Some(Action::Scan));
        assert_eq!(
            parsed.command,
            CliCommand::Run {
                module: Some("personal".to_owned()),
                selectors: BTreeSet::from(["large".to_owned(), "logs".to_owned()]),
                tasks: BTreeSet::from(["cache".to_owned()]),
            }
        );

        let parsed = try_parse(
            &arguments(&["clean", "--module", "personal", "--yes"]),
            None,
            Theme::default(),
        )
        .unwrap();
        assert_eq!(parsed.action, Some(Action::Clean));
        assert!(parsed.yes);
    }

    #[test]
    fn stable_commands_reject_legacy_mode_flags() {
        assert!(try_parse(&arguments(&["scan", "--clean"]), None, Theme::default()).is_err());
    }

    #[test]
    fn parses_theme_set() {
        let parsed = try_parse(
            &arguments(&["theme", "set", "orbit"]),
            None,
            Theme::default(),
        )
        .unwrap();
        assert_eq!(
            parsed.command,
            CliCommand::Theme(ThemeCommand::Set("orbit".to_owned()))
        );
    }

    #[test]
    fn parses_interactive_theme_picker() {
        let parsed = try_parse(&arguments(&["theme"]), None, Theme::default()).unwrap();
        assert_eq!(parsed.command, CliCommand::Theme(ThemeCommand::Pick));
    }

    #[test]
    fn theme_set_requires_an_id() {
        assert!(try_parse(&arguments(&["theme", "set"]), None, Theme::default()).is_err());
    }

    #[test]
    fn parses_recipe_commands() {
        let parsed = try_parse(&arguments(&["recipe"]), None, Theme::default()).unwrap();
        assert_eq!(parsed.command, CliCommand::Recipe(RecipeCommand::Pick));

        let parsed = try_parse(
            &arguments(&["recipe", "set", "personal"]),
            None,
            Theme::default(),
        )
        .unwrap();
        assert_eq!(
            parsed.command,
            CliCommand::Recipe(RecipeCommand::Set("personal".to_owned()))
        );
    }
}
