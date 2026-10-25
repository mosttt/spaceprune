use super::{plist, state::disabled_state, xml_escape};

#[test]
fn escapes_plist_values() {
    assert_eq!(xml_escape("a&<\"'"), "a&amp;&lt;&quot;&apos;");
    let content = plist(
        "io.spaceprune.daily",
        &["/Applications/A B".to_owned()],
        3,
        5,
    );
    assert!(content.contains("<integer>3</integer>"));
    assert!(content.contains("<string>/Applications/A B</string>"));
}

#[test]
fn reads_disabled_state_and_rejects_unknown_formats() {
    let label = "io.spaceprune.daily";
    assert!(!disabled_state("disabled services = {\n}", label).unwrap());
    assert!(
        disabled_state(
            "disabled services = {\n\"io.spaceprune.daily\" => true\n}",
            label
        )
        .unwrap()
    );
    assert!(
        !disabled_state(
            "disabled services = {\n\"io.spaceprune.daily-other\" => true\n}",
            label
        )
        .unwrap()
    );
    for value in [
        "",
        "unknown format",
        "disabled services = {",
        "disabled services = {\n\"io.spaceprune.daily\" => maybe\n}",
        "disabled services = {\n\"io.spaceprune.daily\" => true\n\"io.spaceprune.daily\" => false\n}",
    ] {
        assert!(disabled_state(value, label).is_err(), "{value}");
    }
}

#[cfg(unix)]
#[test]
fn launchd_failures_restore_definition_loaded_and_disabled_state() {
    use super::{install, installed, installed_checked, uninstall};
    use crate::scheduler_test_support::{Fixture, config, run_isolated};
    use std::fs;
    if run_isolated("macos::tests::launchd_failures_restore_definition_loaded_and_disabled_state") {
        return;
    }
    let config = config();
    let runner = std::env::current_exe().unwrap();
    let old = b"old launch agent definition";

    for command in ["bootout", "enable", "bootstrap"] {
        for mode in ["before", "after"] {
            let fixture = Fixture::reset();
            fs::write(&fixture.definition, old).unwrap();
            fs::write(fixture.state.join("loaded"), old).unwrap();
            fixture.fail(command, 1, mode);
            assert!(install(&config, "daily", &runner).is_err());
            assert_eq!(fs::read(&fixture.definition).unwrap(), old);
            assert_eq!(fs::read(fixture.state.join("loaded")).unwrap(), old);
            assert!(!fixture.state.join("disabled").exists());
        }
    }

    for (loaded, disabled) in [(true, false), (false, true), (true, true)] {
        let fixture = Fixture::reset();
        fs::write(&fixture.definition, old).unwrap();
        if loaded {
            fs::write(fixture.state.join("loaded"), old).unwrap();
        }
        if disabled {
            fs::write(fixture.state.join("disabled"), "").unwrap();
        }
        fixture.fail("bootstrap", 1, "after");
        assert!(install(&config, "daily", &runner).is_err());
        assert_eq!(fs::read(&fixture.definition).unwrap(), old);
        assert_eq!(fixture.state.join("loaded").exists(), loaded);
        if loaded {
            assert_eq!(fs::read(fixture.state.join("loaded")).unwrap(), old);
        }
        assert_eq!(fixture.state.join("disabled").exists(), disabled);
        assert!(installed("daily"));
        assert!(installed_checked("daily").unwrap());
    }

    let fixture = Fixture::reset();
    fixture.fail("bootstrap", 1, "after");
    assert!(install(&config, "daily", &runner).is_err());
    assert!(!fixture.definition.exists());
    assert!(!fixture.state.join("loaded").exists());
    assert!(!fixture.state.join("disabled").exists());
    assert!(!installed_checked("daily").unwrap());

    // A command can mutate scheduler state and still report failure.
    let fixture = Fixture::reset();
    fs::write(&fixture.definition, old).unwrap();
    fs::write(fixture.state.join("loaded"), old).unwrap();
    fixture.fail("bootout", 1, "after");
    assert!(uninstall("daily").is_err());
    assert_eq!(fs::read(&fixture.definition).unwrap(), old);
    assert_eq!(fs::read(fixture.state.join("loaded")).unwrap(), old);

    // A second failure must retain both errors and stop the partial schedule.
    let fixture = Fixture::reset();
    fs::write(&fixture.definition, old).unwrap();
    fs::write(fixture.state.join("loaded"), old).unwrap();
    fixture.fail("bootstrap", 1, "after");
    fixture.fail("bootstrap", 2, "after");
    let error = install(&config, "daily", &runner).unwrap_err().to_string();
    assert!(error.contains("bootstrap failure 1"), "{error}");
    assert!(error.contains("bootstrap failure 2"), "{error}");
    assert!(error.contains("disabled and unloaded"), "{error}");
    assert_eq!(fs::read(&fixture.definition).unwrap(), old);
    assert!(!fixture.state.join("loaded").exists());
    assert!(fixture.state.join("disabled").exists());

    // A persistent scheduler failure must not leave the new plist on disk.
    let fixture = Fixture::reset();
    fs::write(&fixture.definition, old).unwrap();
    fs::write(fixture.state.join("loaded"), old).unwrap();
    fixture.fail("bootout", 1, "before");
    fixture.fail("bootout", 2, "before");
    fixture.fail("bootout", 3, "before");
    let error = install(&config, "daily", &runner).unwrap_err().to_string();
    assert!(error.contains("bootout failure 1"), "{error}");
    assert!(error.contains("bootout failure 2"), "{error}");
    assert!(error.contains("bootout failure 3"), "{error}");
    assert_eq!(fs::read(&fixture.definition).unwrap(), old);
    assert!(fixture.state.join("disabled").exists());

    let fixture = Fixture::reset();
    fs::write(&fixture.definition, old).unwrap();
    fs::write(fixture.state.join("loaded"), old).unwrap();
    fs::write(fixture.state.join("replace-definition-with-directory"), "").unwrap();
    let error = uninstall("daily").unwrap_err().to_string();
    assert!(
        error.contains("restoring the previous schedule also failed"),
        "{error}"
    );
    assert!(!fixture.state.join("loaded").exists());
    assert!(fixture.state.join("disabled").exists());

    // Unknown query outcomes must not change the file or the loaded service.
    for flag in ["query-error", "malformed-disabled", "unavailable-domain"] {
        let fixture = Fixture::reset();
        fs::write(&fixture.definition, old).unwrap();
        if flag == "query-error" {
            fixture.fail("print", 1, "before");
        } else {
            fs::write(fixture.state.join(flag), "").unwrap();
        }
        assert!(install(&config, "daily", &runner).is_err());
        assert_eq!(fs::read(&fixture.definition).unwrap(), old);
        assert!(!fixture.trace().contains("bootstrap"));
        assert!(!fixture.trace().contains("bootout"));
    }

    // A missing plist must not prevent removal of an orphaned loaded service.
    let fixture = Fixture::reset();
    fs::write(fixture.state.join("loaded"), old).unwrap();
    assert!(installed_checked("daily").unwrap());
    uninstall("daily").unwrap();
    assert!(!fixture.state.join("loaded").exists());

    let fixture = Fixture::reset();
    install(&config, "daily", &runner).unwrap();
    let current = fs::read(&fixture.definition).unwrap();
    assert_eq!(fs::read(fixture.state.join("loaded")).unwrap(), current);
    uninstall("daily").unwrap();
    assert!(!fixture.definition.exists());
    assert!(!fixture.state.join("loaded").exists());
}
