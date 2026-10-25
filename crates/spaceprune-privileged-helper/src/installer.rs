#![forbid(unsafe_code)]
//! Native installation/registration of Core-owned, fixed-capability components.

fn main() {
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
    #[cfg(target_os = "linux")]
    let code = spaceprune_access::helper::run_installer_from_environment().unwrap_or(64);
    #[cfg(target_os = "macos")]
    let code = spaceprune_access::macos::run_installer_from_environment().unwrap_or(64);
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    let code = 64;
    std::process::exit(code);
}
