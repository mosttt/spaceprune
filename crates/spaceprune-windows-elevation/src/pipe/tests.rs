use super::*;
use std::time::{SystemTime, UNIX_EPOCH};

fn unique_name() -> String {
    format!(
        r"\\.\pipe\SpacePrune.fixture.{}.{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    )
}

#[test]
fn local_pipe_is_unique_nonblocking_and_reports_actual_kernel_peers() {
    let name = unique_name();
    let server = LocalPipe::create(&name).unwrap();
    assert!(LocalPipe::create(&name).is_err());
    assert!(!server.poll_connected().unwrap());
    let client = LocalPipe::connect(&name).unwrap();
    assert!(server.poll_connected().unwrap());
    assert_eq!(server.peer_process_id().unwrap(), std::process::id());
    assert_eq!(client.peer_process_id().unwrap(), std::process::id());
    let mut bytes = [0; 16];
    assert_eq!(
        server.try_read(&mut bytes).unwrap_err().kind(),
        io::ErrorKind::WouldBlock
    );
    assert_eq!(client.try_write(b"request").unwrap(), 7);
    assert_eq!(server.try_read(&mut bytes).unwrap(), 7);
    assert_eq!(&bytes[..7], b"request");
    assert_eq!(server.try_write(b"receipt").unwrap(), 7);
    assert_eq!(client.try_read(&mut bytes).unwrap(), 7);
    assert_eq!(&bytes[..7], b"receipt");
    server.disconnect();
    assert!(client.try_write(b"replay").is_err());
    assert!(LocalPipe::connect(r"\\remote\pipe\SpacePrune.fixture").is_err());
}

#[test]
fn full_transport_does_not_turn_a_cancelable_write_into_a_blocking_call() {
    let name = unique_name();
    let server = LocalPipe::create(&name).unwrap();
    server.poll_connected().unwrap();
    let _client = LocalPipe::connect(&name).unwrap();
    assert!(server.poll_connected().unwrap());
    let bytes = [0; 8192];
    for _ in 0..8192 {
        match server.try_write(&bytes) {
            Ok(_) => {}
            Err(error) => {
                assert_eq!(error.kind(), io::ErrorKind::WouldBlock);
                server.disconnect();
                return;
            }
        }
    }
    panic!("The pipe must expose finite backpressure");
}

#[test]
fn confirmed_plan_larger_than_pipe_capacity_streams_to_the_peer() {
    use std::time::{Duration, Instant};
    let name = unique_name();
    let server = LocalPipe::create(&name).unwrap();
    let client = LocalPipe::connect(&name).unwrap();
    assert!(server.poll_connected().unwrap());
    let payload = vec![b'x'; 1024 * 1024];
    let expected = payload.len();
    let reader = std::thread::spawn(move || {
        let mut total = 0;
        let mut buffer = [0; 8192];
        let deadline = Instant::now() + Duration::from_secs(5);
        while total < expected && Instant::now() < deadline {
            match client.try_read(&mut buffer) {
                Ok(0) => break,
                Ok(n) => total += n,
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(1))
                }
                Err(e) => panic!("{e}"),
            }
        }
        total
    });
    let mut sent = 0;
    let deadline = Instant::now() + Duration::from_secs(5);
    while sent < expected && Instant::now() < deadline {
        match server.try_write(&payload[sent..]) {
            Ok(n) => sent += n,
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(1))
            }
            Err(e) => panic!("{e}"),
        }
    }
    let received = reader.join().unwrap();
    assert_eq!((sent, received), (expected, expected));
}
