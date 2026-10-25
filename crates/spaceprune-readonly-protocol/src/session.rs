use std::{
    io::{BufRead, BufReader, Read, Write},
    path::{Path, PathBuf},
    sync::mpsc::{self, RecvTimeoutError},
    time::Duration,
};

use serde::{Deserialize, Serialize};

use crate::{HelperError, effective_uid, run_helper};

/// Start a Linux helper that accepts individually validated one-time requests.
pub const LINUX_SESSION_ARGUMENT: &str = spaceprune_access::helper::METADATA_SESSION_ARGUMENT;
/// Probe session support without requesting authorization or reading any files.
pub const LINUX_SESSION_SUPPORT_ARGUMENT: &str = "--supports-readonly-session";
/// The first stdout line sent after an elevated session is ready, without a newline.
pub const LINUX_SESSION_READY: &str = "spaceprune-metadata-v6";
/// An authorized helper exits after this interval without a complete request.
pub const LINUX_SESSION_IDLE_TIMEOUT: Duration = Duration::from_secs(15 * 60);

const MAX_REQUEST_LINE_BYTES: usize = 64 * 1024;

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SessionRequest {
    request_path: PathBuf,
    nonce: String,
}

pub(super) fn request_line(path: &Path, nonce: &str) -> Result<Vec<u8>, HelperError> {
    let mut line = serde_json::to_vec(&SessionRequest {
        request_path: path.to_path_buf(),
        nonce: nonce.to_owned(),
    })
    .map_err(|_| HelperError)?;
    line.push(b'\n');
    (line.len() <= MAX_REQUEST_LINE_BYTES)
        .then_some(line)
        .ok_or(HelperError)
}

pub(super) fn run_from_environment() -> Result<(), HelperError> {
    let caller_uid = std::env::var("PKEXEC_UID")
        .ok()
        .and_then(|value| value.parse::<u32>().ok())
        .or_else(|| {
            std::env::var("SUDO_UID")
                .ok()
                .and_then(|value| value.parse().ok())
        });
    if !valid_session_identity(effective_uid(), caller_uid) {
        return Err(HelperError);
    }
    run_session(
        std::io::stdin(),
        std::io::stdout().lock(),
        LINUX_SESSION_IDLE_TIMEOUT,
    )
}

fn valid_session_identity(effective_uid: Option<u32>, caller_uid: Option<u32>) -> bool {
    effective_uid == Some(0) && caller_uid.is_some()
}

fn run_session(
    input: impl Read + Send + 'static,
    mut output: impl Write,
    idle_timeout: Duration,
) -> Result<(), HelperError> {
    let (sender, receiver) = mpsc::sync_channel(1);
    std::thread::Builder::new()
        .name("readonly-session-input".to_owned())
        .spawn(move || {
            let mut input = BufReader::new(input);
            loop {
                let frame = match read_request_line(&mut input) {
                    Ok(Some(frame)) => Ok(frame),
                    Ok(None) => break,
                    Err(error) => Err(error),
                };
                let invalid = frame.is_err();
                if sender.send(frame).is_err() || invalid {
                    break;
                }
            }
        })
        .map_err(|_| HelperError)?;

    writeln!(output, "{LINUX_SESSION_READY}").map_err(|_| HelperError)?;
    output.flush().map_err(|_| HelperError)?;
    loop {
        let frame = match receiver.recv_timeout(idle_timeout) {
            Ok(frame) => frame,
            Err(RecvTimeoutError::Timeout | RecvTimeoutError::Disconnected) => return Ok(()),
        };
        let framing_failed = frame.is_err();
        let result = frame.and_then(|frame| {
            let request: SessionRequest =
                serde_json::from_slice(&frame).map_err(|_| HelperError)?;
            run_helper(&request.request_path, &request.nonce)
        });
        writeln!(output, "{}", if result.is_ok() { 0 } else { 70 }).map_err(|_| HelperError)?;
        output.flush().map_err(|_| HelperError)?;
        if framing_failed {
            return Err(HelperError);
        }
    }
}

fn read_request_line(input: &mut impl BufRead) -> Result<Option<Vec<u8>>, HelperError> {
    let mut line = Vec::new();
    let count = input
        .take((MAX_REQUEST_LINE_BYTES + 1) as u64)
        .read_until(b'\n', &mut line)
        .map_err(|_| HelperError)?;
    if count == 0 {
        return Ok(None);
    }
    if count > MAX_REQUEST_LINE_BYTES || line.last() != Some(&b'\n') {
        return Err(HelperError);
    }
    Ok(Some(line))
}

#[cfg(test)]
mod tests {
    use std::{fs, io::Cursor};

    use super::*;
    use crate::{PreparedRequest, ResponseOutcome};

    fn fixture(size: usize) -> (tempfile::TempDir, PreparedRequest) {
        let directory = tempfile::tempdir().expect("Create fixture directory");
        let root = directory.path().join("cache/data");
        fs::create_dir_all(&root).expect("Create registered root");
        fs::write(root.join("private-name.bin"), vec![0_u8; size]).expect("Write fixture");
        let root = fs::canonicalize(root).expect("Canonicalize fixture root");
        let request = PreparedRequest::create("discovery-source-1", &[root])
            .expect("Prepare one-time request");
        (directory, request)
    }

