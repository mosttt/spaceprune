//! Preserve both the persisted unit definitions and their effective activation state.

use super::{
    AutomationUnits, PlatformError, disable_timer, remove_if_exists, systemctl, systemctl_output,
};
use spaceprune_plugin_api::messages;
use std::{fs, path::Path};

pub(super) struct PreviousInstallation {
    service: Option<Vec<u8>>,
    timer: Option<Vec<u8>>,
    enabled: bool,
    runtime_enabled: bool,
    pub(super) active: bool,
    pub(super) loaded: bool,
}

impl PreviousInstallation {
    pub(super) fn capture(units: &AutomationUnits, id: &str) -> Result<Self, PlatformError> {
        let service = read_optional(&units.service)?;
        let timer = read_optional(&units.timer)?;
        let unit = format!("spaceprune-{id}.timer");
        let output = systemctl_output([
            "show",
            &unit,
            "--property=LoadState",
            "--property=ActiveState",
            "--property=UnitFileState",
            "--no-pager",
        ])?;
        let property = |name: &str| {
            output
                .lines()
                .find_map(|line| {
                    line.split_once('=')
                        .filter(|(key, _)| *key == name)
                        .map(|(_, value)| value)
                })
                .ok_or_else(|| {
                    invalid_state(
                        &unit,
                        &messages::message("linux.missing_property", &[("name", name.to_owned())]),
                    )
                })
        };
        let active = match property("ActiveState")? {
            "active" => true,
            "inactive" | "failed" => false,
            state => {
                return Err(invalid_state(
                    &unit,
                    &messages::message("linux.timer_state_busy", &[("state", state.to_owned())]),
                ));
            }
        };
        let unit_file_state = property("UnitFileState")?;
        let (enabled, runtime_enabled) = match unit_file_state {
            "enabled" => (true, false),
            "enabled-runtime" => (true, true),
            "" | "disabled" | "static" | "indirect" | "generated" | "transient" => (false, false),
            state => {
                return Err(invalid_state(
                    &unit,
                    &messages::message(
                        "linux.unit_state_unpreserved",
                        &[("state", state.to_owned())],
                    ),
                ));
            }
        };
        let loaded = match property("LoadState")? {
            "loaded" | "bad-setting" | "error" => true,
            "not-found" => false,
            state => {
                return Err(invalid_state(
                    &unit,
                    &messages::message(
                        "linux.load_state_unpreserved",
                        &[("state", state.to_owned())],
                    ),
                ));
            }
        };
        Ok(Self {
            service,
            timer,
            enabled,
            runtime_enabled,
            active,
            loaded,
        })
    }

    pub(super) fn recover(
        self,
        operation: PlatformError,
        units: &AutomationUnits,
        id: &str,
        published: bool,
    ) -> PlatformError {
        let unit = format!("spaceprune-{id}.timer");
        let mut failures = Vec::new();
        let mut record = |result: Result<(), PlatformError>| {
            if let Err(error) = result {
                failures.push(error.to_string());
            }
        };
        if published {
            match timer_active(&unit) {
                Ok(true) => record(systemctl(["stop", &unit])),
                Ok(false) => {}
                Err(error) => {
                    record(Err(error));
                    record(systemctl(["stop", &unit]));
                }
            }
            if self.timer.is_none() {
                record(disable_timer(&unit, false));
            }
        }
        record(restore_file(&units.service, self.service.as_deref()));
        record(restore_file(&units.timer, self.timer.as_deref()));
        if published || self.loaded {
            record(systemctl(["daemon-reload"]));
        }
        if self.timer.is_some() {
            record(if self.enabled {
                if self.runtime_enabled {
                    disable_timer(&unit, false)
                        .and_then(|()| systemctl(["enable", "--runtime", &unit]))
                } else {
                    systemctl(["enable", &unit])
                }
            } else {
                disable_timer(&unit, false)
            });
        }
        // Never reactivate an old schedule against definitions we failed to restore.
        if failures.is_empty()
            && self.active
            && let Err(error) = systemctl(["restart", &unit])
        {
            failures.push(error.to_string());
        }
        if failures.is_empty() {
            operation
        } else {
            let safety_stop = match disable_timer(&unit, true) {
                Ok(()) => messages::text("linux.scheduling_disabled").to_owned(),
                Err(error) => {
                    messages::message("linux.disable_failed", &[("error", error.to_string())])
                }
            };
            PlatformError::RecoveryFailed {
                operation: Box::new(operation),
                recovery: format!("{}; {safety_stop}", failures.join("; ")),
            }
        }
    }
}

fn invalid_state(unit: &str, message: &str) -> PlatformError {
    PlatformError::Systemctl {
        action: format!("show {unit}"),
        message: message.to_owned(),
    }
}

fn read_optional(path: &Path) -> Result<Option<Vec<u8>>, PlatformError> {
    match fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(source) => Err(PlatformError::Io {
            path: path.to_owned(),
            source,
        }),
    }
}

fn restore_file(path: &Path, bytes: Option<&[u8]>) -> Result<(), PlatformError> {
    match bytes {
        Some(bytes) => fs::write(path, bytes).map_err(|source| PlatformError::Io {
            path: path.to_owned(),
            source,
        }),
        None => remove_if_exists(path),
    }
}

fn timer_active(unit: &str) -> Result<bool, PlatformError> {
    let output = systemctl_output(["show", unit, "--property=ActiveState", "--no-pager"])?;
    match output
        .lines()
        .find_map(|line| line.strip_prefix("ActiveState="))
    {
        Some("active") => Ok(true),
        Some("inactive" | "failed") => Ok(false),
        _ => Err(invalid_state(
            unit,
            messages::text("linux.timer_state_unknown"),
        )),
    }
}
