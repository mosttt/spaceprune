#![deny(missing_docs)]
#![deny(rustdoc::broken_intra_doc_links)]
#![deny(unsafe_code)]

//! 将 Windows UAC 启动与句柄级只读遍历所需的少量系统调用隔离在桌面宿主之外。

#[cfg(target_os = "windows")]
use std::{ffi::OsString, path::Path, sync::atomic::AtomicBool, time::Duration};

#[cfg(target_os = "windows")]
#[allow(unsafe_code)]
mod native;

#[cfg(target_os = "windows")]
mod shell_path;

#[cfg(target_os = "windows")]
#[allow(unsafe_code)]
mod private_acl;

#[cfg(target_os = "windows")]
#[allow(unsafe_code)]
mod pipe;
#[cfg(target_os = "windows")]
pub use pipe::LocalPipe;

#[cfg(target_os = "windows")]
#[allow(unsafe_code)]
mod readonly;

#[cfg(target_os = "windows")]
#[allow(unsafe_code)]
mod logon;
#[cfg(target_os = "windows")]
pub use logon::LogonIdentity;

#[cfg(target_os = "windows")]
#[allow(unsafe_code)]
mod host_process;
#[cfg(target_os = "windows")]
#[allow(unsafe_code)]
mod installation;
#[cfg(target_os = "windows")]
#[allow(unsafe_code)]
mod installer;
#[cfg(target_os = "windows")]
pub use host_process::{HostProcess, current_process_creation_time};
#[cfg(target_os = "windows")]
pub use installation::{SignedFile, TrustedExecutable};
#[cfg(target_os = "windows")]
pub use installer::install_package_in_worker;
#[cfg(any(windows, test))]
mod system_locations;
#[cfg(windows)]
pub use system_locations::SystemLocations;

/// Hold a signed executable and its directory chain while a native process is being launched.
#[cfg(target_os = "windows")]
pub fn trusted_installed_executable(path: &Path) -> Option<TrustedExecutable> {
    installation::trusted_executable(path)
}

/// Pin signed package data in a local directory without granting permission to execute it.
/// The caller must still match its expected package digest and fixed role before installation.
#[cfg(target_os = "windows")]
pub fn trusted_signed_file(path: &Path) -> Option<SignedFile> {
    installation::signed_file(path)
}

/// Native Program Files location from the OS, never from an environment variable.
#[cfg(target_os = "windows")]
pub fn native_program_files_directory() -> Option<std::path::PathBuf> {
    native::native_program_files_directory()
}

/// Whether the current process already holds an elevated Windows token.
#[cfg(target_os = "windows")]
pub fn is_elevated() -> bool {
    native::is_elevated()
}

/// Windows 句柄级只读遍历返回的聚合元数据。
#[cfg(target_os = "windows")]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ReadonlyMetadata {
    /// 文件逻辑字节总数。
    pub logical_bytes: u64,
    /// 已检查的文件系统条目数。
    pub inspected_entries: u64,
    /// 因权限、reparse point、类型、深度或数量限制跳过的条目数。
    pub skipped_entries: u64,
    /// 成功打开的请求根数量。
    pub accessible_roots: u64,
}

/// Windows 句柄查询得到的稳定普通文件身份。
#[cfg(target_os = "windows")]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadonlyFileIdentity {
    /// 文件所在卷的系统序列号。
    pub volume_serial_number: u64,
    /// Windows `FILE_ID_128` 的原始 16 字节标识。
    pub file_id: [u8; 16],
    /// 文件完整逻辑长度。
    pub logical_bytes: u64,
    /// Windows 文件最后写入时间。
    pub last_write_time: i64,
    /// Windows 文件元数据变化时间。
    pub change_time: i64,
}

/// Windows 句柄级有界正文读取结果。
#[cfg(target_os = "windows")]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoundedReadonlyFile {
    /// 读取前后均复核通过的普通文件身份。
    pub identity: ReadonlyFileIdentity,
    /// 文件完整正文；调用方仍须执行 UTF-8 与敏感信息门禁。
    pub bytes: Vec<u8>,
}

