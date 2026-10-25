#![forbid(unsafe_code)]
#![deny(missing_docs)]
#![deny(rustdoc::broken_intra_doc_links)]

//! SpacePrune 权限 Helper 的一次性协议与有界只读元数据遍历。

use std::{
    ffi::{OsStr, OsString},
    fmt,
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    net::{IpAddr, Ipv4Addr, SocketAddr, TcpListener, TcpStream},
    path::{Component, Path, PathBuf},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

#[cfg(any(target_os = "linux", target_os = "macos"))]
use std::collections::VecDeque;

mod measurement;
mod request;
mod worker;
use measurement::measure_roots;
pub use request::PreparedRequest;
#[cfg(test)]
use worker::read_and_validate_request;
use worker::run_helper;
#[cfg(test)]
use worker::send_response;

#[cfg(target_os = "macos")]
#[path = "client.rs"]
mod administrator;
/// Scoped native metadata sessions shared by terminal and graphical hosts.
#[cfg(any(target_os = "linux", target_os = "windows"))]
pub mod client;
/// Metadata access in the original macOS process; privacy grants never become helper leases.
#[cfg(target_os = "macos")]
#[path = "client/macos.rs"]
pub mod client;
#[cfg(all(test, target_os = "linux"))]
#[path = "client/macos.rs"]
mod macos_client_tests;
#[cfg(any(target_os = "windows", all(test, target_os = "linux")))]
mod mailbox;
#[cfg(target_os = "linux")]
mod session;
#[cfg(target_os = "linux")]
mod snapper;
#[cfg(target_os = "linux")]
pub use session::{
    LINUX_SESSION_ARGUMENT, LINUX_SESSION_IDLE_TIMEOUT, LINUX_SESSION_READY,
    LINUX_SESSION_SUPPORT_ARGUMENT,
};

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use serde::{Deserialize, Serialize};
use spaceprune_access::{OperationContext, OperationIdentity, OperationTransfer, ResourceBudget};

const HELPER_FLAG: &str = "--spaceprune-readonly-helper";
const REQUEST_SCHEMA_VERSION: u16 = 6;
const REQUEST_LIFETIME: Duration = Duration::from_secs(120);
const MAX_REQUEST_BYTES: usize = 4 * 1024 * 1024;
const MAX_ROOTS: usize = 8;
#[cfg(any(target_os = "linux", target_os = "windows", target_os = "macos"))]
const MAX_METADATA_ENTRIES: usize = 4_096;
#[cfg(any(target_os = "linux", target_os = "windows", target_os = "macos"))]
const MAX_METADATA_DEPTH: usize = 3;
/// Probe metadata task scanning without elevation or filesystem inspection.
#[cfg(target_os = "linux")]
pub const LINUX_SCAN_SUPPORT_ARGUMENT: &str = "--supports-filesystem-scan";

/// Probe restricted Snapper snapshot scanning without authorization or filesystem inspection.
#[cfg(target_os = "linux")]
pub const LINUX_SNAPPER_SCAN_SUPPORT_ARGUMENT: &str = "--supports-snapper-scan";

/// Probe this generation of identity-bearing metadata results.
#[cfg(target_os = "linux")]
pub const LINUX_SCOPE_SUPPORT_ARGUMENT: &str = "--supports-scope-v6";

/// Helper 协议或本地 IPC 无法安全继续。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HelperError;

impl fmt::Display for HelperError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("只读 Helper 协议不可用")
    }
}

impl std::error::Error for HelperError {}

/// Helper 返回给桌面宿主的脱敏聚合结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResponseOutcome {
    /// 至少一个根可访问；结果只包含大小、数量和是否不完整。
    Measured {
        /// 文件逻辑字节总数。
        logical_bytes: u64,
        /// 已检查的文件系统条目数。
        inspected_entries: u64,
        /// 是否因为权限、类型、深度或数量限制跳过了条目。
        incomplete: bool,
    },
    /// 所有请求根在提升后仍不可访问。
    Denied,
}

#[cfg(any(target_os = "linux", target_os = "windows", target_os = "macos"))]
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct TaskScanResponse<T> {
    schema_version: u16,
    operation: OperationIdentity,
    source_id: String,
    nonce: String,
    result: Result<T, String>,
    cancelled: bool,
}

