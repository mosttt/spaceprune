# Clean local Docker objects

The developer-tools plugin uses local Docker Engine / Docker Desktop interfaces on Linux, Windows and macOS desktops. Android and iOS are excluded. Each native tool and required subcommand is checked before enumerating objects. Missing Docker skips inspection; missing Buildx affects only build cache. An unreachable daemon, denied access, incompatible interface or remote endpoint produces a specific diagnostic. User Docker commands are not elevated.

Start with build cache and dangling images when rebuilding or downloading is acceptable. Unused tagged images and stopped containers are separate manual-only capabilities: an image may be intentionally retained offline, and a stopped container may contain unsaved writable-layer data and logs.

| Handler | Scope | Default selection |
| --- | --- | --- |
| `builtin.developer-tools/docker-build-cache` | Unused records in the local Engine default Buildx builder; excludes custom/remote builders | Optional; explicit `recommend.enabled=true` can recommend |
| `builtin.developer-tools/docker-dangling-images` | Dangling images referenced by no container | Optional; explicit `recommend.enabled=true` can recommend |
| `builtin.developer-tools/docker-unused-images` | Tagged images referenced by no container | Always manual-only |
| `builtin.developer-tools/docker-stopped-containers` | Containers in exited, created or dead state and their writable layers | Always manual-only |

```toml
[modules.docker]
name = "Local Docker"
[modules.docker.tasks.build-cache]
name = "Docker build cache"
handler = "builtin.developer-tools/docker-build-cache"
platforms = ["linux", "windows", "macos"]
[modules.docker.tasks.build-cache.recommend]
enabled = false
```

Docker `options` is empty: paths, shells, arbitrary commands, builders and elevation settings are not accepted. `recommend` supports `enabled` (default `false`), optional `older-than` and `min-size` conditions. A saved Recipe defines future scan scope. Each execution remains limited to exact full object IDs scanned and confirmed for that execution, excluding later arrivals.

Scanning binds the local endpoint and Engine ID; cleanup checks both again along with object eligibility. Only local `unix://` sockets and Windows `npipe:////./pipe/` endpoints are accepted. TCP, SSH and remote named pipes are rejected, including localhost TCP. The plugin does not switch contexts, start daemons/builders or operate on remote machines. Change a local context in Docker yourself, then scan again.

Cleanup uses official commands with exact object IDs. Image/container deletion is never forced; unselected parent images are retained and running containers are not stopped. Buildx uses anchored ID filters, never an unbounded system prune. Volumes, bind mounts, Docker backing directories and Docker Desktop virtual disks are outside the scope. Docker refusals caused by dependencies, concurrent state or multiple tags remain failed/uncertain outcomes requiring a fresh scan.

A selected object already absent before its cleanup step is verified and reported as an idempotent zero-byte success, including objects removed by an earlier operation in the same batch. An object that merely became busy or protected remains a changed-state failure, not a successful removal. Failed or malformed inventory queries cannot confirm absence.

Image sizes and container writable-layer sizes use Docker integer-byte fields. Rounded Buildx kB/MB/GB output contributes a content-size estimate. Shared layers cannot be summed into freed disk space, and Docker Desktop VM disks may not shrink immediately. Estimated and completed reclaimed bytes therefore remain unknown, not zero; confirmed processed content size is tracked separately. Successful removal is verified by checking object absence independently of eligibility.

API consumers can use `probe_system_capabilities` for available capabilities and installed-interface diagnostics. Entries with an `issue` cannot issue cleanup sources. Existing `discover_system_capabilities` still returns only available tasks. App inventory displays diagnostics without executable tasks; inspecting a known failure returns the diagnosis without enumerating objects or creating Recipe sources.

## Size and age conditions

Docker reports are read through native structured CLI interfaces. Rounded sizes such as `1.5GB` are retained as content estimates, not promises of freed host/VM space. `recommend.older-than` and `recommend.min-size` use the same condition format as filesystem tasks; both must match when both are present. Build cache uses absolute last-use timestamps from Engine disk usage, falling back to creation only for never-used records. Images use creation time; containers use their last stop, or creation if never run. Missing required timestamps/sizes do not match. Tagged images and stopped containers remain manual selections. Relative age descriptions are never treated as exact timestamps. Cancellation and timeout are separate from missing Docker or Engine access errors.
