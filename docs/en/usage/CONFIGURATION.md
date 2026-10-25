# SpacePrune Configuration Reference

SpacePrune uses TOML Schema 2 and separates application configuration from cleanup recipes: `config.toml` stores the current recipe, shared language, and CLI theme; `recipes/<ID>/<ID>.toml` describes what to clean.

## 1. Application configuration

```toml
schema-version = 2
recipe = "personal"
language = "system"

[theme]
id = "aurora"
```

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `schema-version` | Integer | Yes | Currently fixed at `2` |
| `recipe` | Stable ID | Yes | The active local cleanup recipe |
| `language` | String | No | `system` (default), `en`, or `zh-CN`; shared by CLI and Desktop |
| `theme.id` | Stable ID | No | Terminal theme; defaults to `aurora` |

By default, `language` follows the system. Unsupported or unrecognized system languages and unknown saved language values fall back to English. `spaceprune language set <system|en|zh-CN>` saves a manual preference. Reading the language does not load the active Recipe.

`theme.id` accepts `aurora`, `orbit`, `graphite`, and `forest`.
Desktop saves its own theme and light/dark mode separately; this field does not affect them. Both can also be changed independently in Desktop Settings.
Old theme IDs map to the new styles when read, without rewriting configuration; see [Terminal themes](CLI.md#terminal-themes) for the mapping.

`recipe = "personal"` always resolves to `recipes/personal/personal.toml` relative to the application configuration directory. The recipe ID is both its directory name and entry-file name; arbitrary entry paths are no longer configured.

## 2. Default configuration discovery

`--config <PATH>` always takes precedence. Without it, SpacePrune selects the first existing file in order. If none exists, the first write uses the first candidate location:

| Platform | Paths |
| --- | --- |
| Linux / Android | `$XDG_CONFIG_HOME/spaceprune/config.toml`; `$HOME/.config/spaceprune/config.toml` |
| macOS / iOS | `$XDG_CONFIG_HOME/spaceprune/config.toml`; `$HOME/Library/Application Support/SpacePrune/config.toml` |
| Windows | `%APPDATA%\SpacePrune\config.toml`; `%USERPROFILE%\AppData\Roaming\SpacePrune\config.toml` only when `APPDATA` is unavailable |

Empty XDG_CONFIG_HOME, HOME, APPDATA, or USERPROFILE values are ignored. If no application configuration is found, SpacePrune loads `official:minimal`.

Every candidate root uses the same layout. On Linux without `XDG_CONFIG_HOME`, all cleanup configuration files live under:

```text
~/.config/spaceprune/
├── config.toml
└── recipes/
    └── personal/
        ├── personal.toml
        ├── personal.generated.toml
        └── variables.toml
```

`personal.toml` is the recipe entry point. Its `variable-map` and local `include` paths resolve relative to `recipes/personal/`, so each recipe can be copied, backed up, and reviewed independently.

Example recipe entry point:

```toml
schema-version = 2
name = "Personal SpacePrune"
variable-map = "./variables.toml"

include = [
  "official:minimal",
  "./personal.generated.toml",
]
```

| Field | Type | Required | Description |
| --- | --- | --- | --- |
| `schema-version` | Integer | Yes | Currently fixed at `2` |
| `name` | String | Yes | Recipe display name |
| `variable-map` | String path | No | User variable mapping; allowed only in the recipe entry point |
| `include` | Array of strings or conditional tables | No | Configuration loaded and merged first |
| `requires.plugins` | String map | No | Plugin IDs mapped to SemVer requirements |
| `modules` | Table | No | Dynamic modules |
| `automations` | Table | No | Automation definitions |

The recipe entry point and every include file must each declare the integer `schema-version = 2`. SpacePrune validates each physical file before merging. Include files cannot inherit the entry point's version field and are not TOML fragments with optional metadata.

Selecting a user recipe does not implicitly merge an embedded recipe. To use the basic tasks, explicitly include `official:minimal` in the recipe entry point.

Recipe and theme selection commands:

```bash
spaceprune recipe
spaceprune recipe list
spaceprune recipe set personal
spaceprune theme
spaceprune theme list
spaceprune theme set aurora
```

Bare `recipe` and `theme` commands open interactive selection. `set <ID>` switches directly; `list` lists read-only. Both settings update only their corresponding fields in application `config.toml`, preserving other content and comments. If default application configuration does not exist, setting a theme or language for the first time also creates `recipes/default/default.toml`, which includes `official:minimal`.

See the [Recipe authoring guide](RECIPE-GUIDE.md) for manual recipe layout, platform mappings, and validation. CLI reads saved recipes directly.

Fixed scan and cleanup interfaces for scripts do not depend on dynamic module subcommand names:

```bash
spaceprune scan --module personal --selector logs --output json --no-progress
spaceprune scan --module personal --task logs --output json --no-progress
spaceprune clean --module personal --selector logs --yes
```

`--selector` and `--task` are repeatable and both require `--module`. Selectors combine `match-tags` and match them with AND; tasks restrict scope by exact stable task ID. Fixed `scan` / `clean` subcommands cannot be mixed with legacy `--scan`, `--clean`, or `--select` flags.

## 3. Embedded minimal recipe

The only embedded cleanup recipe is:

```text
official:minimal
```

Its source is `recipes/official/minimal.toml`, compiled into `spaceprune-config` with `include_str!`. A built binary does not read the source directory at runtime.

The recipe contains one `cache` module:

| Task | Platforms | root | Recommendation rule |
| --- | --- | --- | --- |
| `user-cache` | Linux, macOS, Android, iOS | `${CACHE_HOME}` | Not modified for 30 days |
| `user-temporary` | Windows | `${TEMP_HOME}` | Not modified for 7 days |

Each task selects only immediate children of its root. The recipe includes no specific applications, Agents, language toolchains, snapshots, system-management commands, or automations.

## 4. Includes

Ordinary includes:

```toml
include = ["official:minimal", "./common.toml"]
```

Platform-conditional includes:

```toml
include = [
  { path = "./linux.toml", platforms = ["linux"] },
  { path = "./desktop.toml", platforms = ["macos", "windows"] },
]
```

Rules:

- `official:<name>` references a resource compiled into the binary; currently only `official:minimal` exists.
- Local paths are relative to the file declaring the include.
- Official resources may include only other `official:` resources.
- Conditional entries for nonmatching platforms are skipped before paths are resolved or files read.
- Supported platform names are `linux`, `macos`, `windows`, `android`, and `ios`.
- Include cycles, unknown platforms, missing files, and type conflicts fail.
- Non-`official:` includes must be local file paths.

### 4.1 Merge rules

1. Resolve includes depth-first in array order.
2. Add included content to the result first.
3. Apply the current file last.
4. Merge tables recursively.
5. Replace arrays and scalars in full.
6. Different types at the same location are an error.

Arrays are not concatenated automatically. When overriding roots, tags, platforms, or modules arrays, write the complete desired array.

`enabled = false` disables an included module, selector, task, or automation.

## 5. Variable mappings

`${NAME}` in cleanup configuration is a logical variable; it does not directly read the environment variable of the same name. Environment variable names, platform directories, and fallbacks are maintained in a separate mapping file.

Built-in mapping source:

```text
mappings/official/variables.toml
```

It is also embedded in the binary through `include_str!`.

### 5.1 User mapping file

The recipe entry point references user mappings through `variable-map`:

```toml
schema-version = 2
name = "Personal SpacePrune"
variable-map = "./variables.toml"
```

`variables.toml`:

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

`variable-map`:

- may appear only in the recipe entry point;
- resolves relative to the recipe entry point's directory;
- must be a local file;
- does not expand `${NAME}` or `~` in its own path;
- uses only built-in mappings when omitted.

### 5.2 Variable definitions

Environment source:

```toml
[variables.TOOL_HOME]
environment = "TOOL_HOME"
fallback = "${HOME}/.tool"
```

Fixed or composed value:

```toml
[variables.TOOL_CACHE]
value = "${TOOL_HOME}/cache"
```

Each variable must choose exactly one:

- `environment`: read the specified process environment variable and use its value literally;
- `value`: use the configured value.

`fallback` is allowed only with `environment` and applies only when the environment variable is absent or empty. Both `value` and `fallback` can reference other mapped variables.

Variable names must start with an ASCII letter or underscore and contain only ASCII letters, digits, and underscores. environment names must be nonempty and cannot contain NUL or `=`, allowing names such as Windows `ProgramFiles(x86)`.

### 5.3 Platform overrides

```toml
[variables.ROOT]
value = "${HOME}/common"

[platforms.linux.variables.ROOT]
value = "${DATA_HOME}/linux-root"

[platforms.windows.variables.ROOT]
value = "${DATA_HOME}/WindowsRoot"
```

Active definitions merge in the following order, with later definitions winning:

1. Built-in common variables.
2. Built-in current-platform variables.
3. User common variables.
4. User current-platform variables.

A variable with the same name replaces the entire definition; fields are not combined. A user override must therefore provide the full `environment` / `value` / `fallback` definition for that variable.

Mapping files accept only the platform names `linux`, `macos`, `windows`, `android`, and `ios`.

### 5.4 Built-in logical variables

| Variable | Linux / Android | macOS / iOS | Windows |
| --- | --- | --- | --- |
| `HOME` | `HOME` environment variable | `HOME` environment variable | `USERPROFILE` |
| `CONFIG_HOME` | `XDG_CONFIG_HOME`, fallback `${HOME}/.config` | `${HOME}/Library/Application Support` | `APPDATA`, fallback `${HOME}/AppData/Roaming` |
| `CACHE_HOME` | `XDG_CACHE_HOME`, fallback `${HOME}/.cache` | `${HOME}/Library/Caches` | `LOCALAPPDATA`, fallback `${HOME}/AppData/Local` |
| `DATA_HOME` | `XDG_DATA_HOME`, fallback `${HOME}/.local/share` | `${HOME}/Library/Application Support` | `LOCALAPPDATA`, fallback `${HOME}/AppData/Local` |
| `STATE_HOME` | `XDG_STATE_HOME`, fallback `${HOME}/.local/state` | `${HOME}/Library/Application Support` | `LOCALAPPDATA`, fallback `${HOME}/AppData/Local` |
| `TEMP_HOME` | `TMPDIR`, fallback `/tmp` | `TMPDIR`, fallback `/tmp` | `TEMP`, fallback `${CACHE_HOME}/Temp` |

These environment variable names are data in the built-in TOML, not special cases in the variable expander.

### 5.5 Expansion rules

Task options:

```toml
[modules.example.tasks.cache.options]
roots = [{ path = "${TOOL_CACHE}", include = ["*"] }]
```

Rules:

- Only `${NAME}` syntax is accepted.
- Define fallbacks in variable mappings, not cleanup-path expressions.
- A leading `~`, `~/`, or `~\` uses the mapped `HOME`.
- No shell, command substitution, or scripts are executed.
- Undefined, currently unavailable, circular, unclosed, or invalid-name expressions fail.
- Expansion applies recursively to `options` of enabled tasks for the current platform.
- `recommend`, include paths, and `variable-map` paths are not expanded.
- Disabled tasks and tasks for other platforms do not require their variables to be available.

Plugins receive the final expanded option values.

## 6. Modules

```toml
[modules.personal]
name = "Personal"
description = "User-selected rebuildable data"
order = 20
enabled = true
```

Module IDs:

- allow only lowercase ASCII letters, digits, dots, and hyphens;
- reserve `module`, `plugin`, `automation`, `recipe`, `theme`, `language`, `authorization`, `scan`, and `clean` for fixed commands;
- become dynamic CLI subcommands directly.

`spaceprune module list` lists enabled modules; `spaceprune module list --all` also lists disabled modules. Other modules can define and use a dynamic selector named `all` normally.

Display order follows `order`, then ID for equal values.

## 7. Selectors

```toml
[modules.personal.selectors.logs]
name = "Logs"
match-tags = ["logs"]
enabled = true
```

`tags` are task attributes and also enter `TaskRequest`; `match-tags` are selector query conditions. Running `spaceprune personal --logs --scan` retains only tasks containing every `match-tags` value. Multiple selectors combine their required tags and still match with AND. Without selectors, all enabled tasks in the module that match the current platform run. Selectors do not change task configuration.

Selector IDs cannot duplicate the fixed long options `config`, `scan`, `clean`, `select`, `yes`, `output`, `no-progress`, `help`, or `version`. `all` is not a fixed long option and is available to ordinary modules.

## 8. Tasks

```toml
[modules.personal.tasks.logs]
name = "Logs"
description = "Old application logs"
handler = "builtin.filesystem/paths"
enabled = true
order = 10
tags = ["personal", "logs"]
platforms = ["linux", "macos"]

[modules.personal.tasks.logs.options]
roots = [{ path = "${TOOL_HOME}/logs", include = ["*.log"] }]

[modules.personal.tasks.logs.recommend]
older-than = "30d"
```

| Field | Required | Description |
| --- | --- | --- |
| `name` | Yes | Display name |
| `description` | No | Description |
| `handler` | Yes | Full `plugin-id/handler-id` |
| `enabled` | No | Defaults to `true` |
| `order` | No | Defaults to `0` |
| `tags` | No | Tags used by selector filtering |
| `platforms` | No | An empty array means platform-neutral |
| `options` | No | Handler configuration |
| `recommend` | No | Handler recommendation rules |

A Handler ID must contain exactly one `/`.

## 9. Filesystem Handler

ID:

```text
builtin.filesystem/paths
```

Complete example:

```toml
[modules.personal.tasks.logs.options]
files = true
directories = true

[[modules.personal.tasks.logs.options.roots]]
path = "${TOOL_HOME}/logs"
include = ["*.log", "archive/**/*.log"]
exclude = ["keep/**", "*.keep"]

[modules.personal.tasks.logs.recommend]
enabled = true
older-than = "30d"
min-size = "1MiB"
```

### 9.1 options

| Field | Default | Description |
| --- | --- | --- |
| `roots` | Required | One or more roots |
| `files` | `true` | Whether to produce file cleanup items |
| `directories` | `true` | Whether to produce directory cleanup items |
| `elevation` | `auto` | On-demand file-cleanup elevation: `auto`, `never`, Linux `sudo` / `pkexec`, Windows `uac`, macOS `native` |

On macOS, `auto` uses the native signed cleanup service. See [macOS native component packaging](../build/MACOS-PACKAGING.md) for packaging, authorization requirements, and current deletion-boundary limitations.

Each root:

| Field | Default | Description |
| --- | --- | --- |
| `path` | Required | Mapped or relative path |
| `include` | `["*"]` | Allowlist relative to root |
| `exclude` | `[]` | Denylist relative to root; takes precedence |

Globs support `*`, `?`, character classes, and `**`. An include or exclude containing a path separator uses recursive matching.

On Windows, `auto` uses a separate signed cleanup component and UAC to establish a session for the currently confirmed plan. Restricted items in the same batch reuse the connection. Only structured AccessDenied items are retried; measurements from partial completion are not added twice. File-in-use and read-only-volume errors are not treated as permission errors elevation can resolve.
Read-only and cleanup components are separate programs. Without existing administrator permission, background work reports that foreground action is needed instead of displaying a hidden prompt. Components must be installed in a protected Program Files directory and pass signature checks. Unsigned or portable development artifacts cannot obtain administrator deletion capability. This wiring still awaits native Windows signed-installation and UAC acceptance; cross-building is not native validation.

Linux cleanup first uses current permissions. Only confirmed items that actually fail for insufficient permissions are combined into one elevation request. CLI `auto` uses sudo in a terminal; Desktop explicitly uses graphical authorization and does not redirect password requests to a terminal merely because it was launched there. sudo first tries existing system authentication and requests a password once only if that has expired. Passwords never pass through the application, recipe, or AI. If the user declines, authorization attempts stop for the current batch rather than prompting for every file. `never` does not request administrator permission.

Read-only inspection and deletion authorization serve different purposes. Snapper sudo authentication can be reused while the system still accepts it; Polkit reuse depends on system authorization for the specific action. The application neither treats a saved Boolean as valid permission nor stores passwords. Elevated retries accept only original selected items bound to scan-time identity and revalidate root, include, and exclude. Directories are opened component by component through file descriptors, without following ancestor symbolic links or crossing mounts. Replaced items require a rescan. Stopping prevents authorization of new cleanup requests and cooperatively stops active ones; deleted content is not restored.

### 9.2 recommend

| Field | Default | Description |
| --- | --- | --- |
| `enabled` | `true` | false makes all items optional |
| `older-than` | None | The latest modification must be older than this duration |
| `min-size` | `0B` | Logical size must meet this threshold |

When both thresholds are present, both must be met.

`older-than` compares how long a cleanup item has been unmodified, not its creation time or a retention period. This documentation calls it “unmodified age.” For directories, it uses the latest modification time anywhere in the directory tree. A modification time that is too recent or unavailable makes the item optional.

### 9.3 Cleanup boundaries

- Symbolic links are not followed.
- A directory root itself cannot become an individual cleanup item.
- Items must still be within their configured root.
- include and exclude are revalidated before cleanup.
- A directory containing excluded descendants is not deleted wholesale.
- Relative roots resolve against the scanning process's current working directory.

If different tasks in one plan select identical paths or parent/child paths, Engine keeps only the outermost item, avoiding duplicate size accounting and deletion. An item already deleted by an earlier item is recorded as an idempotent 0 B success. A root missing at scan time that reappears later is not treated as authorized by the original scan.

## 10. Recommendations, plans, and modes

- recommended: selected by default and included by `--clean`;
- optional: not selected by default; selectable in `--select`;
- unavailable: displayed only; cannot enter a plan.

`--clean` rescans and produces a new plan every time. Automation also rescans rather than reusing historical cleanup items.

## 11. Automation

```toml
[automations.daily]
name = "Daily cleanup"
enabled = true
modules = ["cache", "personal"]
at = "03:00"
```

| Field | Description |
| --- | --- |
| `name` | Display name |
| `enabled` | Defaults to true |
| `modules` | An empty array means all enabled modules |
| `at` | Zero-padded local time `HH:MM`; defaults to `03:00` |

Automation always rescans and executes recommended items. It does not introduce a separate selection mode or trigger abstraction.

Installation writes the normalized path of the current application `config.toml` into a systemd user unit, LaunchAgent, or Windows current-user scheduled task. At runtime, that entry point resolves the current Recipe. The scheduler does not directly reference `recipes/<ID>/<ID>.toml`. Sessions loaded only through Core `load_recipe`, without an application entry point, reject automation installation to avoid creating jobs that CLI cannot reload.
The Core installation API also requires an explicit absolute runner path implementing `automation run <id>`. CLI uses its own path. Desktop and other hosts should provide the CLI runner deployed with the package, not implicitly write the current GUI process into the scheduler.

`official:minimal` has no automation definitions.

## 12. Specialized Handlers

The program provides the cross-platform `builtin.filesystem/paths` in section 9, plus these Linux-specific Handlers that the default recipe does not invoke:

```text
builtin.snapper/snapshots
builtin.system-linux/package-cache
builtin.system-linux/journal
builtin.system-linux/tmpfiles
builtin.system-linux/coredumps
builtin.system-linux/go-modcache  # existing Recipe alias
builtin.developer-tools/go-modcache
builtin.developer-tools/go-buildcache
builtin.developer-tools/cargo-registry-cache
builtin.developer-tools/npm-cache
builtin.developer-tools/docker-build-cache
builtin.developer-tools/docker-dangling-images
builtin.developer-tools/docker-unused-images
builtin.developer-tools/docker-stopped-containers
```

Users or third-party recipes must explicitly configure these Handlers. Prefer the filesystem Handler for ordinary path rules.

### 12.1 Snapper

`builtin.snapper/snapshots` is registered only on Linux.

Two inspection modes are supported. `snapshots` queries snapshot information and measures the logical size of files inside snapshots read-only during recipe scanning. `rust-targets` retains the existing inspection of Rust build artifacts inside snapshots. Existing recipes without `scan-mode` still use `rust-targets`. Logical-size measurement happens when Core scans a recipe.

`options`:

| Field | Default | Description |
| --- | --- | --- |
| `configs` | `[]` | Snapper configuration names; an empty array discovers all |
| `scan-mode` | `rust-targets` | `snapshots` queries snapshot information; `rust-targets` inspects build artifacts inside snapshots |
| `snapshot-ids` | None | Restricts `snapshots` mode to specific snapshot numbers; requires exactly one configuration in `configs` and complete pre/post selections |
| `snapshot-host-id` | None | When snapshot numbers are fixed, the program stores the original computer identity and checks it automatically before scanning and cleanup |
| `snapshot-identities` | None | With fixed snapshot numbers, stores each snapshot's `number`, `date`, `type`, and optional `pre-number` to reject replacements recreated with the same number |
| `snapshot-types` | `[]` | Snapshot-group type scope: `single`, `pre`, `post`, `pre/post`; an empty array includes all; complete pairs use `pre/post` |
| `cleanup-algorithms` | `[]` | Snapper cleanup-tag scope; an empty array includes all, `[""]` includes only untagged snapshots; every member of a group must match |
| `relative-path` | None | Relative area inside a snapshot subvolume; cannot be absolute or contain `..` |
| `snapper` | `snapper` | Snapper executable |
| `find` | `find` | GNU find executable |
| `du` | `du` | GNU du executable |
| `elevation` | `auto` | `auto`, `never`, `sudo`, or `pkexec`; `auto` uses sudo in a terminal and graphical authorization in Desktop |
| `max-depth` | `20` | Maximum depth for finding `target` directories; must be greater than 0 |

`recommend` (defaults depend on inspection mode):

| Field | `rust-targets` default | `snapshots` default | Description |
| --- | --- | --- | --- |
| `enabled` | `true` | `false` | Whether to automatically recommend snapshots meeting the conditions |
| `min-target-size` | `1GiB` | Unused | Size threshold for build-artifact inspection |
| `min-snapshot-size` | Unused | `0B` | Snapshot mode uses usage reported by Snapper; 0 means no size condition |
| `older-than-days` | Unused | `0` | Minimum age in days in snapshot mode; 0 means no time condition; uses the newest snapshot in the group |
| `keep-latest` | `3` | `0` | How many newest snapshots remain manually selectable rather than recommended; 0 means no such retention condition |

Snapshot recipes should explicitly use `snapshots` mode and derive recommendation conditions from the conversation and inspection results, not inherit the old 1GiB or keep-3 defaults. `min-snapshot-size` still uses usage reported by Snapper. If Snapper does not report usage and the recipe requires a minimum size, the item is not automatically recommended; file logical size does not replace this condition.

During recipe scanning, `du --apparent-size --block-size=1` measures logical size within the selected snapshot scope. It traverses only file metadata, without searching for `target`, reading file contents, crossing filesystems, or following symbolic links. Snapshots from the same configuration are measured together; `#0` is excluded. A pre/post pair's size is the sum of both members. Read-only authorization is requested according to `elevation` only after ordinary permissions fail; `never` preserves unknown values. Denial or measurement failure does not present partial data as a complete size or alter the original recommendation. Ordinary measurement supports cancellation; once a system elevation command is running, its exit behavior still constrains cancellation.

Both modes group pre/post snapshots into a single selection unit. `#0`, active, and default snapshots cannot be cleaned. `rust-targets` still lists manual, important, and paired pre/post snapshots as manually selectable by default. `snapshots` recommendations depend on explicitly configured conditions, without extra name-based classification. Users can adjust optional selections. Snapshot state and pair completeness are rechecked before cleanup, which invokes `snapper delete --sync`.

Within one cleanup, selected tasks targeting the same Snapper configuration with the same executable and elevation method merge into one deletion command; each rule still reports its own results. Only confirmed snapshot numbers are merged. Identity and protection state are rechecked before each configuration executes. Password requests normally no longer occur per snapshot. If listing snapshots itself requires administrator permission, pre-deletion validation may also request authorization once. sudo authentication is reused within the cleanup. A deletion failure does not trigger per-item retries or additional elevated checks; unconfirmed outcomes are honestly reported as failures. Stopping waits for an already-started system command to finish before preventing subsequent configurations.

Btrfs blocks may be shared. Snapshot logical size, Snapper-reported usage, and build-artifact size in the older mode do not represent exact reclaimable space. `logical_bytes` is for display only; `reclaimable_bytes` stays unknown. Cleanup results likewise do not count logical size as actual freed space.

Reusable rules omit `snapshot-ids`, `snapshot-host-id`, and `snapshot-identities`. Every execution scan queries the configuration's current snapshots again and produces results based on type, cleanup tag, age, size, and retention count. The example below expresses conditions already agreed by a user and AI; 45 days and 2 snapshots are not built-in product recommendations:

```toml
[modules.recovery.tasks.snapshots]
name = "Old recovery points"
handler = "builtin.snapper/snapshots"
platforms = ["linux"]

[modules.recovery.tasks.snapshots.options]
configs = ["root"]
scan-mode = "snapshots"
snapshot-types = ["single", "pre/post"]
cleanup-algorithms = ["timeline", "number"]
elevation = "auto"

[modules.recovery.tasks.snapshots.recommend]
enabled = true
older-than-days = 45
min-snapshot-size = "0B"
keep-latest = 2
```

`keep-latest` counts the newest-numbered noncurrent snapshots across the whole configuration. If either member of a pair falls within the retained count, the whole group is not recommended by default. Age uses the group's newest creation time. Count, age, and size conditions affect recommendations; users can still explicitly select optional items. Current/default/active snapshots remain uncleanable. Type and cleanup-tag conditions restrict task scope and are rechecked before cleanup.

The scan report binds the current computer, configuration subvolume, snapshot numbers, and creation identities into local selections and revalidates them during cleanup. These are protections for one execution, not fields written into reusable Recipes. Snapshots created after scanning do not automatically enter an existing confirmed scope. Replacement under the same number or a changed configuration requires rescanning.

Older recipes with `snapshot-ids` represent snapshots explicitly selected at that time; their existing constraints remain. Once those snapshots disappear, the recipe does not automatically select future snapshots. To replace a fixed selection with reusable rules, inspect the old task and configuration, disable the replaced task with `enabled = false`, preserve unrelated tasks, then validate and save the new rules. Without an explicit replacement, adding a dynamic rule leaves old tasks intact. A fixed selection synchronized to another computer cannot execute directly. Reusable rules can synchronize, but must still be rescanned on the target computer and checked against its configuration.

### 12.2 Linux system cleanup

The following Linux Handlers share options:

| Field | Default | Description |
| --- | --- | --- |
| `manager` | `auto` | Used by `package-cache`; recognizes zypper, apt/apt-get, dnf, and pacman |
| `vacuum-time` | `30d` | Duration passed to `journalctl --vacuum-time` by `journal` |
| `elevation` | `auto` | `auto`, `never`, `sudo`, or `pkexec` |

Their `recommend.enabled` defaults to true; false makes items optional. Missing commands or an unsupported package manager make items unavailable. Package and systemd cleanup invoke the chosen package manager, `journalctl`, or `systemd-tmpfiles` respectively.
System-cache scanning skips unreadable subtrees, retains other readable sizes, and emits a warning stating that the displayed value is a lower bound. One protected temporary download directory does not discard the whole cache measurement.
`coredumps` uses `systemd-tmpfiles --clean --prefix=/var/lib/systemd/coredump`, following system retention rules. `go-modcache` reads `GOMODCACHE` from the ordinary user's Go environment and binds directory/parent identities during scanning; cleanup does not rediscover paths in an administrator's environment.

Current Go cleanup uses native per-descendant capture, never `go clean` or directory-mode changes. New descendants remain in the selected directory scope; moved-out entries and external symlink targets are not followed. CLI includes its ordinary worker entrypoint; App uses its bundled cleanup helper. Other Rust hosts follow the [worker distribution guide](../build/DEVELOPMENT.md#distributing-the-linux-go-cache-worker). Read-only modules owned by the mapped UID/GID can be cleaned in a user namespace. Only real capability gaps follow `elevation`; `never` requests no authorization. Missing workers produce an explicit component-unavailable error.

Older executables still using `go clean -modcache` have a concurrent-directory boundary risk: moves or replacements can affect content outside the selected subtree. Keep real-cache `go-modcache` tasks at `enabled = false` while running those old executables or release packages, until upgrading to the native adapter and matching components. Updating documentation or source does not update an installed program. Current source no longer requires that disablement, and existing Recipes need no migration. See the [Go cache boundary record](../../zh/development/VALIDATION.md) for implementation and evidence.

### Developer-tool caches

`builtin.developer-tools` supports Docker lifecycle operations on Linux, Windows and macOS desktops, excluding Android/iOS. Its Go/Cargo/npm cache handlers remain Linux-only. Each handler checks its native tool before scanning; missing tools are skipped without cache traversal or authorization. See [Docker cleanup](DOCKER.md). It discovers caches in the ordinary user's tool environment and does not accept arbitrary paths or shell commands.

| Handler | Scope | Consequence |
| --- | --- | --- |
| `go-modcache` | Go module cache | Dependencies may need downloading again |
| `go-buildcache` | Go build cache | Future builds recompile |
| `cargo-registry-cache` | Cargo `registry/cache` archives | Offline builds may need downloads; preserves sources, Git checkouts, credentials and toolchains |
| `npm-cache` | npm content-addressed `_cacache` | Offline installs may need downloads; preserves node_modules, configuration and credentials |

```toml
[modules.developer]
name = "Developer caches"
[modules.developer.tasks.npm]
name = "npm download cache"
handler = "builtin.developer-tools/npm-cache"
platforms = ["linux"]
[modules.developer.tasks.npm.options]
elevation = "auto"
[modules.developer.tasks.npm.recommend]
enabled = false
```

`elevation` accepts `auto`, `never`, `sudo` or `pkexec` and defaults to `auto`; `recommend.enabled` defaults to `true`. The example starts optional so you can review scope and offline requirements. Domain tasks do not support age or size filtering. Scans are read-only; execution uses identity-bound native operations, not cargo/npm/go clean. The existing `builtin.system-linux/go-modcache` entry remains usable under its original system-linux enablement setting.

## 13. Local recipes

Each local recipe uses a stable ID, a matching directory, and a matching entry-file name:

```text
recipes/
└── example/
    ├── example.toml
    ├── example.generated.toml
    └── variables.toml
```

Here, `example` is the recipe ID. Only when `recipes/example/example.toml` exists will `recipe list` list it and `recipe set example` select it. The entry point may still include files from the same directory:

```toml
include = ["official:minimal", "./example.generated.toml"]
```

Except for `official:minimal`, includes must be local TOML files. Review their contents before use. Only files explicitly referenced by the current recipe entry point take effect.

## 14. Recipe bundles

The public configuration API treats `recipes/<ID>/` as a complete bundle rather than synchronizing only its entry point:

```json
{
  "id": "personal",
  "files": {
    "personal.toml": "schema-version = 2\nname = \"Personal\"\n...",
    "personal.generated.toml": "schema-version = 2\n...",
    "variables.toml": "schema-version = 2\n..."
  }
}
```

Keys in `files` must be bundle-relative paths separated by `/`; values must be UTF-8 text. Import requires:

- `id` is a stable Recipe ID, with a matching entry point `<ID>.toml`.
- Empty paths, absolute paths, backslashes, `.`, and `..` are rejected.
- Export accepts only regular files and directories, rejecting symbolic links and other special files.
- All files are first written to a temporary directory. `ConfigLoader` then validates includes, Schema, variables, and tasks from the entry point.
- The target directory is replaced only after successful validation. Failure preserves the original bundle; import does not automatically switch the current Recipe.

`ConfigStore::export_recipe_bundle` and `ConfigStore::upsert_recipe_bundle` are the boundaries Desktop, synchronization clients, and other hosts should reuse. A cloud-stored revision does not replace local semantic validation. After downloading, reimport on the target device and run a read-only scan.

## 15. Complete personal configuration example

`~/.config/spaceprune/config.toml`:

```toml
schema-version = 2
recipe = "personal"

[theme]
id = "aurora"
```

`~/.config/spaceprune/recipes/personal/personal.toml`:

```toml
schema-version = 2
name = "Personal SpacePrune"
variable-map = "./variables.toml"

include = ["official:minimal", "./personal.generated.toml"]

[automations.daily]
name = "Daily cleanup"
modules = ["cache", "personal"]
at = "03:00"
```

`~/.config/spaceprune/recipes/personal/personal.generated.toml`:

```toml
schema-version = 2

[modules.personal]
name = "Personal caches"
order = 20

[modules.personal.tasks.example]
name = "Example cache"
handler = "builtin.filesystem/paths"
tags = ["cache"]

[modules.personal.tasks.example.options]
roots = [
  { path = "${EXAMPLE_CACHE}", include = ["*"], exclude = ["keep/**"] },
]

[modules.personal.tasks.example.recommend]
older-than = "14d"
```

`~/.config/spaceprune/recipes/personal/variables.toml`:

```toml
schema-version = 2

[variables.EXAMPLE_HOME]
environment = "EXAMPLE_HOME"
fallback = "${DATA_HOME}/example"

[variables.EXAMPLE_CACHE]
value = "${EXAMPLE_HOME}/cache"

[platforms.windows.variables.EXAMPLE_HOME]
value = "${DATA_HOME}/Example"
```

Read-only validation:

```bash
spaceprune scan --config /absolute/path/to/config.toml --no-progress
spaceprune scan --config /absolute/path/to/config.toml --output json --no-progress
```

## 16. Common mistakes

- Treating include files as unversioned fragments and omitting the integer `schema-version = 2`.
- Using different IDs for the recipe directory and entry-file name.
- Selecting a recipe ID in application configuration that does not exist under `recipes`.
- Forgetting to include `official:minimal` in the recipe entry point, leaving only custom modules.
- Putting `[theme]` in a recipe rather than application `config.toml`.
- Putting `variable-map` in an include fragment instead of the recipe entry point.
- Using unmapped environment variable names directly in cleanup configuration.
- Putting a fallback inside a path rather than its mapping definition.
- Overriding only part of a user platform variable, leaving the full definition without `environment` or `value`.
- Omitting original entries when replacing a roots array.
- Using `~`, `${NAME}`, or other unexpanded bootstrap expressions in include or mapping-file paths.
- Writing an absolute glob when globs are relative to root.
- Validating new configuration with `--clean --yes` instead of running `--scan` first.

## Public bundle contract

`spaceprune-contracts::bundle::RecipeBundle` is the sole public Rust data definition for bundles. JSON contains only `id` and `files` and rejects unknown top-level fields. The complete file map remains separate from current-platform resolution results. Source-file editing uses config's `rename_recipe_bundle` / `update_bundle_automation`, rather than rebuilding a whole recipe from scan or resolution reports.

All bundle paths and directory prefixes share one portable path validator. Besides traversal, absolute paths, and Windows reserved names, it rejects ASCII case conflicts such as `a.toml` versus `A.toml/x.toml` and `Parts/a.toml` versus `parts/b.toml`.
Local import validates the entire namespace before creating a temporary directory; validation failure does not overwrite the original recipe. This pure validation does not replace native identity, link, and permission checks on the target filesystem or claim coverage of every Unicode filesystem-normalization rule.

## Schema 2 and scope declarations

Application configuration, Recipe entry points, every include, and variable-map all use the integer `schema-version = 2`. Version 1 configurations are explicitly rejected rather than silently interpreted with the new scope semantics. Plugin manifests retain their independent schema 1. To migrate, change the version number and check the newly added usage and scope constraints.

`usage = "recurring"` (default) saves reusable rules that enumerate again at each scan. `usage = "one_shot"` saves a one-time selection bound to host/object identity and cannot enable scheduled automation. One-time files use `exact_file`; directories use `directory_children` or `directory_subtree`. Both directory scopes retain dynamic subtree semantics and hard exclusions for the selected directory. `directory_children` uses only immediate children as selection units and must not be reduced to recursive file matching.

Recurring roots may use a string expression, `{ user_home = [".cache", "example"] }`, or `{ local_absolute = "/local/observed/cache", host_id = "本机身份" }`. Local absolute locations must match the current host identity; switching devices requires rebinding. File identities belong only to one-time bindings and the current execution plan. The current file inventory cannot be saved as a recurring rule.

Public JSON Schemas live in `schemas/` and are generated from Rust types in `spaceprune-contracts`. Run `cargo run --locked -p spaceprune-contracts --bin export-schemas` to update generated files; contract tests check that they match the types. The filesystem Handler's options/recommend schemas are also generated from its actual parsing types and include scope and bindings.
