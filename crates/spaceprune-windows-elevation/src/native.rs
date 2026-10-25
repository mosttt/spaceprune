use std::{
    ffi::{OsStr, OsString},
    mem::size_of,
    os::windows::ffi::{OsStrExt as _, OsStringExt as _},
    path::{Path, PathBuf},
    ptr,
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};

use windows_sys::Win32::{
    Foundation::{CloseHandle, ERROR_CANCELLED, GetLastError, HANDLE, WAIT_OBJECT_0, WAIT_TIMEOUT},
    System::{
        Com::{
            COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE, CoInitializeEx, CoTaskMemFree,
            CoUninitialize,
        },
        Threading::{
            GetExitCodeProcess, GetProcessId, INFINITE, TerminateProcess, WaitForSingleObject,
        },
    },
    UI::{
        Shell::{
            FOLDERID_ProgramFiles, FOLDERID_ProgramFilesX64, FOLDERID_ProgramFilesX86,
            KF_FLAG_DEFAULT, SEE_MASK_NOASYNC, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW,
            SHGetKnownFolderPath, ShellExecuteExW,
        },
        WindowsAndMessaging::SW_HIDE,
    },
};

use crate::{ElevatedExitStatus, ElevationError, quote_windows_argument};

pub(super) fn is_elevated() -> bool {
    use windows_sys::Win32::{
        Security::{GetTokenInformation, TOKEN_ELEVATION, TOKEN_QUERY, TokenElevation},
        System::Threading::{GetCurrentProcess, OpenProcessToken},
    };
    let mut token = ptr::null_mut();
    // SAFETY: The output pointer is writable; ProcessHandle closes the returned token handle.
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &raw mut token) } == 0 {
        return false;
    }
    let token = ProcessHandle(token);
    let mut elevation = TOKEN_ELEVATION::default();
    let mut returned = 0;
    // SAFETY: The token and correctly sized output buffer remain valid for the call.
    (unsafe {
        GetTokenInformation(
            token.0,
            TokenElevation,
            (&raw mut elevation).cast(),
            size_of::<TOKEN_ELEVATION>() as u32,
            &raw mut returned,
        )
    }) != 0
        && elevation.TokenIsElevated != 0
}

const WAIT_INTERVAL_MILLIS: u32 = 100;
const CANCELLED_EXIT_CODE: u32 = 1_223;
const TIMED_OUT_EXIT_CODE: u32 = 1_460;

pub(super) fn program_files_roots() -> Vec<PathBuf> {
    [
        FOLDERID_ProgramFiles,
        FOLDERID_ProgramFilesX64,
        FOLDERID_ProgramFilesX86,
    ]
    .iter()
    .filter_map(known_folder_path)
    .collect()
}

pub(super) fn native_program_files_directory() -> Option<PathBuf> {
    known_folder_path(&FOLDERID_ProgramFiles)
}

fn known_folder_path(folder_id: &windows_sys::core::GUID) -> Option<PathBuf> {
    known_folder_path_with_flags(folder_id, KF_FLAG_DEFAULT as u32)
}

pub(super) fn known_folder_path_with_flags(
    folder_id: &windows_sys::core::GUID,
    flags: u32,
) -> Option<PathBuf> {
    let mut raw = ptr::null_mut();
    // SAFETY: `folder_id` 和输出指针在调用期间有效；空 token 表示当前系统上下文。
    let result = unsafe { SHGetKnownFolderPath(folder_id, flags, ptr::null_mut(), &raw mut raw) };
    if result < 0 {
        if !raw.is_null() {
            // SAFETY: 失败路径若仍返回系统分配缓冲区，也必须由 `CoTaskMemFree` 释放。
            unsafe { CoTaskMemFree(raw.cast()) };
        }
        return None;
    }
    if raw.is_null() {
        return None;
    }
    let mut length = 0_usize;
    // SAFETY: 成功的 `SHGetKnownFolderPath` 返回以 NUL 结尾、由系统分配的 UTF-16 字符串。
    while unsafe { *raw.add(length) } != 0 {
        length += 1;
    }
    // SAFETY: 上述扫描确定了初始化的 UTF-16 范围，随后复制到拥有所有权的 `OsString`。
    let value = OsString::from_wide(unsafe { std::slice::from_raw_parts(raw, length) });
    // SAFETY: `raw` 由 `SHGetKnownFolderPath` 分配且只在此处释放一次。
    unsafe { CoTaskMemFree(raw.cast()) };
    Some(PathBuf::from(value))
}

