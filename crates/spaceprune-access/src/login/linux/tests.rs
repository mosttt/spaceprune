use super::*;
use std::{os::unix::net::UnixStream, sync::atomic::AtomicUsize};
use zbus::zvariant::{ObjectPath, Value};

const SESSION_PATH: &str = "/org/freedesktop/login1/session/fixture";
const USER_PATH: &str = "/org/freedesktop/login1/user/fixture";
#[derive(Default)]
struct State {
    no_pid_session: AtomicBool,
    inactive: AtomicBool,
    closing: AtomicBool,
    state_reads: AtomicUsize,
    stalled_state: AtomicBool,
    remote: AtomicBool,
    no_logind: AtomicBool,
    broken_query: AtomicBool,
}
#[derive(zbus::DBusError, Debug)]
#[zbus(prefix = "org.freedesktop.login1")]
enum LoginError {
    NoSessionForPID(String),
    #[zbus(error)]
    Other(zbus::Error),
}
struct Bus(Arc<State>);
#[zbus::interface(name = "org.freedesktop.DBus")]
impl Bus {
    fn get_name_owner(&self, name: &str) -> zbus::fdo::Result<String> {
        let _ = name;
        if self.0.no_logind.load(Ordering::Acquire) {
            Err(zbus::fdo::Error::NameHasNoOwner("fixture".into()))
        } else {
            Ok(":1.1".into())
        }
    }
}
struct Manager(Arc<State>);
#[zbus::interface(name = "org.freedesktop.login1.Manager")]
impl Manager {
    #[zbus(name = "GetSessionByPID")]
    fn get_session_by_pid(&self, pid: u32) -> Result<OwnedObjectPath, LoginError> {
        let _ = pid;
        if self.0.broken_query.load(Ordering::Acquire) {
            return Err(LoginError::Other(zbus::Error::Failure(
                "fixture service failure".into(),
            )));
        }
        if self.0.no_pid_session.load(Ordering::Acquire) {
            Err(LoginError::NoSessionForPID("fixture".into()))
        } else {
            Ok(OwnedObjectPath::try_from(SESSION_PATH).unwrap())
        }
    }
    fn get_user(&self, uid: u32) -> OwnedObjectPath {
        let _ = uid;
        OwnedObjectPath::try_from(USER_PATH).unwrap()
    }
}
struct SessionProperties(Arc<State>);
#[zbus::interface(name = "org.freedesktop.login1.Session")]
impl SessionProperties {
    #[zbus(property)]
    fn id(&self) -> &str {
        "fixture"
    }
    #[zbus(property)]
    fn user(&self) -> (u32, ObjectPath<'_>) {
        (1000, ObjectPath::try_from(USER_PATH).unwrap())
    }
    #[zbus(property)]
    fn seat(&self) -> (&str, ObjectPath<'_>) {
        if self.0.remote.load(Ordering::Acquire) {
            ("", ObjectPath::try_from("/").unwrap())
        } else {
            (
                "seat0",
                ObjectPath::try_from("/org/freedesktop/login1/seat/seat0").unwrap(),
            )
        }
    }
    #[zbus(property)]
    fn active(&self) -> bool {
        !self.0.inactive.load(Ordering::Acquire)
    }
    #[zbus(property)]
    async fn state(&self) -> &str {
        self.0.state_reads.fetch_add(1, Ordering::SeqCst);
        if self.0.stalled_state.load(Ordering::Acquire) {
            std::future::pending::<()>().await;
        }
        if self.0.closing.load(Ordering::Acquire) {
            "closing"
        } else if self.0.inactive.load(Ordering::Acquire) {
            "online"
        } else {
            "active"
        }
    }
    #[zbus(property)]
    fn timestamp_monotonic(&self) -> u64 {
        42
    }
}
struct UserProperties;
#[zbus::interface(name = "org.freedesktop.login1.User")]
impl UserProperties {
    #[zbus(property, name = "UID")]
    fn uid(&self) -> u32 {
        1000
    }
    #[zbus(property)]
    fn display(&self) -> (&str, ObjectPath<'_>) {
        ("fixture", ObjectPath::try_from(SESSION_PATH).unwrap())
    }
}
struct Fixture {
    server: Connection,
    client: Connection,
    state: Arc<State>,
}
impl Fixture {
    fn new() -> Self {
        let (server, client) = UnixStream::pair().unwrap();
        let state = Arc::new(State::default());
        let shared = state.clone();
        let thread = std::thread::spawn(move || {
            Builder::async_io_unix_stream(server)
                .p2p()
                .server(zbus::Guid::generate())
                .unwrap()
                .serve_at("/org/freedesktop/DBus", Bus(shared.clone()))
                .unwrap()
                .serve_at(MANAGER_PATH, Manager(shared.clone()))
                .unwrap()
                .serve_at(SESSION_PATH, SessionProperties(shared))
                .unwrap()
                .serve_at(USER_PATH, UserProperties)
                .unwrap()
                .build()
                .unwrap()
        });
        let client = Builder::async_io_unix_stream(client)
            .p2p()
            .unique_name(":1.2")
            .unwrap()
            .method_timeout(Duration::from_secs(1))
            .build()
            .unwrap();
        let server = thread.join().unwrap();
        server.inner().set_unique_name(":1.1").unwrap();
        Self {
            server,
            client,
            state,
        }
    }
    fn bind(
        &self,
        interaction: Interaction,
    ) -> (
        Result<NativeBinding, PermissionOutcome>,
        LoginRevocation,
        Arc<AtomicUsize>,
    ) {
        let count = Arc::new(AtomicUsize::new(0));
        let calls = count.clone();
        let revocation = LoginRevocation(Arc::new(super::super::RevocationState {
            revoked: AtomicBool::new(false),
            on_revoke: Box::new(move || {
                calls.fetch_add(1, Ordering::SeqCst);
            }),
        }));
        (
            bind_connection(
                self.client.clone(),
                1000,
                123,
                interaction,
                revocation.clone(),
            ),
            revocation,
            count,
        )
    }
    fn active(&self, active: bool) {
        self.state.inactive.store(!active, Ordering::Release);
        self.server
            .emit_signal(
                None::<&str>,
                SESSION_PATH,
                PROPERTIES,
                "PropertiesChanged",
                &(
                    SESSION,
                    HashMap::from([("Active", Value::from(active))]),
                    Vec::<String>::new(),
                ),
            )
            .unwrap();
    }
    fn removed(&self) {
        self.server
            .emit_signal(
                None::<&str>,
                MANAGER_PATH,
                MANAGER,
                "SessionRemoved",
                &("fixture", ObjectPath::try_from(SESSION_PATH).unwrap()),
            )
            .unwrap();
    }
}
fn wait_revoked(revocation: &LoginRevocation) {
    wait_revoked_within(revocation, Duration::from_secs(2));
}
fn wait_revoked_within(revocation: &LoginRevocation, timeout: Duration) {
    let deadline = Instant::now() + timeout;
    while !revocation.is_revoked() {
        assert!(Instant::now() < deadline, "session event was not observed");
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn wait_two_rechecks(fixture: &Fixture) {
    let expected = fixture.state.state_reads.load(Ordering::SeqCst) + 2;
    let deadline = Instant::now() + Duration::from_secs(3);
    while fixture.state.state_reads.load(Ordering::SeqCst) < expected {
        assert!(
            Instant::now() < deadline,
            "quiet session was not rechecked twice"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn rapid_switch_and_return_invalidate_idle_binding_once() {
    let fixture = Fixture::new();
    let (binding, revocation, calls) = fixture.bind(Interaction::Foreground);
    let binding = binding.unwrap();
    assert!(binding.login_bound);
    assert_eq!(binding.subject, "linux:uid:1000:login:fixture:started:42");
    fixture.active(false);
    fixture.active(true);
    wait_revoked(&revocation);
    drop(binding);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert!(revocation.is_revoked());
}

#[test]
fn user_manager_process_uses_logind_display_but_background_does_not_require_a_seat() {
    let fixture = Fixture::new();
    fixture.state.no_pid_session.store(true, Ordering::Release);
    let (binding, revocation, _) = fixture.bind(Interaction::Foreground);
    let binding = binding.unwrap();
    assert!(binding.login_bound);
    fixture.removed();
    wait_revoked(&revocation);
    drop(binding);
    let fixture = Fixture::new();
    fixture.state.no_pid_session.store(true, Ordering::Release);
    fixture.state.inactive.store(true, Ordering::Release);
    let (binding, _, _) = fixture.bind(Interaction::NonInteractive);
    assert!(!binding.unwrap().login_bound);
}

#[test]
fn known_inactive_or_broken_monitor_is_not_an_unmanaged_process() {
    for broken in [false, true] {
        let fixture = Fixture::new();
        fixture.state.inactive.store(!broken, Ordering::Release);
        fixture.state.broken_query.store(broken, Ordering::Release);
        let (binding, _, _) = fixture.bind(Interaction::Foreground);
        assert_eq!(
            binding.err(),
            Some(PermissionOutcome::AuthorizationUnavailable)
        );
    }
}

#[test]
fn no_logind_preserves_terminal_and_background_entry_points() {
    for interaction in [
        Interaction::Foreground,
        Interaction::Terminal,
        Interaction::NonInteractive,
    ] {
        let fixture = Fixture::new();
        fixture.state.no_logind.store(true, Ordering::Release);
        let (binding, revocation, _) = fixture.bind(interaction);
        assert!(!binding.unwrap().login_bound);
        assert!(!revocation.is_revoked());
    }
}

#[test]
fn remote_login_is_bound_to_its_lifetime_without_a_graphical_active_requirement() {
    let fixture = Fixture::new();
    fixture.state.remote.store(true, Ordering::Release);
    fixture.state.inactive.store(true, Ordering::Release);
    let (binding, revocation, _) = fixture.bind(Interaction::Terminal);
    let binding = binding.unwrap();
    assert!(binding.login_bound);
    fixture.active(false);
    wait_two_rechecks(&fixture);
    assert!(!revocation.is_revoked());
    fixture.removed();
    wait_revoked(&revocation);
    drop(binding);
}

#[test]
fn lost_logind_owner_revokes_and_dropping_a_live_monitor_does_not_invalidate() {
    let fixture = Fixture::new();
    let (binding, revocation, _) = fixture.bind(Interaction::Foreground);
    fixture
        .server
        .send(
            &Message::signal(
                "/org/freedesktop/DBus",
                "org.freedesktop.DBus",
                "NameOwnerChanged",
            )
            .unwrap()
            .sender("org.freedesktop.DBus")
            .unwrap()
            .build(&(LOGIN, ":1.1", ""))
            .unwrap(),
        )
        .unwrap();
    wait_revoked(&revocation);
    drop(binding.unwrap());
    let fixture = Fixture::new();
    let (binding, revocation, calls) = fixture.bind(Interaction::Foreground);
    drop(binding.unwrap());
    assert!(!revocation.is_revoked());
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

#[test]
fn background_login_ignores_foreground_switch_but_not_logout() {
    let fixture = Fixture::new();
    fixture.state.inactive.store(true, Ordering::Release);
    let (binding, revocation, _) = fixture.bind(Interaction::NonInteractive);
    let binding = binding.unwrap();
    assert!(binding.login_bound);
    fixture.active(true);
    fixture.active(false);
    wait_two_rechecks(&fixture);
    assert!(!revocation.is_revoked());
    fixture.removed();
    wait_revoked(&revocation);
    drop(binding);
}

#[test]
fn losing_the_monitor_connection_revokes_the_binding() {
    let fixture = Fixture::new();
    let (binding, revocation, _) = fixture.bind(Interaction::Foreground);
    fixture.server.close().unwrap();
    wait_revoked(&revocation);
    drop(binding.unwrap());
}

#[test]
fn closing_without_a_property_signal_revokes_frontend_and_background_bindings() {
    for interaction in [
        Interaction::Foreground,
        Interaction::Terminal,
        Interaction::NonInteractive,
    ] {
        let fixture = Fixture::new();
        let (binding, revocation, calls) = fixture.bind(interaction);
        let binding = binding.unwrap();
        // logind can keep a closing session object alive without emitting PropertiesChanged.
        fixture.state.closing.store(true, Ordering::Release);
        wait_revoked(&revocation);
        drop(binding);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }
}

#[test]
fn a_stalled_quiet_state_recheck_revokes_instead_of_preserving_unverified_access() {
    let fixture = Fixture::new();
    let (binding, revocation, calls) = fixture.bind(Interaction::NonInteractive);
    let binding = binding.unwrap();
    fixture.state.stalled_state.store(true, Ordering::Release);
    wait_revoked_within(&revocation, Duration::from_secs(3));
    drop(binding);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[test]
fn dropping_a_stalled_state_recheck_does_not_revoke_or_wait_for_its_timeout() {
    let fixture = Fixture::new();
    let (binding, revocation, calls) = fixture.bind(Interaction::NonInteractive);
    let binding = binding.unwrap();
    let initial_reads = fixture.state.state_reads.load(Ordering::SeqCst);
    fixture.state.stalled_state.store(true, Ordering::Release);
    let deadline = Instant::now() + Duration::from_secs(2);
    while fixture.state.state_reads.load(Ordering::SeqCst) == initial_reads {
        assert!(Instant::now() < deadline, "recheck did not start");
        std::thread::sleep(Duration::from_millis(5));
    }
    let started = Instant::now();
    drop(binding);
    assert!(started.elapsed() < Duration::from_millis(500));
    assert!(!revocation.is_revoked());
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}