#[cfg(any(target_os = "linux", target_os = "windows", target_os = "macos"))]
fn filesystem_scan_roots(
    task: &spaceprune_plugin_api::TaskRequest,
) -> Result<Vec<PathBuf>, HelperError> {
    if task.handler_id != "builtin.filesystem/paths" {
        return Err(HelperError);
    }
    let roots = task
        .options
        .get("roots")
        .and_then(serde_json::Value::as_array)
        .ok_or(HelperError)?;
    roots
        .iter()
        .map(|root| {
            root.get("path")
                .and_then(serde_json::Value::as_str)
                .map(PathBuf::from)
                .ok_or(HelperError)
        })
        .collect()
}

/// 判断来源句柄是否符合宿主生成规则。
#[must_use]
pub fn valid_source_id(source_id: &str) -> bool {
    if source_id.len() > 64 {
        return false;
    }
    if source_id
        .strip_prefix("navigation-boundary-")
        .is_some_and(|suffix| {
            suffix.len() == 32 && suffix.bytes().all(|byte| byte.is_ascii_hexdigit())
        })
    {
        return true;
    }
    ["recipe-source-", "discovery-source-"]
        .iter()
        .any(|prefix| {
            source_id.strip_prefix(prefix).is_some_and(|suffix| {
                !suffix.is_empty() && suffix.bytes().all(|byte| byte.is_ascii_digit())
            })
        })
}

/// 若当前进程是内部 Helper 调用，则执行请求并返回进程退出码。
#[must_use]
pub fn run_from_environment() -> Option<i32> {
    #[cfg(target_os = "macos")]
    if let Some(code) = spaceprune_access::macos::service::run_from_environment(
        spaceprune_access::macos::ServiceKind::Metadata,
        |request, operation, _events| {
            worker::run_macos_request(request, operation)
                .map_err(|_| spaceprune_access::PermissionOutcome::PolicyBlocked)
        },
    ) {
        return Some(code);
    }
    run_from_arguments(std::env::args_os())
}

