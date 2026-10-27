#![forbid(unsafe_code)]
#![deny(missing_docs)]
#![deny(rustdoc::broken_intra_doc_links)]

//! SpacePrune 的 Linux 平台集成。
//!
//! 当前实现提供 systemd user 自动化；领域清理由独立插件实现。

#[cfg(target_os = "linux")]
include!("linux.rs");
