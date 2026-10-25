# SpacePrune Architecture

This document describes the repository's current structure, data flow, and boundaries.

## 1. System purpose

SpacePrune is a cleanup executor, not a collection of commands with hard-coded cleanup targets. Its core model is:

```text
Recipe
  -> include merging
  -> platform variable mapping
  -> modules and tasks
  -> Handler scan
  -> structured report
  -> recommended or user selection
  -> cleanup plan
  -> Handler execution
```

The core fixes only the operating modes:

- scan: read-only scanning;
- clean: execute the recommended plan;
- select: let the user select a plan;
- plugin: unified capability catalog, local enablement, and local external plugin management;
- automation: configuration-driven automation management.

Module IDs, selectors, tasks, paths, and recommendation rules all come from configuration.

Desktop applications and other hosts provide or load a Recipe through the public library API, then reuse the same validation, scanning, planning, and execution pipeline.
Core/CLI can be built and run without a model, cloud account, or private repository.

## 2. Design principles

### 2.1 Configuration expresses intent

Configuration defines:

- module and task organization;
- platform constraints;
- roots, include allowlists, and exclude denylists;
- recommendation conditions;
- Handler selection;
- automation scope;
- plugin version requirements;
- the entry point for user variable mappings.

### 2.2 Mappings absorb platform path differences

Tasks reference only `${NAME}`. Actual environment variable names, platform directories, and fallbacks live in a separate variable-mapping TOML file. The variable expander implements only generic parsing, merging, references, and cycle detection. It has no special cases for names such as `HOME`, `APPDATA`, or `XDG_CACHE_HOME`.

### 2.3 Handlers express capabilities

- The filesystem Handler handles ordinary paths (1 Handler).
- The Snapper Handler handles snapshot discovery, grouping, protection, and deletion (1 Handler).
- Linux system Handlers invoke package managers and systemd, retaining the legacy Go entry (5 Handlers).
- Developer-tools Handlers manage Linux Cargo, Go and npm caches (4 Handlers), plus local desktop Docker lifecycle (4 Handlers).
- External process plugins provide locally installed domain capabilities.

Linux statically assembles the first 4 categories, for a total of 11 Handlers. Other desktop platforms register only the applicable subset.

### 2.4 Minimal built-in scope

The binary embeds only `official:minimal`. It provides basic user-cache or temporary-directory cleanup without personal tool preferences. Users supply other recipes as local files.

### 2.5 Deterministic runtime scope

Application configuration, recipes, variable mappings, and plugins are discovered from local paths. The current recipe explicitly determines the entire scope of a run.

### 2.6 Input, permission, and execution boundaries

Recipes and imported bundles must pass local configuration validation. Scan observations are stored separately from the execution plan confirmed for the current operation. Selecting a file does not promote the selection to its parent directory; directory selections still honor hard exclusions and identity/mount boundaries.
The general permission service is shared by the CLI, scanning, cleanup, and other hosts. Read-only authorization does not grant deletion permission, and remote configuration does not grant local permissions.

## 3. Repository structure

```text
spaceprune/
├─ Cargo.toml
├─ crates/
│  ├─ spaceprune/                 High-level library API and application assembly; no CLI/terminal dependencies
│  ├─ spaceprune-cli/             Clap, CLI/TUI, terminal themes, output, and the spaceprune binary
│  ├─ spaceprune-contracts/       Public pure data: bundles, modules/tasks/automations, Handler DTOs, and path validation
│  ├─ spaceprune-config/          Application configuration I/O, recipe discovery, includes, mappings, embedded resources
│  ├─ spaceprune-engine/          Scanning, planning, execution, events
│  ├─ spaceprune-access/          Operation context, general permissions, native access, controlled processes
│  ├─ spaceprune-privileged-helper/  Public metadata/cleanup services and installation entry point
│  ├─ spaceprune-platform/        Automation platform facade
│  ├─ spaceprune-platform-linux/  Linux systemd automation
│  ├─ spaceprune-plugin-api/      Handler execution interfaces, cancellation, progress sink; re-exports public DTOs
│  └─ spaceprune-plugin-host/     External plugin discovery, installation, process protocol
├─ plugins/
│  ├─ filesystem/
│  ├─ snapper/
│  ├─ system-linux/
│  └─ developer-tools/
├─ recipes/
│  └─ official/
│     └─ minimal.toml
├─ mappings/
│  └─ official/
│     └─ variables.toml
├─ docs/
└─ .github/workflows/
```

Other applications integrate through the public Core API and are not members of this Cargo workspace.

## 4. Crate dependency direction

Dependencies are layered by responsibility:

- `spaceprune-plugin-api` and `spaceprune-config` are foundational crates that do not depend on each other.
- `spaceprune-engine` depends on both foundational crates.
- Static Handlers depend on `spaceprune-engine` and `spaceprune-plugin-api`.
- `spaceprune-plugin-host` depends on configuration, core, and the plugin API.
- Platform crates depend on configuration and core; the Linux platform crate also uses the plugin API.
- The high-level `spaceprune` library assembles configuration, Engine, platforms, static Handlers, and the external plugin host.
- `spaceprune-cli` depends on `spaceprune` in one direction only. It owns Clap, terminal I/O, and the TUI; neither the library nor Desktop depends on it.

The boundaries are:

- plugin-api does not depend on the executor.
- config owns discovery, reading, and persistence of application `config.toml` and recipes, without depending on terminals, plugin processes, or platform commands.
- `spaceprune` and `spaceprune-engine` do not depend on Clap, Ratatui, or Crossterm.
- `spaceprune-engine` also does not depend on systemd, launchd, or Windows Task Scheduler.
- Handlers do not choose the global operating mode.
- CLI owns only arguments and terminal I/O; Desktop uses structured interfaces through `spaceprune::api`.