fn run_from_arguments(args: impl IntoIterator<Item = OsString>) -> Option<i32> {
    match parse_helper_invocation(args) {
        HelperInvocation::NotHelper => None,
        HelperInvocation::Invalid => Some(64),
        #[cfg(target_os = "windows")]
        HelperInvocation::WindowsSessionSupport => Some(0),
        #[cfg(target_os = "windows")]
        HelperInvocation::WindowsSession { bootstrap, nonce } => {
            Some(if mailbox::run(&bootstrap, &nonce).is_ok() {
                0
            } else {
                70
            })
        }
        #[cfg(target_os = "linux")]
        HelperInvocation::SessionSupport => Some(0),
        #[cfg(target_os = "linux")]
        HelperInvocation::ScanSupport => Some(0),
        #[cfg(target_os = "linux")]
        HelperInvocation::SnapperScanSupport => Some(0),
        #[cfg(target_os = "linux")]
        HelperInvocation::ScopeSupport => Some(0),
        #[cfg(target_os = "linux")]
        HelperInvocation::Session => Some(if session::run_from_environment().is_ok() {
            0
        } else {
            70
        }),
        HelperInvocation::Run {
            request_path,
            nonce,
        } => Some(if run_helper(&request_path, &nonce).is_ok() {
            0
        } else {
            70
        }),
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
enum HelperOperation {
    MeasureRegisteredSource,
    MeasureNavigationBoundary,
    ScanFilesystemTask,
    ScanSystemCacheTask,
    ScanSnapperTask,
    InspectSnapperTask,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct HelperRequest {
    schema_version: u16,
    context: OperationTransfer,
    operation: HelperOperation,
    source_id: String,
    #[serde(with = "host_paths_serde")]
    roots: Vec<PathBuf>,
    response_port: u16,
    nonce: String,
    issued_at: u64,
    expires_at: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    scan_task: Option<serde_json::Value>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct HelperResponse {
    schema_version: u16,
    operation: OperationIdentity,
    source_id: String,
    nonce: String,
    outcome: HelperResponseOutcome,
    logical_bytes: u64,
    inspected_entries: u64,
    skipped_entries: u64,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
enum HelperResponseOutcome {
    Measured,
    StillDenied,
}

enum HelperInvocation {
    NotHelper,
    Invalid,
    #[cfg(target_os = "windows")]
    WindowsSessionSupport,
    #[cfg(target_os = "windows")]
    WindowsSession {
        bootstrap: PathBuf,
        nonce: String,
    },
    #[cfg(target_os = "linux")]
    Session,
    #[cfg(target_os = "linux")]
    SessionSupport,
    #[cfg(target_os = "linux")]
    ScanSupport,
    #[cfg(target_os = "linux")]
    SnapperScanSupport,
    #[cfg(target_os = "linux")]
    ScopeSupport,
    Run {
        request_path: PathBuf,
        nonce: String,
    },
}

fn validate_response(
    response: HelperResponse,
    expected_source_id: &str,
    expected_nonce: &str,
) -> Result<ResponseOutcome, HelperError> {
    if response.schema_version != REQUEST_SCHEMA_VERSION
        || response.source_id != expected_source_id
        || response.nonce != expected_nonce
    {
        return Err(HelperError);
    }
    match response.outcome {
        HelperResponseOutcome::Measured => Ok(ResponseOutcome::Measured {
            logical_bytes: response.logical_bytes,
            inspected_entries: response.inspected_entries,
            incomplete: response.skipped_entries > 0,
        }),
        HelperResponseOutcome::StillDenied => Ok(ResponseOutcome::Denied),
    }
}

#[derive(Default)]
struct AggregateMetadata {
    logical_bytes: u64,
    inspected_entries: u64,
    skipped_entries: u64,
    accessible_roots: u64,
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    failure: Option<spaceprune_access::IoFailure>,
}

fn parse_helper_invocation(args: impl IntoIterator<Item = OsString>) -> HelperInvocation {
    let mut args = args.into_iter();
    let _executable = args.next();
    let Some(first) = args.next() else {
        return HelperInvocation::NotHelper;
    };
    #[cfg(target_os = "windows")]
    if first.as_os_str() == OsStr::new(&mailbox::support_argument()) {
        return if args.next().is_none() {
            HelperInvocation::WindowsSessionSupport
        } else {
            HelperInvocation::Invalid
        };
    }
    #[cfg(target_os = "windows")]
    if first == mailbox::ARGUMENT {
        let (Some(bootstrap), Some(nonce), None) = (args.next(), args.next(), args.next()) else {
            return HelperInvocation::Invalid;
        };
        let Some(nonce) = nonce.to_str().filter(|nonce| valid_nonce(nonce)) else {
            return HelperInvocation::Invalid;
        };
        return HelperInvocation::WindowsSession {
            bootstrap: PathBuf::from(bootstrap),
            nonce: nonce.to_owned(),
        };
    }
    #[cfg(target_os = "linux")]
    if first == LINUX_SCOPE_SUPPORT_ARGUMENT {
        return if args.next().is_none() {
            HelperInvocation::ScopeSupport
        } else {
            HelperInvocation::Invalid
        };
    }
    #[cfg(target_os = "linux")]
    if first == LINUX_SNAPPER_SCAN_SUPPORT_ARGUMENT {
        return if args.next().is_none() {
            HelperInvocation::SnapperScanSupport
        } else {
            HelperInvocation::Invalid
        };
    }
    #[cfg(target_os = "linux")]
    if first == LINUX_SCAN_SUPPORT_ARGUMENT {
        return if args.next().is_none() {
            HelperInvocation::ScanSupport
        } else {
            HelperInvocation::Invalid
        };
    }
    #[cfg(target_os = "linux")]
    if first == LINUX_SESSION_SUPPORT_ARGUMENT {
        return if args.next().is_none() {
            HelperInvocation::SessionSupport
        } else {
            HelperInvocation::Invalid
        };
    }
    #[cfg(target_os = "linux")]
    if spaceprune_access::helper::HelperKind::Metadata.accepts_session_argument(&first) {
        return if args.next().is_none() {
            HelperInvocation::Session
        } else {
            HelperInvocation::Invalid
        };
    }
    if first != HELPER_FLAG {
        return HelperInvocation::NotHelper;
    }
    let (Some(request_path), Some(nonce), None) = (args.next(), args.next(), args.next()) else {
        return HelperInvocation::Invalid;
    };
    let Some(nonce) = nonce.to_str().filter(|nonce| valid_nonce(nonce)) else {
        return HelperInvocation::Invalid;
    };
    HelperInvocation::Run {
        request_path: PathBuf::from(request_path),
        nonce: nonce.to_owned(),
    }
}

fn helper_should_stop(control_directory: Option<&Path>, expires_at: Option<u64>) -> bool {
    control_directory.is_some_and(|directory| !directory.exists())
        || expires_at
            .is_some_and(|expires_at| unix_timestamp().map_or(true, |now| now > expires_at))
}

fn request_root_shape_is_valid(operation: HelperOperation, roots: &[PathBuf]) -> bool {
    match operation {
        HelperOperation::MeasureRegisteredSource => {
            !roots.is_empty()
                && roots.len() <= MAX_ROOTS
                && roots.iter().all(|root| !root_is_too_broad(root))
        }
        HelperOperation::MeasureNavigationBoundary => {
            roots.len() == 1 && roots.iter().all(|root| !root_is_filesystem_root(root))
        }
        HelperOperation::ScanFilesystemTask | HelperOperation::ScanSystemCacheTask => {
            !roots.is_empty()
                && roots.len() <= 64
                && roots.iter().all(|root| !root_is_too_broad(root))
        }
        HelperOperation::ScanSnapperTask | HelperOperation::InspectSnapperTask => roots.is_empty(),
    }
}

fn helper_root_is_valid(path: &Path, operation: HelperOperation) -> bool {
    // An absent optional recipe root must not block inspection of other registered roots.
    let existing = if matches!(
        operation,
        HelperOperation::ScanFilesystemTask | HelperOperation::ScanSystemCacheTask
    ) {
        path.ancestors().find(|ancestor| {
            !fs::symlink_metadata(ancestor)
                .is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound)
        })
    } else {
        Some(path)
    };
    path.is_absolute()
        && !path
            .components()
            .any(|component| component == Component::ParentDir)
        && existing.is_some_and(|existing| {
            #[cfg(target_os = "windows")]
            {
                spaceprune_windows_elevation::valid_metadata_root(existing)
            }
            #[cfg(not(target_os = "windows"))]
            {
                fs::symlink_metadata(existing)
                    .is_ok_and(|metadata| !metadata.file_type().is_symlink())
                    && fs::canonicalize(existing).is_ok_and(|canonical| canonical == existing)
            }
        })
        && match operation {
            HelperOperation::MeasureRegisteredSource => {
                !root_is_too_broad(path) && privileged_root_is_allowlisted(path)
            }
            HelperOperation::MeasureNavigationBoundary => !root_is_filesystem_root(path),
            HelperOperation::ScanFilesystemTask | HelperOperation::ScanSystemCacheTask => {
                !root_is_too_broad(path)
            }
            HelperOperation::ScanSnapperTask | HelperOperation::InspectSnapperTask => false,
        }
}

fn privileged_root_is_allowlisted(path: &Path) -> bool {
    #[cfg(target_os = "linux")]
    {
        let fixed = [
            "/var/cache/zypp/packages",
            "/var/cache/apt/archives",
            "/var/cache/dnf",
            "/var/cache/pacman/pkg",
            "/var/log/journal",
            "/run/log/journal",
            "/var/lib/systemd/coredump",
        ];
        if fixed
            .into_iter()
            .filter_map(|root| fs::canonicalize(root).ok())
            .any(|root| path == root)
        {
            return true;
        }
        invoking_home_linux()
            .into_iter()
            .chain(fs::canonicalize(std::env::temp_dir()).ok())
            .any(|root| path != root && path.starts_with(root))
    }
    #[cfg(target_os = "macos")]
    {
        // The authenticated metadata service requires its own live OS right; root/path shape
        // and no-follow checks still apply. This never bypasses TCC or system policy.
        let _ = path;
        true
    }
    #[cfg(target_os = "windows")]
    {
        // Windows 由系统 UAC 对本轮精确登记根授权。Helper 仍拒绝宽根和 reparse point，
        // 且只返回有数量/深度上限的聚合元数据，以支持标准用户输入管理员凭据。
        let _ = path;
        true
    }
    #[cfg(not(any(target_os = "linux", target_os = "windows", target_os = "macos")))]
    {
        let _ = path;
        false
    }
}

#[cfg(target_os = "linux")]
fn invoking_home_linux() -> Option<PathBuf> {
    let uid = invoking_uid()?;
    let home = fs::read_to_string("/etc/passwd")
        .ok()?
        .lines()
        .find_map(|line| {
            let fields = line.split(':').collect::<Vec<_>>();
            (fields.len() >= 6
                && fields[2].parse::<u32>().ok() == Some(uid)
                && !fields[5].is_empty())
            .then(|| PathBuf::from(fields[5]))
        })?;
    fs::canonicalize(home).ok()
}

fn root_is_too_broad(path: &Path) -> bool {
    path.components()
        .filter(|component| matches!(component, Component::Normal(_)))
        .count()
        < 2
}

fn root_is_filesystem_root(path: &Path) -> bool {
    path.components()
        .all(|component| !matches!(component, Component::Normal(_)))
}

fn valid_nonce(nonce: &str) -> bool {
    nonce.len() == 43
        && nonce
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

fn request_path_matches_nonce(path: &Path, nonce: &str) -> bool {
    let expected_name = format!("request-{nonce}.json");
    let Some(parent_name) = path
        .parent()
        .and_then(Path::file_name)
        .and_then(OsStr::to_str)
    else {
        return false;
    };
    path.file_name() == Some(OsStr::new(&expected_name))
        && parent_name
            .strip_prefix("spaceprune-readonly-")
            .is_some_and(|suffix| !suffix.is_empty())
}

fn random_nonce() -> Result<String, HelperError> {
    let mut value = [0_u8; 32];
    getrandom::fill(&mut value).map_err(|_| HelperError)?;
    Ok(URL_SAFE_NO_PAD.encode(value))
}

fn unix_timestamp() -> Result<u64, HelperError> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .map_err(|_| HelperError)
}

fn private_file_options() -> OpenOptions {
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;

        let mut options = OpenOptions::new();
        options.mode(0o600);
        options
    }
    #[cfg(not(unix))]
    {
        OpenOptions::new()
    }
}

fn tighten_directory_permissions(path: &Path) -> Result<(), HelperError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700)).map_err(|_| HelperError)?;
    }
    #[cfg(target_os = "windows")]
    spaceprune_windows_elevation::secure_created_metadata_path(
        path,
        spaceprune_windows_elevation::PrivatePathKind::Directory,
    )
    .map_err(|_| HelperError)?;
    #[cfg(not(any(unix, target_os = "windows")))]
    let _ = path;
    Ok(())
}

