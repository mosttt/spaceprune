//! The actual native programs cannot exchange capabilities through command-line dispatch.

#[cfg(target_os = "linux")]
mod linux {
    use std::process::{Command, Stdio};

    #[test]
    fn each_binary_accepts_only_its_own_protocol_probe() {
        let programs = [
            (
                env!("CARGO_BIN_EXE_spaceprune-readonly-helper"),
                "--supports-scope-v6",
            ),
            (
                env!("CARGO_BIN_EXE_spaceprune-cleanup-helper"),
                "--supports-cleanup-v4",
            ),
            (
                env!("CARGO_BIN_EXE_spaceprune-helper-installer"),
                "--supports-native-install-v2",
            ),
        ];
        for (program, own_probe) in programs {
            for (_, probe) in programs {
                let status = Command::new(program)
                    .arg(probe)
                    .stdin(Stdio::null())
                    .status()
                    .unwrap();
                assert_eq!(
                    status.code(),
                    Some(if probe == own_probe { 0 } else { 64 }),
                    "{program}: {probe}"
                );
            }
        }
    }

    #[test]
    fn installer_does_not_accept_a_caller_selected_destination_or_shell_command() {
        for argument in [
            "--destination=/tmp/spaceprune",
            "--command=sh",
            "--spaceprune-cleanup-session",
            "--install-and-start-cleanup",
        ] {
            let status = Command::new(env!("CARGO_BIN_EXE_spaceprune-helper-installer"))
                .arg(argument)
                .stdin(Stdio::null())
                .status()
                .unwrap();
            assert_eq!(status.code(), Some(64));
        }
    }

    #[test]
    fn cleanup_rejects_the_previous_size_receipt_protocol_before_starting_work() {
        let status = Command::new(env!("CARGO_BIN_EXE_spaceprune-cleanup-helper"))
            .arg("--supports-cleanup-v3")
            .stdin(Stdio::null())
            .status()
            .unwrap();
        assert_eq!(status.code(), Some(64));
    }

    #[test]
    fn localized_entrypoints_keep_the_original_role_and_argument_validation() {
        use spaceprune_access::helper::HelperKind;
        for (program, kind, legacy) in [
            (
                env!("CARGO_BIN_EXE_spaceprune-readonly-helper"),
                HelperKind::Metadata,
                "--spaceprune-readonly-session",
            ),
            (
                env!("CARGO_BIN_EXE_spaceprune-cleanup-helper"),
                HelperKind::Cleanup,
                "--spaceprune-cleanup-session",
            ),
        ] {
            let run = |arguments: &[&str]| {
                Command::new(program)
                    .args(arguments)
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .status()
                    .unwrap()
                    .code()
            };
            assert_eq!(run(&[spaceprune_access::helper::PROMPT_SUPPORT]), Some(0));
            let original = run(&[legacy]);
            assert_ne!(
                original,
                Some(64),
                "Legacy entry must dispatch the capability"
            );
            for language in ["en", "zh-CN"] {
                let localized = format!("{legacy}-{language}");
                assert_eq!(run(&[&localized]), original);
                assert!(!matches!(run(&[&localized, "extra"]), Some(0)));
                let other = match kind {
                    HelperKind::Metadata => "--spaceprune-cleanup-session",
                    HelperKind::Cleanup => "--spaceprune-readonly-session",
                };
                assert_eq!(run(&[&format!("{other}-{language}")]), Some(64));
            }
        }
    }

    #[test]
    fn native_components_support_the_current_builtin_handler_contract() {
        for program in [
            env!("CARGO_BIN_EXE_spaceprune-readonly-helper"),
            env!("CARGO_BIN_EXE_spaceprune-cleanup-helper"),
            env!("CARGO_BIN_EXE_spaceprune-helper-installer"),
        ] {
            for probe in [
                spaceprune_access::helper::HANDLERS_SUPPORT,
                spaceprune_access::helper::DEVELOPER_HANDLERS_SUPPORT,
            ] {
                assert!(
                    Command::new(program)
                        .arg(probe)
                        .stdin(Stdio::null())
                        .status()
                        .unwrap()
                        .success()
                );
                // A capability probe must not accept extra operation arguments.
                assert_eq!(
                    Command::new(program)
                        .args([probe, "--spaceprune-cleanup-session"])
                        .stdin(Stdio::null())
                        .status()
                        .unwrap()
                        .code(),
                    Some(64)
                );
            }
        }
    }
}

#[cfg(target_os = "windows")]
#[test]
fn windows_login_bound_helpers_reject_account_bound_bootstrap_probes() {
    use std::process::{Command, Stdio};
    let programs = [
        (
            env!("CARGO_BIN_EXE_spaceprune-readonly-helper"),
            "--supports-windows-metadata-v7",
        ),
        (
            env!("CARGO_BIN_EXE_spaceprune-cleanup-helper"),
            "--supports-cleanup-v5",
        ),
    ];
    for (program, own_probe) in programs {
        for probe in [
            "--supports-windows-metadata-v6",
            "--supports-cleanup-v4",
            "--supports-scope-v6",
            "--supports-windows-metadata-v7",
            "--supports-cleanup-v5",
        ] {
            let status = Command::new(program)
                .arg(probe)
                .stdin(Stdio::null())
                .status()
                .unwrap();
            assert_eq!(
                status.code(),
                Some(if probe == own_probe { 0 } else { 64 }),
                "{program}: {probe}"
            );
        }
    }
}
