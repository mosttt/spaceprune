//! Describe package identities without installing or starting native components.
use spaceprune_access::windows_package::{Architecture, PackageIdentity, PayloadIdentity};

fn main() {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args.len() == 2 && args[0] == "--check-pin" {
        let bytes = std::fs::read(&args[1]).expect("Readable package identity");
        assert!(
            bytes.len() <= 16 * 1024,
            "Package identity exceeds its bound"
        );
        let identity: PackageIdentity =
            serde_json::from_slice(&bytes).expect("Package identity schema");
        identity.validate().expect("Valid package identity");
        println!("{}", serde_json::to_string(&identity).unwrap());
        return;
    }
    assert_eq!(
        args.len(),
        4,
        "Expected target, metadata SHA-256, cleanup SHA-256 and license materials SHA-256"
    );
    let payload = PayloadIdentity {
        architecture: Architecture::for_target(&args[0]).expect("Windows distribution target"),
        metadata_sha256: args[1].clone(),
        cleanup_sha256: args[2].clone(),
        notices_sha256: args[3].clone(),
    };
    println!(
        "{}",
        serde_json::to_string(&payload.layout().expect("Valid payload identity")).unwrap()
    );
}