fn secure_created_file(path: &Path) -> Result<(), HelperError> {
    #[cfg(target_os = "windows")]
    spaceprune_windows_elevation::secure_created_metadata_path(
        path,
        spaceprune_windows_elevation::PrivatePathKind::File,
    )
    .map_err(|_| HelperError)?;
    #[cfg(not(target_os = "windows"))]
    let _ = path;
    Ok(())
}

fn validate_open_ipc_file(path: &Path, file: &File) -> Result<(), HelperError> {
    let path_metadata = fs::symlink_metadata(path).map_err(|_| HelperError)?;
    let metadata = file.metadata().map_err(|_| HelperError)?;
    if !metadata.is_file() || path_metadata.file_type().is_symlink() {
        return Err(HelperError);
    }
    #[cfg(unix)]
    validate_unix_ipc_permissions(path, &path_metadata, &metadata)?;
    #[cfg(target_os = "windows")]
    validate_windows_ipc_location(path, file, &path_metadata)?;
    Ok(())
}

#[cfg(unix)]
fn validate_unix_ipc_permissions(
    path: &Path,
    path_metadata: &fs::Metadata,
    metadata: &fs::Metadata,
) -> Result<(), HelperError> {
    use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};

    let parent = path.parent().ok_or(HelperError)?;
    let parent_metadata = fs::symlink_metadata(parent).map_err(|_| HelperError)?;
    let expected_uid = invoking_uid().ok_or(HelperError)?;
    if metadata.uid() != expected_uid
        || metadata.dev() != path_metadata.dev()
        || metadata.ino() != path_metadata.ino()
        || metadata.nlink() != 1
        || metadata.permissions().mode() & 0o077 != 0
        || parent_metadata.uid() != expected_uid
        || parent_metadata.permissions().mode() & 0o077 != 0
    {
        return Err(HelperError);
    }
    Ok(())
}