`spaceprune::api::SpacePrune` provides configuration loading, `scan`, `execute`, and an event bus. It also re-exports `ScanRequest`, reports, `ConfigStore`, `RecipeBundle`, and plugin/automation management types. A GUI does not launch a CLI subprocess or duplicate assembly logic.

## 5. Startup flow

CLI `main.rs` handles only native installer dispatch and the process entry point. `application.rs` owns the existing startup order, configuration loading, command dispatch, and exit codes. `application/cleanup.rs` owns recommended/manual selection, confirmation, and execution after a scan. Both confirmation paths converge on one execution and report-merging path, without adding a second copy of scan state.

`commands/{modules,plugins,automations,recipes,themes,language}.rs` adapt the respective management commands; the former `management.rs` has been removed. Configuration and platform operations still call public Core. `runtime.rs` retains signals and operation context; `selection`, `progress`, and `output` retain interaction and output. Authorization installation commands still run before theme and active-recipe loading, so a broken recipe does not prevent repair of authorization components.

Cleanup and management commands start as follows:

1. CLI resolves the system display language and reads the shared language preference from explicit `--config` or the default application configuration. This does not load the active Recipe.
2. Authorization management commands branch off first. Other commands use `ConfigStore` to read the theme ID, then construct the fixed command tree.
3. language, plugin, theme, and recipe management commands branch off before cleanup configuration loading. JSON output retains its existing machine contract.
4. `ConfigLoader` reads the recipe ID from application configuration.
5. The ID `personal` resolves by convention to `recipes/personal/personal.toml`.
6. Recipe includes are resolved depth-first.
7. Results are merged and deserialized into `ResolvedConfig`.
8. Schema, IDs, Handler IDs, platforms, and automation references are validated.
9. Embedded variable mappings are loaded and merged with user mappings declared by the recipe entry point.
10. Active variables are resolved and task `options` expanded.
11. Static Handlers are registered, and external plugins actually required by the current recipe are discovered.
12. EngineBuilder validates active tasks before scanning, planning, and execution.

`module`, `plugin`, `automation`, `recipe`, `theme`, `language`, `authorization`, `scan`, and `clean` are reserved by the shared fixed-command constants and cannot be configuration module IDs.
`ConfigStore` lives in `spaceprune-config` and updates application configuration while preserving formatting. Interactive pickers live in `spaceprune-cli`.

## 6. Configuration discovery bootstrap

The application configuration location must be determined before loading recipes and variable mappings. Candidates are ordered by platform:

```text
Linux / Android: XDG_CONFIG_HOME/spaceprune/config.toml
                 HOME/.config/spaceprune/config.toml
macOS / iOS:     XDG_CONFIG_HOME/spaceprune/config.toml
                 HOME/Library/Application Support/SpacePrune/config.toml
Windows:         APPDATA\SpacePrune\config.toml
                 USERPROFILE\AppData\Roaming\SpacePrune\config.toml (when APPDATA is unavailable)
```

Explicit `--config` takes precedence. Default discovery chooses the first existing file. If none exists, settings commands write to the first candidate location. Every candidate root uses the same `config.toml` and `recipes/<ID>/<ID>.toml` layout. This bootstrap does not participate in task-path expansion; empty environment variables are ignored.

Without user configuration, ordinary cleanup commands load `official:minimal`. Setting a theme or language for the first time creates the default application configuration and `recipes/default/default.toml`.

## 7. Include resolution

Each Source is:

```text
File(absolute path)
Official(resource name)
```

The resolver:

1. Loads the current Source.
2. Validates `schema-version`.
3. Extracts includes.
4. Skips conditional includes for nonmatching platforms before accessing paths.
5. Recursively resolves child Sources.
6. Merges child results in declaration order.
7. Merges the current file last.
8. Caches results for this root-loading operation.

Tables merge recursively; arrays and scalars replace earlier values; type conflicts fail. A resolution stack detects cycles.

Editors that need to preserve source files can call `ConfigLoader::load_path_with_sources` to obtain unexpanded declarations, the current execution view, and the ordered local sources actually used. Sources follow the same include merge order; cached subtrees referenced more than once retain every occurrence. Paths for inactive platforms are neither resolved nor read. Embedded resources have no editable local path. The original `load_path` and `load_path_with_definition` entry points keep their return contracts. Saving still uses the complete `RecipeBundle`; an execution view must not replace multi-file source content.

`variable-map` may appear only in the recipe's root Source, keeping its path relative to an unambiguous recipe directory.

## 8. Variable mappings

### 8.1 File model

```toml
schema-version = 2

[variables.TOOL_HOME]
environment = "TOOL_HOME"
fallback = "${HOME}/.tool"

[variables.TOOL_CACHE]
value = "${TOOL_HOME}/cache"

[platforms.windows.variables.TOOL_CACHE]
value = "${CACHE_HOME}/Tool/Cache"
```

`VariableDefinition` contains:

- `environment: Option<String>`;
- `value: Option<String>`;
- `fallback: Option<String>`.

environment and value are mutually exclusive. fallback is allowed only with environment.

### 8.2 Merging

Active definitions override earlier layers in this order:

```text
Embedded common
  -> embedded current platform
  -> user common
  -> user current platform
```

A variable with the same name replaces the entire definition. Embedded and user files share the same Schema.

### 8.3 Resolution

The resolver maintains:

