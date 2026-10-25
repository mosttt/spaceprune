# Core/CLI capabilities and acceptance

The implementation scope follows. [Validation](../development/VALIDATION.md) collects evidence and remaining platform gates. Integration or cross-compilation is not native acceptance.

| Capability | Usage and maintenance |
| --- | --- |
| [Recipes](../usage/RECIPE-GUIDE.md) | Multi-file sources, conditional includes, platform branches, recurring rules and one-time selections. |
| [Execution](../reference/ARCHITECTURE.md) | Identity, scope, exclusions, single-use plans and partial receipts; no automatic deletion replay. |
| [Snapshots](../usage/CONFIGURATION.md) | Snapper rules, visible disabled protection, logical size separate from volume changes. |
| [Linux resources](../reference/PLUGIN-PROTOCOL.md) | Package/systemd lifecycles belong to system-linux. |
| [Developer tools](../usage/DOCKER.md) | Go/Cargo/npm caches on Linux; Docker on Linux/Windows/macOS desktop; missing interfaces are skipped. |
| [Authorization](../reference/NATIVE-AUTHORIZATION.md) | On-demand component preparation, distinct read/delete rights, valid connections and login revocation. |
| [CLI](../usage/CLI.md) | Stable script/JSON commands, dynamic modules, selection, languages and themes. |
| [Plugins](../reference/PLUGIN-PROTOCOL.md) | Built-in/external catalogs, shared enablement and exact lifecycle receipts. |
| [Scheduling](../build/DEPLOYMENT.md) | systemd, Task Scheduler and LaunchAgent, with separate native acceptance. |
| [Distribution](../build/BUILDING.md) | Independent MIT public sources, third-party licenses and required Source Form. |

macOS ancestor [implementation](MACOS-ANCESTRY.md) and [synchronization evidence](MACOS-ANCESTRY-EVIDENCE.md) retain open questions. Android/iOS library targets do not constitute a released mobile application.
