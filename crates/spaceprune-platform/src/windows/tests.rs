use super::{query_result, windows_command_line, windows_quote};
use crate::PlatformError;

#[test]
fn quotes_windows_command_line() {
    assert_eq!(windows_quote("plain"), "plain");
    assert_eq!(windows_quote("a b"), "\"a b\"");
    assert_eq!(
        windows_command_line(&["C:\\A B\\spaceprune.exe".to_owned(), "run".to_owned()]),
        "\"C:\\A B\\spaceprune.exe\" run"
    );
}

#[test]
fn only_missing_task_hresult_is_absence() {
    let error = |_: &[u8]| PlatformError::Backend("query failed".into());
    for code in [0x8007_0002_u32, 0x8007_0003] {
        assert!(
            query_result(Some(code as i32), vec![], error)
                .unwrap()
                .is_none()
        );
    }
    for code in [
        None,
        Some(2),
        Some(0x8007_0005_u32 as i32),
        Some(0x8007_06ba_u32 as i32),
    ] {
        assert!(query_result(code, vec![], error).is_err());
    }
    assert!(query_result(Some(0), vec![], error).is_err());
    let xml = vec![0xff, 0xfe, b'<', 0, b'T', 0];
    assert_eq!(
        query_result(Some(0), xml.clone(), error).unwrap(),
        Some(xml)
    );
}

#[cfg(unix)]
#[test]
fn task_scheduler_failures_restore_xml_or_disable_partial_task() {
    use super::{install, installed, installed_checked, mutate, run_schtasks, uninstall};
    use crate::scheduler_test_support::{Fixture, config, run_isolated};
    use std::fs;
    if run_isolated("windows::tests::task_scheduler_failures_restore_xml_or_disable_partial_task") {
        return;
    }
    let config = config();
    let runner = std::env::current_exe().unwrap();
    let task = r"\SpacePrune\daily";
    let old = b"<Task><Enabled>false</Enabled><Schedule>old</Schedule></Task>";

    let fixture = Fixture::reset();
    fs::write(fixture.state.join("task.xml"), old).unwrap();
    fixture.fail("create", 1, "after");
    assert!(install(&config, "daily", &runner).is_err());
    assert_eq!(fs::read(fixture.state.join("task.xml")).unwrap(), old);
    assert!(fixture.state.join("disabled").exists());
    assert!(installed("daily"));
    assert!(installed_checked("daily").unwrap());

    let fixture = Fixture::reset();
    fixture.fail("create", 1, "after");
    assert!(mutate(task, None, || run_schtasks(&["/Create", "/TN", task])).is_err());
    assert!(!fixture.state.join("task.xml").exists());
    assert!(fixture.trace().contains("/Delete"));

    let fixture = Fixture::reset();
    fs::write(fixture.state.join("task.xml"), old).unwrap();
    fixture.fail("create", 1, "after");
    fixture.fail("create", 2, "before");
    let error = install(&config, "daily", &runner).unwrap_err().to_string();
    assert!(error.contains("create failure 1"), "{error}");
    assert!(error.contains("create failure 2"), "{error}");
    assert!(error.contains("automatic scheduling disabled"), "{error}");
    assert!(fixture.state.join("disabled").exists());

    let fixture = Fixture::reset();
    fs::write(fixture.state.join("task.xml"), old).unwrap();
    fixture.fail("create", 1, "after");
    fixture.fail("create", 2, "before");
    fixture.fail("change", 1, "before");
    let error = install(&config, "daily", &runner).unwrap_err().to_string();
    assert!(
        error.contains("could not disable automatic scheduling"),
        "{error}"
    );
    assert!(error.contains("change failure 1"), "{error}");

    let fixture = Fixture::reset();
    fs::write(fixture.state.join("task.xml"), old).unwrap();
    fixture.fail("delete", 1, "before");
    assert!(uninstall("daily").is_err());
    assert_eq!(fs::read(fixture.state.join("task.xml")).unwrap(), old);
    assert!(fixture.state.join("disabled").exists());

    for action in ["install", "uninstall", "checked"] {
        let fixture = Fixture::reset();
        fs::write(fixture.state.join("task.xml"), old).unwrap();
        fixture.fail("query", 1, "before");
        let result = match action {
            "install" => install(&config, "daily", &runner).map(|_| ()),
            "uninstall" => uninstall("daily").map(|_| ()),
            _ => installed_checked("daily").map(|_| ()),
        };
        assert!(result.is_err());
        assert_eq!(fs::read(fixture.state.join("task.xml")).unwrap(), old);
        assert!(!fixture.trace().contains("/Create"));
        assert!(!fixture.trace().contains("/Delete"));
    }

    let fixture = Fixture::reset();
    fs::write(fixture.state.join("task.xml"), old).unwrap();
    install(&config, "daily", &runner).unwrap();
    assert_ne!(fs::read(fixture.state.join("task.xml")).unwrap(), old);
    uninstall("daily").unwrap();
    assert!(!fixture.state.join("task.xml").exists());
}