- definitions: active definitions;
- cache: resolved values or current unavailability;
- stack: recursive reference stack.

Environment values are used literally; empty values count as missing. A missing value with a fallback resolves that fallback; without one, the variable remains unavailable. References to nonexistent variables, cycles, invalid expressions, and invalid names fail immediately.

### 8.4 Expansion scope

After deserialization, ConfigLoader recursively expands:

- `options` of enabled tasks for the current platform within enabled modules.

It does not expand:

- include paths;
- `variable-map` paths;
- recommendation rules;
- display text;
- tasks that will not run on the current platform.

Static and external Handlers therefore receive TaskRequests with final option values and do not need to read environment variables themselves.

## 9. Embedded minimal recipe

```text
recipes/official/minimal.toml
          │ include_str!
          ▼
official:minimal
```

The recipe contains only the `cache` module:

- Linux, macOS, Android, iOS: immediate children of `${CACHE_HOME}`;
- Windows: immediate children of `${TEMP_HOME}`.

Platform variables come from `mappings/official/variables.toml`. The recipe contains neither environment variable names nor platform fallbacks.

## 10. Recipe input boundary

Application configuration selects a local recipe by stable ID. The directory convention is:

```text
recipes/<ID>/<ID>.toml
```

`ConfigStore` lists only recipes whose ID syntax is valid, whose directory and entry-file names match, and whose entry point contains a string `name`. `recipe set <ID>` can select only a listed recipe, avoiding dangling paths. The selected entry point may reference the compile-time resource `official:minimal` and relative local includes. Runtime does not implicitly enable other recipes or unreferenced files.

### 10.1 Bundle import and export boundary

`RecipeBundle` represents a complete snapshot of `recipes/<ID>/` using a stable `id` and `BTreeMap<relative path, UTF-8 content>`.
`ConfigStore::export_recipe_bundle` recursively collects regular files, rejects symbolic links and special files, and normalizes separators to `/`.

`upsert_recipe_bundle` requires the matching entry point `<ID>.toml` and rejects absolute paths, `.`, `..`, and backslash paths. It first creates staging under the same recipes root, writes all files, then calls `ConfigLoader` on the staged entry point for full semantic validation. Only success allows directory-level replacement; failure removes staging and preserves the original recipe. Importing a bundle does not implicitly switch the active Recipe.

## 11. Engine

`EngineBuilder` holds:

- `ResolvedConfig`;
- `PluginRegistry`;
- `EventBus`.

During construction it:

1. Visits enabled modules and current-platform tasks.
2. Checks that Handlers are registered.
3. Serializes TOML options/recommend to JSON.
4. Calls Handler `validate`.

`Engine` provides:

- `scan`;
- `execute`;
- `config`;
- `event_bus`.

## 12. Scanning

Scanning proceeds as follows:

1. Resolve module scope.
2. Convert selectors to a set of required tags.
3. Validate an optional exact task-ID set.
4. Filter disabled tasks and tasks with nonmatching platforms, task IDs, or tags.
5. Sort by order and task ID.
6. Construct TaskRequest.
7. Call Handler `scan`.
8. Apply stable ordering and validate item IDs, parent references, and an acyclic hierarchy.
9. Aggregate TaskReport, ModuleReport, and ScanReport.

A task error is recorded in the report without stopping other tasks. Duplicate IDs, empty IDs, dangling parents, or hierarchy cycles returned by a Handler fail that task's scan. `ScanReport.cancelled` indicates whether cancellation left the report incomplete. Incomplete reports cannot produce cleanup plans.

## 13. Planning

Report items are classified as:

- recommended;
- optional;
- unavailable.

`plan_recommended` selects only recommended items. `plan_selected` validates user keys:

- An item must exist.
- unavailable items cannot enter a plan.
- Parent/child items must not cause duplicate deletion.
- The plan retains the original TaskRequest, so cleanup uses the same final options as scanning.

## 14. Execution

`Engine::execute` calls Handler `clean` by task group. Each group records:

- attempted;
- succeeded;
- reclaimed_bytes;
- unknown_reclaimed_items (optional; the count of items with unknown processed size among successful, partially applied, or uncertain outcomes);
- item-level failures;
- task-level errors.

Failure in one task group does not stop other groups. Cancellation tokens are checked cooperatively at task and item boundaries.

The public `spaceprune-access::OperationContext` stores the operation ID, generation, interaction mode, resource budget, and absolute deadline. Native child operations preserve identity and budgets and tighten deadlines. Parent cancellation propagates through `CancellationToken::cancel()` to the running subtree; cancelling one child does not cancel siblings. Handler, ScanAccess, and CleanupAccess receive the original context directly; `NativeScanAccess::new()` does not keep a separate clock. Hosts use facade `load*_in_context`, `scan*_in_context`, and `execute_in_context` throughout assembly, validation, scanning, and permission retries. Older token convenience entry points create independent operations only at the top level.
CLI installs Ctrl+C handling before assembling plugins. Interactive cleanup starts its own deadline after user confirmation; automation scanning and cleanup share one deadline. Callers must still distinguish a cancellation request from confirmed termination; a bare atomic is only a signal read by native traversal.
Core validates `attempted = succeeded + failures`; without cancellation, `attempted` must also equal the group's selected-item count.
`ScanReport.cancelled` and `ExecutionReport.cancelled` represent early termination from cancellation or deadline expiry. The optional `stop_reason` is `cancelled` or `deadline_exceeded`; older serialized reports without it use compatibility rules. Terminal events carry the same reason. Event envelopes add `operation` (a string ID and generation) while retaining the original numeric ID in the event body. Timeouts preserve completed effects and measurements; incomplete scans cannot produce cleanup plans. CLI exits with 1 for timeout and 130 for explicit cancellation.

