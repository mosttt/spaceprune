//! Display language must not change native failure classification or selected identities.

use std::{fs, io, path::Path, time::Instant};

use spaceprune_access::{
    CancellationToken, IoFailureKind, OperationContext,
    error::classify,
    filesystem::{read_identity, remove_selected},
};
use spaceprune_contracts::messages::set_display_language;

#[test]
fn localized_native_failures_keep_their_structured_kind_and_preserve_replacements() {
    let temporary = tempfile::tempdir().unwrap();
    for (language, cancelled, deadline, changed) in [
        (
            "en",
            "Operation cancelled",
            "Operation deadline exceeded",
            "was replaced",
        ),
        ("zh-CN", "操作已取消", "操作已超时", "已被替换"),
    ] {
        set_display_language(language);
        let cancellation = CancellationToken::default();
        cancellation.cancel();
        let error = cancellation.check().unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::Interrupted);
        assert_eq!(classify(&error).kind, IoFailureKind::Cancelled);
        assert_eq!(error.to_string(), cancelled);

        let operation = OperationContext {
            deadline: Instant::now(),
            ..OperationContext::default()
        };
        let error = operation.check().unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
        assert_eq!(classify(&error).kind, IoFailureKind::TimedOut);
        assert_eq!(error.to_string(), deadline);

        let root = temporary.path().join(language);
        fs::create_dir(&root).unwrap();
        let selected = root.join("item-{path}");
        fs::write(&selected, b"original").unwrap();
        let root_identity = read_identity(&root).unwrap();
        let original = read_identity(&selected).unwrap();
        fs::rename(&selected, root.join("original")).unwrap();
        fs::write(&selected, b"replacement").unwrap();
        let removal = remove_selected(
            &root,
            &root_identity,
            Path::new("item-{path}"),
            &original,
            &|_| false,
            &OperationContext::default(),
        );
        assert_eq!(removal.removed_entries, 0);
        let error = removal.error.unwrap();
        assert_eq!(classify(&error).kind, IoFailureKind::TargetChanged);
        assert!(error.to_string().contains(changed), "{error}");
        assert_eq!(fs::read(&selected).unwrap(), b"replacement");
        assert_eq!(fs::read(root.join("original")).unwrap(), b"original");
    }
    set_display_language("en");
}
