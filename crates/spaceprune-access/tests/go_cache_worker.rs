//! Fresh-process fixture exercising the same early entrypoint used by ordinary library hosts.
#[cfg(target_os = "linux")]
fn main() {
    use spaceprune_access::{
        NativePath, OperationContext,
        filesystem::{read_identity, remove_go_directory},
        go_cache,
    };
    use spaceprune_contracts::cache::DirectoryBinding;
    use std::{
        fs,
        os::unix::fs::{PermissionsExt, symlink},
    };
    if let Some(code) = go_cache::run_if_requested() {
        std::process::exit(code);
    }
    if !std::env::args().any(|argument| argument == "--ignored" || argument == "--include-ignored")
    {
        println!(
            "ignored: native Go cache worker requires an ordinary Linux user with user namespaces; run with --ignored"
        );
        return;
    }
    go_cache::register_host_entrypoint().unwrap();
    assert!(
        !rustix::process::geteuid().is_root(),
        "This test requires an ordinary user"
    );
    let temp = tempfile::tempdir().unwrap();
    let cache = temp.path().join("cache");
    let module = cache.join("example.test/module@v1.0.0/nested");
    fs::create_dir_all(&module).unwrap();
    fs::write(module.join("source.go"), b"package module\n").unwrap();
    fs::set_permissions(module.join("source.go"), fs::Permissions::from_mode(0o444)).unwrap();
    let external = temp.path().join("outside");
    fs::create_dir(&external).unwrap();
    fs::write(external.join("keep"), b"external data").unwrap();
    fs::set_permissions(&external, fs::Permissions::from_mode(0o555)).unwrap();
    symlink(&external, cache.join("external-link")).unwrap();
    let held = fs::File::open(&module).unwrap();
    let module_parent = module.parent().unwrap();
    fs::set_permissions(&module, fs::Permissions::from_mode(0o555)).unwrap();
    fs::set_permissions(module_parent, fs::Permissions::from_mode(0o555)).unwrap();
    let binding = DirectoryBinding {
        path: NativePath::from_path(&cache),
        parent_identity: read_identity(temp.path()).unwrap(),
        identity: read_identity(&cache).unwrap(),
    };
    let context = OperationContext::default();
    let ordinary = remove_go_directory(&binding, &context);
    assert_eq!(
        ordinary.removed_entries, 0,
        "preflight must precede removal"
    );
    assert_eq!(
        spaceprune_access::error::classify(ordinary.error.as_ref().unwrap()).kind,
        spaceprune_access::IoFailureKind::AccessDenied
    );
    assert_eq!(
        fs::read(module.join("source.go")).unwrap(),
        b"package module\n"
    );
    let cleaned = go_cache::remove(&binding, &context);
    assert!(!cleaned.unconfirmed, "{cleaned:?}");
    assert!(cleaned.removal.error.is_none(), "{cleaned:?}");
    assert_eq!(cleaned.removal.reclaimed_estimate, 15);
    assert!(!cache.exists());
    assert_eq!(held.metadata().unwrap().permissions().mode() & 0o777, 0o555);
    assert_eq!(
        fs::metadata(&external).unwrap().permissions().mode() & 0o777,
        0o555
    );
    assert_eq!(fs::read(external.join("keep")).unwrap(), b"external data");
    assert!(go_cache::remove(&binding, &context).removal.already_absent);
    fs::set_permissions(&external, fs::Permissions::from_mode(0o755)).unwrap();
    println!(
        "ordinary library entrypoint: 0555 capture, 0444 contents, no chmod, external-link preservation, native bytes and idempotency passed"
    );
}

#[cfg(not(target_os = "linux"))]
fn main() {}