Across processes, public `OperationTransfer` carries ID/generation, an absolute system-monotonic deadline, and output budgets. Waiting for authorization or queuing does not renew the work allowance. Receivers may only tighten budgets, enforce noninteractive operation, and receive cancellation through a control directory or pipe EOF. Operation identity alone grants no read/write capability. Ordinary workflows use `OperationContext::workflow`: a 60-minute total deadline, 64 MiB response budget, and 5-minute output wait. Basic native/tool calls retain smaller default budgets, which individual protocols may tighten further. Read-only protocol 6 limits request files to 4 MiB and each request to 120 seconds; cleanup protocol 4 checks operation identity in handshakes, batches, and results.

`MetadataService::close()` first stops accepting old-generation read-only requests and locally cancels requests authorizing, waiting for a connection, or reading results. It waits for them to release their connections, then revokes reusable broker connections. New calls during closing wait under their original cancellation/deadline constraints, then enter a new authorization generation. Child contexts preserve the original ID, generation, deadline, interaction mode, and budget. Closing does not cancel sibling host work, and denial suppression for the same operation remains in force.
`close() -> Result<(), PermissionOutcome>` collects shutdown results for in-flight, cached, and inactive connections. The first unconfirmed error persists: repeated closing or cache clearing cannot turn it into success, and that service instance no longer authorizes new requests. Only connection failures explicitly marked `shutdown_unconfirmed` by the platform enter this state; ordinary authorization denial or unavailability does not.
Linux temporary-process and real pkexec/root-helper checks confirmed that the PID has exited when close returns. macOS explicitly consumes Stopped; a missing, mismatched, or timed-out response returns an error to the caller. Windows synchronous UAC calls must still wait for the system prompt to finish; native-platform acceptance remains open.
Authorization reuse is bound to the OS login identity. Switching away from or logging out of the foreground operation's login session, or losing its monitor, permanently revokes the old binding; switching back does not revive the old operation. For process-scope rules for noninteractive tasks and processes confirmed to have no login session, see [Native permissions and authorization reuse](NATIVE-AUTHORIZATION.md).

Ordinary filesystem scans, previews, and CLI on Linux and Windows share `NativeScanAccess` / `MetadataService`. Scanning first uses current permissions; only structured AccessDenied triggers a retry. `elevation = never` and noninteractive constraints remain in force.
Linux uses the existing stdio connection. Windows starts an independent component through `runas` and reuses the connection with sequence-numbered frames in a private directory. The Windows helper uses an `asInvoker` manifest so unprivileged probes do not require UAC. A real session requires an elevated token, a valid code signature, and protected directories/files under the actual Program Files location. Host and helper hold installation-object handles and reject reparse paths or installations writable by ordinary users.
Sessions bind to the original host process and OS login identity. Host exit, login-binding revocation, connection closure, or 15 minutes of inactivity ends reuse. See [Windows native component packaging](../build/WINDOWS-PACKAGING.md) for identity and IPC constraints. Requests remain single-use and retain their original operation deadlines. Complete task results use bounded TCP transport; control frames do not carry large results. Wiring for ordinary Windows file scanning has been cross-validated; signed installation, cross-account UAC, real cancellation, and revocation still need native validation. Unsigned builds do not gain this read capability.
On macOS, ordinary Recipes, previews, and CLI now use the same facade for read denials: BSD/ACL EACCES enters the separate signed metadata service; EPERM is treated as a system-policy denial, allowing foreground callers to open privacy settings and returning NeedsSystemSettings. EPERM alone does not establish that TCC caused the denial, nor does FDA bypass sandbox/SIP restrictions. Settings requests for the same operation are coalesced; background calls only report that action is needed. The original host verifies privacy recovery at the original path, retaining ordinary results, original rules, and unknown sizes. System settings do not create an authorization connection or persistent grant.
For administrator metadata access, a signed authorization subprocess hands credentials directly to the fixed service, bound to the original host, purpose, and operation. The host does not hold external credentials. The service checks both the application right and built-in administrator right for each request, preserving existing OS policy. Different right names or NoExtendRights alone do not prove purpose isolation. Complete requests and results use bounded Mach fragments with audit-token checks; PID, role, or permission flags inside requests are not trusted.
Windows deletion now uses `NativeCleanupAccess`. An independent cleanup helper receives the original confirmed plan and index selections through a local named pipe. The pipe has an explicit caller-login SID/SYSTEM/Administrators DACL and rejects remote clients. Both sides verify the kernel-reported peer process ID; the helper additionally checks host creation time and SID. The client connection prohibits the server from impersonating the administrator. Read connections do not accept cleanup requests.
Windows deletion opens paths component by component with NtCreateFile relative directory handles, enumerates through the same directory handles, and deletes through validated object handles, rather than recursively calling read_dir on display paths. Existing results, durable item receipts, duplicate-selection rejection, original deadlines, and partial/unknown-effect protocols remain shared.
macOS metadata and filesystem cleanup have service/host wiring. OS permission checks and file work run in supervised independent processes; stop acknowledgement is sent only after actual process reaping. Cleanup consumes only original plan indices, executes one item at a time, and permits the next item after the host persists a receipt. Progress continuously carries the original operation and event sequence. Bundled components and signing/notarization are wired. CLI Terminal uses system security authentication, with a signed agent importing/rechecking credentials; the host never handles passwords. Cancellation reaps the process group and restores the terminal. Native installation/cancellation and text-mode authorization acceptance are not yet complete. Signed-installation acceptance remains open on all three platforms. The new Windows deletion path has only been cross-validated; this does not close native UAC, NTFS/junction, cross-account, or cancellation gates.

