use super::*;
use std::{
    io::{BufRead, Read},
    os::unix::{fs::PermissionsExt, net::UnixListener},
    process::{Child, Stdio},
    time::Instant,
};

#[test]
fn permission_classification_uses_the_error_name_and_preserves_other_failures() {
    let request = zbus::Message::method_call(OBJECT, "GetConfig")
        .unwrap()
        .build(&("home",))
        .unwrap();
    for (name, detail, denied) in [
        ("error.no_permissions", "请求未获准", true),
        ("org.freedesktop.DBus.Error.AccessDenied", "", true),
        ("error.unknown_config", "Permission denied", false),
        ("error.io_error", "error.no_permissions", false),
        (
            "org.freedesktop.DBus.Error.ServiceUnknown",
            "Not allowed",
            false,
        ),
    ] {
        let reply = zbus::Message::error(&request.header(), name)
            .unwrap()
            .build(&detail)
            .unwrap();
        let failure = service_error(reply.into());
        assert_eq!(
            failure
                .io_failure
                .is_some_and(|error| error.kind == IoFailureKind::AccessDenied),
            denied,
            "{name}"
        );
        assert!(failure.message.contains(name));
    }
    let failure = service_error(zbus::Error::InputOutput(std::sync::Arc::new(
        std::io::Error::from_raw_os_error(13),
    )));
    assert_eq!(failure.io_failure.unwrap().os_code, Some(13));
}

#[test]
fn custom_inventory_commands_cannot_request_authorization_with_error_text() {
    let temporary = tempfile::tempdir().unwrap();
    let script = temporary.path().join("snapper");
    fs::write(
        &script,
        "#!/bin/sh\nprintf 'Permission denied; error.no_permissions' >&2\nexit 1\n",
    )
    .unwrap();
    fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
    let failure = list_configs_cancellable(script.to_str().unwrap(), &OperationContext::default())
        .unwrap_err();
    assert_eq!(failure.code, "snapper-failed");
    assert!(failure.io_failure.is_none());
}

#[test]
fn stalled_service_handshake_obeys_cancellation_and_deadlines_and_closes_the_socket() {
    let temporary = tempfile::tempdir().unwrap();
    let path = temporary.path().join("bus");
    let listener = UnixListener::bind(&path).unwrap();
    for cancel in [true, false] {
        let context = OperationContext {
            deadline: Instant::now() + Duration::from_millis(100),
            ..OperationContext::default()
        };
        let start = Instant::now();
        std::thread::scope(|scope| {
            scope.spawn(|| {
                let (mut stream, _) = listener.accept().unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(2)))
                    .unwrap();
                let mut byte = [0];
                assert!(stream.read(&mut byte).unwrap() > 0);
                if cancel {
                    context.cancel();
                }
                let mut remaining = Vec::new();
                stream
                    .read_to_end(&mut remaining)
                    .expect("Stopped probe must release its connection");
            });
            let failure = inspect_at(
                &format!("unix:path={}", path.display()),
                Some("home"),
                &context,
            )
            .unwrap_err();
            assert_eq!(
                failure.io_failure.unwrap().kind,
                if cancel {
                    IoFailureKind::Cancelled
                } else {
                    IoFailureKind::TimedOut
                }
            );
        });
        assert!(start.elapsed() < Duration::from_secs(2));
    }
}

struct Bus(Child);
impl Drop for Bus {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
#[ignore = "Requires dbus-daemon; uses only a private temporary bus and a read-only fixture"]
fn private_bus_reports_native_domain_denials_without_translated_messages() {
    use futures_lite::StreamExt;
    let temporary = tempfile::tempdir().unwrap();
    let mut bus = Bus(Command::new("dbus-daemon")
        .args(["--session", "--nofork", "--nopidfile", "--print-address=1"])
        .arg(format!(
            "--address=unix:tmpdir={}",
            temporary.path().display()
        ))
        .stdout(Stdio::piped())
        .spawn()
        .unwrap());
    let mut address = String::new();
    std::io::BufReader::new(bus.0.stdout.take().unwrap())
        .read_line(&mut address)
        .unwrap();
    let address = address.trim();
    for (config, error_name) in [
        (Some("home"), Some("error.no_permissions")),
        (Some("another"), Some("error.unknown_config")),
        (None, Some("org.freedesktop.DBus.Error.AccessDenied")),
        (Some("allowed"), None),
    ] {
        let service = block_on(
            Builder::address(address)
                .unwrap()
                .internal_executor(false)
                .name(SERVICE)
                .unwrap()
                .build(),
        )
        .unwrap();
        std::thread::scope(|scope| {
            let server = scope.spawn(move || {
                block_on(async {
                    let mut messages = zbus::MessageStream::from(&service);
                    race(
                        async {
                            while let Some(message) = messages.next().await {
                                let message = message.unwrap();
                                if message.message_type() != zbus::message::Type::MethodCall {
                                    continue;
                                }
                                let header = message.header();
                                assert_eq!(header.path().unwrap().as_str(), OBJECT);
                                assert_eq!(header.interface().unwrap().as_str(), SERVICE);
                                assert_eq!(
                                    header.member().unwrap().as_str(),
                                    if config.is_some() {
                                        "GetConfig"
                                    } else {
                                        "ListConfigs"
                                    }
                                );
                                if let Some(config) = config {
                                    assert_eq!(
                                        message.body().deserialize::<String>().unwrap(),
                                        config
                                    );
                                }
                                if let Some(name) = error_name {
                                    service
                                        .reply_error(&header, name, &"无英文权限文字")
                                        .await
                                        .unwrap();
                                } else {
                                    service.reply(&header, &()).await.unwrap();
                                }
                                break;
                            }
                        },
                        async {
                            loop {
                                service.executor().tick().await;
                            }
                        },
                    )
                    .await;
                    service.close().await.unwrap();
                })
            });
            let result = inspect_at(address, config, &OperationContext::default());
            match error_name {
                Some("error.unknown_config") => assert!(result.unwrap_err().io_failure.is_none()),
                Some(_) => assert_eq!(
                    result.unwrap_err().io_failure.unwrap().kind,
                    IoFailureKind::AccessDenied
                ),
                None => assert!(result.is_ok()),
            }
            server.join().unwrap();
        });
    }
}
