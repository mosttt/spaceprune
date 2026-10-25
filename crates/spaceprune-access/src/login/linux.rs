//! logind's system-bus identity and signals, independent of caller-controlled environment hints.

use super::{LoginRevocation, NativeBinding};
use crate::{Interaction, OperationContext, PermissionOutcome};
use futures_lite::{
    StreamExt,
    future::{block_on, or},
};
use std::{
    collections::HashMap,
    os::unix::fs::{FileTypeExt, MetadataExt},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread::JoinHandle,
    time::{Duration, Instant},
};
use zbus::{
    MatchRule, Message,
    blocking::{Connection, MessageIterator, connection::Builder},
    zvariant::{OwnedObjectPath, OwnedValue},
};

const BUS_SOCKET: &str = "/run/dbus/system_bus_socket";
const LOGIN: &str = "org.freedesktop.login1";
const MANAGER_PATH: &str = "/org/freedesktop/login1";
const MANAGER: &str = "org.freedesktop.login1.Manager";
const SESSION: &str = "org.freedesktop.login1.Session";
const USER: &str = "org.freedesktop.login1.User";
const PROPERTIES: &str = "org.freedesktop.DBus.Properties";
const RECHECK_INTERVAL: Duration = Duration::from_secs(1);
type Properties = HashMap<String, OwnedValue>;

struct Session {
    id: String,
    path: OwnedObjectPath,
    uid: u32,
    started: u64,
    seat: String,
    active: bool,
    state: String,
    display_user: Option<OwnedObjectPath>,
    require_active: bool,
}

