//! Embed platform metadata required by the native permission components.

fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        embed_macos_metadata();
    }
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        embed_windows_metadata();
    }
}

// Build scripts run on the host: select resources by Cargo's target OS, so cross-builds work.
fn embed_windows_metadata() {
    embed_resource::compile_for(
        "assets/spaceprune-readonly-helper.rc",
        ["spaceprune-readonly-helper", "spaceprune-cleanup-helper"],
        embed_resource::NONE,
    )
    .manifest_required()
    .expect("Could not embed the Windows helper manifest");
}

fn embed_macos_metadata() {
    for (binary, role) in [
        ("spaceprune-readonly-helper", "METADATA"),
        ("spaceprune-cleanup-helper", "CLEANUP"),
    ] {
        let info = format!("SPACEPRUNE_MACOS_{role}_INFO_PLIST");
        let job = format!("SPACEPRUNE_MACOS_{role}_JOB_PLIST");
        println!("cargo:rerun-if-env-changed={info}");
        println!("cargo:rerun-if-env-changed={job}");
        let pair = (std::env::var_os(&info), std::env::var_os(&job));
        let (info_path, job_path) = match pair {
            (None, None) => continue,
            (Some(info), Some(job)) => (info, job),
            _ => panic!("Both signed-package plist inputs are required for {role}"),
        };
        for (section, path) in [("__info_plist", info_path), ("__launchd_plist", job_path)] {
            let path = std::path::PathBuf::from(path)
                .canonicalize()
                .expect("Missing native package plist");
            assert!(path.is_file(), "Native package plist is not a file");
            println!("cargo:rerun-if-changed={}", path.display());
            for argument in [
                "-sectcreate",
                "__TEXT",
                section,
                path.to_str().expect("Non-UTF8 build path"),
            ] {
                println!("cargo:rustc-link-arg-bin={binary}=-Xlinker");
                println!("cargo:rustc-link-arg-bin={binary}={argument}");
            }
        }
    }
}
