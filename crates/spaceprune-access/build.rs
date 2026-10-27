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
    println!("cargo:rerun-if-env-changed=SPACEPRUNE_WINDOWS_EMBED_PACKAGE");
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
    windows_package_payload(&bytes);
    fs::write(output, bytes).expect("Write immutable Windows package identity");
}

fn windows_package_payload(pin: &[u8]) {
    use sha2::{Digest, Sha256};
    use std::{env, fs, io::Read, path::PathBuf};
    let output = PathBuf::from(env::var_os("OUT_DIR").expect("Cargo output directory"))
        .join("embedded-windows-package.msi");
    let mut bytes = Vec::new();
    // Test the target, not the machine running this build script. Non-Windows targets
    // never open or copy an MSI, including when these variables are inherited.
    if env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows")
        && let Some(path) = env::var_os("SPACEPRUNE_WINDOWS_EMBED_PACKAGE")
    {
        let path = PathBuf::from(path);
        println!("cargo:rerun-if-changed={}", path.display());
        let identity: serde_json::Value = serde_json::from_slice(pin)
            .expect("An embedded Windows package requires its final package identity");
        const LIMIT: u64 = 256 * 1024 * 1024;
        fs::File::open(path)
            .expect("Readable embedded Windows MSI")
            .take(LIMIT + 1)
            .read_to_end(&mut bytes)
            .expect("Read embedded Windows MSI");
        assert!(
            !bytes.is_empty() && bytes.len() as u64 <= LIMIT,
            "Invalid embedded Windows MSI size"
        );
        let architecture = match env::var("CARGO_CFG_TARGET_ARCH").as_deref() {
            Ok("x86_64") => "x64",
            Ok("aarch64") => "arm64",
            _ => panic!("Unsupported embedded Windows MSI architecture"),
        };
        assert_eq!(
            identity["payload"]["architecture"].as_str(),
            Some(architecture),
            "Embedded Windows MSI target mismatch"
        );
        assert_eq!(
            identity["msi_sha256"].as_str(),
            Some(format!("{:x}", Sha256::digest(&bytes)).as_str()),
            "Embedded Windows MSI differs from the signed package pin"
        );
    }
    fs::write(output, bytes).expect("Write immutable embedded Windows MSI");
}
