# SpacePrune External Plugin Protocol

External plugins use the JSON Lines process protocol with `protocol-version = 3`. Prefer `builtin.filesystem/paths` for ordinary path rules. External plugins provide domain-specific scanning and cleanup logic that configuration cannot express.

## 1. Plugin directory

```text
spaceprune-plugin-example/
├─ plugin.toml
└─ spaceprune-plugin-example
```

`plugin.toml`:

```toml
schema-version = 1
id = "community.example"
name = "Example SpacePrune"
version = "1.0.0"
protocol-version = 3
executable = "spaceprune-plugin-example"
platforms = ["linux", "macos", "windows"]
handlers = ["cache", "history"]
```

| Field | Description |
| --- | --- |
| `schema-version` | Fixed at `1` |
| `id` | Global plugin ID |
| `name` | Display name |
| `version` | SemVer |
| `protocol-version` | Fixed at `3` |
| `executable` | Relative executable path within the plugin directory |
| `platforms` | Optional; the default empty array means platform-neutral |
| `handlers` | Array of local Handler IDs |

IDs allow only lowercase ASCII letters, digits, and separating dots or hyphens. Separators cannot be leading, trailing, or consecutive.
Platforms accept only `linux`, `macos`, `windows`, `android`, and `ios`.

`name` must be nonempty, `version` must be SemVer, and Handler IDs must be unique. `executable` must be a regular file inside the plugin directory. Its path cannot be absolute, contain `..`, or escape the directory through symbolic links. The complete Handler set returned by `describe` must exactly match the manifest.

## 2. Discovery and installation

Search directories, in priority order:

| Platform | Directories |
| --- | --- |
| Highest priority on all platforms | `$XDG_DATA_HOME/spaceprune/plugins/` |
| Linux | `$HOME/.local/share/spaceprune/plugins/`, `/usr/lib/spaceprune/plugins/` |
| macOS | `$HOME/Library/Application Support/SpacePrune/plugins/`, `/Library/Application Support/SpacePrune/plugins/` |
| Windows | `%LOCALAPPDATA%\SpacePrune\plugins\`; if LOCALAPPDATA is unavailable, `%USERPROFILE%\AppData\Local\SpacePrune\plugins\`; then `%PROGRAMDATA%\SpacePrune\plugins\` |
| Android | `$HOME/.local/share/spaceprune/plugins/` |
| iOS | `$HOME/Library/Application Support/SpacePrune/plugins/` |

A directory is skipped if its environment variable is missing. For duplicate IDs, the plugin in the earlier directory takes precedence.

Installation accepts only local paths:

```bash
spaceprune plugin install ./spaceprune-plugin-example
spaceprune plugin list
spaceprune plugin inspect community.example
spaceprune plugin remove community.example
```

Plugin packages may contain only regular files and directories, not symbolic links. Installation first copies the package to a temporary directory under the user's plugin root and validates the complete manifest, then atomically moves it to the final ID directory. Failure leaves no discoverable partial installation.

Installation copies the entire plugin directory into the current user's data directory. Existing IDs are not overwritten.

`spaceprune plugin list --output=json` returns discovered plugins; `plugin inspect <id> --output=json` returns one plugin. Both use public `spaceprune_contracts::plugin::InstalledPlugin`, containing `manifest`, `manifest_path`, and `executable`. Manifest fields are defined by `PluginManifest` in the same module. Existing `spaceprune_plugin_host::{PluginManifest, InstalledPlugin}` import paths remain available.

`schemas/installed-plugin.schema.json` is exported from those Rust types and can generate host-consumer types. Omitting `platforms` in a source manifest remains valid; serialized discovery results include `platforms: []`. Paths retain their original native-path serialization. The Schema describes data shape; the host still validates version, SemVer, path boundaries, and Handler consistency. Reading metadata does not start a plugin process.

### Unified capability catalog and enablement

`spaceprune plugin catalog --output=json` returns `PluginCatalogEntry[]`, including the built-in filesystem, Snapper, Linux system resources, and installed external plugins. Built-in groups cannot be uninstalled separately. Capabilities unsupported on the current platform are still shown but cannot be enabled. `supported` describes platform support, not whether system tools are installed or permission has been granted.
`enabled` is a device-local preference; an effective capability also requires `supported=true`. `conflicts_with_builtin` identifies an external installation claiming a reserved built-in ID. Such an entry always has `enabled=false`, while its platform fields remain accurate. The complete shape is generated in `schemas/plugin-catalog-entry.schema.json`.

`builtin.filesystem`, `builtin.snapper`, `builtin.system-linux`, and `builtin.developer-tools` are application-reserved IDs. New external installations using them are rejected before a destination directory is created. Existing or manually placed conflicting packages remain on disk and are displayed separately from genuine built-in capabilities, but their Handlers are neither registered nor started.
`plugin remove <id>` can remove a conflicting external installation without deleting the built-in capability. `plugin enable/disable <id>` always adjusts the genuine built-in capability of that name.
Removal first uses the canonical `<id>` directory under the user's plugin root. If absent, it finds the actually discovered ordinary child directory under that root by manifest ID, removing only one at a time. This user uninstall entry point does not remove system-level installations.
Hosts should identify catalog entries by the combination of `kind` and `id`.

Use `spaceprune plugin disable <id>` or `enable <id>` to save preferences. All plugins are enabled by default. Disabling does not uninstall files, rewrite Recipes, remove version declarations, or synchronize to other devices. Disabled plugins do not participate in subsequent scans or cleanup, and their processes do not start. Installing a local plugin still only installs files; execution occurs only through an enabled task that uses it.

## 3. Process transport

The main program launches the executable specified by the manifest:

- stdin: the main program writes requests;
- stdout: the plugin writes progress events and final responses;
- stderr: plugin diagnostics, inherited by SpacePrune;
- one UTF-8 JSON object per line;
- one plugin process is reused during a SpacePrune run;
- requests to the same plugin are sent serially;
- stdout must contain no nonprotocol text.

Every message includes:

```json
{ "protocol": 3 }
```

A protocol value other than `3`, invalid JSON, a mismatched request ID, a missing final result, or early process exit fails the current call. The protocol does not negotiate versions.

## 4. Requests

```json
{
  "protocol": 3,
  "id": 1,
  "method": "describe",
  "params": {}
}
```

- `id` is a monotonically increasing unsigned integer within the process.
- `method` is `describe`, `validate`, `scan`, or `clean`.
- `params` is the method's parameter object.

## 5. Final responses

Success:

```json
{
  "protocol": 3,
  "id": 2,
  "result": {
    "items": [],
    "issues": []
  }
}
```

Failure:

```json
{
  "protocol": 3,
  "id": 2,
  "error": {
    "code": "scan-failed",
    "message": "Could not read the target directory"
  }
}
```

Success must include `result`. Failure must include a stable `error.code` and an English `error.message`. The response ID must equal the request ID.

## 6. Progress events

An in-process host may take over a task's read authorization through `ScanAccess::manages_authorization`. Engine then calls the Handler's `scan_unprivileged`, and the same host policy decides whether to retry. Ordinary `scan` and Handlers without host-managed authorization retain their existing behavior. Snapper's unprivileged entry point preserves task configuration and selection identity, returning insufficient permissions as scan issues rather than opening its own authentication window. Final retry results still pass Engine's item-ID/hierarchy validation. This is an in-process Rust API; it does not change external plugin protocol 3.

A plugin may send the following before its final response:

```json
{
  "protocol": 3,
  "event": "progress",
  "request_id": 3,
  "data": {
    "phase": "scan",
    "module_id": "example",
    "task_id": "cache",
    "handler_id": "community.example/cache",
    "current": 3,
    "total": 10,
    "message": "Scanning directory 3"
  }
}
```

`phase`:

```text
load  scan  measure  plan  authorize  clean  complete
```

`current` and `total` may be null when the total is unknown. `request_id` must reference the current request.

## 7. describe

Request:

```json
{ "protocol": 3, "id": 1, "method": "describe", "params": {} }
```

Result:

```json
{
  "handlers": [
    {
      "id": "community.example/cache",
      "name": "Example cache",
      "description": "Scan cache produced by Example",
      "platforms": ["linux"],
      "options_schema": {
        "type": "object",
        "properties": {
          "root": { "type": "string" }
        }
      },
      "recommend_schema": {
        "type": "object",
        "properties": {
          "older-than": { "type": "string" }
        }
      }
    }
  ]
}
```

Schemas describe and present the configuration. Plugins must still perform actual validation in validate.

A Handler may declare `size_accounting`: `processed_bytes` (default) uses processed size and unknown-item counts; `snapshot_groups` counts successfully processed snapshot-selection groups separately, without adding them to ordinary file unknown counts. This metadata changes only presentation aggregates, not item results or deletion permissions.

## 8. TaskRequest

`validate`, `scan`, and `clean` share:

```json
{
  "module_id": "example",
  "task_id": "cache",
  "name": "Example cache",
  "handler_id": "community.example/cache",
  "options": {
    "root": "/home/user/.cache/example"
  },
  "recommend": {
    "older-than": "30d"
  },
  "tags": ["example", "cache"]
}
```

Before constructing TaskRequest, ConfigLoader has already:

- merged includes;
- merged built-in and user variable mappings;
- expanded `${NAME}` and leading `~` in current-platform task options.

Plugins therefore receive final options and should not reinterpret `${NAME}` as a process environment variable. `recommend` retains its configured values.

`tags` are task attributes; selector `match-tags` do not enter TaskRequest.

## 9. validate

```json
{
  "protocol": 3,
  "id": 2,
  "method": "validate",
  "params": {
    "task": {}
  }
}
```

Success:

```json
{ "protocol": 3, "id": 2, "result": null }
```

Plugins should validate options, recommend, platform, and their own constraints. They must not scan or modify resources in validate.

## 10. scan

```json
{
  "protocol": 3,
  "id": 3,
  "method": "scan",
  "params": {
    "task": {},
    "context": {
      "operation": { "operation_id": "scan-operation-id", "generation": 0 },
      "interaction": "terminal",
      "remaining_ms": 300000
    }
  }
}
```

`task` uses the complete structure from section 8; its fields are omitted here. `context.operation` is the original operation identity and generation supplied by the host. `interaction` is `foreground`, `terminal`, or `non_interactive`; `remaining_ms` is the remaining deadline in milliseconds when the request is sent. The example value is not a fixed budget. Plugins cannot reset or extend it, or treat interaction mode as permission to elevate. The host independently enforces cancellation and deadlines.

Result:

```json
{
  "items": [
    {
      "id": "cache-2026-01",
      "parent_id": null,
      "name": "January cache",
      "location": "/home/user/.cache/example/2026-01",
      "logical_bytes": 1048576,
      "reclaimable_bytes": 1048576,
      "modified_at": 1769904000,
      "recommendation": "recommended",
      "reason": "Older than thirty days",
      "tags": ["example", "cache"],
      "opaque": { "cache_id": "2026-01" }
    }
  ],
  "issues": [
    {
      "level": "warning",
      "message": "One directory is unreadable",
      "location": "/restricted"
    }
  ]
}
```

Constraints:

- Item IDs are unique within the current task.
- Parent IDs may reference only items in the same result.
- Unknown sizes use null.
- modified_at is Unix seconds or null.
- recommendation is recommended, optional, or unavailable.
- opaque is plugin-defined and returned unchanged by Core.
- Local problems go into issues without blocking other results; error-level issues make the scan return a nonzero status.
- Resources that cannot be acted on are unavailable.

Core treats items with no children referencing them as leaf selection units.

`CleanupItem.logical_size_applicability` is an optional, general field describing logical-size applicability. It defaults to `applicable`, which is omitted during default serialization. `not_applicable` means the domain explicitly declares logical size inapplicable to this object. `logical_bytes` retains its original contract: a measured value (including `0`) or `null`. Do not use `0` to represent not applicable.

Hosts render only from public fields, without guessing from Handler IDs, names, numbers, or `opaque`. Not-applicable items do not count toward logical-size unknowns. Nonempty groups containing only not-applicable items display N/A; empty groups remain `0 B`. Mixed groups continue to total known size and unknown counts for applicable items. `LogicalSizeSummary::from_items` and task/module `logical_size_summary()` provide consistent accounting.

This field describes only logical size. It does not change `reclaimable_bytes`, recommendation state, or selection permission. For example, the current system is an unselectable N/A item. A real snapshot still has applicable logical size even when protected, and remains unknown if measurement is incomplete.

## 11. clean

```json
{
  "protocol": 3,
  "id": 4,
  "method": "clean",
  "params": {
    "task": {},
    "items": [
      {
        "id": "cache-2026-01",
        "opaque": { "cache_id": "2026-01" }
      },
      {
        "id": "cache-2026-02",
        "opaque": { "cache_id": "2026-02" }
      }
    ]
  }
}
```

Actual task and items use the complete public structures. Plugins interpret only opaque data they generated themselves and revalidate that resources can still be operated on safely.

Results must include outcomes for every selected item and match acknowledged receipts exactly:

```json
{
  "outcomes": [
    {"item_id":"cache-2026-01","status":"succeeded","reclaimed_estimate":1048576,"failure":null},
    {"item_id":"cache-2026-02","status":"failed","reclaimed_estimate":0,"failure":{"item_id":"cache-2026-02","message":"Permission denied"}}
  ],
  "attempted": 2,
  "succeeded": 1,
  "reclaimed_bytes": 1048576,
  "unknown_reclaimed_items": 0,
  "failures": [
    {
      "item_id": "cache-2026-02",
      "message": "Permission denied"
    }
  ]
}
```

Individual failures go into failures. Use a top-level error only when the whole call cannot continue. `attempted = succeeded + failures.length` must hold. Without cancellation, `attempted` must also equal the request's item count; after cancellation, unprocessed items do not count toward `attempted`.

`reclaimed_bytes` retains its original field name and represents the known subtotal of estimated processed-content size, including confirmed partial processing. It is not the actual increase in free disk space.
`unknown_reclaimed_items` counts successful, partially applied, or uncertain items with unknown size and must not exceed attempted. Absent and unattempted items add no space; known zero and unknown size must remain distinct. Protocol 3 returns an explicit unknown count consistent with per-item outcomes.
`reclaimed_estimate` estimates the complete processed amount. When null, `reclaimed_lower_bound` preserves the confirmed size subtotal (default 0). When the complete estimate is known, the lower bound must be 0; the two fields must not both be added. Successful, partially applied, and uncertain effects can all have an unknown total with a known subtotal. For example, if processing of 6 bytes is confirmed but a retry's result is unconfirmed, record estimate=null and lower_bound=6, aggregating as at least 6 bytes and 1 unknown-size item. The same original selection remains one unknown item across stages; retransmitted receipts must not double-count size. Protocol 3 and older versions reject each other at startup.
Rust plugins can use `HandlerCleanResult::record_outcome` to calculate consistent aggregates from item outcomes.

Cleanup results may include `space_changes` (default empty array), recording space changes on the volume used by an actual batch. Each entry contains `scope`, `freeBytesBefore`, `freeBytesAfter`, and `error`. Unreadable measurements are null, with the reason in error. Both values are free bytes on the same filesystem, not snapshot logical size. Record each nonoverlapping batch only once; do not attach a shared batch repeatedly to every task or item. Hosts display this observation separately without adding it to reclaimed_bytes. An older result lacking this field does not imply that 0 B was freed.

In-process Rust Handlers can also call `ProgressSink::record_cleanup(task, succeeded, reclaimed_bytes, unknown_size_item_count)` to publish cumulative success subtotals for the same task up to the current moment. Hosts replace the previous subtotal for that task before aggregating; they must not sum every callback. Values increase only after effects are confirmed. This method defaults to no operation and does not change protocol 3 JSON progress frames or existing plugin contracts. Plugins without a live cumulative callback still update size after Engine validates the final task result. Ordinary progress percentages cannot be used to infer cleaned space.

Protocol 3 does not accept older plugins that return only cumulative success counts. After processing each selected item, a plugin must send:

```json
{"protocol":3,"event":"item_finished","request_id":4,"data":{"item_id":"cache-2026-01","status":"succeeded","reclaimed_estimate":1048576,"failure":null}}
```

The host checks the request, original selection, result status, and size, passes the item outcome to the execution journal, then replies:

```json
{"protocol":3,"event":"item_receipt","request_id":4,"data":{"item_id":"cache-2026-01","accepted":true}}
```

A plugin must wait for `accepted=true` for the same request and item before processing the next item. Rejection, EOF, or connection failure stops processing immediately. Raw progress text is not a receipt. Plugins cannot report unselected or duplicate items, or overwrite acknowledged outcomes with a final aggregate.

`status` supports `succeeded`, `already_absent`, `partially_applied`, `failed`, `uncertain`, and `not_attempted`. Absent and unattempted items must have `reclaimed_estimate` 0; an unconfirmed actual processed size uses null. Failed, partially applied, and uncertain outcomes carry a failure for the same item; success carries no failure.

If the process exits mid-call, Engine preserves acknowledged item outcomes and marks the remaining unconfirmed items uncertain, requiring a rescan rather than automatic retry. An old failure aggregate cannot overwrite confirmed success. Cleanup is not transactional rollback: cancellation cannot restore deleted content.

Protocol 3 has no cancel frame. Cancellation terminates the entire plugin process tree and waits for the process and stdin/stdout threads to finish. Calls use the shared OperationContext's cancellation signal and deadline. A plugin has a maximum total duration of 15 minutes, with a maximum 5-minute first-response wait and 5-minute response gap. Each limit is tightened to the caller's remaining deadline and budget; continuing progress does not extend the total deadline. Basic native/tool operations default to 60 seconds for first/idle output and 8 MiB cumulative output. Complete application workflows use 5 minutes and 64 MiB. Regardless of caller configuration, plugins remain subject to the following independent caps.
Each stdout line is limited to 1 MiB, cumulative output per call to 64 MiB, the receive queue to 16 lines, and a request to 16 MiB. stdin writes and waits for serial calls are also subject to cancellation and deadlines. Protocol errors or call failures stop the connection; later calls do not reuse old responses.

In-process Rust Handler scan/clean/batch and ScanAccess/CleanupAccess entry points now receive `&OperationContext`. Native loops check cancellation and deadlines at operation boundaries. Pure-configuration `validate` remains; validators requiring I/O must override `validate_in_context`. Plugin describe/validate and registration startup use the assembler's original deadline. JSONL uses protocol 3. The existing scan `context` object carries operation identity, interaction, and remaining_ms; plugins cannot use these to elevate themselves.

This is a versioned breaking protocol change. Manifest `protocol-version=1` or `2` is explicitly rejected before execution. Plugins must implement both protocol 3 item acknowledgements and final outcomes; changing only the version number is insufficient. Metadata helper protocol 6 and cleanup-authorization helper protocol 4 remain independent. Helper upgrades preserve operation identity and monotonic deadlines across processes without changing the external plugin protocol. Old read-only components are explicitly rejected by version probes and must be replaced with components from the same Core revision; there is no downgrade to an older protocol.

## 12. Lifecycle

```text
Start process
  -> describe
  -> validate (each active task)
  -> scan
  -> SpacePrune generates a plan
  -> clean (only when items are selected)
  -> SpacePrune exits and terminates the process
```

Multiple Handlers from one plugin share a process. Plugins may cache read-only discovery results, but clean must use the current task and items.

Progress events are for presentation. Plugins cannot emit core lifecycle events such as `scan-finished`; spaceprune-engine generates these centrally.

## 13. Trust and failure boundaries

- Users must explicitly install external plugins locally.
- Installation means trusting the plugin's executable code.
- A plugin crash does not corrupt SpacePrune's main-process memory, but fails the current call.
- Missing plugins and version mismatches fail before execution.
- SpacePrune does not guess substitute commands for failed Handlers.
- Plugins do not read terminal confirmations, decide global recipe scope, or parse root includes.

See the [Event system](EVENTS.md) for Core event contracts.

The sole definitions of Handler requests, descriptors, recommendation states, scan/cleanup results, and progress DTOs now live in `spaceprune-contracts::handler`. `spaceprune-plugin-api` continues to re-export these types and retains runtime interfaces such as Handler, ScanAccess, ProgressSink, and CancellationToken. The external process protocol is 3, with item receipts and explicit unknown-size requirements. Unknown sizes remain unknown.
