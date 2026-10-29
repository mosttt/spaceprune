# Native document providers

`spaceprune-plugin-mobile-documents` adds `builtin.mobile-documents/documents` to the existing Engine. A host explicitly registers a `DocumentProvider`; the desktop CLI does not advertise an Android capability or treat a content URI as a filesystem path.

The provider maps native storage to binding, paged children, inspect and delete-file operations. Observations use stable document identities, parent relationships, optional logical byte counts and modification times. Unknown metadata remains unknown; zero remains zero. Rules, recommendations, confirmed selection plans and execution receipts use the shared Core workflow.

Recipes contain a reusable resource slot, name patterns, exclusions, recursion and optional age/size thresholds. Each device authorizes and binds the slot locally. URIs, permission grants, binding revisions and scanned document IDs are not reusable rules. Age and size conditions must both match when enabled; missing metadata cannot satisfy an enabled condition.

```toml
[modules.documents.tasks.logs]
handler = "builtin.mobile-documents/documents"
platforms = ["android"]

[modules.documents.tasks.logs.options]
resource = "saved-logs"
recursive = true
include-names = ["*.log"]
exclude-names = ["keep-*"]

[modules.documents.tasks.logs.recommend]
older-than-days = 30
min-size-bytes = 1048576
```

The native adapter must revalidate the binding, current ancestry, document identity and observed metadata before deleting a confirmed file. Roots, directories, virtual documents and read-only items are not deletion candidates. Rebinding or revoked access invalidates old plans. Newly created files require another scan. A plan is consumed once; uncertain or interrupted native results are never replayed automatically. Cancellation stops subsequent dispatch and waits for an already dispatched native operation to report its outcome.

Logical bytes processed do not promise an identical change in free disk space. Android document providers need not supply sizes, timestamps, atomic compare-and-delete or cancellable deletion; hosts must preserve those distinctions in receipts. The provider contract can support other native storage systems without altering the upper-level selection and execution semantics.

Run `cargo test --locked -p spaceprune-plugin-mobile-documents` for nine Engine/provider fixtures covering matching, exclusions, unknown metadata, revoked/rebound grants, changed ancestors, exact selection, cancellation and uncertainty. Native UI/provider acceptance belongs to the embedding host; these fixtures alone do not establish Android or iOS device support.