pub(super) fn run_elevated_and_wait(
    executable: &Path,
    arguments: &[OsString],
    timeout: Duration,
    cancellation: &AtomicBool,
    started: impl FnOnce(u32),
) -> Result<ElevatedExitStatus, ElevationError> {
    if cancellation.load(Ordering::Acquire) {
        return Err(ElevationError::Cancelled);
    }
    let deadline = Instant::now() + timeout;
    let directory = wide_null(
        executable
            .parent()
            .ok_or(ElevationError::Unavailable)?
            .as_os_str(),
    );
    let executable = wide_null(executable.as_os_str());
    let parameters = command_parameters(arguments);
    let verb = wide_null(OsStr::new("runas"));
    let mut information = SHELLEXECUTEINFOW {
        cbSize: u32::try_from(size_of::<SHELLEXECUTEINFOW>())
            .map_err(|_| ElevationError::Unavailable)?,
        fMask: SEE_MASK_NOASYNC | SEE_MASK_NOCLOSEPROCESS,
        lpVerb: verb.as_ptr(),
        lpFile: executable.as_ptr(),
        lpParameters: parameters.as_ptr(),
        lpDirectory: directory.as_ptr(),
        nShow: SW_HIDE,
        ..Default::default()
    };
    let apartment = ComApartment::new()?;

    // SAFETY: 所有 UTF-16 缓冲区在调用期间保持存活并以 NUL 结尾，结构体尺寸和空字段合法。
    if unsafe { ShellExecuteExW(&raw mut information) } == 0 {
        // SAFETY: Capture the thread-local error before any other system call.
        let error = unsafe { GetLastError() };
        if cancellation.load(Ordering::Acquire) {
            return Err(ElevationError::Cancelled);
        }
        if Instant::now() >= deadline {
            return Err(ElevationError::TimedOut);
        }
        return Err(if error == ERROR_CANCELLED {
            ElevationError::UserDenied
        } else {
            ElevationError::Unavailable
        });
    }
    drop(apartment);
    if information.hProcess.is_null() {
        return Err(ElevationError::Unavailable);
    }
    let process = ProcessHandle(information.hProcess);
    if cancellation.load(Ordering::Acquire) {
        terminate(process.0, CANCELLED_EXIT_CODE)?;
        return Err(ElevationError::Cancelled);
    }
    if Instant::now() >= deadline {
        terminate(process.0, TIMED_OUT_EXIT_CODE)?;
        return Err(ElevationError::TimedOut);
    }
    // SAFETY: The process handle is owned until wait_for_process returns.
    let pid = unsafe { GetProcessId(process.0) };
    if pid == 0 {
        terminate(process.0, CANCELLED_EXIT_CODE)?;
        return Err(ElevationError::Unavailable);
    }
    started(pid);
    wait_for_process(process, deadline, cancellation)
}

fn wait_for_process(
    process: ProcessHandle,
    deadline: Instant,
    cancellation: &AtomicBool,
) -> Result<ElevatedExitStatus, ElevationError> {
    loop {
        // SAFETY: `ProcessHandle` 独占并保持有效进程句柄，等待不修改 Rust 内存。
        match unsafe { WaitForSingleObject(process.0, WAIT_INTERVAL_MILLIS) } {
            WAIT_OBJECT_0 => {
                let mut code = 0_u32;
                // SAFETY: 句柄仍有效，退出码指针指向可写的本地 `u32`。
                if unsafe { GetExitCodeProcess(process.0, &raw mut code) } == 0 {
                    return Err(ElevationError::Unavailable);
                }
                return Ok(ElevatedExitStatus { code });
            }
            WAIT_TIMEOUT if cancellation.load(Ordering::Acquire) => {
                terminate(process.0, CANCELLED_EXIT_CODE)?;
                return Err(ElevationError::Cancelled);
            }
            WAIT_TIMEOUT if Instant::now() < deadline => {}
            WAIT_TIMEOUT => {
                terminate(process.0, TIMED_OUT_EXIT_CODE)?;
                return Err(ElevationError::TimedOut);
            }
            _ => return Err(ElevationError::Unavailable),
        }
    }
}

fn terminate(process: HANDLE, exit_code: u32) -> Result<(), ElevationError> {
    // SAFETY: The caller holds the process handle through termination and the final wait.
    let _ = unsafe { TerminateProcess(process, exit_code) };
    // UAC may grant only query/synchronize rights. The caller must also withdraw its IPC
    // control directory so the helper exits cooperatively when termination is denied.
    // SAFETY: The owned handle remains valid; a timeout must never report a live worker as stopped.
    if unsafe { WaitForSingleObject(process, INFINITE) } == WAIT_OBJECT_0 {
        Ok(())
    } else {
        Err(ElevationError::Unavailable)
    }
}

fn command_parameters(arguments: &[OsString]) -> Vec<u16> {
    let mut parameters = Vec::new();
    for argument in arguments {
        if !parameters.is_empty() {
            parameters.push(b' ' as u16);
        }
        let wide = argument.encode_wide().collect::<Vec<_>>();
        parameters.extend(quote_windows_argument(&wide));
    }
    parameters.push(0);
    parameters
}

fn wide_null(value: &OsStr) -> Vec<u16> {
    value.encode_wide().chain(Some(0)).collect()
}

struct ProcessHandle(HANDLE);

struct ComApartment;
impl ComApartment {
    fn new() -> Result<Self, ElevationError> {
        // SAFETY: Native authorization runs on its own thread, with no existing COM apartment.
        let result = unsafe {
            CoInitializeEx(
                ptr::null(),
                (COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE) as u32,
            )
        };
        if result < 0 {
            Err(ElevationError::Unavailable)
        } else {
            Ok(Self)
        }
    }
}
impl Drop for ComApartment {
    fn drop(&mut self) {
        // SAFETY: This balances a successful CoInitializeEx on the same thread.
        unsafe { CoUninitialize() };
    }
}

impl Drop for ProcessHandle {
    fn drop(&mut self) {
        // SAFETY: `ProcessHandle` 独占非空句柄且只在此处关闭一次。
        let _ = unsafe { CloseHandle(self.0) };
        self.0 = ptr::null_mut();
    }
}
