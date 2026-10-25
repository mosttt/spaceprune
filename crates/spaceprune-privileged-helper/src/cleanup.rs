#![forbid(unsafe_code)]
#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]
//! Dedicated native cleanup capability; no host GUI or CLI command dispatch.

fn main() {
    #[cfg(target_os = "linux")]
    if let Some(code) = spaceprune_access::go_cache::run_if_requested() {
        std::process::exit(code);
    }
    #[cfg(target_os = "linux")]
    if [
        spaceprune_access::helper::HANDLERS_SUPPORT,
        spaceprune_access::helper::DEVELOPER_HANDLERS_SUPPORT,
        spaceprune_access::helper::PROMPT_SUPPORT,
    ]
    .iter()
    .any(|probe| std::env::args_os().skip(1).eq([*probe]))
    {
        return;
    }
    #[cfg(any(target_os = "linux", target_os = "windows", target_os = "macos"))]
    let code = spaceprune::run_cleanup_helper_if_requested().unwrap_or(64);
    #[cfg(not(any(target_os = "linux", target_os = "windows", target_os = "macos")))]
    let code = 64;
    std::process::exit(code);
}
