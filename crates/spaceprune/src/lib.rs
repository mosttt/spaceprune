#![forbid(unsafe_code)]
#![deny(missing_docs)]
#![deny(rustdoc::broken_intra_doc_links)]

//! SpacePrune 的应用级核心库。
//!
//! 本 crate 统一装配配置、扫描引擎、内置 Handler、外部插件和平台能力，供 CLI 与桌面端共同调用。
//! 它不依赖命令行解析或终端界面。

mod assembly;
mod automation;
#[cfg(any(target_os = "linux", target_os = "windows", target_os = "macos"))]
mod cleanup_access;
mod native_access;
mod plugins;
mod system_capabilities;

/// 面向 CLI、桌面端和其他应用的结构化核心接口。
pub mod api;

pub use api::*;

pub(crate) use assembly::{build_engine, build_readonly_engine};

/// 当前 SpacePrune 核心库版本。
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Dispatch fixed native component installation before initializing an application host.
/// This entry never runs in the model process and does not authorize scanning or deletion.
#[doc(hidden)]
pub fn run_native_installer_if_requested() -> Option<i32> {
    #[cfg(target_os = "windows")]
    return spaceprune_access::windows_native::run_installer_if_requested();
    #[cfg(not(target_os = "windows"))]
    None
}

/// Dispatch the dedicated cleanup helper; application hosts must not call this entry.
#[doc(hidden)]
pub fn run_cleanup_helper_if_requested() -> Option<i32> {
    #[cfg(any(target_os = "linux", target_os = "windows", target_os = "macos"))]
    return cleanup_access::run_if_requested();
    #[cfg(not(any(target_os = "linux", target_os = "windows", target_os = "macos")))]
    None
}

/// Dispatch the ordinary-user Linux Go cache worker before initializing application threads.
/// Calling this also registers the current executable as a worker entrypoint, so a CLI or
/// embedded host can clean owned read-only Go modules without installing a privileged helper.
#[doc(hidden)]
pub fn run_go_cache_worker_if_requested() -> Option<i32> {
    #[cfg(target_os = "linux")]
    {
        if let Some(code) = spaceprune_access::go_cache::run_if_requested() {
            return Some(code);
        }
        let _ = spaceprune_access::go_cache::register_host_entrypoint();
    }
    None
}
