//! Product wrappers translate while native command output and caller paths remain literal.

use spaceprune_contracts::messages::set_display_language;
use spaceprune_platform::PlatformError;

#[test]
fn platform_errors_preserve_external_output_and_user_values_in_both_languages() {
    for (language, command_prefix, source_prefix, recovery_text) in [
        (
            "en",
            "Platform command",
            "Automation installation requires",
            "restoring the previous schedule also failed",
        ),
        ("zh-CN", "平台命令", "安装自动化需要", "恢复先前调度也失败"),
    ] {
        set_display_language(language);
        let raw = "RAW {message}: access denied";
        let command = "fixture --argument={command}";
        let operation = PlatformError::Command {
            command: command.into(),
            message: raw.into(),
        };
        let error = operation.to_string();
        assert!(error.starts_with(command_prefix));
        assert!(error.contains(raw));
        assert!(error.contains(command));
        let source = "/fixture/{source}.toml";
        let error = PlatformError::ApplicationConfigMissing(source.into()).to_string();
        assert!(error.starts_with(source_prefix));
        assert!(error.contains(source));
        let error = PlatformError::Recovery {
            operation: Box::new(operation),
            recovery: Box::new(PlatformError::Backend("RAW {recovery}".into())),
            safety_stop: "RAW {safety_stop}".into(),
        }
        .to_string();
        assert!(error.contains(recovery_text));
        for literal in [raw, command, "RAW {recovery}", "RAW {safety_stop}"] {
            assert!(error.contains(literal));
        }
    }
    set_display_language("en");
}