/// Windows 句柄级文件身份或正文读取失败原因。
#[cfg(target_os = "windows")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadonlyFileError {
    /// 路径、组件、类型、reparse point 或卷边界不符合只读约束。
    Ineligible,
    /// 文件身份在发现、授权或读取期间发生变化。
    Changed,
    /// 文件超过调用方声明的完整读取上限。
    TooLarge,
    /// 当前系统无法建立所需安全句柄。
    Unavailable,
}

/// Windows 私有持久化路径的预期类型。
#[cfg(target_os = "windows")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrivatePathKind {
    /// 只允许普通目录，并让受限 DACL 继承到其后代。
    Directory,
    /// 只允许普通文件，不接受 reparse point。
    File,
}

/// Windows 私有持久化路径 ACL 操作失败原因。
#[cfg(target_os = "windows")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrivatePathError {
    /// 目标类型、reparse point 或所有者不符合私有路径约束。
    Ineligible,
    /// DACL does not match the account- or login-scoped policy required by this entry point.
    Permissions,
    /// 系统无法安全打开、设置或查询目标 ACL。
    Unavailable,
}

/// 已提升权限的 Helper 进程退出状态。
#[cfg(target_os = "windows")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ElevatedExitStatus {
    code: u32,
}

#[cfg(target_os = "windows")]
impl ElevatedExitStatus {
    /// 进程是否成功退出。
    #[must_use]
    pub fn success(self) -> bool {
        self.code == 0
    }

    /// 返回进程退出码；超出 `i32` 范围时返回 `None`。
    #[must_use]
    pub fn code(self) -> Option<i32> {
        i32::try_from(self.code).ok()
    }
}

/// Windows UAC Helper 启动失败原因。
#[cfg(target_os = "windows")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ElevationError {
    /// 用户拒绝或关闭了系统 UAC 提示。
    UserDenied,
    /// 调用方在 Helper 启动后取消了等待。
    Cancelled,
    /// Helper 未在限定时间内退出。
    TimedOut,
    /// 系统无法安全启动或查询 Helper。
    Unavailable,
}

/// 通过 Windows `runas` 系统 UI 启动固定 Helper，并等待其退出。
///
/// UAC 安全桌面由 Windows 控制；系统提示仍在显示时，调用方不能替用户关闭它。
/// Callers must withdraw the helper's IPC directory before cancellation so it can exit
/// cooperatively when the returned UAC process handle does not permit termination.
#[cfg(target_os = "windows")]
pub fn run_elevated_and_wait(
    executable: &Path,
    arguments: &[OsString],
    timeout: Duration,
    cancellation: &AtomicBool,
) -> Result<ElevatedExitStatus, ElevationError> {
    native::run_elevated_and_wait(executable, arguments, timeout, cancellation, |_| {})
}

/// Launch the elevated process and report its kernel PID before waiting for actual termination.
/// The callback is for authenticating an IPC peer; the owned process handle prevents PID reuse.
#[cfg(target_os = "windows")]
pub fn run_elevated_with_started(
    executable: &Path,
    arguments: &[OsString],
    timeout: Duration,
    cancellation: &AtomicBool,
    started: impl FnOnce(u32),
) -> Result<ElevatedExitStatus, ElevationError> {
    native::run_elevated_and_wait(executable, arguments, timeout, cancellation, started)
}

/// 判断现有文件是否位于 Windows Known Folder API 返回的 Program Files 目录内。
///
/// 该判断不读取可被调用进程修改的 `ProgramFiles` 环境变量。
#[cfg(target_os = "windows")]
#[must_use]
pub fn is_under_program_files(path: &Path) -> bool {
    let Ok(path) = std::fs::canonicalize(path) else {
        return false;
    };
    native::program_files_roots()
        .into_iter()
        .filter_map(|root| std::fs::canonicalize(root).ok())
        .any(|root| path != root && path.starts_with(root))
}