#[cfg(target_os = "windows")]
fn validate_windows_ipc_location(
    path: &Path,
    file: &File,
    path_metadata: &fs::Metadata,
) -> Result<(), HelperError> {
    use std::os::windows::fs::MetadataExt as _;

    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;

    let parent = path.parent().ok_or(HelperError)?;
    let parent_metadata = fs::symlink_metadata(parent).map_err(|_| HelperError)?;
    if path_metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
        || parent_metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
        || !spaceprune_windows_elevation::valid_metadata_root(parent)
    {
        return Err(HelperError);
    }
    use spaceprune_windows_elevation::{PrivatePathKind, metadata_path_identity};
    let owner = spaceprune_windows_elevation::metadata_open_file_identity(path, file)
        .map_err(|_| HelperError)?;
    if metadata_path_identity(parent, PrivatePathKind::Directory).map_err(|_| HelperError)? != owner
    {
        return Err(HelperError);
    }
    Ok(())
}

#[cfg(unix)]
fn invoking_uid() -> Option<u32> {
    std::env::var("PKEXEC_UID")
        .ok()
        .and_then(|value| value.parse().ok())
        .or_else(|| {
            std::env::var("SUDO_UID")
                .ok()
                .and_then(|value| value.parse().ok())
        })
        .or_else(effective_uid)
}