    #[test]
    fn session_reuses_authorization_but_validates_and_consumes_each_request() {
        let (_first_directory, first) = fixture(23);
        let (_second_directory, second) = fixture(31);
        let first_line = first.session_request_line().expect("Encode first request");
        let mut input = first_line.clone();
        input.extend(
            second
                .session_request_line()
                .expect("Encode second request"),
        );
        input.extend(first_line);
        let mut output = Vec::new();
        run_session(Cursor::new(input), &mut output, Duration::from_secs(1))
            .expect("Process until EOF");
        assert_eq!(output, b"spaceprune-metadata-v6\n0\n0\n70\n");
        for (request, logical_bytes) in [(first, 23), (second, 31)] {
            assert!(!request.request_path.exists());
            assert_eq!(
                request
                    .receive_response()
                    .expect("Receive aggregate result"),
                ResponseOutcome::Measured {
                    logical_bytes,
                    inspected_entries: 1,
                    incomplete: false,
                }
            );
        }
        assert!(
            !String::from_utf8(output)
                .expect("ASCII acknowledgments")
                .contains("private-name")
        );
    }

    #[test]
    fn invalid_nonce_and_root_do_not_allow_a_session_request() {
        let (_first_directory, first) = fixture(17);
        let (_second_directory, second) = fixture(19);
        let mut forbidden: crate::HelperRequest =
            serde_json::from_slice(&fs::read(&second.request_path).expect("Read request"))
                .expect("Decode request");
        forbidden.roots = vec![PathBuf::from("/")];
        fs::write(
            &second.request_path,
            serde_json::to_vec(&forbidden).expect("Encode request"),
        )
        .expect("Replace root with forbidden root");
        let mut input =
            request_line(&first.request_path, &"a".repeat(43)).expect("Encode wrong nonce");
        input.extend(
            second
                .session_request_line()
                .expect("Encode forbidden request"),
        );
        input.extend(first.session_request_line().expect("Encode valid request"));
        let mut output = Vec::new();
        run_session(Cursor::new(input), &mut output, Duration::from_secs(1))
            .expect("Process until EOF");
        assert_eq!(output, b"spaceprune-metadata-v6\n70\n70\n0\n");
        assert!(second.request_path.exists());
        assert!(first.receive_response().is_ok());
    }

    #[test]
    fn session_rejects_unbounded_and_unterminated_frames() {
        for frame in [vec![b'x'; MAX_REQUEST_LINE_BYTES + 1], b"{}".to_vec()] {
            let mut output = Vec::new();
            assert!(run_session(Cursor::new(frame), &mut output, Duration::from_secs(1)).is_err());
            assert_eq!(output, b"spaceprune-metadata-v6\n70\n");
        }
        let mut output = Vec::new();
        run_session(Cursor::new(Vec::new()), &mut output, Duration::from_secs(1))
            .expect("EOF terminates the session");
        assert_eq!(output, b"spaceprune-metadata-v6\n");
    }

    #[test]
    fn idle_session_exits_even_when_the_parent_keeps_an_incomplete_frame_open() {
        let (input, mut parent) =
            std::os::unix::net::UnixStream::pair().expect("Create input pipe");
        parent
            .write_all(b"{\"requestPath\":")
            .expect("Write partial frame");
        let mut output = Vec::new();
        run_session(input, &mut output, Duration::from_millis(20))
            .expect("Idle session releases its authorization");
        assert_eq!(output, b"spaceprune-metadata-v6\n");
        drop(parent);
    }

    #[test]
    fn session_requires_pkexec_identity_and_accepts_no_additional_arguments() {
        assert!(!valid_session_identity(Some(1000), Some(1000)));
        assert!(!valid_session_identity(Some(0), None));
        assert!(!valid_session_identity(None, Some(1000)));
        assert!(valid_session_identity(Some(0), Some(1000)));
        assert!(matches!(
            crate::parse_helper_invocation(["helper".into(), LINUX_SESSION_ARGUMENT.into()]),
            crate::HelperInvocation::Session
        ));
        assert!(matches!(
            crate::parse_helper_invocation([
                "helper".into(),
                LINUX_SESSION_ARGUMENT.into(),
                "extra".into()
            ]),
            crate::HelperInvocation::Invalid
        ));
    }

    #[test]
    fn session_support_probe_needs_no_elevated_identity_and_rejects_extra_arguments() {
        assert_eq!(
            crate::run_from_arguments(["helper".into(), LINUX_SESSION_SUPPORT_ARGUMENT.into()]),
            Some(0)
        );
        assert_eq!(
            crate::run_from_arguments([
                "helper".into(),
                LINUX_SESSION_SUPPORT_ARGUMENT.into(),
                "extra".into()
            ]),
            Some(64)
        );
    }
}