/// 从固定根句柄逐级打开子项并聚合元数据，不跟随任何 reparse point。
///
/// `should_stop` 会在打开每个根和处理每个目录项前调用。该函数只请求目录列举和文件属性权限，
/// 不打开文件内容，也不返回名称或路径。
#[cfg(target_os = "windows")]
pub fn measure_readonly_metadata(
    roots: &[std::path::PathBuf],
    max_entries: u64,
    max_depth: usize,
    should_stop: impl FnMut() -> bool,
) -> ReadonlyMetadata {
    readonly::measure(roots, max_entries, max_depth, should_stop)
}

/// Open a local metadata root component by component, rejecting reparse points and UNC paths.
#[cfg(target_os = "windows")]
pub fn valid_metadata_root(path: &Path) -> bool {
    readonly::valid_metadata_root(path)
}

/// 从授权根句柄逐组件打开普通文件并返回稳定身份，不跟随任何 reparse point。
#[cfg(target_os = "windows")]
pub fn identify_readonly_file(
    root: &Path,
    file: &Path,
) -> Result<ReadonlyFileIdentity, ReadonlyFileError> {
    readonly::identify_file(root, file)
}

/// 从授权根句柄逐组件完整读取普通文件，并在读取前后复核同一身份。
#[cfg(target_os = "windows")]
pub fn read_bounded_file(
    root: &Path,
    file: &Path,
    expected: &ReadonlyFileIdentity,
    max_bytes: usize,
) -> Result<BoundedReadonlyFile, ReadonlyFileError> {
    readonly::read_bounded_file(root, file, expected, max_bytes)
}

/// 将现有 Windows 普通文件或目录改为受保护的私有 DACL，并在同一句柄上复核。
#[cfg(target_os = "windows")]
pub fn harden_private_path(path: &Path, kind: PrivatePathKind) -> Result<(), PrivatePathError> {
    private_acl::harden(path, kind)
}

/// 验证现有 Windows 普通文件或目录拥有受保护且主体受限的私有 DACL。
#[cfg(target_os = "windows")]
pub fn validate_private_path(path: &Path, kind: PrivatePathKind) -> Result<(), PrivatePathError> {
    private_acl::validate(path, kind)
}

/// Give a newly created IPC object the invoking user as owner and a protected private DACL.
/// Call only for files or directories created by this operation, never for discovered user data.
#[cfg(target_os = "windows")]
pub fn secure_created_private_path(
    path: &Path,
    kind: PrivatePathKind,
) -> Result<(), PrivatePathError> {
    private_acl::secure_created(path, kind)
}

/// Owner/login identity validated from a temporary IPC object's exact protected ACL.
#[cfg(target_os = "windows")]
pub use private_acl::logon::LogonIpcIdentity;

/// Secure a newly created temporary IPC object for this login, SYSTEM and administrators.
/// Account ownership remains unchanged in meaning; ordinary profile/configuration paths must
/// continue using the account-scoped private-path APIs instead.
#[cfg(target_os = "windows")]
pub fn secure_created_logon_path(
    path: &Path,
    kind: PrivatePathKind,
) -> Result<(), PrivatePathError> {
    private_acl::logon::secure_created(path, kind)
}

/// Validate an IPC object's exact login DACL and return its original owner/login identity.
#[cfg(target_os = "windows")]
pub fn logon_path_identity(
    path: &Path,
    kind: PrivatePathKind,
) -> Result<LogonIpcIdentity, PrivatePathError> {
    private_acl::logon::identity(path, kind)
}

/// Validate a held IPC file's login DACL and identity against its no-reparse named path.
#[cfg(target_os = "windows")]
pub fn logon_open_file_identity(
    path: &Path,
    file: &std::fs::File,
) -> Result<LogonIpcIdentity, PrivatePathError> {
    private_acl::logon::file_identity(path, file)
}

/// Login-bound metadata IPC identity, with a narrow OS-verified LocalSystem process variant.
#[cfg(target_os = "windows")]
pub use private_acl::metadata::MetadataIpcIdentity;

/// Secure newly created metadata IPC for a login, or an elevated session-zero LocalSystem host.
/// This compatibility entry point does not extend cleanup or ordinary account ACL permissions.
#[cfg(target_os = "windows")]
pub fn secure_created_metadata_path(
    path: &Path,
    kind: PrivatePathKind,
) -> Result<(), PrivatePathError> {
    private_acl::metadata::secure_created(path, kind)
}