The filesystem no longer has a separate elevation process. Ordinary Handlers perform only work allowed by current permissions; the facade's shared CleanupAccess retries restricted items and merges their original cumulative results. Using Engine/Handler alone no longer implicitly starts filesystem authentication.
Cleanup-side IPC writes are cancellable. Closing releases pipes before waiting for the process tree and I/O threads to exit. Privileged Snapper rejects custom program paths. Shared trust primitives check fixed system-tool paths and ownership; privileged commands do not inherit user configuration environment or working directories.

Engine calls `CleanupAccess::finish()` before committing the final report and journal to explicitly confirm that this operation's native connections have stopped. An unconfirmed shutdown is stored as `cleanup-stop-unconfirmed` in `ExecutionReport.operation_failures`, with a structured authorization outcome. Older reports default this field to empty; empty values are omitted. It does not change committed group or item results. Final-journal failure is also an operation-level error, not a fabricated failure of the last task. `has_errors` and `failure_count` include operation-level errors; successful items and size still aggregate only from original groups. Automation reports Failed, rather than ordinary Cancelled, when cancellation was requested but shutdown is unconfirmed. During final-report recovery, per-group evidence must still exactly match existing `group_finished` records. Operation-level errors are restored unchanged from the final report without rerunning cleanup.

Execution results and completion events retain both the known-size subtotal and the count of items with unknown processed size. Partial or uncertain effects must not be accumulated as known zero. When an item's `reclaimed_estimate=null`, `reclaimed_lower_bound` preserves the confirmed processed-size subtotal; when the complete estimate is known, the lower bound must be 0. Merging ordinary and authorized stages sums known subtotals from different stages. Reconciling repeated cumulative receipts takes the maximum known value rather than adding it again.

`ExecutionReport::unknown_reclaimed_items()` totals these unknown items. External plugins must use protocol 3 and provide aggregates consistent with item receipts; older versions are rejected before execution. Task errors and item failures share `ExecutionReport::failure_count()` to avoid double-counting.

Execution-journal schema 4 in the public facade records each item's latest cumulative outcome and fixes each task group's plugin accounting mode in its start record. Ordinary execution and general authorized execution on Linux/Windows/macOS share the same receipt entry point. An authorized worker waits for the host's durable acknowledgement before processing the next item. A broken connection preserves confirmed effects; unconfirmed items are `Uncertain` and require a rescan.
Implementation and validation details are in `docs/zh/development/ITEM-EXECUTION-JOURNAL-2026-10-12.md`.
Known zero and unknown, and subtotals and complete sizes, remain distinct. Items left unprocessed after failure or cancellation do not count toward successful size.

Hosts that need to show known results after process exit can implement `ProgressSink::execution_journal_started` to persist the Engine-provided `ExecutionJournalReference`. This callback runs before deletion dispatch. If a host participating in recovery cannot save the association, it returns an error and Engine does not start deleting. Existing hosts that do not implement the callback retain its default no-op implementation.

A later call to `spaceprune::api::recover_execution(&reference)` only reads the original journal. It does not rescan, request authorization, or execute cleanup. A committed complete final report yields `finished=true`; otherwise recovery retains the committed prefix and known-size lower bound and marks unconfirmed effects `Uncertain`.
At a normal journal end, groups without a start record remain `NotAttempted`. If later records are corrupt or truncated, the prefix cannot establish that subsequent selections were never executed. Snapshots recover according to plugin accounting metadata in the journal; AlreadyAbsent and existing volume-space observations are preserved separately. The journal path and plan ID associate observations and cannot be converted into execution capabilities. Schema 3 lacks the accounting metadata needed for recovery, so the new reader explicitly returns Unsupported. It neither guesses from Handler names nor associates older activity with another journal by time or similar paths.
See `docs/zh/development/EXECUTION-RECOVERY-2026-10-16.md` for recovery format and validation.
`reclaimed_bytes` and per-item sizes remain estimates of processed content. HandlerDescriptor `size_accounting` specifies presentation accounting: the default `processed_bytes` preserves existing behavior; Snapper uses `snapshot_groups`. Engine projects `ExecutionReport::size_summary()` from registered metadata. Ordinary unknown-item counts exclude snapshots; raw per-item evidence and `unknown_reclaimed_items()` are unchanged.

Snapper combines deletion for each configuration into one `delete --sync`, reading free-block counts on the same filesystem immediately before and after the command. Batch `space_changes` appear only in one coordinating task's result and persist in the final IPC result and execution journal; the batch observation is not distributed across snapshots or tasks. Different configurations are measured sequentially with nonoverlapping windows. Zero, negative changes, and unavailable measurements remain distinct; a changed filesystem identity prevents calculating a difference. Measurement failure does not block deletion of already fixed snapshots. Other activity on the same volume may affect the net change, and snapshot logical size still cannot stand in for freed space.

## 15. Filesystem Handler

The filesystem Handler:

- deserializes roots, files, and directories;
- compiles include/exclude globs;
- scans without following symbolic links;
- calculates logical size and latest modification time;
- applies unmodified-age and size recommendation rules;
- reconstructs CleanupRoot and validates each item before cleanup.

Execution constraints:

