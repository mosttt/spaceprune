//! Resolve persistent root expressions without inventing paths or changing task scope.

use crate::{ConfigError, RootSpec, TaskDefinition};
use spaceprune_contracts::messages;
use std::path::{Component, PathBuf};

pub(super) fn resolve_task_roots(
    task: &mut TaskDefinition,
    source: &str,
) -> Result<(), ConfigError> {
    if task.handler != "builtin.filesystem/paths" {
        return Ok(());
    }
    let Some(roots) = task
        .options
        .get_mut("roots")
        .and_then(toml::Value::as_array_mut)
    else {
        return Ok(());
    };
    for root in roots {
        let Some(value) = root.get_mut("path") else {
            continue;
        };
        let spec: RootSpec = value.clone().try_into().map_err(|error| {
            invalid(
                source,
                messages::message("config.invalid_root", &[("error", error.to_string())]),
            )
        })?;
        let path = match spec {
            RootSpec::Expression(path) => path,
            RootSpec::UserHome { user_home } => {
                let mut path = spaceprune_access::host::user_home()
                    .map_err(|error| invalid(source, error.to_string()))?;
                for part in user_home {
                    if part.is_empty()
                        || part.contains(['/', '\\'])
                        || !matches!(
                            std::path::Path::new(&part).components().next(),
                            Some(Component::Normal(_))
                        )
                    {
                        return Err(invalid(source, messages::text("config.home_components")));
                    }
                    path.push(part);
                }
                path.into_os_string()
                    .into_string()
                    .map_err(|_| invalid(source, messages::text("config.home_encoding")))?
            }
            RootSpec::LocalAbsolute {
                local_absolute,
                host_id,
            } => {
                if !PathBuf::from(&local_absolute).is_absolute() {
                    return Err(invalid(source, messages::text("config.absolute_root")));
                }
                if spaceprune_access::host::identity()
                    .map_err(|error| invalid(source, error.to_string()))?
                    != host_id
                {
                    return Err(invalid(source, messages::text("config.root_other_host")));
                }
                local_absolute
            }
        };
        *value = toml::Value::String(path);
    }
    Ok(())
}

fn invalid(source: &str, message: impl Into<String>) -> ConfigError {
    ConfigError::Invalid {
        source_name: source.to_owned(),
        message: message.into(),
    }
}
