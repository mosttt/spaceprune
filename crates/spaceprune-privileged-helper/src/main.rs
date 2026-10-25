#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]
#![forbid(unsafe_code)]
#![deny(missing_docs)]
#![deny(rustdoc::broken_intra_doc_links)]

//! SpacePrune 只读权限 Helper 入口。

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
    let code = spaceprune_readonly_protocol::run_from_environment().unwrap_or(64);
    std::process::exit(code);
}