- root and include must be nonempty; a directory root itself cannot be a deletion item.
- Selections bind to native root/object identities captured during scanning; ordinary and authorized execution call the same `remove_selected`.
- Unix opens paths through directory descriptors without following links, atomically captures selected objects, then checks identity.
- Empty descendant directories of a selected directory are deleted atomically. Other runtime descendants are captured one at a time into the same private execution directory before traversal and deletion; the scan-time leaf list is not frozen.
- Exclusions are evaluated against original relative paths and excluded content is retained. Cancellation, failure, or retained directory contents trigger NOREPLACE restoration without overwriting new objects.
- Linux/Android compare both device and mount IDs on held descriptors, preserving subvolume boundaries and detecting same-device bind mounts. Failure to capture a mount point returns a boundary error.

For deep selections, Linux adds inotify monitoring of unselected ancestors and a post-capture getdents notification barrier. Moves, including moves away and back, are rejected before deletion. Linux relies on local VFS notification/lock ordering. macOS now checks parent-directory generations and retained vnode events; native APFS/HFS acceptance remains outstanding. See [implementation and acceptance](MACOS-ANCESTRY.md). Slots left by restoration conflicts or process exit carry NativePath mappings to original paths. They do not overwrite new objects with the same name or automatically resume deletion. See the [Capture and boundary audit](../../zh/development/VALIDATION.md) for implementation scope and native gates not yet passed.
Variable and `~` expansion is complete before paths enter the Handler.

## 16. Snapper Handler

`builtin.snapper/snapshots` is statically registered on Linux. It:

- discovers Snapper configurations;
- parses JSON snapshot lists;
- groups pre/post snapshots into a single selection unit;
- marks `#0`, active, and default snapshots unavailable;
- recommends by Recipe conditions in `snapshots` mode; in `rust-targets` mode, marks latest, manual, important, and paired pre/post snapshots optional;
- measures snapshot-file logical size read-only during recipe scans, or calculates referenced size for specified build artifacts;
- executes deletion and aggregates failures.

This capability is not included in `official:minimal`; configuration must reference it explicitly.

Snapper list queries and file logical-size measurements are separate. List queries do not access files inside snapshots, and the AI read-only list interface still uses this metadata query. Core `snapshots` scans use GNU du apparent-size to measure selected snapshots' file metadata, requesting authorization according to configuration only after a permission failure. Logical size still includes shared blocks and cannot count as reclaimable space. Measurement failures remain unknown; CLI grouping does not turn unknown subtotals into known zero.

Snapper source is split into `inventory` (listing), `scan` (scan aggregation), `size` (measurement), `policy` (grouping and recommendation), `identity` (original selections and identity), `selection` (selection protection), `validation` (configuration validation), `command` (command boundaries), and `cleanup` (lifecycle execution). Tests are separate. Domain modules no longer launch sudo/pkexec themselves.
After an ordinary read fails, the shared metadata service authorizes on demand. Cleanup uses the shared cleanup service, retaining pre-execution enumeration, original identities, pair completeness, and partial batch-failure handling. `api::snapper_configs` and `api::snapper_snapshots` also use the shared service. The domain plugin exposes inventory primitives that do not authorize independently. Native filenames in build-artifact measurement do not pass through display-string conversion.

After find/du failure, the measurement adapter checks actual metadata access within the original depth, pruning, and filesystem scope. OS errors determine whether authorization is needed; command text does not replace native evidence. A disappeared path reports NotFound; accessible metadata with command failure reports an ordinary command failure. Partial system-cache measurement also retains native errors. A total that has not been fully inspected stays unknown, with readable bytes reported as a lower bound in an issue.

## 17. Linux system Handlers

Core Linux filesystem cleanup supports on-demand elevation within a batch. Ordinary execution first returns real results; only selected items that failed for permission reasons are passed to the shared cleanup service. The service starts the fixed, independent `spaceprune-cleanup-helper`; it no longer elevates CLI/GUI or dispatches a privileged worker through the main program.
The component does not load external plugins, AI credentials, or user interfaces. sudo/Polkit handles authorization, while private stdin/stdout carries bounded requests and receipts. Even after readiness, the parent must confirm startup. Cancellation or disconnection prevents operations not yet started and cooperatively stops traversals already underway.

The elevated worker revalidates each selection's rules and scan-time file identity, deleting through directory descriptors without following symbolic links, preserving exclusions, and rejecting mount crossings. Successful receipts merge with ordinary results without counting items or sizes twice. Missing receipts retain unconfirmed outcomes. Hosts explicitly select graphical authorization; CLI and Snapper can reuse system sudo authentication. No persistent permission flag can substitute for system validation.

System-cache and Snapper domain commands no longer launch authenticators independently. Reads, cleanup, and automatic preparation of bundled components share the public permission infrastructure while retaining separate capabilities and original operation deadlines. When permissions are needed, current bundled components are applied automatically and the original operation continues after success. See [Native authorization](NATIVE-AUTHORIZATION.md).

`plugins/system-linux` registers (the old Go entry forwards to developer-tools):

```text
builtin.system-linux/package-cache
builtin.system-linux/journal
builtin.system-linux/tmpfiles
builtin.system-linux/coredumps
builtin.system-linux/go-modcache
```

Each Handler produces domain cleanup items and validates during clean that item IDs still match the current action. Task options control package manager, journal retention duration, and elevation policy. Package/systemd handlers use fixed command sets; the Go alias retains identity-bound native deletion. These capabilities are also outside the minimal default scope.

Developer tools additionally provides `go-modcache`, `go-buildcache`, `cargo-registry-cache` and `npm-cache`. Tool caches reuse native identity and authorization infrastructure; Go module caches retain the same-UID worker without invoking `go clean`.