/// Validate a metadata IPC object's exact protected ACL before attaching to its original host.
#[cfg(target_os = "windows")]
pub fn metadata_path_identity(
    path: &Path,
    kind: PrivatePathKind,
) -> Result<MetadataIpcIdentity, PrivatePathError> {
    private_acl::metadata::identity(path, kind)
}

/// Validate a held metadata file's ACL and identity against its no-reparse named path.
#[cfg(target_os = "windows")]
pub fn metadata_open_file_identity(
    path: &Path,
    file: &std::fs::File,
) -> Result<MetadataIpcIdentity, PrivatePathError> {
    private_acl::metadata::file_identity(path, file)
}

/// Return the current process user's canonical SID, independent of account environment variables.
#[cfg(target_os = "windows")]
pub fn current_user_sid() -> Result<String, PrivatePathError> {
    private_acl::current_user_sid()
}

/// Check a private IPC object's owner and exact DACL against the session's invoking user.
#[cfg(target_os = "windows")]
pub fn validate_private_path_for_owner(
    path: &Path,
    kind: PrivatePathKind,
    owner: &str,
) -> Result<(), PrivatePathError> {
    private_acl::validate_for_owner(path, kind, owner)
}

/// Identify an IPC creator only after validating its protected owner/System/Administrators DACL.
#[cfg(target_os = "windows")]
pub fn private_path_owner(path: &Path, kind: PrivatePathKind) -> Result<String, PrivatePathError> {
    private_acl::owner(path, kind)
}

/// Validate a held IPC file's private ACL and identity against its no-reparse named path.
#[cfg(target_os = "windows")]
pub fn private_open_file_owner(
    path: &Path,
    file: &std::fs::File,
) -> Result<String, PrivatePathError> {
    private_acl::open_file_owner(path, file)
}

#[cfg(any(target_os = "windows", test))]
fn quote_windows_argument(value: &[u16]) -> Vec<u16> {
    if !value.is_empty()
        && !value
            .iter()
            .any(|value| b" \t\"".iter().any(|unit| *value == u16::from(*unit)))
    {
        return value.to_vec();
    }

    let mut quoted = Vec::with_capacity(value.len() + 2);
    quoted.push(b'"' as u16);
    let mut backslashes = 0_usize;
    for unit in value {
        if *unit == b'\\' as u16 {
            backslashes += 1;
            continue;
        }
        if *unit == b'"' as u16 {
            quoted.extend(std::iter::repeat_n(b'\\' as u16, backslashes * 2 + 1));
        } else {
            quoted.extend(std::iter::repeat_n(b'\\' as u16, backslashes));
        }
        backslashes = 0;
        quoted.push(*unit);
    }
    quoted.extend(std::iter::repeat_n(b'\\' as u16, backslashes * 2));
    quoted.push(b'"' as u16);
    quoted
}

#[cfg(test)]
mod tests {
    use super::quote_windows_argument;

    fn quote(value: &str) -> String {
        String::from_utf16(&quote_windows_argument(
            &value.encode_utf16().collect::<Vec<_>>(),
        ))
        .expect("有效 UTF-16")
    }

    #[test]
    fn quotes_windows_arguments_for_command_line_to_argv_w() {
        assert_eq!(quote("plain"), "plain");
        assert_eq!(quote(""), "\"\"");
        assert_eq!(quote("two words"), "\"two words\"");
        assert_eq!(quote("a\\\"b"), "\"a\\\\\\\"b\"");
        assert_eq!(quote("C:\\with space\\"), "\"C:\\with space\\\\\"");
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn known_program_files_roots_do_not_trust_the_build_directory() {
        let roots = super::native::program_files_roots();
        assert!(!roots.is_empty());
        assert!(roots.iter().all(|root| root.is_absolute()));
        assert!(!super::is_under_program_files(
            &std::env::current_exe().expect("当前测试程序")
        ));
    }
}
