//! Query Snapper's native access decision without parsing translated command diagnostics.

use super::*;
use futures_lite::future::{block_on, race};
use spaceprune_access::{IoFailure, IoFailureKind};
use zbus::{Connection, connection::Builder};

const ADDRESS: &str = "unix:path=/run/dbus/system_bus_socket";
const SERVICE: &str = "org.opensuse.Snapper";
const OBJECT: &str = "/org/opensuse/Snapper";

pub(super) fn is_system_snapper(program: &str) -> bool {
    matches!(
        program,
        "snapper" | "/usr/bin/snapper" | "/usr/sbin/snapper" | "/bin/snapper" | "/sbin/snapper"
    )
}

pub(super) fn inspect_access(config: Option<&str>, context: &OperationContext) -> PluginResult<()> {
    inspect_at(ADDRESS, config, context)
}

fn inspect_at(address: &str, config: Option<&str>, context: &OperationContext) -> PluginResult<()> {
    let context = context.child(Duration::from_secs(5));
    context.check().map_err(native_error)?;
    block_on(race(
        async {
            // Fixed system bus, never the caller's DBUS_SYSTEM_BUS_ADDRESS. Drive the connection
            // in this operation so a cancellation does not leave a detached probe thread.
            let connection = Builder::address(address)
                .map_err(service_error)?
                .internal_executor(false)
                .max_queued(1)
                .build()
                .await
                .map_err(service_error)?;
            let result = race(query(&connection, config, &context), async {
                loop {
                    connection.executor().tick().await;
                }
            })
            .await;
            let _ = connection.close().await;
            result
        },
        async {
            loop {
                context.check().map_err(native_error)?;
                async_io::Timer::after(Duration::from_millis(20)).await;
            }
        },
    ))
}

async fn query(
    connection: &Connection,
    config: Option<&str>,
    context: &OperationContext,
) -> PluginResult<()> {
    // GetConfig and ListSnapshots enforce the same per-configuration permission in snapperd.
    // This checks access without requesting a second potentially large snapshot inventory.
    let response = match config {
        Some(config) => {
            connection
                .call_method(
                    Some(SERVICE),
                    OBJECT,
                    Some(SERVICE),
                    "GetConfig",
                    &(config,),
                )
                .await
        }
        None => {
            connection
                .call_method(Some(SERVICE), OBJECT, Some(SERVICE), "ListConfigs", &())
                .await
        }
    }
    .map_err(service_error)?;
    context.check().map_err(native_error)?;
    if response.body().len() > context.budget.output_bytes {
        return Err(PluginError::new(
            "snapper-service-response-too-large",
            spaceprune_plugin_api::messages::text("snapper.access_budget"),
        ));
    }
    Ok(())
}

fn native_error(error: std::io::Error) -> PluginError {
    let mut result = PluginError::new(
        "snapper-service-unavailable",
        spaceprune_plugin_api::messages::message(
            "snapper.access_check_failed",
            &[("error", (error).to_string())],
        ),
    );
    result.io_failure = Some(spaceprune_access::error::classify(&error));
    result
}

fn service_error(error: zbus::Error) -> PluginError {
    let mut result = PluginError::new(
        "snapper-service-failed",
        spaceprune_plugin_api::messages::message(
            "snapper.service_access_failed",
            &[("error", (error).to_string())],
        ),
    );
    match error {
        zbus::Error::MethodError(name, _, _)
            if matches!(
                name.as_str(),
                "error.no_permissions"
                    | "org.freedesktop.DBus.Error.AccessDenied"
                    | "org.freedesktop.DBus.Error.AuthFailed"
            ) =>
        {
            result.code = "snapper-permission-required".into();
            result.io_failure = Some(IoFailure {
                kind: IoFailureKind::AccessDenied,
                os_code: None,
            });
        }
        zbus::Error::InputOutput(error) | zbus::Error::Connection(error, _) => {
            result.io_failure = Some(spaceprune_access::error::classify(&error));
        }
        _ => {}
    }
    result
}

#[cfg(test)]
mod tests;