`builtin.developer-tools` adds four local Docker handlers: `docker-build-cache`, `docker-dangling-images`, `docker-unused-images` and `docker-stopped-containers`. The plugin checks native interfaces before enumeration, binds endpoint and Engine identity, and executes exact IDs using the ordinary-user Docker CLI. Docker does not enter native read-only or cleanup-helper grants. Mobile registries exclude these handlers. The four existing Linux cache handlers retain their directory identity and authorization paths. See [Docker cleanup](../usage/DOCKER.md).

## 18. External plugins

Application-layer `spaceprune::plugins` projects built-in groups and PluginHost installation metadata into public `PluginCatalogEntry` values without executing plugins. Global preferences live in `plugins.toml` under the user's configuration directory, separate from portable Recipes. Actual enablement changes increment revision.
Engine's separate disabled set preserves original Recipe values. Scanning skips disabled tasks; explicit task selection and cleanup reject disabled Handlers. Application sessions recheck the fixed revision at scan and cleanup entry points. Cross-process changes require reloading and rescanning; an executing call does not switch policy mid-operation.
See the [Plugin enablement implementation record](../../zh/reference/PLUGIN-PROTOCOL.md).

An external plugin directory contains `plugin.toml` and an executable. PluginHost:

1. Discovers manifests in platform-standard directories.
2. Validates Schema, ID, SemVer, platform, a unique Handler list, and the in-package executable.
3. Starts only plugins required by the current recipe.
4. Calls describe, validate, scan, and clean through JSON Lines.
5. Checks request IDs and protocol versions.
6. Forwards progress messages to the shared ProgressSink.
7. Terminates plugin processes on Ctrl+C or five consecutive minutes without a response.

Plugin packages cannot contain symbolic links. Installation first copies the package to a temporary location under the same data directory and validates it fully, then atomically renames it into the final directory.

## 19. Event system

EventBus lives in core and emits operation events:

```text
scan-started
task-scan-started
task-scan-finished
scan-finished
cleanup-started
cleanup-group-started
cleanup-group-finished
cleanup-finished
```

Events include an operation ID and monotonic sequence. Listeners run synchronously on the emitting thread and cannot alter engine control flow.

## 20. Automation

Configuration defines daily schedules using zero-padded local time, `at = "HH:MM"`. The platform facade maps these to:

- Linux: systemd user service + timer;
- macOS: LaunchAgent;
- Windows: current-user scheduled task.

Installed files invoke only `spaceprune --config <application entry point> automation run <id>`. `ConfigLoader` retains the Recipe source and normalized application `config.toml` source separately; schedulers write only the latter. After a Recipe switch, the next automation run loads the current recipe rather than passing `recipes/<ID>/<ID>.toml` to `--config`, which accepts only an application entry point. Sessions created directly through `load_recipe` without an application entry point cannot install automation.
Platform APIs also require an explicit absolute runner path supporting `automation run <id>`. CLI passes itself; other hosts must pass the CLI runner deployed with their product, not implicitly use the current GUI process. Runtime reloads configuration, rescans, and executes only recommended items.

`official:minimal` defines no automations. Users must explicitly add them to a recipe before installation.

## 21. CLI and TUI

CLI uses Clap's runtime command tree for two-stage parsing:

1. Load the terminal theme from application `config.toml` `[theme].id`, then parse fixed options and management commands.
2. Load the cleanup recipe identified by application configuration, then register and parse dynamic modules and selectors.

Scripts can avoid dynamic module command names with fixed interfaces:

```text
spaceprune scan  [--module ID] [--selector ID ...] [--task ID ...]
spaceprune clean [--module ID] [--selector ID ...] [--task ID ...] --yes
```

Both `--selector` and `--task` require `--module`. The former matches tags with AND; the latter matches exact task IDs. Fixed subcommands cannot be combined with legacy mode flags. People can still use Recipe-generated commands such as `spaceprune <module> --<selector> --scan`.

`--all` means include disabled modules only in `spaceprune module list`. Ordinary configuration modules can still define `all` as a dynamic selector.

Themes persist by stable ID in application configuration's `[theme]` table. `spaceprune theme` uses a live Ratatui previewer: changing the candidate redraws immediately, but only confirmation writes the setting. `theme set <ID>` sets it directly; `theme list` lists it read-only.

The recipe ID persists in application configuration's `recipe` field. `spaceprune recipe` interactively selects a local recipe; `recipe set <ID>` sets one directly; `recipe list` lists names, paths, and active status. Both pickers share the cleanup selector's alternate-screen and terminal-restoration entry points.

Format-preserving TOML I/O and recipe enumeration live in `spaceprune-config`. CLI explicitly passes only the theme palette to Clap, text output, progress, and TUI. JSON and nonterminal output contain no ANSI styling.

CLI progress subscribes to existing Core scan/cleanup lifecycle events and updates counts from actual task results. The terminal panel refreshes periodically during operations. `progress/screen.rs` exclusively owns the alternate screen during live operations, using Crossterm synchronized frame redraws. It neither relies on logical line counts from before resizing nor queries cursor position or reads stdin. System-input start/end events and Drop restore the original screen and cursor; progress resumes only after nested input ends. Completion events print elapsed-time summaries before allowing stdout reports or selection UI output.
Handler ProgressEvent provides only the current step and counts the Handler actually knows, not whole-disk coverage or guessed cleanup percentages. The filesystem reports locations and rate-limited counts at configured roots, candidate measurement, and local cleanup. Snapper explicitly distinguishes authorization, logical-size measurement, and synchronous deletion waits.