struct Monitor {
    connection: Connection,
    stopping: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
}
impl Drop for Monitor {
    fn drop(&mut self) {
        self.stopping.store(true, Ordering::Release);
        let _ = self.connection.clone().close();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

pub(super) fn bind(
    operation: &OperationContext,
    revocation: LoginRevocation,
) -> Result<NativeBinding, PermissionOutcome> {
    let uid = rustix::process::geteuid().as_raw();
    let pid = rustix::process::getpid().as_raw_pid() as u32;
    let Some(address) = system_bus()? else {
        return Ok(process_binding(uid, pid));
    };
    let timeout = operation
        .deadline
        .saturating_duration_since(Instant::now())
        .min(Duration::from_secs(2));
    if timeout.is_zero() {
        return Err(PermissionOutcome::TimedOut);
    }
    let connection = Builder::address(address.as_str())
        .map_err(unavailable)?
        .method_timeout(timeout)
        .build()
        .map_err(unavailable)?;
    bind_connection(connection, uid, pid, operation.interaction, revocation)
}

fn system_bus() -> Result<Option<String>, PermissionOutcome> {
    let socket = match std::fs::symlink_metadata(BUS_SOCKET) {
        Ok(socket) => socket,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(unavailable(error)),
    };
    if !socket.file_type().is_socket() || socket.uid() != 0 {
        return Err(PermissionOutcome::AuthorizationUnavailable);
    }
    for directory in ["/", "/run", "/run/dbus"] {
        let metadata = std::fs::symlink_metadata(directory).map_err(unavailable)?;
        if !metadata.is_dir() || metadata.uid() != 0 || metadata.mode() & 0o022 != 0 {
            return Err(PermissionOutcome::AuthorizationUnavailable);
        }
    }
    Ok(Some(format!("unix:path={BUS_SOCKET}")))
}

fn process_binding(uid: u32, pid: u32) -> NativeBinding {
    let sid = rustix::process::getsid(None)
        .map(|sid| sid.as_raw_pid())
        .unwrap_or_default();
    NativeBinding {
        subject: format!("linux:uid:{uid}:process:{pid}:sid:{sid}"),
        login_bound: false,
        _monitor: Box::new(()),
    }
}

fn unavailable(_: impl std::fmt::Debug) -> PermissionOutcome {
    PermissionOutcome::AuthorizationUnavailable
}
fn named_error(error: &zbus::Error, name: &str) -> bool {
    matches!(error, zbus::Error::MethodError(actual, _, _) if actual.as_str() == name)
}
fn properties(
    connection: &Connection,
    owner: &str,
    path: &str,
    interface: &str,
) -> Result<Properties, PermissionOutcome> {
    connection
        .call_method(Some(owner), path, Some(PROPERTIES), "GetAll", &(interface,))
        .map_err(unavailable)?
        .body()
        .deserialize()
        .map_err(unavailable)
}
fn take<T: TryFrom<OwnedValue>>(
    values: &mut Properties,
    name: &str,
) -> Result<T, PermissionOutcome> {
    values
        .remove(name)
        .ok_or(PermissionOutcome::AuthorizationUnavailable)?
        .try_into()
        .map_err(|_| PermissionOutcome::AuthorizationUnavailable)
}

fn discover(
    connection: &Connection,
    owner: &str,
    uid: u32,
    pid: u32,
    interaction: Interaction,
) -> Result<Option<Session>, PermissionOutcome> {
    let direct = connection.call_method(
        Some(owner),
        MANAGER_PATH,
        Some(MANAGER),
        "GetSessionByPID",
        &(pid,),
    );
    let (path, display_user): (OwnedObjectPath, _) = match direct {
        Ok(reply) => (reply.body().deserialize().map_err(unavailable)?, None),
        Err(error) if named_error(&error, "org.freedesktop.login1.NoSessionForPID") => {
            if interaction == Interaction::NonInteractive {
                return Ok(None);
            }
            let user = match connection.call_method(
                Some(owner),
                MANAGER_PATH,
                Some(MANAGER),
                "GetUser",
                &(uid,),
            ) {
                Ok(reply) => reply
                    .body()
                    .deserialize::<OwnedObjectPath>()
                    .map_err(unavailable)?,
                Err(error) if named_error(&error, "org.freedesktop.login1.NoSuchUser") => {
                    return Ok(None);
                }
                Err(error) => return Err(unavailable(error)),
            };
            let mut values = properties(connection, owner, user.as_str(), USER)?;
            let actual: u32 = take(&mut values, "UID")?;
            if actual != uid {
                return Err(PermissionOutcome::PolicyBlocked);
            }
            let (id, path): (String, OwnedObjectPath) = take(&mut values, "Display")?;
            if id.is_empty() {
                return Ok(None);
            }
            (path, Some(user))
        }
        Err(error) => return Err(unavailable(error)),
    };
    let mut values = properties(connection, owner, path.as_str(), SESSION)?;
    let id = take(&mut values, "Id")?;
    let (actual, _): (u32, OwnedObjectPath) = take(&mut values, "User")?;
    if actual != uid {
        return Err(PermissionOutcome::PolicyBlocked);
    }
    let (seat, _): (String, OwnedObjectPath) = take(&mut values, "Seat")?;
    let session = Session {
        id,
        path,
        uid,
        seat,
        started: take(&mut values, "TimestampMonotonic")?,
        active: take(&mut values, "Active")?,
        state: take(&mut values, "State")?,
        display_user,
        require_active: interaction != Interaction::NonInteractive,
    };
    if !session_usable(&session.state, session.active, &session) {
        return Err(PermissionOutcome::AuthorizationUnavailable);
    }
    Ok(Some(session))
}

fn session_usable(state: &str, active: bool, session: &Session) -> bool {
    state != "closing" && (!session.require_active || session.seat.is_empty() || active)
}

fn recheck(
    connection: &Connection,
    owner: &str,
    session: &Session,
) -> Result<bool, PermissionOutcome> {
    let mut values = properties(connection, owner, session.path.as_str(), SESSION)?;
    let id: String = take(&mut values, "Id")?;
    let (uid, _): (u32, OwnedObjectPath) = take(&mut values, "User")?;
    let started: u64 = take(&mut values, "TimestampMonotonic")?;
    let state: String = take(&mut values, "State")?;
    let active: bool = take(&mut values, "Active")?;
    Ok(id == session.id
        && uid == session.uid
        && started == session.started
        && session_usable(&state, active, session))
}

fn bind_connection(
    connection: Connection,
    uid: u32,
    pid: u32,
    interaction: Interaction,
    revocation: LoginRevocation,
) -> Result<NativeBinding, PermissionOutcome> {
    let owner: String = match connection.call_method(
        Some("org.freedesktop.DBus"),
        "/org/freedesktop/DBus",
        Some("org.freedesktop.DBus"),
        "GetNameOwner",
        &(LOGIN,),
    ) {
        Ok(reply) => reply.body().deserialize().map_err(unavailable)?,
        Err(error) if named_error(&error, "org.freedesktop.DBus.Error.NameHasNoOwner") => {
            return Ok(process_binding(uid, pid));
        }
        Err(error) => return Err(unavailable(error)),
    };
    // Subscribe before reading the initial snapshot so a quick switch away and back remains visible.
    let signals = MessageIterator::for_match_rule(
        MatchRule::builder()
            .msg_type(zbus::message::Type::Signal)
            .sender(owner.as_str())
            .map_err(unavailable)?
            .path_namespace(MANAGER_PATH)
            .map_err(unavailable)?
            .build(),
        &connection,
        Some(256),
    )
    .map_err(unavailable)?
    .into_inner();
    let owners = MessageIterator::for_match_rule(
        MatchRule::builder()
            .msg_type(zbus::message::Type::Signal)
            .sender("org.freedesktop.DBus")
            .map_err(unavailable)?
            .interface("org.freedesktop.DBus")
            .map_err(unavailable)?
            .member("NameOwnerChanged")
            .map_err(unavailable)?
            .add_arg(LOGIN)
            .map_err(unavailable)?
            .build(),
        &connection,
        Some(8),
    )
    .map_err(unavailable)?
    .into_inner();
    let Some(session) = discover(&connection, &owner, uid, pid, interaction)? else {
        return Ok(process_binding(uid, pid));
    };
    let subject = format!(
        "linux:uid:{uid}:login:{}:started:{}",
        session.id, session.started
    );
    let stopping = Arc::new(AtomicBool::new(false));
    let stopped = Arc::clone(&stopping);
    let monitor_connection = connection.clone();
    let worker = std::thread::spawn(move || {
        let mut messages = signals.or(owners);
        let mut recheck_at = Instant::now() + RECHECK_INTERVAL;
        loop {
            if stopped.load(Ordering::Acquire) || revocation.is_revoked() {
                return;
            }
            if Instant::now() >= recheck_at {
                // logind can retain State=closing without emitting a property edge. Keep the
                // event subscription for quick switches, and bound this quiet-state recheck
                // with the connection's existing method timeout.
                let valid = recheck(&monitor_connection, &owner, &session);
                if stopped.load(Ordering::Acquire) {
                    return;
                }
                if !valid.unwrap_or(false) {
                    revocation.invalidate();
                    return;
                }
                recheck_at = Instant::now() + RECHECK_INTERVAL;
            }
            let message = block_on(or(async { Some(messages.next().await) }, async {
                async_io::Timer::at(recheck_at).await;
                None
            }));
            if stopped.load(Ordering::Acquire) {
                return;
            }
            let Some(message) = message else {
                continue;
            };
            if message.as_ref().is_none_or(|message| {
                message
                    .as_ref()
                    .map_or(true, |message| invalidates(message, &session))
            }) {
                revocation.invalidate();
                return;
            }
        }
    });
    Ok(NativeBinding {
        subject,
        login_bound: true,
        _monitor: Box::new(Monitor {
            connection,
            stopping,
            worker: Some(worker),
        }),
    })
}

fn invalidates(message: &Message, session: &Session) -> bool {
    let header = message.header();
    let member = header.member().map(|value| value.as_str());
    let path = header.path().map(|value| value.as_str());
    match member {
        Some("NameOwnerChanged") => true,
        Some("SessionRemoved") => message
            .body()
            .deserialize::<(String, OwnedObjectPath)>()
            .map_or(true, |(id, _)| id == session.id),
        Some("UserRemoved") => message
            .body()
            .deserialize::<(u32, OwnedObjectPath)>()
            .map_or(true, |(uid, _)| uid == session.uid),
        Some("PropertiesChanged") if path == Some(session.path.as_str()) => {
            let Ok((interface, mut changed, invalidated)) =
                message
                    .body()
                    .deserialize::<(String, Properties, Vec<String>)>()
            else {
                return true;
            };
            if interface != SESSION {
                return false;
            }
            if ["Id", "User", "TimestampMonotonic", "State"]
                .iter()
                .any(|key| invalidated.iter().any(|value| value == key))
            {
                return true;
            }
            if changed.contains_key("Id")
                || changed.contains_key("User")
                || changed.contains_key("TimestampMonotonic")
            {
                return true;
            }
            if let Some(state) = changed.remove("State") {
                let Ok(state) = String::try_from(state) else {
                    return true;
                };
                if state == "closing"
                    || (session.require_active && !session.seat.is_empty() && state == "online")
                {
                    return true;
                }
            }
            if session.require_active && !session.seat.is_empty() {
                if invalidated.iter().any(|value| value == "Active") {
                    return true;
                }
                if let Some(active) = changed.remove("Active") {
                    return !bool::try_from(active).unwrap_or(false);
                }
            }
            false
        }
        Some("PropertiesChanged")
            if session
                .display_user
                .as_ref()
                .is_some_and(|user| path == Some(user.as_str())) =>
        {
            let Ok((interface, mut changed, invalidated)) =
                message
                    .body()
                    .deserialize::<(String, Properties, Vec<String>)>()
            else {
                return true;
            };
            if interface != USER {
                return false;
            }
            if invalidated
                .iter()
                .any(|value| matches!(value.as_str(), "Display" | "UID"))
            {
                return true;
            }
            if changed.contains_key("UID") {
                return true;
            }
            changed.remove("Display").is_some_and(|value| {
                <(String, OwnedObjectPath)>::try_from(value)
                    .map_or(true, |(id, _)| id != session.id)
            })
        }
        _ => false,
    }
}

#[cfg(test)]
#[path = "linux/tests.rs"]
mod tests;