#[cfg(unix)]
fn effective_uid() -> Option<u32> {
    Some(rustix::process::geteuid().as_raw())
}

mod host_paths_serde {
    use std::{ffi::OsString, path::PathBuf};

    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    #[derive(Serialize, Deserialize)]
    #[serde(rename_all = "snake_case")]
    enum EncodedPath {
        Unix(Vec<u8>),
        Windows(Vec<u16>),
        Text(String),
    }

    pub(super) fn serialize<S>(paths: &[PathBuf], serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        paths
            .iter()
            .map(|path| encode(path))
            .collect::<Vec<_>>()
            .serialize(serializer)
    }

    pub(super) fn deserialize<'de, D>(deserializer: D) -> Result<Vec<PathBuf>, D::Error>
    where
        D: Deserializer<'de>,
    {
        Vec::<EncodedPath>::deserialize(deserializer)?
            .into_iter()
            .map(|path| decode(path).map_err(serde::de::Error::custom))
            .collect()
    }

    fn encode(path: &std::path::Path) -> EncodedPath {
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStrExt as _;
            EncodedPath::Unix(path.as_os_str().as_bytes().to_vec())
        }
        #[cfg(windows)]
        {
            use std::os::windows::ffi::OsStrExt as _;
            EncodedPath::Windows(path.as_os_str().encode_wide().collect())
        }
        #[cfg(not(any(unix, windows)))]
        {
            EncodedPath::Text(path.to_string_lossy().into_owned())
        }
    }

    fn decode(path: EncodedPath) -> Result<PathBuf, &'static str> {
        match path {
            EncodedPath::Unix(bytes) => {
                #[cfg(unix)]
                {
                    use std::os::unix::ffi::OsStringExt as _;
                    Ok(PathBuf::from(OsString::from_vec(bytes)))
                }
                #[cfg(not(unix))]
                {
                    let _ = bytes;
                    Err("Helper 请求路径平台不匹配")
                }
            }
            EncodedPath::Windows(wide) => {
                #[cfg(windows)]
                {
                    use std::os::windows::ffi::OsStringExt as _;
                    Ok(PathBuf::from(OsString::from_wide(&wide)))
                }
                #[cfg(not(windows))]
                {
                    let _ = wide;
                    Err("Helper 请求路径平台不匹配")
                }
            }
            EncodedPath::Text(value) => {
                #[cfg(not(any(unix, windows)))]
                {
                    Ok(PathBuf::from(value))
                }
                #[cfg(any(unix, windows))]
                {
                    let _ = value;
                    Err("Helper 请求路径平台不匹配")
                }
            }
        }
    }
}

#[cfg(test)]
mod tests;