ProgressSink adds default no-op input-state notifications. ProgressInputGuard pauses/resumes live output in matched pairs while system commands read terminal passwords; error returns also restore output. It coordinates presentation only, without storing credentials or granting permission. Graphical hosts and existing plugin receivers remain compatible with the original interface. Background refresh is disabled during selection, confirmation, and ordinary reports; exit restores the cursor. Nonterminal and no-progress modes start no animation thread.

The TUI uses Ratatui/Crossterm and consumes ScanReport directly:

- module, task, and item trees;
- recommended items selected by default;
- optional items toggleable;
- unavailable items read-only;
- Space to toggle, arrow keys or `h/l` to expand the current row, `-`/`+` to collapse/expand all modules, search, and restore recommendations;
- another confirmation before execution.

On entry, the TUI saves the original title on the terminal title stack and sets the interactive-cleanup title; it restores the title on exit. Item keys and module/task selection scopes are cached on entry; visible rows are constructed only once per frame. `Ctrl+C` cancels consistently at every UI layer.

## 22. Platform layer

`spaceprune-platform` is the automation facade; `spaceprune-platform-linux` provides systemd automation. Domain logic lives in `plugins/system-linux` and `plugins/developer-tools`; existing public Rust exports are thin compatibility re-exports. `spaceprune-engine` and `spaceprune-config` remain platform-neutral.

Mobile targets build only the library layer, not the desktop CLI, process-plugin host, or Linux-specific capabilities.

## 23. External hosts and recipe exchange

Applications reuse local capabilities through `spaceprune::api`; services can consume the pure bundle contracts in `spaceprune-contracts` without depending on the execution engine.

After public path/size validation, an externally supplied complete Recipe bundle still needs configuration parsing, Handler/options validation, and scanning on the target device. Portable file layouts support conditional includes and task platform conditions; they do not make execution identity from one device reusable on another. Scan results, the current plan, permission connections, and confirmed selections remain inside the local execution boundary. Network synchronization cannot replace these steps.

The public repository owns general local authorization components and the entry points needed for standalone CLI operation. Each application owns its own sessions and UI interactions.
See [Native authorization](NATIVE-AUTHORIZATION.md) for installation protocols and validated platform scope.

## 24. Validation

Core validation commands:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --all-targets --locked
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --locked
cargo build --workspace --release --locked
```

CI also runs:

- actionlint;
- cargo-audit;
- Android and iOS library-target checks;
- native macOS and Windows Clippy and tests.

See the [Feature matrix](../../zh/reference/FEATURE-MATRIX.md) for current features, validation evidence, and known limitations. Local protocol/build checks do not replace signed installation, authorization, revocation, and native-filesystem validation on each target platform.

The Release workflow first calls the same reusable CI. Once all CI jobs succeed, it builds archives for Linux GNU/ARM64/musl, macOS Intel/Apple Silicon, and Windows x64.

## 25. Extension paths

To add an ordinary cleanup rule:

1. Create `recipes/<ID>/<ID>.toml`.
2. Create a user variable-map if logical paths are needed.
3. Use the filesystem Handler.
4. Run `spaceprune scan` first.
5. Select and validate it with `recipe set <ID>`.

To add a domain capability:

1. Add a Handler to an existing static plugin, or create an external process plugin.
2. Define options and recommend Schemas.
3. Implement validate, scan, and clean.
4. Add a configuration recipe.
5. Validate scan, recommendation, selection, and cleanup boundaries.

`spaceprune-contracts` is the sole definition source for public data. `spaceprune-config`, `spaceprune-plugin-api`, and `spaceprune-engine` re-export the corresponding types; facade callers do not maintain structural copies. Complete-bundle editing uses `rename_recipe_bundle` and `update_bundle_automation`, kept in config's `bundle_edit` module. Pure contracts do not introduce `toml_edit`, disk I/O, or runtime dependencies.

Scan scopes, task/module reports, plan observations, execution results, and size summaries are defined in the contracts' `report` and `size_summary` modules. `export-schemas` generates public JSON Schema from these Rust types. Report-only consumers can depend directly on pure contracts without linking Engine or native permission services.
Engine's `ScanReport` holds an in-process single-consumption marker alongside the pure report; `into_report()` returns data without that marker. `ScanReport::from_report(ScanReportData { ... })` is the entry point for hosts to assemble reports; it does not issue OS permissions.
`CleanupPlanData` can be serialized, deserialized, and displayed. An executable `CleanupPlan` is still constructed from scan selections, is immutable, and cannot be created from JSON or a data DTO. Existing facade APIs, serialized fields, and single-execution checks remain unchanged. Authorization leases, execution grants, and cancellation objects do not enter pure contracts.

The configuration crate's entry point retains only constants and exports. `loader` owns source loading/include orchestration; `parse` owns fragments and merging; `resolve` owns variable expansion; `validate` owns semantic validation; `model` retains application and locally resolved views; `paths` owns platform configuration discovery. Domain tests live alongside their corresponding modules; storage integration regressions live in `store/tests.rs`.

External plugins use JSONL protocol 3. `spaceprune-plugin-host::process` owns bounded, cancellable bidirectional process transport. `process::receipts` binds item results to original selections and passes them to the same execution journal. A plugin starts the next item only after acknowledgement, and its final aggregate must match item receipts. Pure result validation is provided by `spaceprune-contracts::handler::execution`, shared by in-process Handlers and the external host. A broken stream preserves confirmed effects and requires rescanning unconfirmed effects. See the [Plugin protocol](PLUGIN-PROTOCOL.md).
