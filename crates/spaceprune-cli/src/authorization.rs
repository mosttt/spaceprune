//! Native helper setup independent of the active cleanup recipe.

use crate::{
    cli::{CliCommand, ParsedArgs},
    theme::Theme,
};
use anyhow::{Result, bail};

pub(crate) fn run(parsed: &ParsedArgs, theme: Theme) -> Result<u8> {
    if parsed.action.is_some() || parsed.yes || parsed.no_progress {
        bail!("{}", crate::i18n::text("commands.authorization_arguments"));
    }
    let CliCommand::Authorization(command) = &parsed.command else {
        unreachable!("authorization command dispatch")
    };
    #[cfg(target_os = "linux")]
    return linux::run(command, parsed.output, theme);
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (command, theme);
        bail!("{}", crate::i18n::text("commands.authorization_linux"));
    }
}

#[cfg(target_os = "linux")]
mod linux {
    use super::*;
    use crate::{
        cli::{AuthorizationCommand, OutputMode},
        runtime::signal_cancellation,
        theme::Tone,
    };
    use spaceprune::api::{Interaction, OperationContext, PermissionOutcome};
    use spaceprune_access::helper as install;
    use spaceprune_readonly_protocol::client::trust;
    use std::{
        io::{self, IsTerminal},
        time::Duration,
    };

    pub(super) fn run(
        command: &AuthorizationCommand,
        output: OutputMode,
        theme: Theme,
    ) -> Result<u8> {
        let interaction = if matches!(command, AuthorizationCommand::Install { graphical: true }) {
            Interaction::Foreground
        } else if io::stdin().is_terminal() && io::stderr().is_terminal() {
            Interaction::Terminal
        } else {
            Interaction::NonInteractive
        };
        let operation =
            OperationContext::new(signal_cancellation()?, interaction, Duration::from_secs(90));
        let host = std::env::current_exe()?;
        if matches!(command, AuthorizationCommand::Install { .. }) {
            if output == OutputMode::Human {
                eprintln!(
                    "{}",
                    crate::i18n::message(
                        "commands.installing_helper",
                        &[("p0", (install::INSTALL_DIRECTORY).to_string())]
                    )
                );
            }
            install::install(&host, &operation).map_err(|error| {
                anyhow::anyhow!(match error {
                    install::InstallError::SourceMissing =>
                        crate::i18n::text("commands.source_missing"),
                    install::InstallError::SourceOutdated =>
                        crate::i18n::text("commands.source_outdated"),
                    install::InstallError::UnsafeDestination =>
                        crate::i18n::text("commands.unsafe_destination"),
                    install::InstallError::InstallerMissing =>
                        crate::i18n::text("commands.installer_missing"),
                    install::InstallError::Authorization(
                        PermissionOutcome::Denied | PermissionOutcome::Cancelled,
                    ) => crate::i18n::text("commands.install_declined"),
                    install::InstallError::Authorization(PermissionOutcome::NeedsAuthorization) =>
                        crate::i18n::text("commands.install_needs_auth"),
                    install::InstallError::Authorization(PermissionOutcome::TimedOut) =>
                        crate::i18n::text("commands.install_timeout"),
                    install::InstallError::Authorization(_) =>
                        crate::i18n::text("commands.install_verification"),
                    install::InstallError::Failed => crate::i18n::text("commands.installer_failed"),
                })
            })?;
        }
        let outcome = install::probe_installation(&operation).err();
        let (status, detail) = match outcome {
            None => (
                "ready",
                "Read and cleanup components are ready. Each capability requests system authorization when needed.",
            ),
            Some(PermissionOutcome::HelperMissing) => (
                "helper_missing",
                "Helper is not installed; authorization was not requested. Run: spaceprune authorization install",
            ),
            Some(PermissionOutcome::HelperOutdated) => (
                "helper_outdated",
                "Helper protocol is outdated; authorization was not requested. Run: spaceprune authorization install",
            ),
            Some(PermissionOutcome::HelperUntrusted) => (
                "helper_untrusted",
                "Helper installation is not trusted; authorization was not requested. Run: spaceprune authorization install",
            ),
            _ => (
                "unavailable",
                "Helper verification failed; system authorization was not requested.",
            ),
        };
        if output == OutputMode::Json {
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({
                    "status": status, "detail": detail, "helperPath": trust::HELPER_PATH,
                    "requiredProtocol": spaceprune_readonly_protocol::LINUX_SCOPE_SUPPORT_ARGUMENT,
                    "components": [
                        {"capability": "metadata", "path": install::HelperKind::Metadata.path(), "protocol": install::HelperKind::Metadata.support_argument()},
                        {"capability": "cleanup", "path": install::HelperKind::Cleanup.path(), "protocol": install::HelperKind::Cleanup.support_argument()},
                    ],
                    "canInstall": install::can_install(&host, interaction),
                }))?
            );
        } else {
            println!(
                "{}\n{}\n{}",
                theme.stdout(
                    crate::i18n::text(match outcome {
                        None => "commands.helper_ready",
                        Some(PermissionOutcome::HelperMissing) => "commands.helper_missing",
                        Some(PermissionOutcome::HelperOutdated) => "commands.helper_outdated",
                        Some(PermissionOutcome::HelperUntrusted) => "commands.helper_untrusted",
                        _ => "commands.helper_unavailable",
                    }),
                    if outcome.is_none() {
                        Tone::Success
                    } else {
                        Tone::Warning
                    }
                ),
                crate::i18n::text(match outcome {
                    None => "commands.helper_ready_detail",
                    Some(PermissionOutcome::HelperMissing) => "commands.helper_missing_detail",
                    Some(PermissionOutcome::HelperOutdated) => "commands.helper_outdated_detail",
                    Some(PermissionOutcome::HelperUntrusted) => "commands.helper_untrusted_detail",
                    _ => "commands.helper_unavailable_detail",
                }),
                trust::HELPER_PATH
            );
        }
        Ok(u8::from(outcome.is_some()))
    }
}
