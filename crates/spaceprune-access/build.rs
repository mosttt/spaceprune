//! Bind platform distribution metadata at build time, never from runtime environment variables.
fn main() {
    println!("cargo:rerun-if-env-changed=SPACEPRUNE_MACOS_DISTRIBUTION");
    let distribution =
        std::env::var("SPACEPRUNE_MACOS_DISTRIBUTION").unwrap_or_else(|_| "cli".into());
    assert!(
        matches!(distribution.as_str(), "cli" | "desktop"),
        "Unsupported macOS native distribution"
    );
    println!("cargo:rustc-env=SPACEPRUNE_MACOS_DISTRIBUTION={distribution}");
    windows_package_pin();
}

fn windows_package_pin() {
    use std::{env, fs, io::Read, path::PathBuf};
    println!("cargo:rerun-if-env-changed=SPACEPRUNE_WINDOWS_PACKAGE_IDENTITY");
    let output = PathBuf::from(env::var_os("OUT_DIR").expect("Cargo output directory"))
        .join("windows-package-identity.json");
    let mut bytes = Vec::new();
    if env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows")
        && let Some(path) = env::var_os("SPACEPRUNE_WINDOWS_PACKAGE_IDENTITY")
    {
        let path = PathBuf::from(path);
        println!("cargo:rerun-if-changed={}", path.display());
        fs::File::open(path)
            .expect("Readable Windows package identity")
            .take(16 * 1024 + 1)
            .read_to_end(&mut bytes)
            .expect("Windows package identity bytes");
        assert!(
            !bytes.is_empty() && bytes.len() <= 16 * 1024,
            "Invalid Windows package identity size"
        );
    }
    fs::write(output, bytes).expect("Write immutable Windows package identity");
}
